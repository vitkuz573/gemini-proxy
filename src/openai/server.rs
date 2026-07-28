use std::convert::Infallible;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use axum::{Json as AxumJson, Router};
use futures::StreamExt;
use serde_json::{json, Value};
use tokio_stream::wrappers::ReceiverStream;
use tracing::error;

use crate::config::Config;
use crate::error::ProxyError;
use crate::gemini::client::GeminiClient;
use crate::gemini::types::{
    Candidate, GenerateContentResponse, ResponseContent, ResponsePart, TextResponsePart,
};
use crate::openai::converter::{gemini_to_openai_response, openai_to_gemini_request};
use crate::openai::types::{ChatCompletionRequest, Model, ModelsResponse};

#[derive(Clone)]
pub struct AppState {
    pub gemini_client: GeminiClient,
    pub config: Config,
}

pub fn create_router(state: AppState) -> Router {
    Router::new()
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/models", get(list_models))
        .route("/v1/models/{model}", get(get_model))
        .route("/health", get(health_check))
        .route("/", get(root_info))
        .with_state(state)
}

async fn root_info() -> Json<Value> {
    Json(json!({
        "name": "gemini2openai",
        "version": "0.1.0",
        "description": "Gemini-to-OpenAI compatible API proxy"
    }))
}

async fn health_check() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

async fn list_models(
    State(state): State<AppState>,
) -> std::result::Result<Json<Value>, ProxyError> {
    let gemini_models = state.gemini_client.list_models().await?;

    let models: Vec<Model> = gemini_models
        .models
        .into_iter()
        .map(|m| Model {
            id: m.name.clone(),
            object: "model".into(),
            created: 0,
            owned_by: "google".into(),
            permission: vec![],
            root: m.name,
            parent: None,
        })
        .collect();

    Ok(Json(serde_json::to_value(ModelsResponse {
        object: "list".into(),
        data: models,
    })?))
}

async fn get_model(
    State(state): State<AppState>,
    Path(model_id): Path<String>,
) -> std::result::Result<Json<Value>, ProxyError> {
    let gemini_models = state.gemini_client.list_models().await?;

    let model = gemini_models
        .models
        .into_iter()
        .find(|m| m.name == model_id || m.name.ends_with(&format!("/{model_id}")));

    match model {
        Some(m) => {
            let model = Model {
                id: m.name.clone(),
                object: "model".into(),
                created: 0,
                owned_by: "google".into(),
                permission: vec![],
                root: m.name,
                parent: None,
            };
            Ok(Json(serde_json::to_value(model)?))
        }
        None => Err(ProxyError::ModelNotFound(model_id)),
    }
}

async fn chat_completions(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumJson(request): AxumJson<ChatCompletionRequest>,
) -> std::result::Result<Response, ProxyError> {
    if let Some(ref auth_token) = state.config.auth_token {
        let token = extract_bearer_token(&headers).ok_or_else(|| {
            ProxyError::Auth("Missing or invalid Authorization header".into())
        })?;
        if token != *auth_token {
            return Err(ProxyError::Auth("Invalid authentication token".into()));
        }
    }

    let model = if request.model.is_empty() {
        state.config.default_model.clone()
    } else {
        request.model.clone()
    };

    let gemini_request = openai_to_gemini_request(&request)?;

    if request.stream.unwrap_or(false) {
        let response = state
            .gemini_client
            .stream_content(&model, &gemini_request)
            .await?;
        Ok(build_sse_response(response, &model))
    } else {
        let gemini_response = state
            .gemini_client
            .generate_content(&model, &gemini_request)
            .await?;
        let openai_response = gemini_to_openai_response(gemini_response, &model)?;
        Ok(Json(serde_json::to_value(openai_response)?).into_response())
    }
}

