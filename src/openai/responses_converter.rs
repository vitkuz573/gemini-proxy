use crate::error::{ProxyError, Result};
use crate::gemini::types::*;
use crate::gemini::types::Tool as GeminiTool;
use super::responses::*;
use uuid::Uuid;

pub fn generate_response_id() -> String {
    format!("resp_{}", Uuid::new_v4().to_string().replace('-', ""))
}

pub fn generate_msg_id() -> String {
    format!("msg_{}", Uuid::new_v4().to_string().replace('-', ""))
}

pub fn generate_call_id() -> String {
    format!("call_{}", Uuid::new_v4().to_string().replace('-', ""))
}

pub fn openai_response_to_gemini_request(req: &CreateResponse) -> Result<GenerateContentRequest> {
    let mut system_instruction: Option<Parts> = None;
    let mut contents: Vec<Content> = Vec::new();
    let mut function_declarations: Vec<FunctionDeclaration> = Vec::new();

    // Extract system instructions
    if let Some(ref instructions) = req.instructions {
        system_instruction = Some(Parts {
            parts: vec![Part::Text(TextPart { text: instructions.clone() })],
        });
    }

    // Convert input items to Gemini contents
    let input_items = match &req.input {
        ResponseInput::Text(text) => {
            vec![InputItem {
                role: "user".into(),
                content: Some(InputContent::Text(text.clone())),
                item_type: None,
                call_id: None,
                output: None,
            }]
        }
        ResponseInput::Items(items) => items.clone(),
    };

    for item in &input_items {
        match item.role.as_str() {
            "user" => {
                if let Some(ref content) = item.content {
                    let parts = input_content_to_parts(content)?;
                    if !parts.is_empty() {
                        contents.push(Content {
                            role: "user".into(),
                            parts,
                        });
                    }
                }
            }
            "assistant" => {
                if let Some(ref content) = item.content {
                    let parts = input_content_to_parts(content)?;
                    if !parts.is_empty() {
                        contents.push(Content {
                            role: "model".into(),
                            parts,
                        });
                    }
                }
            }
            "developer" | "system" => {
                if let Some(ref content) = item.content {
                    if let InputContent::Text(text) = content {
                        system_instruction = Some(Parts {
                            parts: vec![Part::Text(TextPart { text: text.clone() })],
                        });
                    }
                }
            }
            _ => {}
        }

        // Handle function_call_output
        if item.item_type.as_deref() == Some("function_call_output") {
            if let (Some(call_id), Some(output)) = (&item.call_id, &item.output) {
                let response: serde_json::Value = serde_json::from_str(output)
                    .unwrap_or(serde_json::Value::String(output.clone()));
                contents.push(Content {
                    role: "user".into(),
                    parts: vec![Part::FunctionResponse(FunctionResponsePart {
                        function_response: FunctionResponse {
                            name: call_id.clone(),
                            response,
                        },
                    })],
                });
            }
        }
    }

    // Convert tools
    if let Some(tools) = &req.tools {
        for tool in tools {
            if tool.tool_type == "function" {
                if let Some(ref func) = tool.function {
                    function_declarations.push(FunctionDeclaration {
                        name: func.name.clone(),
                        description: func.description.clone(),
                        parameters: func.parameters.clone(),
                    });
                }
            }
        }
    }

    // Build generation config
    let mut generation_config = GenerationConfig {
        temperature: req.temperature,
        top_p: req.top_p,
        top_k: None,
        max_output_tokens: req.max_output_tokens,
        stop_sequences: req.stop.clone(),
        candidate_count: None,
        presence_penalty: None,
        frequency_penalty: None,
        response_mime_type: None,
        response_schema: None,
        thinking_config: None,
        seed: None,
    };

    // Handle text.format
    if let Some(ref text_config) = req.text {
        if let Some(ref format) = text_config.format {
            if format.format_type == "json_object" {
                generation_config.response_mime_type = Some("application/json".into());
            }
        }
    }

    // Handle reasoning
    if let Some(ref reasoning) = req.reasoning {
        if let Some(ref effort) = reasoning.effort {
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
    }

    let mut tools_list: Vec<GeminiTool> = Vec::new();
    if !function_declarations.is_empty() {
        tools_list.push(GeminiTool {
            function_declarations,
        });
    }

    // Tool config
    let tool_config = if !tools_list.is_empty() {
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
    };

    Ok(GenerateContentRequest {
        contents,
        system_instruction,
        generation_config: Some(generation_config),
        tools: if tools_list.is_empty() { None } else { Some(tools_list) },
        tool_config,
    })
}

fn input_content_to_parts(content: &InputContent) -> Result<Vec<Part>> {
    match content {
        InputContent::Text(text) => Ok(vec![Part::Text(TextPart { text: text.clone() })]),
        InputContent::Parts(parts) => {
            let mut gemini_parts = Vec::new();
            for part in parts {
                match part.part_type.as_str() {
                    "input_text" => {
                        if let Some(ref text) = part.text {
                            gemini_parts.push(Part::Text(TextPart { text: text.clone() }));
                        }
                    }
                    "input_image" => {
                        if let Some(ref url) = part.image_url {
                            if let Some((mime, data)) = parse_data_url(url) {
                                gemini_parts.push(Part::InlineData(InlineDataPart {
                                    inline_data: InlineData {
                                        mime_type: mime,
                                        data,
                                    },
                                }));
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(gemini_parts)
        }
    }
}

fn parse_data_url(url: &str) -> Option<(String, String)> {
    if let Some(rest) = url.strip_prefix("data:") {
        if let Some((header, data)) = rest.split_once(",") {
            let mime = header.split(';').next()?.to_string();
            return Some((mime, data.to_string()));
        }
    }
    None
}

pub fn gemini_to_openai_response(
    resp: GenerateContentResponse,
    model: &str,
    req: &CreateResponse,
) -> Result<Response> {
    let candidate = resp.candidates.into_iter().next().ok_or(ProxyError::NoCandidates)?;

    let mut output_items: Vec<OutputItem> = Vec::new();

    // Add reasoning if present
    let mut reasoning_text = String::new();
    let mut content_text = String::new();
    let mut function_calls: Vec<OutputFunctionCall> = Vec::new();

    if let Some(resp_content) = candidate.content {
        for part in resp_content.parts {
            match part {
                ResponsePart::Text(tp) => {
                    content_text.push_str(&tp.text);
                }
                ResponsePart::Thought(tp) => {
                    reasoning_text.push_str(&tp.text);
                }
                ResponsePart::FunctionCall(fc) => {
                    let args_str = serde_json::to_string(&fc.function_call.args)
                        .unwrap_or_else(|_| "{}".into());
                    function_calls.push(OutputFunctionCall {
                        id: generate_call_id(),
                        call_id: generate_call_id(),
                        name: fc.function_call.name,
                        arguments: args_str,
                    });
                }
            }
        }
    }

    // Add reasoning output item
    if !reasoning_text.is_empty() {
        output_items.push(OutputItem::Reasoning(OutputReasoning {
            id: generate_call_id(),
            summary: vec![ReasoningSummary::SummaryText { text: reasoning_text }],
        }));
    }

    // Add message output item
    if !content_text.is_empty() || (!content_text.is_empty() && function_calls.is_empty()) {
        let mut content_parts = Vec::new();
        if !content_text.is_empty() {
            content_parts.push(OutputContent::OutputText {
                text: content_text,
                annotations: vec![],
            });
        }
        if !content_parts.is_empty() {
            output_items.push(OutputItem::Message(OutputMessage {
                id: generate_msg_id(),
                status: "completed".into(),
                role: "assistant".into(),
                content: content_parts,
            }));
        }
    }

    // Add function call output items
    for fc in function_calls {
        output_items.push(OutputItem::FunctionCall(fc));
    }

    let finish_reason = candidate.finish_reason.unwrap_or_else(|| "STOP".into());
    let status = match finish_reason.as_str() {
        "STOP" => "completed",
        "MAX_TOKENS" => "incomplete",
        "SAFETY" | "RECITATION" => "failed",
        _ => "completed",
    };

    let usage = resp.usage_metadata.map(|um| ResponseUsage {
        input_tokens: um.prompt_token_count,
        output_tokens: um.candidates_token_count,
        total_tokens: um.total_token_count,
        input_tokens_details: if um.cached_content_token_count > 0 {
            Some(InputTokenDetails {
                cached_tokens: Some(um.cached_content_token_count),
            })
        } else {
            None
        },
        output_tokens_details: None,
    }).unwrap_or_else(|| ResponseUsage {
        input_tokens: 0,
        output_tokens: 0,
        total_tokens: 0,
        input_tokens_details: None,
        output_tokens_details: None,
    });

    let now = chrono::UTC::now().timestamp();

    Ok(Response {
        id: generate_response_id(),
        object: "response".into(),
        created_at: now,
        completed_at: Some(now),
        status: status.into(),
        model: model.to_string(),
        output: output_items,
        error: None,
        usage,
        instructions: req.instructions.clone(),
        parallel_tool_calls: req.parallel_tool_calls.unwrap_or(true),
        store: req.store.unwrap_or(false),
        temperature: req.temperature.unwrap_or(1.0),
        top_p: req.top_p.unwrap_or(1.0),
        text: req.text.clone().unwrap_or(TextConfig { format: None }),
        tool_choice: req.tool_choice.as_ref().map_or("auto".into(), |v| {
            v.as_str().unwrap_or("auto").to_string()
        }),
        tools: req.tools.clone().unwrap_or_default(),
        max_output_tokens: req.max_output_tokens,
        metadata: req.metadata.clone(),
        previous_response_id: req.previous_response_id.clone(),
    })
}

pub fn gemini_chunk_to_response_stream_event(
    resp: &GenerateContentResponse,
    _model: &str,
    seq: u32,
) -> Option<Vec<ResponseStreamEvent>> {
    let candidate = resp.candidates.first()?;

    let mut events = Vec::new();
    let mut text = String::new();
    let mut function_call = None;

    if let Some(ref content) = candidate.content {
        for part in &content.parts {
            match part {
                ResponsePart::Text(tp) => {
                    text.push_str(&tp.text);
                }
                ResponsePart::Thought(_tp) => {
                    // Reasoning not streamed in detail
                }
                ResponsePart::FunctionCall(fc) => {
                    function_call = Some(fc);
                }
            }
        }
    }

    if !text.is_empty() {
        events.push(ResponseStreamEvent {
            event_type: "response.output_text.delta".into(),
            item_id: Some(format!("msg_{}", uuid::Uuid::new_v4().to_string().replace('-', ""))),
            output_index: Some(0),
            content_index: Some(0),
            delta: Some(text),
            sequence_number: Some(seq),
            text: None,
            response: None,
        });
    }

    if let Some(fc) = function_call {
        let args_str = serde_json::to_string(&fc.function_call.args).unwrap_or_else(|_| "{}".into());
        events.push(ResponseStreamEvent {
            event_type: "response.function_call_arguments.delta".into(),
            item_id: Some(format!("call_{}", uuid::Uuid::new_v4().to_string().replace('-', ""))),
            output_index: Some(0),
            content_index: None,
            delta: Some(args_str),
            sequence_number: Some(seq),
            text: None,
            response: None,
        });
    }

    if events.is_empty() {
        return None;
    }

    Some(events)
}
