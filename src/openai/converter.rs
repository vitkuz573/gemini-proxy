use super::types::*;
#[allow(unused_imports)]
use super::types::{FunctionCall, Tool};
use crate::error::{ProxyError, Result};
use crate::gemini::types::*;
use crate::gemini::types::{FunctionCall as GeminiFunctionCall, Tool as GeminiTool};
use chrono::UTC;
use uuid::Uuid;

pub fn generate_id() -> String {
    format!("chatcmpl-{}", Uuid::new_v4().to_string().replace('-', ""))
}

fn map_gemini_finish_reason(reason: Option<String>) -> String {
    match reason.as_deref() {
        Some("STOP") => "stop".into(),
        Some("MAX_TOKENS") => "length".into(),
        Some("SAFETY") => "content_filter".into(),
        Some("RECITATION") => "content_filter".into(),
        _ => "stop".into(),
    }
}

pub fn gemini_finish_reason(reason: Option<String>) -> String {
    map_gemini_finish_reason(reason)
}

pub fn openai_to_gemini_request(req: &ChatCompletionRequest) -> Result<GenerateContentRequest> {
    if let Some(n) = req.n
        && (n == 0 || n > 8) {
            return Err(ProxyError::BadRequest(format!("n must be between 1 and 8, got {n}")));
        }

    let mut system_instruction: Option<Parts> = None;
    let mut contents: Vec<Content> = Vec::new();
    let mut function_declarations: Vec<FunctionDeclaration> = Vec::new();
    let mut has_tool_config = false;

    for msg in &req.messages {
        match msg.role.as_str() {
            "system" | "developer" => {
                if let Some(text) = &msg.content {
                    system_instruction = Some(Parts {
                        parts: vec![Part::Text(TextPart { text: text.clone() })],
                    });
                }
            }
            "user" => {
                let parts = msg_to_parts(msg)?;
                contents.push(Content {
                    role: "user".into(),
                    parts,
                });
            }
            "assistant" => {
                let mut parts: Vec<Part> = Vec::new();
                if let Some(text) = &msg.content
                    && !text.is_empty() {
                        parts.push(Part::Text(TextPart { text: text.clone() }));
                    }
                if let Some(tool_calls) = &msg.tool_calls {
                    for tc in tool_calls {
                        let args: serde_json::Value =
                            serde_json::from_str(&tc.function.arguments)
                                .unwrap_or(serde_json::Value::Object(Default::default()));
                        parts.push(Part::FunctionCall(FunctionCallPart {
                            function_call: GeminiFunctionCall {
                                name: tc.function.name.clone(),
                                args,
                            },
                        }));
                    }
                }
                if !parts.is_empty() {
                    contents.push(Content {
                        role: "model".into(),
                        parts,
                    });
                }
            }
            "tool" => {
                let tool_call_id = msg.tool_call_id.clone().unwrap_or_default();
                let name = msg.name.clone().unwrap_or_else(|| {
                    let response: serde_json::Value =
                        serde_json::from_str(msg.content.as_deref().unwrap_or("{}"))
                            .unwrap_or(serde_json::Value::Object(Default::default()));
                    response
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or(&tool_call_id)
                        .to_string()
                });
                let response: serde_json::Value =
                    serde_json::from_str(msg.content.as_deref().unwrap_or("{}"))
                        .unwrap_or(serde_json::Value::Object(Default::default()));
                contents.push(Content {
                    role: "user".into(),
                    parts: vec![Part::FunctionResponse(FunctionResponsePart {
                        function_response: FunctionResponse {
                            name,
                            response,
                        },
                    })],
                });
            }
            _ => {}
        }
    }

    if let Some(tools) = &req.tools {
        for tool in tools {
            function_declarations.push(FunctionDeclaration {
                name: tool.function.name.clone(),
                description: tool.function.description.clone(),
                parameters: tool.function.parameters.clone(),
            });
        }
        has_tool_config = true;
    }

    let mut generation_config = GenerationConfig {
        temperature: req.temperature,
        top_p: req.top_p,
        top_k: None,
        max_output_tokens: req.max_completion_tokens.or(req.max_tokens),
        stop_sequences: req.stop.clone(),
        candidate_count: req.n,
        presence_penalty: req.presence_penalty,
        frequency_penalty: req.frequency_penalty,
        response_mime_type: None,
        response_schema: None,
        thinking_config: None,
        seed: req.seed,
    };

    if let Some(rf) = &req.response_format {
        match rf.format_type.as_str() {
            "json_object" => {
                generation_config.response_mime_type = Some("application/json".into());
            }
            "json_schema" => {
                generation_config.response_mime_type = Some("application/json".into());
                if let Some(schema) = &rf.json_schema {
                    generation_config.response_schema = Some(schema.clone());
                }
            }
            _ => {}
        }
    }

    if let Some(ref effort) = req.reasoning_effort {
        let budget = match effort.as_str() {
            "low" => Some(1024),
            "medium" => Some(8192),
            "high" => Some(24576),
            _ => None,
        };
        generation_config.thinking_config = Some(ThinkingConfig {
            include_thoughts: Some(true),
            thinking_budget: budget,
        });
    }

    let mut tools_list: Vec<GeminiTool> = Vec::new();
    if !function_declarations.is_empty() {
        tools_list.push(GeminiTool {
            function_declarations,
        });
    }

    Ok(GenerateContentRequest {
        contents,
        system_instruction,
        generation_config: Some(generation_config),
        tools: if tools_list.is_empty() {
            None
        } else {
            Some(tools_list)
        },
        tool_config: if has_tool_config {
            match req.tool_choice.as_ref() {
                Some(tc) => {
                    if let Some(name) = tc.get("function").and_then(|f| f.get("name")).and_then(|n| n.as_str()) {
                        Some(ToolConfig {
                            function_calling_config: FunctionCallingConfig {
                                mode: "ANY".into(),
                                allowed_function_names: Some(vec![name.to_string()]),
                            },
                        })
                    } else {
                        match tc.as_str() {
                            Some("auto") => Some(ToolConfig {
                                function_calling_config: FunctionCallingConfig {
                                    mode: "AUTO".into(),
                                    allowed_function_names: None,
                                },
                            }),
                            Some("none") => Some(ToolConfig {
                                function_calling_config: FunctionCallingConfig {
                                    mode: "NONE".into(),
                                    allowed_function_names: None,
                                },
                            }),
                            Some("required") => Some(ToolConfig {
                                function_calling_config: FunctionCallingConfig {
                                    mode: "ANY".into(),
                                    allowed_function_names: None,
                                },
                            }),
                            _ => None,
                        }
                    }
                }
                None => None,
            }
        } else {
            None
        },
    })
}

