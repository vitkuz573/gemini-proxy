use std::sync::Arc;
use reqwest::Client;
use tokio::sync::Mutex;
use tracing::{debug, error};

use crate::error::{ProxyError, Result};

use super::auth::GeminiAuth;
use super::types::{
    GenerateContentRequest, GenerateContentResponse, ModelInfo, ModelListResponse,
};
use super::web_frontend::WebModelInfo;

#[derive(Clone)]
pub struct GeminiClient {
    client: Client,
    auth: GeminiAuth,
    base_url: String,
    max_retries: u32,
    web_session: Arc<Mutex<Option<super::web_frontend::WebSession>>>,
    web_models: Arc<Mutex<Option<Vec<WebModelInfo>>>>,
}

impl GeminiClient {
    pub fn new(auth: GeminiAuth, base_url: String) -> Result<Self> {
        Self::new_with_browser_path(auth, base_url, None)
    }

    pub fn new_with_browser_path(
        auth: GeminiAuth,
        base_url: String,
        browser_path: Option<String>,
    ) -> Result<Self> {
        let client = Client::builder()
            .pool_max_idle_per_host(20)
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(300))
            .build()
            .map_err(|e| ProxyError::Config(format!("Failed to build HTTP client: {e}")))?;

        let web_session = if auth.is_cookie_auth() {
            Some(super::web_frontend::WebSession::new(browser_path))
        } else {
            None
        };

        // Load max_retries from config via the auth cookie map if present,
        // otherwise fall back to a sane default.
        let max_retries = std::env::var("MAX_RETRIES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2);

        Ok(GeminiClient {
            client,
            auth,
            base_url,
            max_retries,
            web_session: Arc::new(Mutex::new(web_session)),
            web_models: Arc::new(Mutex::new(None)),
        })
    }

    pub async fn generate_content(
        &self,
        model: &str,
        request: &GenerateContentRequest,
    ) -> Result<GenerateContentResponse> {
        if self.auth.is_cookie_auth() {
            self.generate_content_via_web(model, request).await
        } else {
            self.generate_content_via_api(model, request).await
        }
    }

    pub async fn stream_content(
        &self,
        model: &str,
        request: &GenerateContentRequest,
    ) -> Result<reqwest::Response> {
        if self.auth.is_cookie_auth() {
            self.stream_content_via_web(model, request).await
        } else {
            self.stream_content_via_api(model, request).await
        }
    }

    pub async fn list_models(&self) -> Result<ModelListResponse> {
        if self.auth.is_cookie_auth() {
            return self.list_models_via_web().await;
        }

        let url = self.build_url("/v1beta/models");
        let req = self.client.get(&url);

        debug!("sending listModels request");

        let response = req.send().await?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            error!(status = %status, body = %body, "Gemini list models error");
            return Err(ProxyError::GeminiApi(format!(
                "HTTP {status}: {body}"
            )));
        }

