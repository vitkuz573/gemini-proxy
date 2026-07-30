use std::collections::HashMap;

use reqwest::Client;
use serde_json::{json, Value};
use tracing::{debug, error, warn};

use crate::error::{ProxyError, Result};

const WEB_BASE_URL: &str = "https://gemini.google.com";
const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36";

/// Hex mode IDs from browser captures (NOT u32 numbers)
const HEX_MODE_FAST: &str = "fbb127bbb056c959";
const HEX_MODE_FLASH_LITE: &str = "cf41b0e0dd7d53e5";
const HEX_MODE_THINKING: &str = "e6fa609c3fa255c0";
const HEX_MODE_PRO: &str = "9d8ca3786ebdfbea";

/// Model information discovered from the Gemini web frontend model picker.
#[derive(Debug, Clone)]
pub struct WebModelInfo {
    /// Google's internal hex mode ID (used by StreamGenerate side-channel).
    pub id: String,
    /// Short title shown in the UI (e.g. "Flash").
    pub title: String,
    /// Longer description (e.g. "All-around help").
    pub description: String,
    /// Versioned name if available (e.g. "3.6 Flash").
    pub versioned_name: Option<String>,
    /// Mode category: FAST, THINKING, PRO, AUTO, FLASH_LITE, etc.
    pub category: String,
}

#[derive(Debug, Clone)]
pub struct WebSession {
    pub access_token: Option<String>,
    pub build_label: Option<String>,
    pub session_id: Option<String>,
    pub language: String,
}

impl Default for WebSession {
    fn default() -> Self {
        Self {
            access_token: None,
            build_label: None,
            session_id: None,
            language: "en".to_string(),
        }
    }
}

pub struct WebFrontendClient {
    client: Client,
    cookies: HashMap<String, String>,
    session: WebSession,
}

impl WebFrontendClient {
    pub fn new(cookies: HashMap<String, String>) -> Result<Self> {
        let client = Client::builder()
            .pool_max_idle_per_host(20)
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| ProxyError::Config(format!("Failed to build HTTP client: {e}")))?;