fn msg_to_parts(msg: &Message) -> Result<Vec<Part>> {
    let mut parts: Vec<Part> = Vec::new();

    if let Some(content) = &msg.content {
        match serde_json::from_str::<MessageContent>(content) {
            Ok(MessageContent::Parts(content_parts)) => {
                for cp in &content_parts {
                    match cp.part_type.as_str() {
                        "text" => {
                            if let Some(text) = &cp.text {
                                parts.push(Part::Text(TextPart { text: text.clone() }));
                            }
                        }
                        "image_url" => {
                            if let Some(iu) = &cp.image_url
                                && let Some((mime, data)) = parse_data_url(&iu.url) {
                                    parts.push(Part::InlineData(InlineDataPart {
                                        inline_data: InlineData {
                                            mime_type: mime,
                                            data,
                                        },
                                    }));
                                }
                        }
                        _ => {}
                    }
                }
            }
            Ok(MessageContent::Text(text)) => {
                parts.push(Part::Text(TextPart { text }));
            }
            Err(_) => {
                parts.push(Part::Text(TextPart {
                    text: content.clone(),
                }));
            }
        }
    }

    if parts.is_empty() {
        parts.push(Part::Text(TextPart {
            text: String::new(),
        }));
    }

    Ok(parts)
}

fn parse_data_url(url: &str) -> Option<(String, String)> {
    if let Some(rest) = url.strip_prefix("data:")
        && let Some((header, data)) = rest.split_once(",") {
            let mime = header.split(';').next()?.to_string();
            return Some((mime, data.to_string()));
        }
    None
}

