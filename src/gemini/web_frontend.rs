use std::collections::HashMap;

use reqwest::Client;
use serde_json::{json, Value};
use tracing::{debug, error, warn};

use crate::error::{ProxyError, Result};

const WEB_BASE_URL: &str = "https://gemini.google.com";
const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/133.0.0.0 Safari/537.36";

/// Conversation state extracted from a StreamGenerate response and replayed into
/// the next request's inner_req_list[2] (field 3).
///
/// Live browser captures show the 10-element format:
///   [conversationId, responseId, responsePartId, null, null, null, null, null, null, continuationToken]
///
/// - `conversation_id` (`c_...`) is returned in the main response payload at index [1][0].
/// - `response_id` (`r_...`) is returned at main response [1][1] and in the meta entry [1][1].
/// - `response_part_id` (`rc_...`) is the first element of the response part array (main [4][0][0]).
/// - `continuation_token` comes from the small meta response entry at object key `"26"`.
#[derive(Debug, Clone)]
pub struct BrowserAttestationPayload {
    pub inner_req_list: Vec<Value>,
}

#[derive(Debug, Clone, Default)]
pub struct WebConversationState {
    pub conversation_id: String,
    pub response_id: String,
    pub response_part_id: String,
    pub continuation_token: String,
}

impl WebConversationState {
    fn to_inner_meta(&self) -> Value {
        json!([
            self.conversation_id,
            self.response_id,
            self.response_part_id,
            null,
            null,
            null,
            null,
            null,
            null,
            self.continuation_token,
        ])
    }
}



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
    /// Raw category enum value from the web frontend (used in inner_req_list[30]).
    pub category_enum: u64,
}

impl WebModelInfo {
    /// Human-readable OpenAI-style ID derived from the versioned name.
    ///
    /// "3.6 Flash" -> "gemini-3.6-flash", "3.1 Pro" -> "gemini-3.1-pro".
    /// Falls back to a slug of the short title if no versioned name is present.
    pub(crate) fn derive_category_enum(id: &str, title: &str) -> u64 {
        derive_category_enum_inner(id, title)
    }

    pub fn human_id(&self) -> String {
        let source = self
            .versioned_name
            .as_deref()
            .or(Some(self.title.as_str()))
            .unwrap_or("unknown")
            .to_lowercase();
        let parts: Vec<&str> = source.split_whitespace().collect();
        if parts.is_empty() {
            return "gemini-unknown".into();
        }
        let mut normalized = vec!["gemini".to_string()];
        normalized.extend(parts.iter().map(|s| s.to_string()));
        normalized.join("-")
    }
}

#[derive(Debug, Clone)]
pub struct WebSession {
    pub access_token: Option<String>,
    pub build_label: Option<String>,
    pub session_id: Option<String>,
    pub language: String,
    /// Multi-turn conversation state carried across StreamGenerate calls.
    pub conversation_state: Option<WebConversationState>,
    /// Cookies used by this session so the session is self-contained when
    /// restored from the shared `GeminiClient` cache.
    pub cookies: HashMap<String, String>,
    /// Path to the Chrome/Chromium executable used for browser attestation.
    pub browser_path: Option<String>,
}

impl Default for WebSession {
    fn default() -> Self {
        Self::new(None)
    }
}

impl WebSession {
    pub fn new(browser_path: Option<String>) -> Self {
        Self {
            access_token: None,
            build_label: None,
            session_id: None,
            language: "en".to_string(),
            conversation_state: None,
            cookies: HashMap::new(),
            browser_path,
        }
    }
}

/// Placeholder type used internally so the browser-payload parameter has a
/// stable name regardless of whether the `browser-attestation` feature is
/// enabled.  When the feature is off the only variant that exists is
/// `Disabled`; when the feature is on the `Feature` variant carries the real
/// captured payload.
#[derive(Debug, Clone)]
pub enum BrowserPayloadPlaceholder {
    #[cfg(feature = "browser-attestation")]
    Feature(super::browser_attestation::BrowserAttestationPayload),
    #[cfg(not(feature = "browser-attestation"))]
    Disabled,
}

pub struct WebFrontendClient {
    client: Client,
    cookies: HashMap<String, String>,
    session: WebSession,
    #[cfg(feature = "browser-attestation")]
    browser_client: Option<super::browser_attestation::BrowserAttestationClient>,
}

impl WebFrontendClient {
    pub fn new(cookies: HashMap<String, String>, browser_path: Option<String>) -> Result<Self> {
        let client = Client::builder()
            .pool_max_idle_per_host(20)
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| ProxyError::Config(format!("Failed to build HTTP client: {e}")))?;

        #[cfg(feature = "browser-attestation")]
        let browser_client = browser_path.clone().map(super::browser_attestation::BrowserAttestationClient::new);

