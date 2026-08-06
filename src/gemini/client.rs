use std::collections::HashMap;

use gemini_sdk::{
    ChatMessage, ChatResponse as SdkChatResponse, ContentPart, GeminiClient as SdkClient,
    ImageSource, ModelCategory, ModelInfo as SdkModelInfo, ThinkingLevel,
};

use crate::config::Config;
use crate::error::{ProxyError, Result};

const DEFAULT_TIMEOUT_SECS: u64 = 300;

/// Thin wrapper around the local `gemini-sdk` client.
///
/// The proxy previously implemented its own Gemini/Bard web frontend client.
/// This wrapper keeps the proxy-facing API stable while delegating all
/// transport logic to the SDK.
#[derive(Clone)]
pub struct GeminiClient {
    inner: SdkClient,
    #[allow(dead_code)]
    max_retries: u32,
}

impl GeminiClient {
    /// Build a client from the proxy configuration.
    ///
    /// Cookie auth is required; API-key auth was removed in favor of the SDK's
    /// web-frontend flow.
    pub fn from_config(config: &Config) -> Result<Self> {
        let cookie_header = build_cookie_header(&config.gemini_cookies);
        if cookie_header.is_empty() {
            return Err(ProxyError::Config(
                "GEMINI_COOKIES is required (API-key auth is no longer supported)".into(),
            ));
        }

        Self::from_cookie_header(&cookie_header, config.max_retries)
    }

    /// Build a client from a raw cookie header string.
    pub fn from_cookie_header(cookie_header: &str, max_retries: u32) -> Result<Self> {
        let inner = SdkClient::from_cookie_header(cookie_header)
            .map_err(|e| ProxyError::Config(format!("failed to create Gemini SDK client: {e}")))?;

        let inner = inner.with_timeout(std::time::Duration::from_secs(DEFAULT_TIMEOUT_SECS));

        Ok(Self { inner, max_retries })
    }

    /// List models available to the signed-in account.
    pub async fn list_models(&self) -> Result<ModelListResponse> {
        let sdk_models = self.inner.list_models().await.map_err(map_sdk_error)?;
        Ok(ModelListResponse {
            models: sdk_models.into_iter().map(map_sdk_model_info).collect(),
        })
    }

    /// Send a non-streaming chat request.
    pub async fn generate_content(
        &self,
        model: &str,
        request: &ChatRequest,
    ) -> Result<ChatResponse> {
        let (category, config) = build_category_and_config(model, request);
        let message = build_sdk_message(request)?;
        let response = self
            .inner
            .chat()
            .with_category(category)
            .with_config(config)
            .send_message_with_content(message)
            .await
            .map_err(map_sdk_error)?;
        let mut response = map_sdk_response(response);
        response.finish_reason = Some("STOP".into());
        Ok(response)
    }

    /// Start a streaming chat request and return the raw upstream response.
    ///
    /// Callers are responsible for parsing the WIZ frames. Conversation state
    /// is updated by the SDK after each turn when `generate()` is used; the
    /// proxy's streaming paths update state from the consumed body separately.
    pub async fn stream_content(
        &self,
        model: &str,
        request: &ChatRequest,
    ) -> Result<reqwest::Response> {
        let (category, config) = build_category_and_config(model, request);
        let message = build_sdk_message(request)?;
        self.inner
            .stream_generate(&message, category, Some(config))
            .await
            .map_err(map_sdk_error)
    }

    /// Update the shared conversation state from a fully consumed response body.
    ///
    /// This is a no-op wrapper kept for API compatibility with the old client.
    /// The SDK already extracts conversation state internally after each
    /// non-streaming turn.
    pub async fn update_conversation_state_from_body(&self, _body: &str) {
        // Streaming responses currently do not carry usable continuation state
        // in a form that the SDK's public API accepts. Multi-turn streaming is
        // therefore not supported; non-streaming multi-turn works through the
        // SDK's internal session.
    }
}

/// Proxy-internal request type that mirrors the SDK's expected shape.
#[derive(Debug, Clone, Default)]
pub struct ChatRequest {
    pub contents: Vec<ChatContent>,
    pub system_instruction: Option<String>,
    pub generation_config: Option<GenerationConfig>,
}