        Ok(Self {
            client,
            cookies,
            session: WebSession::default(),
        })
    }

    pub fn build_cookie_header(&self) -> String {
        self.cookies
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; ")
    }

    pub fn build_headers(&self) -> Vec<(String, String)> {
        vec![
            (
                "Content-Type".into(),
                "application/x-www-form-urlencoded;charset=UTF-8".into(),
            ),
            ("User-Agent".into(), USER_AGENT.into()),
            ("Origin".into(), WEB_BASE_URL.into()),
            ("Referer".into(), format!("{WEB_BASE_URL}/app")),
            ("X-Same-Domain".into(), "1".into()),
            ("Cache-Control".into(), "no-cache".into()),
            ("Pragma".into(), "no-cache".into()),
        ]
    }

    async fn init_session(&mut self) -> Result<()> {
        debug!("Initializing web session - fetching page data");

        let url = format!("{WEB_BASE_URL}/app?hl={}", self.session.language);
        let cookie_header = self.build_cookie_header();

        let headers = self.build_headers();
        let mut request = self
            .client
            .get(&url)
            .header("Cookie", &cookie_header)
            .header("User-Agent", USER_AGENT);

        for (key, value) in &headers {
            // Avoid overriding the Cookie/User-Agent we just set.
            if *key != "Cookie" && *key != "User-Agent" {
                request = request.header(key.as_str(), value.as_str());
            }
        }

        let response = request
            .send()
            .await
            .map_err(|e| ProxyError::GeminiApi(format!("Failed to fetch Gemini page: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            return Err(ProxyError::GeminiApi(format!(
                "Failed to fetch Gemini page: HTTP {status}"
            )));
        }

        let body = response
            .text()
            .await
            .map_err(|e| ProxyError::GeminiApi(format!("Failed to read response body: {e}")))?;

        // SNlM0e token is optional — Google removed it from HTML in April 2026.
        // Requests still work without it for basic text generation.
        if let Some(token) = extract_snlim0e(&body) {
            debug!("Extracted SNlM0e token");
            self.session.access_token = Some(token);
        } else {
            warn!("SNlM0e token not found in page — proceeding without it (may fail for authenticated requests)");
        }

        if let Some(label) = extract_build_label(&body) {
            debug!(label = %label, "Extracted build label");
            self.session.build_label = Some(label);
        }

        if let Some(sid) = extract_session_id(&body) {
            debug!(sid = %sid, "Extracted session ID");
            self.session.session_id = Some(sid);
        }

        Ok(())
    }

    pub async fn generate_content(
        &mut self,
        model: &str,
        prompt: &str,
    ) -> Result<String> {
        if self.session.access_token.is_none() && self.session.build_label.is_none() {
            self.init_session().await?;
        }

        let reqid = {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();
            ((ts % 900_000) + 100_000).to_string()
        };

        let mut params = vec![
            ("hl", self.session.language.as_str()),
            ("_reqid", reqid.as_str()),
            ("rt", "c"),
            ("pageId", "none"),
        ];

        if let Some(ref bl) = self.session.build_label {
            params.push(("bl", bl.as_str()));
        }
        if let Some(ref sid) = self.session.session_id {
            params.push(("f.sid", sid.as_str()));
        }

        let url = format!("{WEB_BASE_URL}/_/BardChatUi/data/assistant.lamda.BardFrontendService/StreamGenerate");

        let mode_id = resolve_model_mode(model);
        let inner_req_list = build_inner_req_list(prompt);
        let inner_json = serde_json::to_string(&inner_req_list).unwrap_or_default();
        let f_req = json!([null, inner_json]);
        let f_req_str = serde_json::to_string(&f_req).unwrap_or_default();

        let side_channel = build_side_channel_header(mode_id);
        let side_channel_str = side_channel.to_string();

        let form_data = [format!("f.req={}", urlencoding::encode(&f_req_str)),
            format!("at={}", urlencoding::encode(&side_channel_str))];

        let body = form_data.join("&");

        let headers = self.build_headers();

        debug!(model, mode_id, "Sending request to web frontend");

        let mut request = self.client.post(&url)
            .query(&params)
            .body(body);

        for (key, value) in &headers {
            request = request.header(key.as_str(), value.as_str());
        }

        request = request.header("Cookie", self.build_cookie_header());

        let response = request
            .send()
            .await
            .map_err(|e| ProxyError::GeminiApi(format!("Web frontend request failed: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            error!(status = %status, body = %body, "Web frontend error");
            return Err(ProxyError::GeminiApi(format!(
                "HTTP {status}: {body}"
            )));
        }

        let body = response
            .text()
            .await
            .map_err(|e| ProxyError::GeminiApi(format!("Failed to read response: {e}")))?;

        debug!(body_len = body.len(), "Response from Gemini");

        let text = parse_stream_response(&body)?;

        Ok(text)
    }

    pub async fn stream_generate(
        &mut self,
        model: &str,
        prompt: &str,
    ) -> Result<reqwest::Response> {
        if self.session.access_token.is_none() && self.session.build_label.is_none() {
            self.init_session().await?;
        }

        let reqid = {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();
            ((ts % 900_000) + 100_000).to_string()
        };

        let mut params = vec![
            ("hl", self.session.language.as_str()),
            ("_reqid", reqid.as_str()),
            ("rt", "c"),
            ("pageId", "none"),
        ];

        if let Some(ref bl) = self.session.build_label {
            params.push(("bl", bl.as_str()));
        }
        if let Some(ref sid) = self.session.session_id {
            params.push(("f.sid", sid.as_str()));
        }

        let url = format!("{WEB_BASE_URL}/_/BardChatUi/data/assistant.lamda.BardFrontendService/StreamGenerate");

        let mode_id = resolve_model_mode(model);
        let inner_req_list = build_inner_req_list(prompt);
        let inner_json = serde_json::to_string(&inner_req_list).unwrap_or_default();
        let f_req = json!([null, inner_json]);
        let f_req_str = serde_json::to_string(&f_req).unwrap_or_default();

        let side_channel = build_side_channel_header(mode_id);
        let side_channel_str = side_channel.to_string();

        let form_data = [format!("f.req={}", urlencoding::encode(&f_req_str)),
            format!("at={}", urlencoding::encode(&side_channel_str))];

        let body = form_data.join("&");

        let headers = self.build_headers();

        debug!(model, mode_id, "Sending streaming request to web frontend");

        let mut request = self.client.post(&url)
            .query(&params)
            .body(body);

        for (key, value) in &headers {
            request = request.header(key.as_str(), value.as_str());
        }

        request = request.header("Cookie", self.build_cookie_header());

        let response = request
            .send()
            .await
            .map_err(|e| ProxyError::GeminiApi(format!("Web frontend streaming request failed: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            error!(status = %status, body = %body, "Web frontend streaming error");
            return Err(ProxyError::GeminiApi(format!(
                "HTTP {status}: {body}"
            )));
        }

        Ok(response)
    }

    pub fn session(&self) -> &WebSession {
        &self.session
    }

    pub fn set_session(&mut self, session: WebSession) {
        self.session = session;
    }

    pub async fn close(&mut self) {
        self.session = WebSession::default();
    }

    /// Discover the models available to this account via the web frontend model picker.
    ///
    /// Calls `BardFrontendService.GetUserStatus` (rpcid `otAQ7b`) through the batchexecute
    /// transport and parses the returned mode list.
    pub async fn list_models(&mut self) -> Result<Vec<WebModelInfo>> {
        if self.session.access_token.is_none() && self.session.build_label.is_none() {
            self.init_session().await?;
        }

        let reqid = {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();
            ((ts % 900_000) + 100_000).to_string()
        };

        let mut params = vec![
            ("rpcids", "otAQ7b"),
            ("source-path", "/app"),
            ("hl", self.session.language.as_str()),
            ("_reqid", reqid.as_str()),
            ("rt", "c"),
            ("pageId", "none"),
        ];

        if let Some(ref bl) = self.session.build_label {
            params.push(("bl", bl.as_str()));
        }
        if let Some(ref sid) = self.session.session_id {
            params.push(("f.sid", sid.as_str()));
        }

        let url = format!("{WEB_BASE_URL}/_/BardChatUi/data/batchexecute");

        // Body used by the Gemini web frontend for GetUserStatus.
        // f.req is a WIZ batch array: outer array -> batch -> [rpc_id, payload, null, "generic"].
        let f_req_payload = json!([[["otAQ7b", "[]", null, "generic"]]]);
        let f_req_str = serde_json::to_string(&f_req_payload).unwrap_or_default();

        let at = self
            .session
            .access_token
            .as_deref()
            .unwrap_or("");

        let form_data = [format!("f.req={}", urlencoding::encode(&f_req_str)),
            format!("at={}", urlencoding::encode(at))];
        let body = form_data.join("&");

        let headers = self.build_headers();

        debug!(url = %url, body = %body, "sending GetUserStatus request for model list");

        let mut request = self
            .client
            .post(&url)
            .query(&params)
            .body(body);

        for (key, value) in &headers {
            request = request.header(key.as_str(), value.as_str());
        }
        request = request.header("Cookie", self.build_cookie_header());

        let response = request
            .send()
            .await
            .map_err(|e| ProxyError::GeminiApi(format!("GetUserStatus request failed: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            error!(status = %status, body = %body, "GetUserStatus error");
            return Err(ProxyError::GeminiApi(format!(
                "GetUserStatus HTTP {status}: {body}"
            )));
        }

        let text = response
            .text()
            .await
            .map_err(|e| ProxyError::GeminiApi(format!("Failed to read GetUserStatus body: {e}")))?;

        debug!(body_len = text.len(), "received GetUserStatus response");

        parse_user_status_model_list(&text)
    }
}

/// Parse the batchexecute GetUserStatus response and extract the mode list.
///
/// Response shape: `)]}'\n\n[[["wrb.fr","otAQ7b",null,"<json-string>",...]]]`
fn parse_user_status_model_list(body: &str) -> Result<Vec<WebModelInfo>> {
    // The response is WIZ anti-XSSI text: `)]}'\n\n[[...]]\n58\n[[...]]\n...`
    // We only need the first JSON array, so strip the prefix and stop at the
    // first line that does not continue the JSON structure.
    let json_start = body.find('[').ok_or_else(|| {
        ProxyError::GeminiApi("GetUserStatus response does not contain JSON array".into())
    })?;

    let after_prefix = &body[json_start..];
    // batchexecute returns each JSON structure on its own line; take the first one.
    let json_end = after_prefix.find('\n').unwrap_or(after_prefix.len());
    let payload = &after_prefix[..json_end];

    let outer: Value = serde_json::from_str(payload).map_err(|e| {
        ProxyError::GeminiApi(format!("Failed to parse GetUserStatus JSON: {e}"))
    })?;

    let outer_array = outer.as_array().ok_or_else(|| {
        ProxyError::GeminiApi("GetUserStatus response is not a JSON array".into())
    })?;

    // Find the otAQ7b RPC response.
    let rpc_entry = outer_array.iter().find(|entry| {
        entry
            .get(1)
            .and_then(|v| v.as_str())
            .map(|s| s == "otAQ7b")
            .unwrap_or(false)
    });

    let rpc_entry = rpc_entry.ok_or_else(|| {
        ProxyError::GeminiApi("GetUserStatus response does not contain otAQ7b entry".into())
    })?;

    // The payload string is at index 2 in the wrb.fr entry.
    let payload_str = rpc_entry
        .get(2)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            ProxyError::GeminiApi("GetUserStatus response payload missing".into())
        })?;

    let inner: Value = serde_json::from_str(payload_str).map_err(|e| {
        ProxyError::GeminiApi(format!("Failed to parse GetUserStatus inner payload: {e}"))
    })?;

    // The mode list is at index 15 of the inner GetUserStatus array.
    let modes = inner
        .get(15)
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            ProxyError::GeminiApi("GetUserStatus inner payload does not contain mode list".into())
        })?;

    let mut result = Vec::with_capacity(modes.len());
    for mode in modes {
        let Some(mode_arr) = mode.as_array() else {
            continue;
        };
        if mode_arr.is_empty() {
            continue;
        }

        let id = mode_arr.first()
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if id.is_empty() {
            continue;
        }

        let title = mode_arr
            .get(1)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let description = mode_arr
            .get(2)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // Versioned name: field 11 is preferred; field 19 is a fallback.
        let versioned_name = mode_arr
            .get(11)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .or_else(|| mode_arr.get(19).and_then(|v| v.as_str()).filter(|s| !s.is_empty()))
            .map(|s| s.to_string());

        // Category enum at field 17; fall back to deriving from title/hex constants.
        let category = mode_arr
            .get(17)
            .and_then(|v| v.as_u64())
            .map(category_from_enum)
            .unwrap_or_else(|| derive_category(&id, &title));

        result.push(WebModelInfo {
            id,
            title,
            description,
            versioned_name,
            category,
        });
    }

    if result.is_empty() {
        return Err(ProxyError::GeminiApi(
            "GetUserStatus returned empty model list".into(),
        ));
    }

    Ok(result)
}

fn category_from_enum(value: u64) -> String {
    match value {
        1 => "FAST",
        2 => "THINKING",
        3 => "PRO",
        4 => "AUTO",
        5 => "FAST_DYNAMIC_THINKING",
        6 => "FLASH_LITE",
        _ => "UNSPECIFIED",
    }
    .to_string()
}

fn derive_category(id: &str, title: &str) -> String {
    let combined = format!("{id} {title}").to_lowercase();
    if combined.contains("lite") {
        "FLASH_LITE".to_string()
    } else if combined.contains("thinking") || combined.contains("deep") {
        "THINKING".to_string()
    } else if combined.contains("pro") {
        "PRO".to_string()
    } else if combined.contains("auto") {
        "AUTO".to_string()
    } else {
        "FAST".to_string()
    }
}

/// Resolve model name to Google's internal mode ID (hex string).
///
/// Accepts either a raw hex mode ID (with or without the `models/` prefix) or a
/// human-readable alias. This keeps both `/v1/models` IDs and legacy aliases working.
fn resolve_model_mode(model: &str) -> &'static str {
    let stripped = model
        .strip_prefix("models/")
        .unwrap_or(model)
        .to_lowercase();

    // Exact hex IDs discovered via GetUserStatus take precedence.
    if is_hex_mode_id(&stripped) {
        if stripped == HEX_MODE_FAST {
            return HEX_MODE_FAST;
        }
        if stripped == HEX_MODE_FLASH_LITE {
            return HEX_MODE_FLASH_LITE;
        }
        if stripped == HEX_MODE_THINKING {
            return HEX_MODE_THINKING;
        }
        if stripped == HEX_MODE_PRO {
            return HEX_MODE_PRO;
        }
        // Unknown hex ID: fall through to the heuristic below for a best-effort mapping.
    }

    let model_lower = stripped;
    if model_lower.contains("lite") {
        HEX_MODE_FLASH_LITE
    } else if model_lower.contains("deep") || model_lower.contains("thinking") {
        HEX_MODE_THINKING
    } else if model_lower.contains("pro") {
        HEX_MODE_PRO
    } else {
        HEX_MODE_FAST
    }
}