        let mut session = WebSession::new(browser_path);
        session.cookies = cookies.clone();

        Ok(Self {
            client,
            cookies,
            session,
            #[cfg(feature = "browser-attestation")]
            browser_client,
        })
    }

    /// Reconstruct a client from a cached session.  The browser path is taken
    /// from the session object; the caller should invoke
    /// `refresh_browser_if_needed` if the session was just loaded from cache.
    pub fn from_session(session: WebSession) -> Self {
        let cookies = session.cookies.clone();
        #[cfg(feature = "browser-attestation")]
        let browser_client = session.browser_path.as_ref().map(|p| {
            super::browser_attestation::BrowserAttestationClient::new(p.clone())
        });
        Self {
            client: Client::builder()
                .pool_max_idle_per_host(20)
                .connect_timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("HTTP client must build"),
            cookies,
            session,
            #[cfg(feature = "browser-attestation")]
            browser_client,
        }
    }

    /// Create a client that uses the configured browser path and supplied
    /// cookies.  Kept for callers that do not have a cached session.
    pub fn new_with_browser_path(cookies: HashMap<String, String>, browser_path: Option<String>) -> Result<Self> {
        Self::new_with_session(cookies, WebSession::new(browser_path))
    }

    fn new_with_session(cookies: HashMap<String, String>, session: WebSession) -> Result<Self> {
        let client = Client::builder()
            .pool_max_idle_per_host(20)
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| ProxyError::Config(format!("Failed to build HTTP client: {e}")))?;

        #[cfg(feature = "browser-attestation")]
        let browser_client = session.browser_path.as_ref().map(|p| {
            super::browser_attestation::BrowserAttestationClient::new(p.clone())
        });

        Ok(Self {
            client,
            cookies,
            session,
            #[cfg(feature = "browser-attestation")]
            browser_client,
        })
    }

    /// If the session carries a browser path but no live browser client has
    /// been attached yet, create one.  This is used when restoring a session
    /// from the shared `GeminiClient` cache.
    #[cfg(feature = "browser-attestation")]
    pub async fn refresh_browser_if_needed(&mut self) -> Result<()> {
        if self.browser_client.is_none() && self.session.browser_path.is_some() {
            self.browser_client = self
                .session
                .browser_path
                .as_ref()
                .map(|p| super::browser_attestation::BrowserAttestationClient::new(p.clone()));
        }
        Ok(())
    }

    #[cfg(not(feature = "browser-attestation"))]
    pub async fn refresh_browser_if_needed(&mut self) -> Result<()> {
        Ok(())
    }

    /// Close the browser process if one was started.
    #[cfg(feature = "browser-attestation")]
    pub async fn close_browser(&mut self) {
        if let Some(ref mut browser) = self.browser_client {
            browser.close().await;
        }
    }

    #[cfg(not(feature = "browser-attestation"))]
    pub async fn close_browser(&mut self) {}

    /// Try to obtain a fresh StreamGenerate payload from the headless browser.
    /// Returns `None` when browser support is disabled, not configured, or the
    /// browser interaction fails.
    #[cfg(feature = "browser-attestation")]
    async fn get_browser_payload(
        &mut self,
        prompt: &str,
    ) -> Option<BrowserPayloadPlaceholder> {
        let browser = self.browser_client.as_ref()?;
        let conversation_id = self
            .session
            .conversation_state
            .as_ref()
            .map(|s| s.conversation_id.as_str());
        match browser
            .get_stream_generate_payload(&self.cookies, prompt, conversation_id)
            .await
        {
            Ok(payload) => {
                debug!("Browser attestation payload acquired");
                Some(BrowserPayloadPlaceholder::Feature(payload))
            }
            Err(e) => {
                warn!(error = %e, "Failed to obtain browser attestation payload; falling back");
                None
            }
        }
    }

    #[cfg(not(feature = "browser-attestation"))]
    async fn get_browser_payload(
        &mut self,
        _prompt: &str,
    ) -> Option<BrowserPayloadPlaceholder> {
        None
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

        // SNlM0e ("at" parameter) was removed from the /app HTML around mid-2026.
        // batchexecute requests now succeed with an empty or dummy `at` value,
        // so we keep the extractor only as a defensive fallback and never warn.
        if let Some(token) = extract_snlim0e(&body) {
            debug!("Extracted SNlM0e token");
            self.session.access_token = Some(token);
        } else {
            debug!("SNlM0e token not present in /app HTML; using empty `at`");
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
        mode_id: &str,
        category_enum: u64,
        request: &crate::gemini::types::GenerateContentRequest,
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

        let language = self.session.language.clone();
        let build_label = self.session.build_label.clone();
        let session_id = self.session.session_id.clone();
        let access_token = self.session.access_token.clone();

        let mut params = vec![
            ("hl", language.as_str()),
            ("_reqid", reqid.as_str()),
            ("rt", "c"),
            ("pageId", "none"),
        ];

        if let Some(ref bl) = build_label {
            params.push(("bl", bl.as_str()));
        }
        if let Some(ref sid) = session_id {
            params.push(("f.sid", sid.as_str()));
        }

        let url = format!("{WEB_BASE_URL}/_/BardChatUi/data/assistant.lamda.BardFrontendService/StreamGenerate");

        let prompt = serialize_request_to_prompt(request);
        let browser_payload = self.get_browser_payload(&prompt).await;
        let browser_payload_ref = browser_payload.as_ref().map(|p| match p {
            #[cfg(feature = "browser-attestation")]
            BrowserPayloadPlaceholder::Feature(payload) => payload,
            #[cfg(not(feature = "browser-attestation"))]
            BrowserPayloadPlaceholder::Disabled => unreachable!(),
        });
        let (inner_req_list, used_browser) = build_inner_req_list(
            request,
            category_enum,
            self.session.conversation_state.as_ref(),
            browser_payload_ref,
        );
        let body = build_stream_generate_body(&inner_req_list, access_token.as_deref().unwrap_or(""));

        let headers = self.build_headers();

        debug!(mode_id, category_enum, used_browser, "Sending request to web frontend");

        let mut req = self.client.post(&url)
            .query(&params)
            .body(body);

        for (key, value) in &headers {
            req = req.header(key.as_str(), value.as_str());
        }

        req = req.header("Cookie", self.build_cookie_header());

        let response = req
            .send()
            .await
            .map_err(|e| ProxyError::GeminiApi(format!("Web frontend request failed: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let err_body = response.text().await.unwrap_or_default();
            error!(status = %status, body = %err_body, "Web frontend error");
            // 1096 / attestation errors invalidate cached browser state.
            if is_attestation_error(&err_body) {
                self.session.conversation_state = None;
                if used_browser {
                    warn!("Attestation rejected (1096); clearing conversation state");
                }
            }
            return Err(ProxyError::GeminiApi(format!(
                "HTTP {status}: {err_body}"
            )));
        }

        let body = response
            .text()
            .await
            .map_err(|e| ProxyError::GeminiApi(format!("Failed to read response: {e}")))?;

        debug!(body_len = body.len(), "Response from Gemini");

        // Extract conversation state for the next turn.  Even if this fails we
        // still return the body so the current response can be parsed.
        if let Some(state) = extract_conversation_state(&body) {
            debug!(?state, "extracted conversation state");
            self.session.conversation_state = Some(state);
        }

        // Return the raw response body so callers can parse structured parts
        // (text, thought, functionCall) using parse_response_parts.
        Ok(body)
    }

    pub async fn stream_generate(
        &mut self,
        mode_id: &str,
        category_enum: u64,
        request: &crate::gemini::types::GenerateContentRequest,
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

        let language = self.session.language.clone();
        let build_label = self.session.build_label.clone();
        let session_id = self.session.session_id.clone();
        let access_token = self.session.access_token.clone();

        let mut params = vec![
            ("hl", language.as_str()),
            ("_reqid", reqid.as_str()),
            ("rt", "c"),
            ("pageId", "none"),
        ];

        if let Some(ref bl) = build_label {
            params.push(("bl", bl.as_str()));
        }
        if let Some(ref sid) = session_id {
            params.push(("f.sid", sid.as_str()));
        }

        let url = format!("{WEB_BASE_URL}/_/BardChatUi/data/assistant.lamda.BardFrontendService/StreamGenerate");

        let prompt = serialize_request_to_prompt(request);
        let browser_payload = self.get_browser_payload(&prompt).await;
        let browser_payload_ref = browser_payload.as_ref().map(|p| match p {
            #[cfg(feature = "browser-attestation")]
            BrowserPayloadPlaceholder::Feature(payload) => payload,
            #[cfg(not(feature = "browser-attestation"))]
            BrowserPayloadPlaceholder::Disabled => unreachable!(),
        });
        let (inner_req_list, used_browser) = build_inner_req_list(
            request,
            category_enum,
            self.session.conversation_state.as_ref(),
            browser_payload_ref,
        );
        let body = build_stream_generate_body(&inner_req_list, access_token.as_deref().unwrap_or(""));

        let headers = self.build_headers();

        debug!(mode_id, category_enum, used_browser, "Sending streaming request to web frontend");

        let mut req = self.client.post(&url)
            .query(&params)
            .body(body);

        for (key, value) in &headers {
            req = req.header(key.as_str(), value.as_str());
        }

        req = req.header("Cookie", self.build_cookie_header());

        let response = req
            .send()
            .await
            .map_err(|e| ProxyError::GeminiApi(format!("Web frontend streaming request failed: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            let err_body = response.text().await.unwrap_or_default();
            error!(status = %status, body = %err_body, "Web frontend streaming error");
            if is_attestation_error(&err_body) {
                self.session.conversation_state = None;
                if used_browser {
                    warn!("Attestation rejected (1096); clearing conversation state");
                }
            }
            return Err(ProxyError::GeminiApi(format!(
                "HTTP {status}: {err_body}"
            )));
        }

        // For streaming we cannot read the body here; callers are responsible
        // for consuming the stream.  Conversation state extraction is therefore
        // left to the caller via `extract_conversation_state`.

        Ok(response)
    }

    pub fn session(&self) -> &WebSession {
        &self.session
    }

    pub fn set_session(&mut self, session: WebSession) {
        self.session = session;
    }

    pub async fn close(&mut self) {
        self.close_browser().await;
        self.session = WebSession::default();
    }

    /// Returns true if the request should be considered a new conversation.
    /// Used to decide whether to start a fresh browser page context.
    pub fn is_new_conversation(&self) -> bool {
        self.session.conversation_state.is_none()
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
            ("rpcids", "Fd0Qje"),
            ("source-path", "/app"),
            ("hl", self.session.language.as_str()),
            ("_reqid", reqid.as_str()),
            ("rt", "c"),
            ("pageId", "none"),
            ("authuser", "0"),
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
        let category_enum = mode_arr
            .get(17)
            .and_then(|v| v.as_u64())
            .unwrap_or_else(|| derive_category_enum_inner(&id, &title));
        let category = category_from_enum(category_enum);

        result.push(WebModelInfo {
            id,
            title,
            description,
            versioned_name,
            category,
            category_enum,
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

fn derive_category_enum_inner(id: &str, title: &str) -> u64 {
    let combined = format!("{id} {title}").to_lowercase();
    if combined.contains("lite") {
        6
    } else if combined.contains("thinking") || combined.contains("deep") {
        2
    } else if combined.contains("pro") {
        3
    } else if combined.contains("auto") {
        4
    } else {
        1
    }
}



/// Build the inner request list (97-slot array) matching browser captures.
///
/// Field-to-slot mapping (slot = field number - 1) observed from the live
/// `assistant.lamda.BardFrontendService/StreamGenerate` captures:
/// - slot 0  -> field 1  -> current user text (JN submessage)
/// - slot 1  -> field 2  -> locale ("en")
/// - slot 2  -> field 3  -> conversation metadata (10-element array)
/// - slot 3  -> field 4  -> Web Attestation token (Ijb) - optional/empty
/// - slot 4  -> field 5  -> attestation uuid (Jjb) - optional/empty
/// - slot 17 -> field 18 -> turn counter ([[0]] first turn, [[1]] subsequent)
/// - slot 30 -> field 31 -> mode category enum
/// - slot 33 -> field 34  -> system instruction (AE submessage)
/// - slot 53 -> field 54 -> unknown boolean
/// - slot 59 -> field 60 -> client request uuid
/// - slot 61 -> field 62 -> unknown empty array
/// - slot 66 -> field 67 -> timestamp
/// - slot 68 -> field 69 -> unknown int
/// - slot 79 -> field 80 -> unknown int
/// - slot 91 -> field 92 -> unknown int
/// - slot 96 -> field 97 -> unknown int
///
/// For multi-turn cookie-auth mode, `conversation_state` is replayed into slot
/// 2 and slot 17 becomes [[1]].  Single-turn requests keep slot 2 empty and
/// slot 17 [[0]].
///
/// If `browser_payload` is present, the proxy starts from the browser-captured
/// 97-slot array and overrides slot 0 (prompt), slot 30 (category), and slot 59
/// (request UUID).  This lets the browser supply valid slots 2/3/4/17 and any
/// continuation tokens it generated.  When the browser payload is absent we
/// fall back to the flattened-prompt path (slot 2 empty, slots 3/4 empty).
#[cfg(feature = "browser-attestation")]
fn build_inner_req_list(
    request: &crate::gemini::types::GenerateContentRequest,
    category_enum: u64,
    conversation_state: Option<&WebConversationState>,
    browser_payload: Option<&BrowserAttestationPayload>,
) -> (Vec<Value>, bool) {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let prompt = serialize_request_to_prompt(request);

    // If a browser payload is available, use it as the source of truth for the
    // tricky attestation/state slots.  The caller already logged failure if the
    // browser interaction did not succeed.
    let mut inner = if let Some(payload) = browser_payload {
        let mut slots = payload.inner_req_list.clone();
        // Ensure exactly 97 slots to match the protocol.
        match slots.len().cmp(&97) {
            std::cmp::Ordering::Less => slots.resize(97, Value::Null),
            std::cmp::Ordering::Greater => slots.truncate(97),
            std::cmp::Ordering::Equal => {}
        }
        slots
    } else {
        let mut slots = vec![Value::Null; 97];
        slots[2] = match conversation_state {
            Some(state) => state.to_inner_meta(),
            None => json!(["", "", "", null, null, null, null, null, null, ""]),
        };
        slots[3] = json!("");
        slots[4] = json!("");
        slots[17] = if conversation_state.is_some() {
            json!([[1]])
        } else {
            json!([[0]])
        };
        slots
    };

    inner[0] = json!([prompt, 0, null, null, null, null, 0]);
    inner[1] = json!(["en"]);
    inner[7] = json!(1);
    inner[10] = json!(1);
    inner[11] = json!(0);
    inner[18] = json!(0);
    inner[27] = json!(1);
    inner[30] = json!([category_enum]);
    inner[41] = json!([2]);
    inner[53] = json!(0);
    inner[59] = json!(uuid::Uuid::new_v4().to_string().to_uppercase());
    inner[61] = json!([]);
    inner[68] = json!(1);
    inner[79] = json!(6);
    inner[91] = json!(0);
    inner[96] = json!(0);

    // Live browser captures show slot 6 as [0] and slot 66 as null.  The
    // non-browser fallback path used to hard-code [1] and [ts,0], but when we
    // are replaying a real browser payload we must preserve the browser's own
    // values for these attestation-sensitive slots.  Only override them when
    // no browser payload is available.
    if browser_payload.is_none() {
        inner[6] = json!([1]);
        inner[66] = json!([ts, 0]);
    }

    (inner, browser_payload.is_some())
}

#[cfg(not(feature = "browser-attestation"))]
fn build_inner_req_list(
    request: &crate::gemini::types::GenerateContentRequest,
    category_enum: u64,
    conversation_state: Option<&WebConversationState>,
    _browser_payload: Option<&BrowserAttestationPayload>,
) -> (Vec<Value>, bool) {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let prompt = serialize_request_to_prompt(request);

    let mut inner = vec![Value::Null; 97];
    inner[0] = json!([prompt, 0, null, null, null, null, 0]);
    inner[1] = json!(["en"]);
    inner[2] = match conversation_state {
        Some(state) => state.to_inner_meta(),
        None => json!(["", "", "", null, null, null, null, null, null, ""]),
    };
    inner[3] = json!("");
    inner[4] = json!("");
    inner[6] = json!([1]);
    inner[7] = json!(1);
    inner[10] = json!(1);
    inner[11] = json!(0);
    inner[17] = if conversation_state.is_some() {
        json!([[1]])
    } else {
        json!([[0]])
    };
    inner[18] = json!(0);
    inner[27] = json!(1);
    inner[30] = json!([category_enum]);
    inner[41] = json!([2]);
    inner[53] = json!(0);
    inner[59] = json!(uuid::Uuid::new_v4().to_string().to_uppercase());
    inner[61] = json!([]);
    inner[66] = json!([ts, 0]);
    inner[68] = json!(1);
    inner[79] = json!(6);
    inner[91] = json!(0);
    inner[96] = json!(0);

    (inner, false)
}

/// Build the URL-encoded `f.req` body for StreamGenerate.
fn build_stream_generate_body(inner_req_list: &[Value], at: &str) -> String {
    let inner_json = serde_json::to_string(inner_req_list).unwrap_or_default();
    let f_req = json!([null, inner_json]);
    let f_req_str = serde_json::to_string(&f_req).unwrap_or_default();
    let form_data = [
        format!("f.req={}", urlencoding::encode(&f_req_str)),
        format!("at={}", urlencoding::encode(at)),
    ];
    form_data.join("&")
}

/// Heuristic detection of Google attestation / invalid-state errors.
fn is_attestation_error(body: &str) -> bool {
    body.contains("1096") || body.contains("BardErrorInfo") || body.contains("rs:108")
}

/// Serialize a full GenerateContentRequest into a single prompt string.
///
/// The Gemini web frontend only reads the current turn from slot 0, so we have
/// to embed system instructions, prior turns, tool declarations, and thinking
/// hints into the text itself.  The format uses explicit XML-style markers so
/// the model can distinguish roles and structured data.
fn serialize_request_to_prompt(request: &crate::gemini::types::GenerateContentRequest) -> String {
    use crate::gemini::types::Part;

    let mut sections: Vec<String> = Vec::new();

    // System / developer instruction.
    if let Some(sys) = &request.system_instruction {
        let text = sys
            .parts
            .iter()
            .filter_map(|p| match p {
                Part::Text(t) => Some(t.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        if !text.is_empty() {
            sections.push(format!("<system>\n{}\n</system>", text));
        }
    }

    // Tool declarations.
    if let Some(tools) = &request.tools {
        let decls: Vec<String> = tools
            .iter()
            .flat_map(|t| t.function_declarations.iter())
            .map(|d| {
                let params = d
                    .parameters
                    .as_ref()
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "{}".to_string());
                format!(
                    "<tool name=\"{}\" description=\"{}\">\n{}\n</tool>",
                    d.name,
                    d.description.as_deref().unwrap_or(""),
                    params
                )
            })
            .collect();
        if !decls.is_empty() {
            sections.push(format!(
                "<tools>\n{}\n</tools>",
                decls.join("\n")
            ));
        }
    }

    // Multi-turn history and current user turn.
    if request.contents.len() > 1 {
        // Include history only when there are prior turns.
        let history: Vec<String> = request
            .contents
            .iter()
            .map(|c| {
                let role_marker = match c.role.as_str() {
                    "user" => "user",
                    "model" => "assistant",
                    _ => c.role.as_str(),
                };
                let text = c
                    .parts
                    .iter()
                    .map(|p| match p {
                        Part::Text(t) => t.text.clone(),
                        Part::InlineData(_) => "[inline data]".to_string(),
                        Part::FunctionCall(fc) => format!(
                            "<function_call name=\"{}\">{}</function_call>",
                            fc.function_call.name,
                            fc.function_call.args
                        ),
                        Part::FunctionResponse(fr) => format!(
                            "<function_response name=\"{}\">{}</function_response>",
                            fr.function_response.name,
                            fr.function_response.response
                        ),
                    })
                    .collect::<Vec<_>>()
                    .join("");
                format!("<{}>{}</{}>", role_marker, text, role_marker)
            })
            .collect();
        sections.push(history.join("\n"));
    } else if let Some(first) = request.contents.first() {
        // Single turn: just append the text directly.
        let text = first
            .parts
            .iter()
            .map(|p| match p {
                Part::Text(t) => t.text.clone(),
                Part::InlineData(_) => "[inline data]".to_string(),
                Part::FunctionCall(fc) => format!(
                    "<function_call name=\"{}\">{}</function_call>",
                    fc.function_call.name,
                    fc.function_call.args
                ),
                Part::FunctionResponse(fr) => format!(
                    "<function_response name=\"{}\">{}</function_response>",
                    fr.function_response.name,
                    fr.function_response.response
                ),
            })
            .collect::<Vec<_>>()
            .join("");
        if !text.is_empty() {
            sections.push(text);
        }
    }

    // Thinking hint.
    if let Some(config) = request.generation_config.as_ref()
        && config.thinking_config.is_some()
    {
        sections.push(
            "<thinking>Please show your step-by-step reasoning before answering.</thinking>"
                .to_string(),
        );
    }

    if sections.is_empty() {
        return "Hello".to_string();
    }
    sections.join("\n\n")
}

/// Build the side channel header (12-element array) with mode ID
#[allow(dead_code)]
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

/// Extract SNlM0e token from the `/app` HTML page.
///
/// The token lives inside `window.WIZ_global_data` as `"SNlM0e":"<token>:<timestamp>"`.
/// Both the base part and the `:` + 13-digit timestamp suffix are required by
/// batchexecute; stripping the suffix causes HTTP 400.
fn extract_snlim0e(body: &str) -> Option<String> {
    // Primary pattern used in current HTML.
    if let Some(idx) = body.find("\"SNlM0e\":\"") {
        let start = idx + "\"SNlM0e\":\"".len();
        if let Some(end) = body[start..].find('"') {
            let token = &body[start..start + end];
            if token.len() > 10 {
                return Some(token.to_string());
            }
        }
    }

    // Fallback for older/obfuscated page variants.
    if let Some(idx) = body.find("SNlM0e") {
        let search_area = &body[idx..];
        if let Some(eq_idx) = search_area.find("=\"") {
            let start = eq_idx + 2;
            if let Some(end) = search_area[start..].find('"') {
                let token = &search_area[start..start + end];
                if token.len() > 10 {
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

/// Extract multi-turn conversation state from a raw StreamGenerate response.
///
/// The response is a length-prefixed sequence of WIZ JSON arrays.  We look for
/// two entries:
/// - the main response entry (`["wrb.fr", null, "[<inner array>, ...]"]`) where
///   `inner[1]` holds `[conversation_id, response_id]` and `inner[4]` holds the
///   response parts;
/// - the small meta entry (`["wrb.fr", null, "[null,[null,<r_id>],{\"26\":<token>,...}]"]`) which
///   carries the continuation token at key `"26"`.
pub fn extract_conversation_state(body: &str) -> Option<WebConversationState> {
    let mut main_entry: Option<Value> = None;
    let mut continuation_token: Option<String> = None;

    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || line.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
            continue;
        }
        let entry: Value = serde_json::from_str(line).ok()?;
        let entry_arr = entry.as_array()?;
        let rpc_id = entry_arr.first().and_then(|v| v.as_str())?;
        if rpc_id != "wrb.fr" {
            continue;
        }
        let payload_str = entry_arr.get(2).and_then(|v| v.as_str())?;
        let payload: Value = serde_json::from_str(payload_str).ok()?;
        let payload_arr = payload.as_array()?;

        // Meta entry: 3 elements, third is an object containing "26" token.
        if payload_arr.len() == 3 {
            if let Some(obj) = payload_arr.get(2).and_then(|v| v.as_object())
                && let Some(token) = obj.get("26").and_then(|v| v.as_str())
            {
                continuation_token = Some(token.to_string());
            }
            continue;
        }

        // Main entry: at least 5 elements, inner[4] is array of response parts.
        if payload_arr.len() >= 5 && payload_arr.get(4).and_then(|v| v.as_array()).is_some() {
            main_entry = Some(payload);
        }
    }

    let main = main_entry?;
    let main_arr = main.as_array()?;

    let ids = main_arr.get(1).and_then(|v| v.as_array())?;
    let conversation_id = ids.first().and_then(|v| v.as_str())?.to_string();
    let response_id = ids.get(1).and_then(|v| v.as_str())?.to_string();

    let parts = main_arr.get(4).and_then(|v| v.as_array())?;
    let first_part = parts.first().and_then(|v| v.as_array())?;
    let response_part_id = first_part.first().and_then(|v| v.as_str())?.to_string();

    Some(WebConversationState {
        conversation_id,
        response_id,
        response_part_id,
        continuation_token: continuation_token.unwrap_or_default(),
    })
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


pub fn parse_response_parts(body: &str) -> Result<Vec<crate::gemini::types::ResponsePart>> {
    use crate::gemini::types::{
        FunctionCall, FunctionCallPart, ResponsePart, TextResponsePart, ThoughtPart,
    };

    // Fast path: responses that the original text parser already handles.
    if let Ok(text) = parse_stream_response(body) {
        return Ok(vec![ResponsePart::Text(TextResponsePart { text })]);
    }

    let mut all_parts: Vec<ResponsePart> = Vec::new();

    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let json_start = line.find('[').unwrap_or(0);
        let json_line = &line[json_start..];
        if json_line.is_empty() {
            continue;
        }
        // Length-prefixed chunked responses from Gemini may have a numeric
        // prefix before the JSON array and an extra trailing bracket. Find the
        // first balanced top-level JSON array instead of parsing the rest.
        let mut depth: i32 = 0;
        let mut outer_start: Option<usize> = None;
        let mut outer_end: Option<usize> = None;
        for (i, c) in json_line.char_indices() {
            if c == '[' {
                if depth == 0 {
                    outer_start = Some(i);
                }
                depth += 1;
            } else if c == ']' {
                depth -= 1;
                if depth == 0 && outer_start.is_some() {
                    outer_end = Some(i + 1);
                    break;
                }
            }
        }
        let balanced = match (outer_start, outer_end) {
            (Some(s), Some(e)) => &json_line[s..e],
            _ => json_line,
        };
        let parsed: Value = match serde_json::from_str(balanced) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let arr = match parsed.as_array() {
            Some(a) => a,
            None => continue,
        };
        for item in arr {
            let entry = match item.as_array() {
                Some(e) if e.len() >= 3 => e,
                _ => continue,
            };
            let rpc_id = entry[0].as_str().unwrap_or("");
            if rpc_id != "wrb.fr" {
                continue;
            }
            let json_str = match entry[2].as_str() {
                Some(s) => s,
                None => continue,
            };
            let inner_parsed: Value = match serde_json::from_str(json_str) {
                Ok(v) => v,
                Err(_) => continue,
            };
            // StreamGenerate payload shape: the JSON string inside the
            // wrb.fr entry is either the inner array directly (live 2026:
            // 48 elements, inner[4] holds parts) or wrapped in a one-element
            // array (some test fixtures and older responses). We try both.
            let inner_arr = match inner_parsed.as_array() {
                Some(a) => a,
                None => continue,
            };
            let parts_json = if let Some(parts) = inner_arr.get(4).and_then(|v| v.as_array()) {
                parts
            } else if let Some(first) = inner_arr.first().and_then(|v| v.as_array()) {
                match first.get(4).and_then(|v| v.as_array()) {
                    Some(parts) => parts,
                    None => continue,
                }
            } else {
                continue;
            };
            for part in parts_json {
                let part_arr = match part.as_array() {
                    Some(a) => a,
                    None => continue,
                };
                let content_list = match part_arr.get(1).and_then(|v| v.as_array()) {
                    Some(a) => a,
                    None => continue,
                };
                let mut current_text: Option<String> = None;
                for content in content_list {
                    if let Some(s) = content.as_str() {
                        if s.is_empty()
                            || (s.starts_with("r_") && s.len() > 2)
                            || (s.starts_with("c_") && s.len() > 2)
                        {
                            continue;
                        }
                        current_text = Some(match current_text {
                            Some(prev) => format!("{}{}", prev, s),
                            None => s.to_string(),
                        });
                        continue;
                    }
                    if let Some(prev) = current_text.take() {
                        all_parts.push(ResponsePart::Text(TextResponsePart { text: prev }));
                    }
                    if let Some(obj) = content.as_object() {
                        if let Some(fc) = obj.get("functionCall").and_then(|v| v.as_object()) {
                            if let Some(name) = fc.get("name").and_then(|v| v.as_str()) {
                                let args = fc.get("args").cloned().unwrap_or_else(|| json!({}));
                                all_parts.push(ResponsePart::FunctionCall(FunctionCallPart {
                                    function_call: FunctionCall { name: name.to_string(), args },
                                }));
                            }
                            continue;
                        }
                        if let Some(text) = obj.get("text").and_then(|v| v.as_str()) {
                            let is_thought =
                                obj.get("thought").and_then(|v| v.as_bool()).unwrap_or(false);
                            all_parts.push(ResponsePart::Thought(ThoughtPart {
                                thought: is_thought,
                                text: text.to_string(),
                            }));
                            continue;
                        }
                    }
                }
                if let Some(prev) = current_text.take() {
                    all_parts.push(ResponsePart::Text(TextResponsePart { text: prev }));
                }
            }
        }
    }

    if all_parts.is_empty() {
        Err(ProxyError::GeminiApi(
            "Could not parse response from Gemini web frontend".into(),
        ))
    } else {
        Ok(all_parts)
    }
}

#[cfg(test)]
mod parse_response_parts_tests {
    use super::parse_response_parts;
    use crate::gemini::types::{ResponsePart, TextResponsePart};

    #[test]
    fn parses_simple_text_response() {
        let body = r#"[["wrb.fr", null, "[[null, null, null, null, [[\"rc_123\", [\"Hello, world!\"]]]]]"]]"#;
        let parts = parse_response_parts(body).unwrap();
        assert_eq!(parts.len(), 1);
        match &parts[0] {
            ResponsePart::Text(TextResponsePart { text }) => assert_eq!(text, "Hello, world!"),
            _ => panic!("expected text part"),
        }
    }

    #[test]
    fn parses_function_call_response() {
        let body = r#"[["wrb.fr", null, "[[null, null, null, null, [[\"rc_1\", [{\"functionCall\": {\"name\": \"get_weather\", \"args\": {\"city\": \"Paris\"}}}]]]]]"]]"#;
        let parts = parse_response_parts(body).unwrap();
        assert_eq!(parts.len(), 1);
        match &parts[0] {
            ResponsePart::FunctionCall(fc) => {
                assert_eq!(fc.function_call.name, "get_weather");
                assert_eq!(fc.function_call.args["city"], "Paris");
            }
            _ => panic!("expected function call part"),
        }
    }

    #[test]
    fn parses_thought_response() {
        let body = r#"[["wrb.fr", null, "[[null, null, null, null, [[\"rc_1\", [{\"text\": \"I should think step by step\", \"thought\": true}]]]]]"]]"#;
        let parts = parse_response_parts(body).unwrap();
        assert_eq!(parts.len(), 1);
        match &parts[0] {
            ResponsePart::Thought(t) => {
                assert!(t.thought);
                assert_eq!(t.text, "I should think step by step");
            }
            _ => panic!("expected thought part"),
        }
    }

    #[test]
    fn concatenates_consecutive_text_strings() {
        let body = r#"[["wrb.fr", null, "[[null, null, null, null, [[\"rc_1\", [\"Hello \", \"world!\"]]]]]"]]"#;
        let parts = parse_response_parts(body).unwrap();
        assert_eq!(parts.len(), 1);
        match &parts[0] {
            ResponsePart::Text(TextResponsePart { text }) => assert_eq!(text, "Hello world!"),
            _ => panic!("expected text part"),
        }
    }
}