        let response: ModelListResponse = response.json().await?;
        Ok(response)
    }

    async fn list_models_via_web(&self) -> Result<ModelListResponse> {
        use super::web_frontend::WebFrontendClient;

        let mut web_client = {
            let session_guard = self.web_session.lock().await;
            if let Some(ref session) = *session_guard {
                let mut client = WebFrontendClient::from_session(session.clone());
                client.refresh_browser_if_needed().await?;
                client
            } else {
                WebFrontendClient::new_with_browser_path(self.auth.cookies.clone(), None)?
            }
        };

        let web_models = web_client.list_models().await?;

        {
            let mut session_guard = self.web_session.lock().await;
            *session_guard = Some(web_client.session().clone());
        }

        // Build OpenAI-style human-readable IDs and keep the raw list cached so
        // that chat requests can resolve `gemini-X.Y-category` back to a hex ID.
        let models: Vec<ModelInfo> = web_models
            .iter()
            .map(|m| super::types::ModelInfo {
                name: m.human_id(),
                display_name: m
                    .versioned_name
                    .clone()
                    .unwrap_or_else(|| m.title.clone()),
                input_token_limit: 1048576,
                output_token_limit: 65536,
                root: Some(format!("models/{}", m.id)),
            })
            .collect();

        {
            let mut cache = self.web_models.lock().await;
            *cache = Some(web_models);
        }

        Ok(ModelListResponse { models })
    }

    /// Resolve a user-supplied model identifier to the hex mode ID and category
    /// required by the web frontend.
    ///
    /// Accepted forms:
    /// - `models/<hex>` — returned unchanged; category is inferred from cached models
    ///   or heuristics.
    /// - `gemini-<version>-<category>` — matched against the most recent `/v1/models`
    ///   list; if the cache is empty it is populated first.
    pub async fn resolve_web_model(&self, model: &str) -> Result<(String, u64)> {
        let stripped = model.strip_prefix("models/").unwrap_or(model);

        // Direct hex ID.
        if stripped.len() == 16 && stripped.chars().all(|c| c.is_ascii_hexdigit()) {
            let cat = self
                .web_models
                .lock()
                .await
                .as_ref()
                .and_then(|models| models.iter().find(|m| m.id == stripped))
                .map(|m| m.category_enum)
                .unwrap_or_else(|| super::web_frontend::WebModelInfo::derive_category_enum(stripped, ""));
            return Ok((stripped.to_string(), cat));
        }

        // Ensure the model cache is populated.
        {
            let cache = self.web_models.lock().await;
            if let Some(ref models) = *cache
                && let Some(m) = models.iter().find(|m| m.human_id() == model)
            {
                return Ok((m.id.clone(), m.category_enum));
            }
        }

        debug!(model, "model not in cache, refreshing web model list");
        self.list_models_via_web().await?;

        {
            let cache = self.web_models.lock().await;
            if let Some(ref models) = *cache
                && let Some(m) = models.iter().find(|m| m.human_id() == model)
            {
                return Ok((m.id.clone(), m.category_enum));
            }
        }

        Err(ProxyError::BadRequest(format!(
            "Unknown model '{model}'. Call /v1/models to list available models."
        )))
    }

    async fn generate_content_via_web(
        &self,
        model: &str,
        request: &GenerateContentRequest,
    ) -> Result<GenerateContentResponse> {
        use super::web_frontend::WebFrontendClient;

        let (mode_id, category_enum) = self.resolve_web_model(model).await?;

        let mut web_client = {
            let session_guard = self.web_session.lock().await;
            if let Some(ref session) = *session_guard {
                let mut client = WebFrontendClient::from_session(session.clone());
                client.refresh_browser_if_needed().await?;
                client
            } else {
                WebFrontendClient::new_with_browser_path(self.auth.cookies.clone(), None)?
            }
        };

        let response_text = web_client
            .generate_content(&mode_id, category_enum, request, self.max_retries)
            .await?;

        {
            let mut session_guard = self.web_session.lock().await;
            *session_guard = Some(web_client.session().clone());
        }

        // Parse the web response into typed parts so that thoughts and function
        // calls survive the conversion to OpenAI/Anthropic formats.
        let parts = super::web_frontend::parse_response_parts(&response_text)?;

        Ok(GenerateContentResponse {
            candidates: vec![super::types::Candidate {
                content: Some(super::types::ResponseContent {
                    role: "model".to_string(),
                    parts,
                }),
                finish_reason: Some("STOP".to_string()),
                index: 0,
                safety_ratings: None,
            }],
            usage_metadata: None,
            model_version: Some(model.to_string()),
            response_id: None,
        })
    }

    async fn stream_content_via_web(
        &self,
        model: &str,
        request: &GenerateContentRequest,
    ) -> Result<reqwest::Response> {
        use super::web_frontend::WebFrontendClient;

        let (mode_id, category_enum) = self.resolve_web_model(model).await?;

        let mut web_client = {
            let session_guard = self.web_session.lock().await;
            if let Some(ref session) = *session_guard {
                let mut client = WebFrontendClient::from_session(session.clone());
                client.refresh_browser_if_needed().await?;
                client
            } else {
                WebFrontendClient::new_with_browser_path(self.auth.cookies.clone(), None)?
            }
        };
        let response = web_client
            .stream_generate(&mode_id, category_enum, request, self.max_retries)
            .await?;
        {
            let mut session_guard = self.web_session.lock().await;
            *session_guard = Some(web_client.session().clone());
        }
        Ok(response)
    }

    /// Update the shared web session conversation state from a fully consumed
    /// response body.  Called by streaming endpoints after the upstream body has
    /// been read to completion.
    pub async fn update_conversation_state_from_body(&self, body: &str) {
        match super::web_frontend::extract_conversation_state(body) {
            Ok(state) => {
                let mut guard = self.web_session.lock().await;
                if let Some(ref mut session) = *guard {
                    debug!(?state, "Updating conversation state from streamed response");
                    session.conversation_state = Some(state);
                }
            }
            Err(e) => {
                debug!(error = %e, "failed to extract conversation state from streamed response");
            }
        }
    }

    async fn generate_content_via_api(
        &self,
        model: &str,
        request: &GenerateContentRequest,
    ) -> Result<GenerateContentResponse> {
        let client = self.client.clone();
        let url = self.build_url(&format!("/v1beta/models/{model}:generateContent"));
        let request = request.clone();
        let api_key = self.auth.api_key.clone();

        debug!(model, "sending generateContent request");

        let response = crate::retry::send_retryable_request(self.max_retries, move || {
            let client = client.clone();
            let url = url.clone();
            let request = request.clone();
            let api_key = api_key.clone();
            async move {
                let mut req = client.post(&url).json(&request);
                if let Some(ref key) = api_key {
                    req = req.header("X-Goog-Api-Key", key);
                }
                req.send().await
            }
        })
        .await?;

        let response: GenerateContentResponse = response.json().await?;
        Ok(response)
    }

    async fn stream_content_via_api(
        &self,
        model: &str,
        request: &GenerateContentRequest,
    ) -> Result<reqwest::Response> {
        let client = self.client.clone();
        let url = self.build_url(&format!(
            "/v1beta/models/{model}:streamGenerateContent?alt=sse"
        ));
        let request = request.clone();
        let api_key = self.auth.api_key.clone();
        let max_retries = self.max_retries;

        debug!(model, "sending streamGenerateContent request");

        let response = crate::retry::send_retryable_request(max_retries, move || {
            let client = client.clone();
            let url = url.clone();
            let request = request.clone();
            let api_key = api_key.clone();
            async move {
                let mut req = client.post(&url).json(&request);
                if let Some(ref key) = api_key {
                    req = req.header("X-Goog-Api-Key", key);
                }
                req.send().await
            }
        })
        .await?;

        Ok(response)
    }

    fn build_url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    #[cfg(test)]
    fn apply_api_auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(ref api_key) = self.auth.api_key {
            req.header("X-Goog-Api-Key", api_key)
        } else {
            req
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_api_key_client() -> GeminiClient {
        let auth = GeminiAuth {
            cookies: HashMap::new(),
            api_key: Some("test_api_key".into()),
        };
        GeminiClient::new(auth, "https://generativelanguage.googleapis.com".into()).unwrap()
    }

    fn make_cookie_client() -> GeminiClient {
        let mut cookies = HashMap::new();
        cookies.insert("__Secure-1PAPISID".into(), "test_papisid".into());
        cookies.insert("__Secure-1PSID".into(), "test_psid".into());
        let auth = GeminiAuth {
            cookies,
            api_key: None,
        };
        GeminiClient::new_with_browser_path(
            auth,
            "https://generativelanguage.googleapis.com".into(),
            None,
        )
        .unwrap()
    }

    #[test]
    fn test_build_url_with_api_key() {
        let client = make_api_key_client();
        let url = client.build_url("/v1beta/models");
        assert_eq!(
            url,
            "https://generativelanguage.googleapis.com/v1beta/models"
        );
    }

    #[test]
    fn test_build_url_with_api_key_existing_query() {
        let client = make_api_key_client();
        let url = client.build_url("/v1beta/models/current-model:streamGenerateContent?alt=sse");
        assert!(url.contains("alt=sse"));
        assert!(!url.contains("key="));
    }

    #[test]
    fn test_build_url_without_api_key() {
        let client = make_cookie_client();
        let url = client.build_url("/v1beta/models");
        assert_eq!(url, "https://generativelanguage.googleapis.com/v1beta/models");
    }

    #[test]
    fn test_auth_mode_detection() {
        let api_client = make_api_key_client();
        assert!(api_client.auth.is_api_key_auth());
        assert!(!api_client.auth.is_cookie_auth());

        let cookie_client = make_cookie_client();
        assert!(!cookie_client.auth.is_api_key_auth());
        assert!(cookie_client.auth.is_cookie_auth());
    }
}
