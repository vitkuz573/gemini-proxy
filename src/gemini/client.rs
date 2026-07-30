use std::sync::Arc;
use reqwest::Client;
use tokio::sync::Mutex;
use tracing::{debug, error};

use crate::error::{ProxyError, Result};

use super::auth::GeminiAuth;
use super::types::{
    GenerateContentRequest, GenerateContentResponse, ModelListResponse,
};

#[derive(Clone)]
pub struct GeminiClient {
    client: Client,
    auth: GeminiAuth,
    base_url: String,
    web_session: Arc<Mutex<Option<super::web_frontend::WebSession>>>,
}

impl GeminiClient {
    pub fn new(auth: GeminiAuth, base_url: String) -> Result<Self> {
        let client = Client::builder()
            .pool_max_idle_per_host(20)
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(300))
            .build()
            .map_err(|e| ProxyError::Config(format!("Failed to build HTTP client: {e}")))?;

        Ok(GeminiClient {
            client,
            auth,
            base_url,
            web_session: Arc::new(Mutex::new(None)),
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

        let mut web_client = WebFrontendClient::new(self.auth.cookies.clone())?;
        {
            let session_guard = self.web_session.lock().await;
            if let Some(ref session) = *session_guard {
                web_client.set_session(session.clone());
            }
        }

        let web_models = web_client.list_models().await?;

        {
            let mut session_guard = self.web_session.lock().await;
            *session_guard = Some(web_client.session().clone());
        }

        let models = web_models
            .into_iter()
            .map(|m| super::types::ModelInfo {
                name: format!("models/{}", m.id),
                display_name: m
                    .versioned_name
                    .unwrap_or_else(|| m.title.clone()),
                input_token_limit: 1048576,
                output_token_limit: 65536,
            })
            .collect();

        Ok(ModelListResponse { models })
    }

    async fn generate_content_via_web(
        &self,
        model: &str,
        request: &GenerateContentRequest,
    ) -> Result<GenerateContentResponse> {
        use super::web_frontend::WebFrontendClient;

        let mut web_client = WebFrontendClient::new(self.auth.cookies.clone())?;
        {
            let session_guard = self.web_session.lock().await;
            if let Some(ref session) = *session_guard {
                web_client.set_session(session.clone());
            }
        }

        let prompt = extract_prompt_text(request);

        let response_text = web_client.generate_content(model, &prompt).await?;

        {
            let mut session_guard = self.web_session.lock().await;
            *session_guard = Some(web_client.session().clone());
        }

        // Convert web frontend response to standard API response format
        Ok(GenerateContentResponse {
            candidates: vec![super::types::Candidate {
                content: Some(super::types::ResponseContent {
                    role: "model".to_string(),
                    parts: vec![super::types::ResponsePart::Text(super::types::TextResponsePart {
                        text: response_text,
                    })],
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

        let mut web_client = WebFrontendClient::new(self.auth.cookies.clone())?;
        {
            let session_guard = self.web_session.lock().await;
            if let Some(ref session) = *session_guard {
                web_client.set_session(session.clone());
            }
        }
        let prompt = extract_prompt_text(request);
        let response = web_client.stream_generate(model, &prompt).await?;
        {
            let mut session_guard = self.web_session.lock().await;
            *session_guard = Some(web_client.session().clone());
        }
        Ok(response)
    }

    async fn generate_content_via_api(
        &self,
        model: &str,
        request: &GenerateContentRequest,
    ) -> Result<GenerateContentResponse> {
        let url = self.build_url(&format!("/v1beta/models/{model}:generateContent"));
        let mut req = self.client.post(&url).json(request);
        req = self.apply_api_auth(req);

        debug!(model, "sending generateContent request");

        let response = req.send().await?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            error!(status = %status, body = %body, "Gemini API error");
            return Err(ProxyError::GeminiApi(format!(
                "HTTP {status}: {body}"
            )));
        }

        let response: GenerateContentResponse = response.json().await?;
        Ok(response)
    }

    async fn stream_content_via_api(
        &self,
        model: &str,
        request: &GenerateContentRequest,
    ) -> Result<reqwest::Response> {
        let url = self.build_url(&format!(
            "/v1beta/models/{model}:streamGenerateContent?alt=sse"
        ));
        let mut req = self.client.post(&url).json(request);
        req = self.apply_api_auth(req);

        debug!(model, "sending streamGenerateContent request");

        let response = req.send().await?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            error!(status = %status, body = %body, "Gemini streaming error");
            return Err(ProxyError::GeminiApi(format!(
                "HTTP {status}: {body}"
            )));
        }

        Ok(response)
    }

    fn build_url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    fn apply_api_auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(ref api_key) = self.auth.api_key {
            req.header("X-Goog-Api-Key", api_key)
        } else {
            req
        }
    }
}

fn extract_prompt_text(request: &GenerateContentRequest) -> String {
    let mut text_parts = Vec::new();

    for content in &request.contents {
        for part in &content.parts {
            match part {
                super::types::Part::Text(text_part) => {
                    text_parts.push(text_part.text.clone());
                }
                _ => {
                    // For non-text parts, we'll include a placeholder
                    text_parts.push("[non-text content]".to_string());
                }
            }
        }
    }

    if text_parts.is_empty() {
        return "Hello".to_string();
    }

    text_parts.join("\n")
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
        GeminiClient::new(auth, "https://generativelanguage.googleapis.com".into()).unwrap()
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
        let url = client.build_url("/v1beta/models/gemini-2.5-flash:streamGenerateContent?alt=sse");
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
