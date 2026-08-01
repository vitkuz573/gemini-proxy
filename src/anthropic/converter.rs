use super::types::*;
use crate::error::{ProxyError, Result};
use crate::gemini::types::{
    Content, FunctionCall as GeminiFunctionCall, FunctionCallPart, FunctionDeclaration, FunctionResponse,
    FunctionResponsePart, GenerateContentRequest, GenerateContentResponse, InlineData,
    InlineDataPart, Part, Parts, TextPart, ThinkingConfig as GeminiThinkingConfig, Tool as GeminiTool,
    FunctionCallingConfig, ToolConfig, GenerationConfig, ResponsePart,
};
use uuid::Uuid;

pub fn generate_msg_id() -> String {
    format!("msg_{}", Uuid::new_v4().to_string().replace('-', ""))
}

pub fn generate_tool_id() -> String {
    format!("toolu_{}", Uuid::new_v4().to_string().replace('-', ""))
}

pub fn map_gemini_finish_reason(reason: Option<String>) -> String {
    match reason.as_deref() {
        Some("STOP") => "end_turn".into(),
        Some("MAX_TOKENS") => "max_tokens".into(),
        Some("SAFETY") => "end_turn".into(),
        Some("RECITATION") => "end_turn".into(),
        _ => "end_turn".into(),
    }
}

pub fn anthropic_to_gemini_request(req: &MessagesRequest) -> Result<GenerateContentRequest> {
    let mut system_instruction: Option<Parts> = None;
    let mut contents: Vec<Content> = Vec::new();
    let mut function_declarations: Vec<FunctionDeclaration> = Vec::new();
    let mut has_tool_config = false;

    // Handle system prompt
    if let Some(system) = &req.system {
        match system {
            SystemContent::Text(text) => {
                system_instruction = Some(Parts {
                    parts: vec![Part::Text(TextPart { text: text.clone() })],
                });
            }
            SystemContent::Blocks(blocks) => {
                let parts: Vec<Part> = blocks
                    .iter()
                    .map(|block| match block {
                        SystemBlock::Text { text } => Part::Text(TextPart { text: text.clone() }),
                    })
                    .collect();
                if !parts.is_empty() {
                    system_instruction = Some(Parts { parts });
                }
            }
        }
    }

    // Build a map from tool_use_id -> function_name by looking at preceding
    // assistant ToolUse blocks.  This lets us label ToolResult blocks with the
    // correct function name even when the client omits it.
    let mut tool_use_id_to_name: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for msg in &req.messages {
        if msg.role == "assistant" {
            if let MessageContent::Blocks(ref blocks) = msg.content {
                for block in blocks {
                    if let ContentBlock::ToolUse { id, name, .. } = block {
                        tool_use_id_to_name.insert(id.clone(), name.clone());
                    }
                }
            }
        }
    }

    // Convert messages
    for msg in &req.messages {
        match msg.role.as_str() {
            "user" => {
                let mut parts = convert_message_content_to_parts(&msg.content)?;
                if let MessageContent::Blocks(ref blocks) = msg.content {
                    for block in blocks {
                        if let ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                        } = block
                        {
                            let response_value = match content {
                                Some(ToolResultContent::Text(text)) => {
                                    serde_json::json!({"result": text})
                                }
                                Some(ToolResultContent::Blocks(blocks)) => {
                                    let mut result_parts = Vec::new();
                                    for b in blocks {
                                        if let ContentBlock::Text { text } = b {
                                            result_parts.push(text.clone());
                                        }
                                    }
                                    serde_json::json!({"result": result_parts.join(" ")})
                                }
                                None => serde_json::json!({}),
                            };
                            let name = tool_use_id_to_name
                                .get(tool_use_id)
                                .cloned()
                                .unwrap_or_else(|| tool_use_id.clone());
                            parts.push(Part::FunctionResponse(FunctionResponsePart {
                                function_response: FunctionResponse {
                                    name,
                                    response: if is_error.unwrap_or(false) {
                                        serde_json::json!({"error": response_value})
                                    } else {
                                        response_value
                                    },
                                },
                            }));
                        }
                    }
                }
                contents.push(Content {
                    role: "user".into(),
                    parts,
                });
            }
            "assistant" => {
                let parts = convert_message_content_to_parts(&msg.content)?;
                if !parts.is_empty() {
                    contents.push(Content {
                        role: "model".into(),
                        parts,
                    });
                }
            }
            _ => {}
        }
    }

    // Handle tools
    if let Some(tools) = &req.tools {
        for tool in tools {
            function_declarations.push(FunctionDeclaration {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: tool.input_schema.clone(),
            });
        }
        has_tool_config = true;
    }

    // Build generation config
    let mut generation_config = GenerationConfig {
        temperature: req.temperature,
        top_p: req.top_p,
        top_k: req.top_k,
        max_output_tokens: req.max_tokens,
        stop_sequences: req.stop_sequences.clone(),
        candidate_count: None,
        presence_penalty: None,
        frequency_penalty: None,
        response_mime_type: None,
        response_schema: None,
        thinking_config: None,
        seed: None,
    };

    // Handle thinking/extended thinking
    if let Some(thinking) = &req.thinking
        && thinking.enabled.unwrap_or(false) {
            generation_config.thinking_config = Some(GeminiThinkingConfig {
                include_thoughts: Some(true),
                thinking_budget: thinking.budget_tokens,
            });
        }

    // Build tools
    let mut tools_list: Vec<GeminiTool> = Vec::new();
    if !function_declarations.is_empty() {
        tools_list.push(GeminiTool {
            function_declarations,
        });
    }

    // Build tool config
    let tool_config = if has_tool_config {
        match &req.tool_choice {
            Some(choice) => match choice {
                ToolChoice::Auto(_) => Some(ToolConfig {
                    function_calling_config: FunctionCallingConfig {
                        mode: "AUTO".into(),
                        allowed_function_names: None,
                    },
                }),
                ToolChoice::Any(_) => Some(ToolConfig {
                    function_calling_config: FunctionCallingConfig {
                        mode: "ANY".into(),
                        allowed_function_names: None,
                    },
                }),
                ToolChoice::Tool(tool_choice) => Some(ToolConfig {
                    function_calling_config: FunctionCallingConfig {
                        mode: "ANY".into(),
                        allowed_function_names: Some(vec![tool_choice.name.clone()]),
                    },
                }),
            },
            None => Some(ToolConfig {
                function_calling_config: FunctionCallingConfig {
                    mode: "AUTO".into(),
                    allowed_function_names: None,
                },
            }),
        }
    } else {
        None
    };

    Ok(GenerateContentRequest {
        contents,
        system_instruction,
        generation_config: Some(generation_config),
        tools: if tools_list.is_empty() {
            None
        } else {
            Some(tools_list)
        },
        tool_config,
    })
}