pub fn gemini_to_openai_response(
    resp: GenerateContentResponse,
    model: &str,
) -> Result<ChatCompletionResponse> {
    let candidate = resp
        .candidates
        .into_iter()
        .next()
        .ok_or(ProxyError::NoCandidates)?;

    let mut content: Option<String> = None;
    let mut tool_calls: Vec<ToolCall> = Vec::new();

    if let Some(resp_content) = candidate.content {
        for part in resp_content.parts {
            match part {
                ResponsePart::Text(text_part) => {
                    let existing = content.unwrap_or_default();
                    content = Some(format!("{}{}", existing, text_part.text));
                }
                ResponsePart::Thought(_) => {}
                ResponsePart::FunctionCall(fc) => {
                    let args_str = serde_json::to_string(&fc.function_call.args)
                        .unwrap_or_else(|_| "{}".into());
                    tool_calls.push(ToolCall {
                        id: generate_id(),
                        tool_type: "function".into(),
                        function: FunctionCall {
                            name: fc.function_call.name,
                            arguments: args_str,
                        },
                    });
                }
            }
        }
    }

    let usage = resp.usage_metadata.map(|um| Usage {
        prompt_tokens: um.prompt_token_count,
        completion_tokens: um.candidates_token_count,
        total_tokens: um.total_token_count,
        prompt_tokens_details: None,
        completion_tokens_details: None,
    });

    let finish_reason = map_gemini_finish_reason(candidate.finish_reason);

    Ok(ChatCompletionResponse {
        id: generate_id(),
        object: "chat.completion".into(),
        created: UTC::now().timestamp(),
        model: model.to_string(),
        choices: vec![Choice {
            index: 0,
            message: ResponseMessage {
                role: "assistant".into(),
                content,
                tool_calls: if tool_calls.is_empty() {
                    None
                } else {
                    Some(tool_calls)
                },
            },
            finish_reason: Some(finish_reason),
            logprobs: None,
        }],
        usage,
        system_fingerprint: None,
        service_tier: None,
    })
}

