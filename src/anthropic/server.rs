use std::convert::Infallible;

use axum::extract::{State};
use axum::http::{HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use futures::StreamExt;
use serde_json::{json, Value};
use tokio_stream::wrappers::ReceiverStream;
use tracing::error;

use crate::error::ProxyError;
use crate::gemini::client::GeminiClient;
use crate::gemini::types::{
    Candidate, GenerateContentResponse, ResponseContent, ResponsePart, TextResponsePart,
};
use crate::anthropic::converter::{anthropic_to_gemini_request, gemini_to_anthropic_response};
use crate::anthropic::types::MessagesRequest;
use crate::config::Config;

#[derive(Clone)]
pub struct AnthropicAppState {
    pub gemini_client: GeminiClient,
    pub config: Config,
}

pub fn create_anthropic_router(state: AnthropicAppState) -> Router {
    Router::new()
        .route("/v1/messages", post(messages))
        .with_state(state)
}

async fn messages(
    State(state): State<AnthropicAppState>,
    headers: HeaderMap,
    Json(request): Json<MessagesRequest>,
) -> std::result::Result<Response, ProxyError> {
    if let Some(ref auth_token) = state.config.auth_token {
        let token = extract_bearer_token(&headers).ok_or_else(|| {
            ProxyError::Auth("Missing or invalid Authorization header".into())
        })?;
        if token != *auth_token {
            return Err(ProxyError::Auth("Invalid authentication token".into()));
        }
    }

    if request.model.is_empty() {
        return Err(ProxyError::BadRequest(
            "Missing 'model' field. Call /v1/models to list available models.".into(),
        ));
    }
    let model = request.model.clone();

    let gemini_request = anthropic_to_gemini_request(&request)?;

    if request.stream.unwrap_or(false) {
        let response = state
            .gemini_client
            .stream_content(&model, &gemini_request)
            .await?;
        Ok(build_anthropic_sse_response(response, &model))
    } else {
        let gemini_response = state
            .gemini_client
            .generate_content(&model, &gemini_request)
            .await?;
        let anthropic_response = gemini_to_anthropic_response(gemini_response, &model)?;
        Ok(Json(serde_json::to_value(anthropic_response)?).into_response())
    }
}

fn build_anthropic_sse_response(response: reqwest::Response, model: &str) -> Response {
    let stream = response.bytes_stream();
    let model = model.to_string();
    let (tx, rx) = tokio::sync::mpsc::channel::<std::result::Result<String, Infallible>>(64);

    tokio::spawn(async move {
        let mut buffer = String::new();
        let mut stream = std::pin::pin!(stream);
        let mut accumulated_text = String::new();
        let msg_id = crate::anthropic::converter::generate_msg_id();
        let mut sent_message_start = false;
        let mut sent_content_block_start = false;
        let content_block_index = 0;

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

                let data = if let Some(d) = line.strip_prefix("data: ") {
                    if d == "[DONE]" {
                        break;
                    }
                    d.to_string()
                } else {
                    continue;
                };

                let gemini_chunk = match serde_json::from_str::<GenerateContentResponse>(&data) {
                    Ok(c) => Some(c),
                    Err(_) => {
                        serde_json::from_str::<Value>(&data)
                            .ok()
                            .and_then(|parsed| {
                                crate::gemini::web_frontend::extract_text_from_parsed_response(&parsed)
                                    .map(|text| GenerateContentResponse {
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
                            })
                    }
                };

                if let Some(chunk) = gemini_chunk {
                    let candidate = match chunk.candidates.first() {
                        Some(c) => c,
                        None => continue,
                    };

                    let mut chunk_text = String::new();
                    if let Some(ref content) = candidate.content {
                        for part in &content.parts {
                            if let ResponsePart::Text(tp) = part {
                                chunk_text.push_str(&tp.text);
                            }
                        }
                    }

                    // Send message_start event
                    if !sent_message_start {
                        sent_message_start = true;
                        let message_start = json!({
                            "type": "message_start",
                            "message": {
                                "id": msg_id,
                                "type": "message",
                                "role": "assistant",
                                "content": [],
                                "model": model,
                                "stop_reason": null,
                                "stop_sequence": null,
                                "usage": {"input_tokens": 0, "output_tokens": 0}
                            }
                        });
                        if let Ok(s) = serde_json::to_string(&message_start) {
                            let _ = tx.send(Ok(format!("event: message_start\ndata: {s}\n\n"))).await;
                        }
                    }

                    // Send content_block_start event
                    if !sent_content_block_start && !chunk_text.is_empty() {
                        sent_content_block_start = true;
                        let content_block_start = json!({
                            "type": "content_block_start",
                            "index": content_block_index,
                            "content_block": {
                                "type": "text",
                                "text": ""
                            }
                        });
                        if let Ok(s) = serde_json::to_string(&content_block_start) {
                            let _ = tx.send(Ok(format!("event: content_block_start\ndata: {s}\n\n"))).await;
                        }
                    }

                    // Send content_block_delta events
                    if !chunk_text.is_empty() && chunk_text.len() > accumulated_text.len() {
                        let delta: String = chunk_text
                            .chars()
                            .skip(accumulated_text.chars().count())
                            .collect();
                        accumulated_text = chunk_text;

                        let delta_event = json!({
                            "type": "content_block_delta",
                            "index": content_block_index,
                            "delta": {
                                "type": "text_delta",
                                "text": delta
                            }
                        });
                        if let Ok(s) = serde_json::to_string(&delta_event) {
                            let _ = tx.send(Ok(format!("event: content_block_delta\ndata: {s}\n\n"))).await;
                        }
                    }

                    // Handle finish reason
                    if let Some(ref reason) = candidate.finish_reason {
                        // Send content_block_stop
                        let content_block_stop = json!({
                            "type": "content_block_stop",
                            "index": content_block_index
                        });
                        if let Ok(s) = serde_json::to_string(&content_block_stop) {
                            let _ = tx.send(Ok(format!("event: content_block_stop\ndata: {s}\n\n"))).await;
                        }

                        // Send message_delta
                        let stop_reason = match reason.as_str() {
                            "STOP" => "end_turn",
                            "MAX_TOKENS" => "max_tokens",
                            _ => "end_turn",
                        };
                        let message_delta = json!({
                            "type": "message_delta",
                            "delta": {
                                "stop_reason": stop_reason,
                                "stop_sequence": null
                            },
                            "usage": {"output_tokens": 0}
                        });
                        if let Ok(s) = serde_json::to_string(&message_delta) {
                            let _ = tx.send(Ok(format!("event: message_delta\ndata: {s}\n\n"))).await;
                        }

                        // Send message_stop
                        let message_stop = json!({
                            "type": "message_stop"
                        });
                        if let Ok(s) = serde_json::to_string(&message_stop) {
                            let _ = tx.send(Ok(format!("event: message_stop\ndata: {s}\n\n"))).await;
                        }
                    }
                }
            }
        }

        // Ensure events are sent if stream ended without finish_reason
        if sent_content_block_start && accumulated_text.is_empty() {
            let content_block_stop = json!({
                "type": "content_block_stop",
                "index": content_block_index
            });
            if let Ok(s) = serde_json::to_string(&content_block_stop) {
                let _ = tx.send(Ok(format!("event: content_block_stop\ndata: {s}\n\n"))).await;
            }
        }

        let _ = tx.send(Ok("data: [DONE]\n\n".to_string())).await;
    });

    let body_stream = ReceiverStream::new(rx).map(|result| match result {
        Ok(s) => Ok::<_, Infallible>(s.into_bytes()),
        Err(e) => Err(e),
    });

    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", HeaderValue::from_static("text/event-stream"));
    headers.insert("Cache-Control", HeaderValue::from_static("no-cache"));
    headers.insert("Connection", HeaderValue::from_static("keep-alive"));
    headers.insert("X-Accel-Buffering", HeaderValue::from_static("no"));

    let response = axum::body::Body::from_stream(body_stream);
    (headers, response).into_response()
}

fn extract_bearer_token(headers: &HeaderMap) -> Option<String> {
    let auth = headers.get("authorization")?.to_str().ok()?;
    let token = auth
        .strip_prefix("Bearer ")
        .or_else(|| auth.strip_prefix("bearer "))
        .or_else(|| auth.strip_prefix("BEARER "))?;
    Some(token.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_bearer_token() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_static("Bearer test_token_123"),
        );
        let token = extract_bearer_token(&headers);
        assert_eq!(token, Some("test_token_123".into()));
    }

    #[test]
    fn test_extract_bearer_token_lowercase() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_static("bearer test_token_456"),
        );
        let token = extract_bearer_token(&headers);
        assert_eq!(token, Some("test_token_456".into()));
    }
}