fn convert_message_content_to_parts(content: &MessageContent) -> Result<Vec<Part>> {
    match content {
        MessageContent::Text(text) => Ok(vec![Part::Text(TextPart { text: text.clone() })]),
        MessageContent::Blocks(blocks) => {
            let mut parts = Vec::new();
            for block in blocks {
                match block {
                    ContentBlock::Text { text } => {
                        parts.push(Part::Text(TextPart { text: text.clone() }));
                    }
                    ContentBlock::Image { source } => {
                        if let Some(data) = &source.data {
                            let mime = source
                                .media_type
                                .clone()
                                .unwrap_or_else(|| "image/png".into());
                            parts.push(Part::InlineData(InlineDataPart {
                                inline_data: InlineData {
                                    mime_type: mime,
                                    data: data.clone(),
                                },
                            }));
                        }
                    }
                    ContentBlock::ToolUse { id: _, name, input } => {
                        let args = input
                            .clone()
                            .unwrap_or(serde_json::Value::Object(Default::default()));
                        parts.push(Part::FunctionCall(FunctionCallPart {
                            function_call: GeminiFunctionCall {
                                name: name.clone(),
                                args,
                            },
                        }));
                    }
                    ContentBlock::ToolResult { .. } => {
                        // ToolResult blocks are handled at the message level so
                        // we can resolve the function name from prior assistant
                        // ToolUse blocks.
                    }
                    ContentBlock::Thinking { .. } => {
                        // Thinking blocks from user are ignored
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
    }
}

pub fn gemini_to_anthropic_response(
    resp: GenerateContentResponse,
    model: &str,
) -> Result<MessagesResponse> {
    let candidate = resp
        .candidates
        .into_iter()
        .next()
        .ok_or(ProxyError::NoCandidates)?;

    let mut content_blocks = Vec::new();

    if let Some(resp_content) = candidate.content {
        for part in resp_content.parts {
            match part {
                ResponsePart::Text(text_part) => {
                    if !text_part.text.is_empty() {
                        content_blocks.push(ContentBlock::Text {
                            text: text_part.text,
                        });
                    }
                }
                ResponsePart::Thought(thought_part) => {
                    content_blocks.push(ContentBlock::Thinking {
                        thinking: thought_part.text,
                        signature: None,
                    });
                }
                ResponsePart::FunctionCall(fc) => {
                    content_blocks.push(ContentBlock::ToolUse {
                        id: format!("toolu_{}", Uuid::new_v4().to_string().replace('-', "")),
                        name: fc.function_call.name,
                        input: Some(fc.function_call.args),
                    });
                }
            }
        }
    }

    // If no content blocks, add an empty text block
    if content_blocks.is_empty() {
        content_blocks.push(ContentBlock::Text {
            text: String::new(),
        });
    }

    let stop_reason = map_gemini_finish_reason(candidate.finish_reason);

    let usage = Usage {
        input_tokens: resp
            .usage_metadata
            .as_ref()
            .map(|um| um.prompt_token_count)
            .unwrap_or(0),
        output_tokens: resp
            .usage_metadata
            .as_ref()
            .map(|um| um.candidates_token_count)
            .unwrap_or(0),
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
    };

    Ok(MessagesResponse {
        id: generate_msg_id(),
        response_type: "message".into(),
        role: "assistant".into(),
        content: content_blocks,
        model: model.to_string(),
        stop_reason: Some(stop_reason),
        stop_sequence: None,
        usage,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gemini::types::{Candidate, ResponseContent, TextResponsePart, UsageMetadata};

    #[test]
    fn test_generate_msg_id() {
        let id = generate_msg_id();
        assert!(id.starts_with("msg_"));
        assert!(id.len() > 10);
    }

    #[test]
    fn test_anthropic_to_gemini_request_simple() {
        let req = MessagesRequest {
            model: "claude-3-5-sonnet-20241022".into(),
            messages: vec![Message {
                role: "user".into(),
                content: MessageContent::Text("Hello".into()),
            }],
            max_tokens: Some(1024),
            system: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            stream: None,
            tools: None,
            tool_choice: None,
            thinking: None,
            metadata: None,
        };

        let gemini_req = anthropic_to_gemini_request(&req).unwrap();
        assert_eq!(gemini_req.contents.len(), 1);
        assert_eq!(gemini_req.contents[0].role, "user");
        assert_eq!(gemini_req.generation_config.as_ref().unwrap().max_output_tokens, Some(1024));
    }

    #[test]
    fn test_anthropic_to_gemini_request_with_system() {
        let req = MessagesRequest {
            model: "claude-3-5-sonnet-20241022".into(),
            messages: vec![Message {
                role: "user".into(),
                content: MessageContent::Text("Hi".into()),
            }],
            max_tokens: None,
            system: Some(SystemContent::Text("Be helpful.".into())),
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            stream: None,
            tools: None,
            tool_choice: None,
            thinking: None,
            metadata: None,
        };

        let gemini_req = anthropic_to_gemini_request(&req).unwrap();
        assert!(gemini_req.system_instruction.is_some());
        let sys = gemini_req.system_instruction.unwrap();
        assert_eq!(sys.parts.len(), 1);
    }

    #[test]
    fn test_anthropic_to_gemini_request_with_tools() {
        let req = MessagesRequest {
            model: "claude-3-5-sonnet-20241022".into(),
            messages: vec![Message {
                role: "user".into(),
                content: MessageContent::Text("Weather?".into()),
            }],
            max_tokens: None,
            system: None,
            temperature: None,
            top_p: None,
            top_k: None,
            stop_sequences: None,
            stream: None,
            tools: Some(vec![Tool {
                name: "get_weather".into(),
                description: Some("Get weather".into()),
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {"city": {"type": "string"}}
                })),
            }]),
            tool_choice: Some(ToolChoice::Auto("auto".into())),
            thinking: None,
            metadata: None,
        };

        let gemini_req = anthropic_to_gemini_request(&req).unwrap();
        assert!(gemini_req.tools.is_some());
        assert!(gemini_req.tool_config.is_some());
    }

    #[test]
    fn test_gemini_to_anthropic_response_text() {
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

        let anthropic_resp = gemini_to_anthropic_response(gemini_resp, "current-model").unwrap();
        assert_eq!(anthropic_resp.response_type, "message");
        assert_eq!(anthropic_resp.role, "assistant");
        assert_eq!(anthropic_resp.content.len(), 1);
        assert_eq!(anthropic_resp.stop_reason, Some("end_turn".into()));
        assert_eq!(anthropic_resp.usage.input_tokens, 10);
        assert_eq!(anthropic_resp.usage.output_tokens, 5);
    }

    #[test]
    fn test_gemini_to_anthropic_response_function_call() {
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

        let anthropic_resp = gemini_to_anthropic_response(gemini_resp, "current-model").unwrap();
        assert_eq!(anthropic_resp.content.len(), 1);
        match &anthropic_resp.content[0] {
            ContentBlock::ToolUse { name, .. } => {
                assert_eq!(name, "get_weather");
            }
            _ => panic!("Expected ToolUse block"),
        }
    }

    #[test]
    fn test_finish_reason_mapping() {
        assert_eq!(map_gemini_finish_reason(Some("STOP".into())), "end_turn");
        assert_eq!(
            map_gemini_finish_reason(Some("MAX_TOKENS".into())),
            "max_tokens"
        );
        assert_eq!(map_gemini_finish_reason(None), "end_turn");
    }
}
