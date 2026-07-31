pub mod auth;
pub mod client;
pub mod types;
pub mod web_frontend;

#[cfg(feature = "browser-attestation")]
pub mod browser_attestation;

pub use auth::GeminiAuth;
pub use client::GeminiClient;
pub use types::{
    Content, FunctionCall, FunctionCallPart, FunctionCallingConfig, FunctionDeclaration,
    FunctionResponse, FunctionResponsePart, GenerateContentRequest, GenerateContentResponse,
    GenerationConfig, InlineData, InlineDataPart, ModelInfo, ModelListResponse, Part, Parts,
    SafetyRating, TextPart, ThinkingConfig, Tool, ToolConfig, UsageMetadata,
};