fn is_hex_mode_id(s: &str) -> bool {
    s.len() == 16 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Build the inner request list (69-slot array) matching browser captures
fn build_inner_req_list(prompt: &str) -> Vec<Value> {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let mut inner: Vec<Value> = vec![Value::Null; 69];
    inner[0] = json!([prompt, 0, null, null, null, null, 0]);
    inner[1] = json!(["en"]);
    inner[2] = json!(["", "", "", null, null, null, null, null, null, ""]);
    inner[6] = json!([1]);
    inner[7] = json!(1);
    inner[10] = json!(1);
    inner[11] = json!(0);
    inner[17] = json!([[0]]);
    inner[18] = json!(0);
    inner[27] = json!(1);
    inner[30] = json!([4]);
    inner[53] = json!(0);
    inner[59] = json!("CD1035A5-0E0E-4B68-B744-23C2D8960DF5");
    inner[61] = json!([]);
    inner[66] = json!([ts, 0]);
    inner[68] = json!(2);
    inner
}

/// Build the side channel header (12-element array) with mode ID
fn build_side_channel_header(mode_id: &str) -> Value {
    json!([
        1,
        null,
        null,
        null,
        mode_id,
        null,
        null,
        0,
        [4],
        null,
        null,
        3
    ])
}

/// Extract SNlM0e token from HTML page
fn extract_snlim0e(body: &str) -> Option<String> {
    let patterns = [
        "SNlM0e\":\"",
        "AF_initDataCallback({key: 'ds:1', hash:",
        "window.APP_OPTIONS",
    ];

    for pattern in &patterns {
        if let Some(idx) = body.find(pattern) {
            let start = idx + pattern.len();
            if let Some(end) = body[start..].find('"') {
                let token = &body[start..start + end];
                if !token.is_empty() {
                    return Some(token.to_string());
                }
            }
        }
    }

    // Fallback: look for SNlM0e in various formats
    if let Some(idx) = body.find("SNlM0e") {
        let search_area = &body[idx..];
        if let Some(eq_idx) = search_area.find("=\"") {
            let start = eq_idx + 2;
            if let Some(end) = search_area[start..].find('"') {
                let token = &search_area[start..start + end];
                if !token.is_empty() {
                    return Some(token.to_string());
                }
            }
        }
    }

    None
}

/// Extract build label from HTML page
fn extract_build_label(body: &str) -> Option<String> {
    let patterns = [
        "boq_assistant-bard-web-server_",
        "boq_assistant-bard-web-frontend_",
    ];

    for pattern in &patterns {
        if let Some(idx) = body.find(pattern) {
            let start = idx;
            let search_area = &body[start..];
            for end_char in ['"', '\\', '\'', '`'] {
                if let Some(end) = search_area.find(end_char) {
                    let label = &search_area[..end];
                    if label.len() > 10 {
                        return Some(label.to_string());
                    }
                }
            }
        }
    }

    None
}

/// Extract session ID from HTML page
fn extract_session_id(body: &str) -> Option<String> {
    let patterns = ["\"FdrFJe\":\"", "session_id\":\""];

    for pattern in &patterns {
        if let Some(idx) = body.find(pattern) {
            let start = idx + pattern.len();
            if let Some(end) = body[start..].find('"') {
                let sid = &body[start..start + end];
                if !sid.is_empty() {
                    return Some(sid.to_string());
                }
            }
        }
    }

    None
}

/// Parse the StreamGenerate response
fn parse_stream_response(body: &str) -> Result<String> {
    let mut texts: Vec<String> = Vec::new();

    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if line.starts_with('[')
            && let Ok(parsed) = serde_json::from_str::<Value>(line)
                && let Some(text) = extract_text_from_parsed_response(&parsed) {
                    texts.push(text);
                }
    }

    // Return the last non-empty text (complete response)
    for text in texts.iter().rev() {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }

    // Fallback: try to parse entire body as single response
    if let Some(text) = extract_text_from_single_response(body) {
        return Ok(text);
    }

    error!(body_len = body.len(), "Could not extract text from response");
    Err(ProxyError::GeminiApi(
        "Could not parse response from Gemini web frontend".into(),
    ))
}

