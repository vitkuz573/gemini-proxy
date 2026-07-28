use std::collections::HashMap;

use reqwest::Client;
use serde_json::{json, Value};
use tracing::{debug, error, warn};

use crate::error::{ProxyError, Result};

const WEB_BASE_URL: &str = "https://gemini.google.com";
const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36";

/// Model mode IDs from Google's MODE_CATEGORY enum
const MODE_FAST: u32 = 1;
const MODE_THINKING: u32 = 2;
const MODE_PRO: u32 = 3;

#[derive(Debug, Clone)]
pub struct WebSession {
    pub access_token: Option<String>,
    pub build_label: Option<String>,
    pub session_id: Option<String>,
    pub language: String,
    pub reqid: u32,
}

impl Default for WebSession {
    fn default() -> Self {
        Self {
            access_token: None,
            build_label: None,
            session_id: None,
            language: "en".to_string(),
            reqid: 100000,
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

    pub fn generate_sapisidhash(&self) -> Option<String> {
        use sha1::{Digest, Sha1};
        use std::time::{SystemTime, UNIX_EPOCH};

        let sapisid = self
            .cookies
            .get("__Secure-1PAPISID")
            .or_else(|| self.cookies.get("SAPISID"))?;

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let payload = format!("{timestamp} {sapisid} {WEB_BASE_URL}");
        let mut hasher = Sha1::new();
        hasher.update(payload.as_bytes());
        let hash = hex::encode(hasher.finalize());

        Some(format!("SAPISIDHASH {timestamp}_{hash}"))
    }

    pub fn build_headers(&self) -> Vec<(String, String)> {
        let mut headers = vec![
            (
                "Content-Type".into(),
                "application/x-www-form-urlencoded;charset=UTF-8".into(),
            ),
            ("User-Agent".into(), USER_AGENT.into()),
            ("Origin".into(), WEB_BASE_URL.into()),
            ("Referer".into(), format!("{WEB_BASE_URL}/app")),
            ("X-Same-Domain".into(), "1".into()),
        ];

        if let Some(hash) = self.generate_sapisidhash() {
            headers.push(("Authorization".into(), hash));
        }

        headers.push(("x-goog-authuser".into(), "0".into()));

        headers
    }

    async fn init_session(&mut self) -> Result<()> {
        debug!("Initializing web session - fetching page data");

        let url = format!("{WEB_BASE_URL}/?hl={}", self.session.language);
        let cookie_header = self.build_cookie_header();

        let response = self
            .client
            .get(&url)
            .header("Cookie", &cookie_header)
            .header("User-Agent", USER_AGENT)
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

        let reqid = self.session.reqid;
        self.session.reqid += 100000;

        let reqid_str = reqid.to_string();
        let bl_val = self.session.build_label.clone().unwrap_or_default();

        let mut params = vec![
            ("hl", self.session.language.as_str()),
            ("_reqid", reqid_str.as_str()),
            ("rt", "c"),
        ];

        if !bl_val.is_empty() {
            params.push(("bl", bl_val.as_str()));
        }

        let url = format!("{WEB_BASE_URL}/_/BardChatUi/data/assistant.lamda.BardFrontendService/StreamGenerate");

        let mode = resolve_model_mode(model);
        let inner_req_list = build_inner_req_list(prompt, mode);

        let f_req = json!([
            null,
            serde_json::to_string(&inner_req_list).unwrap_or_default()
        ]);

        let f_req_str = serde_json::to_string(&f_req).unwrap_or_default();

        let mut form_data = vec![
            format!("f.req={}", urlencoding::encode(&f_req_str)),
        ];

        // Include the SNlM0e token as the 'at' parameter if available
        if let Some(ref token) = self.session.access_token {
            form_data.push(format!("at={}", urlencoding::encode(token)));
        }

        let body = form_data.join("&");

        let headers = self.build_headers();

        debug!(model, mode, "Sending request to web frontend");

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

        let reqid = self.session.reqid;
        self.session.reqid += 100000;

        let reqid_str = reqid.to_string();
        let bl_val = self.session.build_label.clone().unwrap_or_default();

        let mut params = vec![
            ("hl", self.session.language.as_str()),
            ("_reqid", reqid_str.as_str()),
            ("rt", "c"),
        ];

        if !bl_val.is_empty() {
            params.push(("bl", bl_val.as_str()));
        }

        let url = format!("{WEB_BASE_URL}/_/BardChatUi/data/assistant.lamda.BardFrontendService/StreamGenerate");

        let mode = resolve_model_mode(model);
        let inner_req_list = build_inner_req_list(prompt, mode);

        let f_req = json!([
            null,
            serde_json::to_string(&inner_req_list).unwrap_or_default()
        ]);

        let f_req_str = serde_json::to_string(&f_req).unwrap_or_default();

        let mut form_data = vec![
            format!("f.req={}", urlencoding::encode(&f_req_str)),
        ];

        if let Some(ref token) = self.session.access_token {
            form_data.push(format!("at={}", urlencoding::encode(token)));
        }

        let body = form_data.join("&");

        let headers = self.build_headers();

        debug!(model, mode, "Sending streaming request to web frontend");

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
}

/// Resolve model name to Google's internal MODE_CATEGORY
fn resolve_model_mode(model: &str) -> u32 {
    let model_lower = model.to_lowercase();
    if model_lower.contains("pro") {
        MODE_PRO
    } else if model_lower.contains("thinking") {
        MODE_THINKING
    } else {
        MODE_FAST
    }
}

/// Build the inner request list (102-slot array) matching Google's current protocol
fn build_inner_req_list(prompt: &str, mode: u32) -> Vec<Value> {
    let mut list: Vec<Value> = vec![Value::Null; 102];

    // [0] - User prompt text
    list[0] = json!([prompt, 0, null, null, null, null, 0]);

    // [1] - Language
    list[1] = json!(["en"]);

    // [2] - Empty metadata
    list[2] = json!(["", "", "", null, null, null, null, null, null, ""]);

    // [6] - Unknown (zeros)
    list[6] = json!([0]);

    // [7] - Unknown flag
    list[7] = json!(1);

    // [10] - Unknown flag
    list[10] = json!(1);

    // [11] - Unknown
    list[11] = json!(0);

    // [17] - Thinking mode (wrapped in double array)
    list[17] = json!([[mode]]);

    // [18] - Unknown
    list[18] = json!(0);

    // [27] - Unknown flag
    list[27] = json!(1);

    // [30] - Unknown
    list[30] = json!([4]);

    // [41] - Unknown
    list[41] = json!([2]);

    // [53] - Unknown
    list[53] = json!(0);

    // [59] - Unique request ID (UUID)
    list[59] = json!(uuid::Uuid::new_v4().to_string());

    // [61] - Empty list
    list[61] = json!([]);

    // [68] - Unknown flag
    list[68] = json!(1);

    // [79] - Model mode ID
    list[79] = json!(mode);

    list
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

        if line.starts_with('[') {
            if let Ok(parsed) = serde_json::from_str::<Value>(line) {
                if let Some(text) = extract_text_from_parsed_response(&parsed) {
                    texts.push(text);
                }
            }
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

    if let Some(start) = body.find("[[") {
        if let Ok(parsed) = serde_json::from_str::<Value>(&body[start..]) {
            return extract_text_from_parsed_response(&parsed);
        }
    }

    None
}

/// Extract text from a parsed JSON response
pub fn extract_text_from_parsed_response(parsed: &Value) -> Option<String> {
    let arr = parsed.as_array()?;

    // Response format: [["wrb.fr", null, "json-payload"]]
    // Each entry in the outer array is [rpc_id, metadata, payload_string]
    for item in arr {
        if let Some(entry) = item.as_array() {
            if entry.len() >= 3 {
                if let Some(rpc_id) = entry[0].as_str() {
                    if rpc_id == "wrb.fr" {
                        // Payload is at position [2]
                        if let Some(json_str) = entry[2].as_str() {
                            if let Ok(inner_parsed) =
                                serde_json::from_str::<Value>(json_str)
                            {
                                if let Some(text) =
                                    extract_text_from_inner_response(&inner_parsed)
                                {
                                    return Some(text);
                                }
                            }
                        }
                    }
                }
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
    if let Some(parts) = arr.get(4) {
        if let Some(parts_arr) = parts.as_array() {
            for part in parts_arr {
                if let Some(part_arr) = part.as_array() {
                    // part[1] is an array of text strings
                    if let Some(text_list) = part_arr.get(1) {
                        if let Some(text_list_arr) = text_list.as_array() {
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
        }
    }

    None
}