fn build_sse_response(response: reqwest::Response, model: &str) -> Response {
    let stream = response.bytes_stream();
    let model = model.to_string();
    let (tx, rx) = tokio::sync::mpsc::channel::<std::result::Result<String, Infallible>>(64);

    tokio::spawn(async move {
        let mut buffer = String::new();
        let mut stream = std::pin::pin!(stream);
        let mut accumulated_text = String::new();
        let id = crate::openai::converter::generate_id();
        let created = chrono::UTC::now().timestamp();
        let mut sent_initial = false;

        while let Some(chunk_result) = stream.next().await {
            let bytes = match chunk_result {
                Ok(b) => b,
                Err(e) => {
                    error!("stream read error: {e}");
                    break;
                }
            };

            buffer.push_str(&String::from_utf8_lossy(&bytes));

            while let Some(newline_pos) = buffer.find('\n') {
                let line = buffer[..newline_pos].trim().to_string();
                buffer = buffer[newline_pos + 1..].to_string();

                if line.is_empty() {
                    continue;
                }

                if let Some(data) = line.strip_prefix("data: ") {
                    if data == "[DONE]" {
                        break;
                    }

                    let gemini_chunk = match serde_json::from_str::<GenerateContentResponse>(data) {
                        Ok(c) => Some(c),
                        Err(_) => {
                            if let Ok(parsed) = serde_json::from_str::<Value>(data) {
                                if let Some(text) =
                                    crate::gemini::web_frontend::extract_text_from_parsed_response(
                                        &parsed,
                                    )
                                {
                                    Some(GenerateContentResponse {
                                        candidates: vec![Candidate {
                                            content: Some(ResponseContent {
                                                role: "model".to_string(),
                                                parts: vec![ResponsePart::Text(
                                                    TextResponsePart { text },
                                                )],
                                            }),
                                            finish_reason: None,
                                            index: 0,
                                            safety_ratings: None,
                                        }],
                                        usage_metadata: None,
                                        model_version: None,
                                        response_id: None,
                                    })
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        }
                    };

                    if let Some(chunk) = gemini_chunk {
                        let candidate = match chunk.candidates.first() {
                            Some(c) => c,
                            None => continue,
                        };

                        // Extract full text from this chunk
                        let mut chunk_text = String::new();
                        if let Some(ref content) = candidate.content {
                            for part in &content.parts {
                                if let crate::gemini::types::ResponsePart::Text(tp) = part {
                                    chunk_text.push_str(&tp.text);
                                }
                            }
                        }

                        let finish_reason = candidate.finish_reason.as_ref().map(|r| {
                            crate::openai::converter::gemini_finish_reason(Some(r.clone()))
                        });

                        // Send initial chunk with role on first content
                        if !sent_initial && !chunk_text.is_empty() {
                            sent_initial = true;
                            let initial = json!({
                                "id": id,
                                "object": "chat.completion.chunk",
                                "created": created,
                                "model": model,
                                "choices": [{
                                    "index": 0,
                                    "delta": {"role": "assistant", "content": ""},
                                    "finish_reason": null
                                }]
                            });
                            if let Ok(s) = serde_json::to_string(&initial) {
                                let _ = tx.send(Ok(format!("data: {s}\n\n"))).await;
                            }
                        }

                        // Compute delta from accumulated text
                        if !chunk_text.is_empty() && chunk_text.len() > accumulated_text.len() {
                            let delta = if chunk_text.starts_with(&accumulated_text) {
                                chunk_text[accumulated_text.len()..].to_string()
                            } else {
                                chunk_text[accumulated_text.len()..].to_string()
                            };
                            accumulated_text = chunk_text;
                            let chunk_json = json!({
                                "id": id,
                                "object": "chat.completion.chunk",
                                "created": created,
                                "model": model,
                                "choices": [{
                                    "index": 0,
                                    "delta": {"content": delta},
                                    "finish_reason": null
                                }]
                            });
                            if let Ok(s) = serde_json::to_string(&chunk_json) {
                                let _ = tx.send(Ok(format!("data: {s}\n\n"))).await;
                            }
                        }

                        // Send finish chunk if this candidate has a finish_reason
                        if finish_reason.is_some() {
                            let finish_chunk = json!({
                                "id": id,
                                "object": "chat.completion.chunk",
                                "created": created,
                                "model": model,
                                "choices": [{
                                    "index": 0,
                                    "delta": {},
                                    "finish_reason": finish_reason.unwrap_or_else(|| "stop".into())
                                }]
                            });
                            if let Ok(s) = serde_json::to_string(&finish_chunk) {
                                let _ = tx.send(Ok(format!("data: {s}\n\n"))).await;
                            }
                        }
                    }
                } else if line.starts_with('[') {
                    // Web frontend ?alt=sse format: raw JSON arrays per line
                    if let Ok(parsed) = serde_json::from_str::<Value>(&line) {
                        if let Some(text) =
                            crate::gemini::web_frontend::extract_text_from_parsed_response(
                                &parsed,
                            )
                        {
                            let gemini_chunk = GenerateContentResponse {
                                candidates: vec![Candidate {
                                    content: Some(ResponseContent {
                                        role: "model".to_string(),
                                        parts: vec![ResponsePart::Text(
                                            TextResponsePart { text },
                                        )],
                                    }),
                                    finish_reason: None,
                                    index: 0,
                                    safety_ratings: None,
                                }],
                                usage_metadata: None,
                                model_version: None,
                                response_id: None,
                            };

                            let candidate = match gemini_chunk.candidates.first() {
                                Some(c) => c,
                                None => continue,
                            };

                            let mut chunk_text = String::new();
                            if let Some(ref content) = candidate.content {
                                for part in &content.parts {
                                    if let crate::gemini::types::ResponsePart::Text(tp) = part {
                                        chunk_text.push_str(&tp.text);
                                    }
                                }
                            }

                            if !sent_initial && !chunk_text.is_empty() {
                                sent_initial = true;
                                let initial = json!({
                                    "id": id,
                                    "object": "chat.completion.chunk",
                                    "created": created,
                                    "model": model,
                                    "choices": [{
                                        "index": 0,
                                        "delta": {"role": "assistant", "content": ""},
                                        "finish_reason": null
                                    }]
                                });
                                if let Ok(s) = serde_json::to_string(&initial) {
                                    let _ = tx.send(Ok(format!("data: {s}\n\n"))).await;
                                }
                            }

                            if !chunk_text.is_empty() && chunk_text.len() > accumulated_text.len() {
                                let delta = chunk_text[accumulated_text.len()..].to_string();
                                accumulated_text = chunk_text;
                                let chunk_json = json!({
                                    "id": id,
                                    "object": "chat.completion.chunk",
                                    "created": created,
                                    "model": model,
                                    "choices": [{
                                        "index": 0,
                                        "delta": {"content": delta},
                                        "finish_reason": null
                                    }]
                                });
                                if let Ok(s) = serde_json::to_string(&chunk_json) {
                                    let _ = tx.send(Ok(format!("data: {s}\n\n"))).await;
                                }
                            }
                        }
                    }
                }
            }
        }

        // Send final chunk if we didn't get a finish_reason from Gemini
        let final_chunk = json!({
            "id": id,
            "object": "chat.completion.chunk",
            "created": created,
            "model": model,
            "choices": [{
                "index": 0,
                "delta": {},
                "finish_reason": "stop"
            }]
        });
        if let Ok(s) = serde_json::to_string(&final_chunk) {
            let _ = tx.send(Ok(format!("data: {s}\n\n"))).await;
        }

        let _ = tx
            .send(Ok("data: [DONE]\n\n".to_string()))
            .await;
    });

    let body_stream = ReceiverStream::new(rx).map(|result| match result {
        Ok(s) => Ok::<_, Infallible>(s.into_bytes()),
        Err(e) => Err(e),
    });

    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", HeaderValue::from_static("text/event-stream"));
    headers.insert("Cache-Control", HeaderValue::from_static("no-cache"));
    headers.insert("Connection", HeaderValue::from_static("keep-alive"));

    let response = axum::body::Body::from_stream(body_stream);
    (headers, response).into_response()
}

fn extract_bearer_token(headers: &HeaderMap) -> Option<String> {
    let auth = headers.get("authorization")?.to_str().ok()?;
    let token = auth.strip_prefix("Bearer ")?;
    Some(token.to_string())
}