/// Extract text from a single response (non-streaming)
fn extract_text_from_single_response(body: &str) -> Option<String> {
    if let Ok(parsed) = serde_json::from_str::<Value>(body) {
        return extract_text_from_parsed_response(&parsed);
    }

    if let Some(start) = body.find("[[")
        && let Ok(parsed) = serde_json::from_str::<Value>(&body[start..]) {
            return extract_text_from_parsed_response(&parsed);
        }

    None
}

/// Extract text from a parsed JSON response
pub fn extract_text_from_parsed_response(parsed: &Value) -> Option<String> {
    let arr = parsed.as_array()?;

    // Response format: [["wrb.fr", null, "json-payload"]]
    // Each entry in the outer array is [rpc_id, metadata, payload_string]
    for item in arr {
        if let Some(entry) = item.as_array()
            && entry.len() >= 3
                && let Some(rpc_id) = entry[0].as_str()
                    && rpc_id == "wrb.fr" {
                        // Payload is at position [2]
                        if let Some(json_str) = entry[2].as_str()
                            && let Ok(inner_parsed) =
                                serde_json::from_str::<Value>(json_str)
                                && let Some(text) =
                                    extract_text_from_inner_response(&inner_parsed)
                                {
                                    return Some(text);
                                }
                    }
    }

    None
}

/// Extract text from the inner wrb.fr response
fn extract_text_from_inner_response(parsed: &Value) -> Option<String> {
    let arr = parsed.as_array()?;

    // Format (2026): position [4] contains response parts
    // inner[4] is an array of parts, each part[1] is an array of text strings
    if let Some(parts) = arr.get(4)
        && let Some(parts_arr) = parts.as_array() {
            for part in parts_arr {
                if let Some(part_arr) = part.as_array() {
                    // part[1] is an array of text strings
                    if let Some(text_list) = part_arr.get(1)
                        && let Some(text_list_arr) = text_list.as_array() {
                            let mut combined = String::new();
                            for text_val in text_list_arr {
                                if let Some(text_str) = text_val.as_str() {
                                    // Skip ID-like strings (r_ or c_ prefixed)
                                    if !text_str.is_empty()
                                        && !text_str.starts_with("r_")
                                        && !text_str.starts_with("c_")
                                    {
                                        combined.push_str(text_str);
                                    }
                                }
                            }
                            if !combined.is_empty() {
                                return Some(combined);
                            }
                        }
                }
            }
        }

    None
}