pub fn gemini_chunk_to_openai_chunk(
    resp: &crate::gemini::types::GenerateContentResponse,
    model: &str,
) -> Option<ChatCompletionChunk> {
    let candidate = resp.candidates.first()?;

    let mut content: Option<String> = None;
    let mut tool_calls: Vec<ToolCallDelta> = Vec::new();
    let mut finish_reason: Option<String> = None;

    if let Some(ref resp_content) = candidate.content {
        for part in &resp_content.parts {
            match part {
                crate::gemini::types::ResponsePart::Text(text_part) => {
                    let existing = content.unwrap_or_default();
                    content = Some(format!("{}{}", existing, text_part.text));
                }
                crate::gemini::types::ResponsePart::Thought(_) => {}
                crate::gemini::types::ResponsePart::FunctionCall(fc) => {
                    let args_str = serde_json::to_string(&fc.function_call.args)
                        .unwrap_or_else(|_| "{}".into());
                    tool_calls.push(ToolCallDelta {
                        index: 0,
                        id: Some(generate_id()),
                        tool_type: Some("function".into()),
                        function: Some(FunctionCallDelta {
                            name: Some(fc.function_call.name.clone()),
                            arguments: Some(args_str),
                        }),
                    });
                }
            }
        }
    }

    if let Some(ref reason) = candidate.finish_reason {
        finish_reason = Some(map_gemini_finish_reason(Some(reason.clone())));
    }

    Some(ChatCompletionChunk {
        id: generate_id(),
        object: "chat.completion.chunk".into(),
        created: UTC::now().timestamp(),
        model: model.to_string(),
        choices: vec![ChunkChoice {
            index: 0,
            delta: Delta {
                role: Some("assistant".into()),
                content,
                tool_calls: if tool_calls.is_empty() {
                    None
                } else {
                    Some(tool_calls)
                },
            },
            finish_reason,
        }],
        system_fingerprint: None,
        service_tier: None,
        usage: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_openai_request() -> ChatCompletionRequest {
        ChatCompletionRequest {
            model: "current-model".into(),
            messages: vec![
                Message {
                    role: "system".into(),
                    content: Some("You are a helpful assistant.".into()),
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                },
                Message {
                    role: "user".into(),
                    content: Some("Hello!".into()),
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                },
            ],
            temperature: Some(0.7),
            top_p: Some(0.9),
            max_tokens: Some(1024),
            max_completion_tokens: None,
            stream: None,
            stream_options: None,
            stop: Some(vec!["END".into()]),
            presence_penalty: Some(0.1),
            frequency_penalty: Some(0.2),
            tools: None,
            tool_choice: None,
            response_format: None,
            seed: None,
            n: None,
            user: None,
            parallel_tool_calls: None,
            reasoning_effort: None,
            service_tier: None,
            store: None,
            metadata: None,
        }
    }

    #[test]
    fn convert_system_and_user_messages() {
        let req = test_openai_request();
        let gemini_req = openai_to_gemini_request(&req).unwrap();

        let sys = gemini_req.system_instruction.unwrap();
        assert_eq!(sys.parts.len(), 1);
        match &sys.parts[0] {
            Part::Text(t) => assert_eq!(t.text, "You are a helpful assistant."),
            _ => panic!("Expected Text part"),
        }

        assert_eq!(gemini_req.contents.len(), 1);
        assert_eq!(gemini_req.contents[0].role, "user");
        match &gemini_req.contents[0].parts[0] {
            Part::Text(t) => assert_eq!(t.text, "Hello!"),
            _ => panic!("Expected Text part"),
        }
    }

    #[test]
    fn convert_generation_config() {
        let req = test_openai_request();
        let gemini_req = openai_to_gemini_request(&req).unwrap();

        let config = gemini_req.generation_config.unwrap();
        assert_eq!(config.temperature, Some(0.7));
        assert_eq!(config.top_p, Some(0.9));
        assert_eq!(config.max_output_tokens, Some(1024));
        assert_eq!(config.stop_sequences, Some(vec!["END".into()]));
        assert_eq!(config.presence_penalty, Some(0.1));
        assert_eq!(config.frequency_penalty, Some(0.2));
    }

    #[test]
    fn convert_assistant_with_tool_calls() {
        let req = ChatCompletionRequest {
            model: "current-model".into(),
            messages: vec![Message {
                role: "assistant".into(),
                content: Some("Let me check.".into()),
                tool_calls: Some(vec![ToolCall {
                    id: "call_123".into(),
                    tool_type: "function".into(),
                    function: FunctionCall {
                        name: "get_weather".into(),
                        arguments: "{\"city\":\"Paris\"}".into(),
                    },
                }]),
                tool_call_id: None,
                name: None,
            }],
            temperature: None,
            top_p: None,
            max_tokens: None,
            max_completion_tokens: None,
            stream: None,
            stream_options: None,
            stop: None,
            presence_penalty: None,
            frequency_penalty: None,
            tools: None,
            tool_choice: None,
            response_format: None,
            seed: None,
            n: None,
            user: None,
            parallel_tool_calls: None,
            reasoning_effort: None,
            service_tier: None,
            store: None,
            metadata: None,
        };
        let gemini_req = openai_to_gemini_request(&req).unwrap();

        assert_eq!(gemini_req.contents.len(), 1);
        assert_eq!(gemini_req.contents[0].role, "model");
        assert_eq!(gemini_req.contents[0].parts.len(), 2);
        match &gemini_req.contents[0].parts[1] {
            Part::FunctionCall(fc) => {
                assert_eq!(fc.function_call.name, "get_weather");
                assert_eq!(fc.function_call.args["city"], "Paris");
            }
            _ => panic!("Expected FunctionCall part"),
        }
    }

    #[test]
    fn convert_tool_response_message() {
        let req = ChatCompletionRequest {
            model: "current-model".into(),
            messages: vec![Message {
                role: "tool".into(),
                content: Some("{\"temp\":\"22C\"}".into()),
                tool_calls: None,
                tool_call_id: Some("call_123".into()),
                name: Some("get_weather".into()),
            }],
            temperature: None,
            top_p: None,
            max_tokens: None,
            max_completion_tokens: None,
            stream: None,
            stream_options: None,
            stop: None,
            presence_penalty: None,
            frequency_penalty: None,
            tools: None,
            tool_choice: None,
            response_format: None,
            seed: None,
            n: None,
            user: None,
            parallel_tool_calls: None,
            reasoning_effort: None,
            service_tier: None,
            store: None,
            metadata: None,
        };
        let gemini_req = openai_to_gemini_request(&req).unwrap();

        assert_eq!(gemini_req.contents.len(), 1);
        assert_eq!(gemini_req.contents[0].role, "user");
        match &gemini_req.contents[0].parts[0] {
            Part::FunctionResponse(fr) => {
                assert_eq!(fr.function_response.name, "get_weather");
            }
            _ => panic!("Expected FunctionResponse part"),
        }
    }

    #[test]
    fn convert_tools_to_declarations() {
        let req = ChatCompletionRequest {
            model: "current-model".into(),
            messages: vec![Message {
                role: "user".into(),
                content: Some("Hi".into()),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            }],
            temperature: None,
            top_p: None,
            max_tokens: None,
            max_completion_tokens: None,
            stream: None,
            stream_options: None,
            stop: None,
            presence_penalty: None,
            frequency_penalty: None,
            tools: Some(vec![Tool {
                tool_type: "function".into(),
                function: FunctionDef {
                    name: "get_weather".into(),
                    description: Some("Get weather".into()),
                    parameters: Some(serde_json::json!({
                        "type": "object",
                        "properties": {"city": {"type": "string"}}
                    })),
                },
            }]),
            tool_choice: Some(serde_json::json!("auto")),
            response_format: None,
            seed: None,
            n: None,
            user: None,
            parallel_tool_calls: None,
            reasoning_effort: None,
            service_tier: None,
            store: None,
            metadata: None,
        };
        let gemini_req = openai_to_gemini_request(&req).unwrap();

        let tools = gemini_req.tools.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].function_declarations.len(), 1);
        assert_eq!(tools[0].function_declarations[0].name, "get_weather");
    }

    #[test]
    fn convert_json_response_format() {
        let req = ChatCompletionRequest {
            model: "current-model".into(),
            messages: vec![Message {
                role: "user".into(),
                content: Some("Hi".into()),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            }],
            temperature: None,
            top_p: None,
            max_tokens: None,
            max_completion_tokens: None,
            stream: None,
            stream_options: None,
            stop: None,
            presence_penalty: None,
            frequency_penalty: None,
            tools: None,
            tool_choice: None,
            response_format: Some(ResponseFormat {
                format_type: "json_object".into(),
                json_schema: None,
            }),
            seed: None,
            n: None,
            user: None,
            parallel_tool_calls: None,
            reasoning_effort: None,
            service_tier: None,
            store: None,
            metadata: None,
        };
        let gemini_req = openai_to_gemini_request(&req).unwrap();

        let config = gemini_req.generation_config.unwrap();
        assert_eq!(
            config.response_mime_type,
            Some("application/json".into())
        );
    }

    #[test]
    fn convert_gemini_response_text() {
        let gemini_resp = GenerateContentResponse {
            candidates: vec![Candidate {
                content: Some(ResponseContent {
                    role: "model".into(),
                    parts: vec![ResponsePart::Text(TextResponsePart {
                        text: "Hello from Gemini!".into(),
                    })],
                }),
                finish_reason: Some("STOP".into()),
                index: 0,
                safety_ratings: None,
            }],
            usage_metadata: Some(UsageMetadata {
                prompt_token_count: 10,
                candidates_token_count: 5,
                total_token_count: 15,
                cached_content_token_count: 0,
            }),
            model_version: None,
            response_id: None,
        };

        let openai_resp = gemini_to_openai_response(gemini_resp, "current-model").unwrap();

        assert_eq!(openai_resp.object, "chat.completion");
        assert_eq!(openai_resp.model, "current-model");
        assert_eq!(openai_resp.choices.len(), 1);
        assert_eq!(
            openai_resp.choices[0].message.content,
            Some("Hello from Gemini!".into())
        );
        assert_eq!(openai_resp.choices[0].finish_reason, Some("stop".into()));
        assert!(openai_resp.choices[0].message.tool_calls.is_none());

        let usage = openai_resp.usage.unwrap();
        assert_eq!(usage.prompt_tokens, 10);
        assert_eq!(usage.completion_tokens, 5);
        assert_eq!(usage.total_tokens, 15);
    }

    #[test]
    fn convert_gemini_response_function_call() {
        let gemini_resp = GenerateContentResponse {
            candidates: vec![Candidate {
                content: Some(ResponseContent {
                    role: "model".into(),
                    parts: vec![ResponsePart::FunctionCall(FunctionCallPart {
                        function_call: GeminiFunctionCall {
                            name: "get_weather".into(),
                            args: serde_json::json!({"city": "Paris"}),
                        },
                    })],
                }),
                finish_reason: Some("STOP".into()),
                index: 0,
                safety_ratings: None,
            }],
            usage_metadata: None,
            model_version: None,
            response_id: None,
        };

        let openai_resp = gemini_to_openai_response(gemini_resp, "current-model").unwrap();

        assert!(openai_resp.choices[0].message.content.is_none());
        let tool_calls = openai_resp.choices[0].message.tool_calls.as_ref().unwrap();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].function.name, "get_weather");
        assert!(tool_calls[0].id.starts_with("chatcmpl-"));
    }

    #[test]
    fn convert_gemini_response_empty_content() {
        let gemini_resp = GenerateContentResponse {
            candidates: vec![Candidate {
                content: None,
                finish_reason: Some("SAFETY".into()),
                index: 0,
                safety_ratings: None,
            }],
            usage_metadata: None,
            model_version: None,
            response_id: None,
        };

        let openai_resp = gemini_to_openai_response(gemini_resp, "current-model").unwrap();

        assert!(openai_resp.choices[0].message.content.is_none());
        assert_eq!(
            openai_resp.choices[0].finish_reason,
            Some("content_filter".into())
        );
    }

    #[test]
    fn convert_gemini_no_candidates_returns_error() {
        let gemini_resp = GenerateContentResponse {
            candidates: vec![],
            usage_metadata: None,
            model_version: None,
            response_id: None,
        };

        let result = gemini_to_openai_response(gemini_resp, "current-model");
        assert!(result.is_err());
    }

    #[test]
    fn finish_reason_mapping() {
        assert_eq!(gemini_finish_reason(Some("STOP".into())), "stop");
        assert_eq!(gemini_finish_reason(Some("MAX_TOKENS".into())), "length");
        assert_eq!(
            gemini_finish_reason(Some("SAFETY".into())),
            "content_filter"
        );
        assert_eq!(
            gemini_finish_reason(Some("RECITATION".into())),
            "content_filter"
        );
        assert_eq!(gemini_finish_reason(None), "stop");
        assert_eq!(gemini_finish_reason(Some("UNKNOWN".into())), "stop");
    }

    #[test]
    fn generate_id_format() {
        let id = generate_id();
        assert!(id.starts_with("chatcmpl-"));
        assert_eq!(id.len(), 41);
    }

    #[test]
    fn parse_data_url_works() {
        let url = "data:image/png;base64,iVBORw0KGgo=";
        let result = parse_data_url(url).unwrap();
        assert_eq!(result.0, "image/png");
        assert_eq!(result.1, "iVBORw0KGgo=");
    }

    #[test]
    fn parse_data_url_returns_none_for_regular_url() {
        assert!(parse_data_url("https://example.com/image.png").is_none());
    }
}