impl From<&crate::gemini::types::GenerateContentRequest> for ChatRequest {
    fn from(req: &crate::gemini::types::GenerateContentRequest) -> Self {
        Self {
            contents: req
                .contents
                .iter()
                .map(|c| ChatContent {
                    role: c.role.clone(),
                    parts: c
                        .parts
                        .iter()
                        .filter_map(|p| match p {
                            crate::gemini::types::Part::Text(t) => {
                                Some(ChatPart::Text(t.text.clone()))
                            }
                            crate::gemini::types::Part::InlineData(d) => {
                                Some(ChatPart::InlineData {
                                    mime_type: d.inline_data.mime_type.clone(),
                                    data: d.inline_data.data.clone(),
                                })
                            }
                            _ => None,
                        })
                        .collect(),
                })
                .collect(),
            system_instruction: req.system_instruction.as_ref().and_then(|p| {
                p.parts.iter().find_map(|part| match part {
                    crate::gemini::types::Part::Text(t) => Some(t.text.clone()),
                    _ => None,
                })
            }),
            generation_config: req.generation_config.as_ref().map(|gc| GenerationConfig {
                temperature: gc.temperature,
                top_p: gc.top_p,
                top_k: gc.top_k,
                max_output_tokens: gc.max_output_tokens,
                stop_sequences: gc.stop_sequences.clone(),
                candidate_count: gc.candidate_count,
                presence_penalty: gc.presence_penalty,
                frequency_penalty: gc.frequency_penalty,
                response_mime_type: gc.response_mime_type.clone(),
                response_schema: gc.response_schema.clone(),
                thinking_budget: gc
                    .thinking_config
                    .as_ref()
                    .and_then(|tc| tc.thinking_budget),
                seed: gc.seed,
            }),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChatContent {
    pub role: String,
    pub parts: Vec<ChatPart>,
}

#[derive(Debug, Clone)]
pub enum ChatPart {
    Text(String),
    InlineData { mime_type: String, data: String },
    FunctionCall { name: String, args: serde_json::Value },
    FunctionResponse { name: String, response: serde_json::Value },
}

#[derive(Debug, Clone, Default)]
pub struct GenerationConfig {
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub top_k: Option<u32>,
    pub max_output_tokens: Option<u32>,
    pub stop_sequences: Option<Vec<String>>,
    pub candidate_count: Option<u32>,
    pub presence_penalty: Option<f64>,
    pub frequency_penalty: Option<f64>,
    pub response_mime_type: Option<String>,
    pub response_schema: Option<serde_json::Value>,
    pub thinking_budget: Option<u32>,
    pub seed: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct ChatResponse {
    pub text: String,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ModelListResponse {
    pub models: Vec<ModelInfo>,
}

#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub name: String,
    pub display_name: String,
    pub input_token_limit: u32,
    pub output_token_limit: u32,
    pub root: Option<String>,
}

fn build_cookie_header(cookies: &HashMap<String, String>) -> String {
    cookies
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("; ")
}

fn map_sdk_error(e: gemini_sdk::Error) -> ProxyError {
    let message = e.to_string();
    let code = gemini_sdk::extract_bard_error_code(&message);
    let mapped = match code {
        Some(1096) => {
            "Gemini rejected the turn attestation (1096). If this is an image request, browser attestation is required but unavailable or failed.".to_string()
        }
        Some(1100) => {
            "Gemini rejected the image/file attestation (1100). A real browser must generate valid slot 3/4 tokens for image requests.".to_string()
        }
        Some(1155) => {
            "Gemini session/parameter mismatch (1155). Try a fresh conversation or enable browser attestation.".to_string()
        }
        Some(other) => format!("Gemini returned BardErrorInfo [{other}]"),
        None => message,
    };
    ProxyError::GeminiApi(mapped)
}

fn map_sdk_model_info(m: SdkModelInfo) -> ModelInfo {
    ModelInfo {
        name: human_id(&m),
        display_name: m.display_name(),
        input_token_limit: 1048576,
        output_token_limit: 65536,
        root: Some(format!("models/{}", m.id)),
    }
}

fn human_id(m: &SdkModelInfo) -> String {
    let name = m
        .versioned_name
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(&m.title);
    let source = name.to_lowercase();
    let mut normalized = vec!["gemini".to_string()];
    for p in source.split_whitespace() {
        if p == "gemini" {
            continue;
        }
        normalized.push(p.to_string());
    }
    normalized.join("-")
}

fn build_category_and_config(
    model: &str,
    request: &ChatRequest,
) -> (ModelCategory, gemini_sdk::GenerationConfig) {
    let category = resolve_model_category(model);
    let mut config = gemini_sdk::GenerationConfig::default();
    if let Some(gc) = &request.generation_config {
        config.temperature = gc.temperature.map(|v| v as f32);
        config.top_p = gc.top_p.map(|v| v as f32);
        config.top_k = gc.top_k;
        config.max_output_tokens = gc.max_output_tokens;
        config.stop_sequences.clone_from(&gc.stop_sequences);
        if gc.thinking_budget.is_some() {
            config.thinking_level = Some(ThinkingLevel::Extended);
        }
    }
    (category, config)
}

fn resolve_model_category(model: &str) -> ModelCategory {
    let lowered = model.to_lowercase();
    if lowered.contains("pro") {
        ModelCategory::Pro
    } else if lowered.contains("thinking") || lowered.contains("deep") {
        ModelCategory::Thinking
    } else if lowered.contains("lite") || lowered.contains("flash-lite") {
        ModelCategory::FlashLite
    } else if lowered.contains("flash") {
        ModelCategory::Fast
    } else {
        ModelCategory::Auto
    }
}

fn build_sdk_message(request: &ChatRequest) -> Result<ChatMessage> {
    let mut text_parts: Vec<String> = Vec::new();
    let mut images: Vec<ImageSource> = Vec::new();

    // The SDK's high-level chat API flattens multi-turn history and only sends
    // the latest user turn. We therefore concatenate all text from the final
    // user content and append any images. Prior assistant/user turns are
    // dropped; this matches the old proxy's behaviour for cookie-auth web
    // frontend mode.
    let user_content = request
        .contents
        .iter()
        .rfind(|c| c.role == "user")
        .ok_or_else(|| ProxyError::BadRequest("request must contain a user message".into()))?;

    if let Some(system) = &request.system_instruction
        && !system.is_empty()
    {
        text_parts.push(format!("System instruction:\n{system}\n"));
    }

    for part in &user_content.parts {
        match part {
            ChatPart::Text(t) => text_parts.push(t.clone()),
            ChatPart::InlineData { mime_type, data } => {
                images.push(ImageSource::InlineData {
                    mime_type: mime_type.clone(),
                    data: data.clone(),
                });
            }
            _ => {}
        }
    }

    let prompt = text_parts.join("\n");
    if prompt.is_empty() && images.is_empty() {
        return Err(ProxyError::BadRequest("prompt is empty".into()));
    }

    let mut message = ChatMessage::user(prompt);
    for image in images {
        message.parts.push(ContentPart::Image(image));
    }
    Ok(message)
}

pub fn map_client_response_to_generate_content_response(resp: ChatResponse) -> crate::gemini::types::GenerateContentResponse {
    crate::gemini::types::GenerateContentResponse {
        candidates: vec![crate::gemini::types::Candidate {
            content: Some(crate::gemini::types::ResponseContent {
                role: "model".to_string(),
                parts: vec![crate::gemini::types::ResponsePart::Text(crate::gemini::types::TextResponsePart { text: resp.text })],
            }),
            finish_reason: resp.finish_reason,
            index: 0,
            safety_ratings: None,
        }],
        usage_metadata: None,
        model_version: None,
        response_id: None,
    }
}

fn map_sdk_response(response: SdkChatResponse) -> ChatResponse {
    ChatResponse {
        text: response.text,
        finish_reason: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_model_category() {
        assert!(matches!(resolve_model_category("gemini-3.6-flash"), ModelCategory::Fast));
        assert!(matches!(resolve_model_category("gemini-3.1-pro"), ModelCategory::Pro));
        assert!(matches!(resolve_model_category("gemini-thinking"), ModelCategory::Thinking));
        assert!(matches!(resolve_model_category("gemini-auto"), ModelCategory::Auto));
    }

    #[test]
    fn test_build_cookie_header() {
        let mut cookies = HashMap::new();
        cookies.insert("a".into(), "1".into());
        cookies.insert("b".into(), "2".into());
        let header = build_cookie_header(&cookies);
        assert!(header.contains("a=1"));
        assert!(header.contains("b=2"));
    }

    #[test]
    fn test_human_id_prefers_versioned_name() {
        let info = SdkModelInfo {
            id: "abc".into(),
            title: "Flash".into(),
            description: String::new(),
            versioned_name: Some("Gemini 3.6 Flash".into()),
            category: ModelCategory::Fast,
            category_enum: 1,
        };
        assert_eq!(human_id(&info), "gemini-3.6-flash");
    }
}
