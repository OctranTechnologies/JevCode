use super::{ProviderRequest, ProviderResponse};
use crate::{
    domain::*,
    error::{AppError, AppResult},
};
use serde_json::{json, Value};

fn role(role: &MessageRole) -> &'static str {
    match role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    }
}

fn system(messages: &[AgentMessage]) -> String {
    messages
        .iter()
        .filter(|message| message.role == MessageRole::System)
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn encode(
    protocol: &ProviderProtocol,
    request: &ProviderRequest<'_>,
) -> AppResult<(String, Value)> {
    let (path, mut payload) = encode_inner(protocol, request)?;
    if request.tools.is_empty() {
        if let Some(object) = payload.as_object_mut() {
            object.remove("tools");
        }
    }
    Ok((path, payload))
}

fn encode_inner(
    protocol: &ProviderProtocol,
    request: &ProviderRequest<'_>,
) -> AppResult<(String, Value)> {
    let model = request.model_id;
    let tools = request.tools;
    match protocol {
        ProviderProtocol::OpenAiChat => {
            let messages: Vec<Value> = request.messages.iter().map(|message| {
                if message.role == MessageRole::Assistant {
                    if let Some(raw) = &message.provider_data { return raw.clone(); }
                }
                if let Some(result) = &message.tool_result { return json!({"role":"tool","tool_call_id":result.tool_call_id,"content":result.content}); }
                json!({"role":role(&message.role),"content":message.content})
            }).collect();
            let definitions: Vec<_> = tools.iter().map(|tool| json!({"type":"function","function":{"name":tool.name,"description":tool.description,"parameters":tool.input_schema}})).collect();
            Ok((
                "chat/completions".into(),
                json!({"model":model,"messages":messages,"tools":definitions,"stream":false}),
            ))
        }
        ProviderProtocol::OpenAiResponses => {
            let mut input = Vec::new();
            for message in request
                .messages
                .iter()
                .filter(|message| message.role != MessageRole::System)
            {
                if let Some(raw) = message.provider_data.as_ref().and_then(Value::as_array) {
                    input.extend(raw.clone());
                } else if let Some(result) = &message.tool_result {
                    input.push(json!({"type":"function_call_output","call_id":result.tool_call_id,"output":result.content}));
                } else {
                    input.push(json!({"role":role(&message.role),"content":message.content}));
                }
            }
            let definitions: Vec<_> = tools.iter().map(|tool| json!({"type":"function","name":tool.name,"description":tool.description,"parameters":tool.input_schema,"strict":true})).collect();
            // Preserve encrypted reasoning items so stateless tool continuations work.
            Ok((
                "responses".into(),
                json!({"model":model,"instructions":system(request.messages),"input":input,"tools":definitions,"store":false,"include":["reasoning.encrypted_content"]}),
            ))
        }
        ProviderProtocol::Anthropic => {
            let mut messages: Vec<Value> = Vec::new();
            for message in request
                .messages
                .iter()
                .filter(|message| message.role != MessageRole::System)
            {
                let (role, content) = if let Some(result) = &message.tool_result {
                    (
                        "user",
                        json!([{"type":"tool_result","tool_use_id":result.tool_call_id,"content":result.content,"is_error":result.is_error}]),
                    )
                } else if let Some(raw) = &message.provider_data {
                    ("assistant", raw.clone())
                } else {
                    (
                        role(&message.role),
                        json!([{"type":"text","text":message.content}]),
                    )
                };
                // Multiple tool results must be grouped in the same user message.
                if let Some(last) = messages.last_mut().filter(|last| last["role"] == role) {
                    if let (Some(previous), Some(current)) =
                        (last["content"].as_array_mut(), content.as_array())
                    {
                        previous.extend(current.clone());
                    }
                } else {
                    messages.push(json!({"role":role,"content":content}));
                }
            }
            let definitions: Vec<_> = tools.iter().map(|tool| json!({"name":tool.name,"description":tool.description,"input_schema":tool.input_schema})).collect();
            Ok((
                "messages".into(),
                json!({"model":model,"max_tokens":4096,"system":system(request.messages),"messages":messages,"tools":definitions}),
            ))
        }
        ProviderProtocol::Gemini => {
            let mut contents: Vec<Value> = Vec::new();
            for message in request
                .messages
                .iter()
                .filter(|message| message.role != MessageRole::System)
            {
                let (role, parts) = if let Some(result) = &message.tool_result {
                    let mut function_response = json!({"name":result.name,"response":{"content":result.content,"isError":result.is_error}});
                    // Older Gemini models omit wire IDs. The runtime still assigns a
                    // local tracking ID, which must not be invented on the API wire.
                    let has_wire_id = request.messages.iter().any(|source| {
                        source
                            .provider_data
                            .as_ref()
                            .and_then(Value::as_array)
                            .is_some_and(|parts| {
                                parts.iter().any(|part| {
                                    part["functionCall"]["id"].as_str()
                                        == Some(result.tool_call_id.as_str())
                                })
                            })
                    });
                    if has_wire_id {
                        function_response["id"] = json!(result.tool_call_id);
                    }
                    ("user", json!([{"functionResponse":function_response}]))
                } else if let Some(raw) = &message.provider_data {
                    ("model", raw.clone())
                } else {
                    (
                        if message.role == MessageRole::Assistant {
                            "model"
                        } else {
                            "user"
                        },
                        json!([{"text":message.content}]),
                    )
                };
                if let Some(last) = contents.last_mut().filter(|last| last["role"] == role) {
                    if let (Some(previous), Some(current)) =
                        (last["parts"].as_array_mut(), parts.as_array())
                    {
                        previous.extend(current.clone());
                    }
                } else {
                    contents.push(json!({"role":role,"parts":parts}));
                }
            }
            let declarations: Vec<_> = tools
                .iter()
                .map(|tool| {
                    let mut schema = tool.input_schema.clone();
                    if let Some(object) = schema.as_object_mut() {
                        object.remove("additionalProperties");
                    }
                    json!({"name":tool.name,"description":tool.description,"parameters":schema})
                })
                .collect();
            Ok((
                format!("models/{model}:generateContent"),
                json!({"systemInstruction":{"parts":[{"text":system(request.messages)}]},"contents":contents,"tools":[{"functionDeclarations":declarations}]}),
            ))
        }
        ProviderProtocol::Preview => Err(AppError::new(
            "provider_format",
            "The preview provider does not use HTTP.",
        )),
    }
}

fn text(value: &Value, key: &str) -> AppResult<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(String::from)
        .ok_or_else(|| {
            AppError::new(
                "provider_format",
                format!("The provider omitted the {key} field."),
            )
        })
}

fn arguments(value: &Value) -> AppResult<Value> {
    let parsed = if let Some(encoded) = value.as_str() {
        serde_json::from_str(encoded).map_err(|_| {
            AppError::new(
                "provider_format",
                "The provider returned invalid tool arguments.",
            )
        })?
    } else {
        value.clone()
    };
    if !parsed.is_object() {
        return Err(AppError::new(
            "provider_format",
            "Tool arguments must be a JSON object.",
        ));
    }
    Ok(parsed)
}

pub fn decode(protocol: &ProviderProtocol, value: Value) -> AppResult<ProviderResponse> {
    let mut response = ProviderResponse {
        content: String::new(),
        tool_calls: vec![],
        provider_data: None,
        input_tokens: 0,
        output_tokens: 0,
    };
    match protocol {
        ProviderProtocol::OpenAiChat => {
            let message = &value["choices"][0]["message"];
            if !message.is_object() {
                return Err(AppError::new(
                    "provider_format",
                    "The provider response has no assistant message.",
                ));
            }
            response.content = message["content"].as_str().unwrap_or_default().into();
            if let Some(calls) = message["tool_calls"].as_array() {
                for call in calls {
                    response.tool_calls.push(ToolCall {
                        id: text(call, "id")?,
                        name: text(&call["function"], "name")?,
                        arguments: arguments(&call["function"]["arguments"])?,
                    });
                }
            }
            response.provider_data = Some(message.clone());
            response.input_tokens = value["usage"]["prompt_tokens"].as_u64().unwrap_or(0);
            response.output_tokens = value["usage"]["completion_tokens"].as_u64().unwrap_or(0);
        }
        ProviderProtocol::OpenAiResponses => {
            let output = value["output"].as_array().ok_or_else(|| {
                AppError::new("provider_format", "The provider response has no output.")
            })?;
            for item in output {
                if item["type"] == "function_call" {
                    response.tool_calls.push(ToolCall {
                        id: text(item, "call_id")?,
                        name: text(item, "name")?,
                        arguments: arguments(&item["arguments"])?,
                    });
                }
                if item["type"] == "message" {
                    if let Some(content) = item["content"].as_array() {
                        for part in content {
                            if let Some(text) =
                                part["text"].as_str().or_else(|| part["refusal"].as_str())
                            {
                                response.content.push_str(text);
                            }
                        }
                    }
                }
            }
            response.provider_data = Some(Value::Array(output.clone()));
            response.input_tokens = value["usage"]["input_tokens"].as_u64().unwrap_or(0);
            response.output_tokens = value["usage"]["output_tokens"].as_u64().unwrap_or(0);
        }
        ProviderProtocol::Anthropic => {
            let content = value["content"].as_array().ok_or_else(|| {
                AppError::new("provider_format", "The provider response has no content.")
            })?;
            for part in content {
                if part["type"] == "text" {
                    response
                        .content
                        .push_str(part["text"].as_str().unwrap_or_default());
                }
                if part["type"] == "tool_use" {
                    response.tool_calls.push(ToolCall {
                        id: text(part, "id")?,
                        name: text(part, "name")?,
                        arguments: arguments(&part["input"])?,
                    });
                }
            }
            response.provider_data = Some(Value::Array(content.clone()));
            response.input_tokens = value["usage"]["input_tokens"].as_u64().unwrap_or(0);
            response.output_tokens = value["usage"]["output_tokens"].as_u64().unwrap_or(0);
        }
        ProviderProtocol::Gemini => {
            let parts = value["candidates"][0]["content"]["parts"]
                .as_array()
                .ok_or_else(|| {
                    AppError::new(
                        "provider_format",
                        "The provider returned no candidate. The request may have been blocked.",
                    )
                })?;
            for part in parts {
                if part["thought"] != true {
                    if let Some(text) = part["text"].as_str() {
                        response.content.push_str(text);
                    }
                }
                if part["functionCall"].is_object() {
                    let call = &part["functionCall"];
                    response.tool_calls.push(ToolCall {
                        id: call["id"].as_str().map(String::from).unwrap_or_else(id),
                        name: text(call, "name")?,
                        arguments: arguments(&call["args"])?,
                    });
                }
            }
            response.provider_data = Some(Value::Array(parts.clone()));
            response.input_tokens = value["usageMetadata"]["promptTokenCount"]
                .as_u64()
                .unwrap_or(0);
            response.output_tokens = value["usageMetadata"]["candidatesTokenCount"]
                .as_u64()
                .unwrap_or(0)
                + value["usageMetadata"]["thoughtsTokenCount"]
                    .as_u64()
                    .unwrap_or(0);
        }
        ProviderProtocol::Preview => {
            return Err(AppError::new(
                "provider_format",
                "The preview provider does not use HTTP.",
            ))
        }
    }
    if response.content.is_empty() && response.tool_calls.is_empty() {
        return Err(AppError::new(
            "provider_format",
            "The provider returned no text or tool calls.",
        ));
    }
    if response.tool_calls.len() > 16 {
        return Err(AppError::new(
            "tool_limit",
            "The provider requested too many tools in one response.",
        ));
    }
    let mut ids = std::collections::HashSet::new();
    if response
        .tool_calls
        .iter()
        .any(|call| call.id.is_empty() || !ids.insert(&call.id))
    {
        return Err(AppError::new(
            "provider_format",
            "The provider returned duplicate or empty tool call IDs.",
        ));
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalizes_tool_calls_for_every_wire_protocol() {
        let fixtures = [
            (
                ProviderProtocol::OpenAiChat,
                json!({"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call","function":{"name":"list_files","arguments":"{\"path\":\".\"}"}}]}}]}),
            ),
            (
                ProviderProtocol::OpenAiResponses,
                json!({"output":[{"type":"function_call","call_id":"call","name":"list_files","arguments":"{\"path\":\".\"}"}]}),
            ),
            (
                ProviderProtocol::Anthropic,
                json!({"content":[{"type":"tool_use","id":"call","name":"list_files","input":{"path":"."}}]}),
            ),
            (
                ProviderProtocol::Gemini,
                json!({"candidates":[{"content":{"parts":[{"functionCall":{"id":"call","name":"list_files","args":{"path":"."}},"thoughtSignature":"opaque"}]}}]}),
            ),
        ];
        for (protocol, fixture) in fixtures {
            let response = decode(&protocol, fixture).unwrap();
            assert_eq!(response.tool_calls[0].name, "list_files");
            assert_eq!(response.tool_calls[0].arguments["path"], ".");
            let mut message = AgentMessage::text(MessageRole::Assistant, response.content);
            message.provider_data = response.provider_data;
            message.tool_calls = response.tool_calls;
            let messages = vec![message];
            let tools = crate::tools::definitions();
            let (_, encoded) = encode(
                &protocol,
                &ProviderRequest {
                    model_id: "test",
                    messages: &messages,
                    tools: &tools,
                },
            )
            .unwrap();
            if protocol == ProviderProtocol::Gemini {
                assert_eq!(
                    encoded["contents"][0]["parts"][0]["thoughtSignature"],
                    "opaque"
                );
            }
        }
    }
    #[test]
    fn rejects_invalid_arguments_instead_of_executing_them() {
        assert!(decode(&ProviderProtocol::OpenAiResponses, json!({"output":[{"type":"function_call","call_id":"x","name":"read_file","arguments":"bad"}]})).is_err());
    }

    #[test]
    fn omits_empty_tools_for_text_only_models() {
        let messages = vec![AgentMessage::text(MessageRole::User, "Hello")];
        for protocol in [
            ProviderProtocol::OpenAiChat,
            ProviderProtocol::OpenAiResponses,
            ProviderProtocol::Anthropic,
            ProviderProtocol::Gemini,
        ] {
            let (_, payload) = encode(
                &protocol,
                &ProviderRequest {
                    model_id: "test",
                    messages: &messages,
                    tools: &[],
                },
            )
            .unwrap();
            assert!(payload.get("tools").is_none());
        }
    }

    #[test]
    fn gemini_does_not_invent_wire_ids_for_older_models() {
        let response = decode(&ProviderProtocol::Gemini, json!({"candidates":[{"content":{"parts":[{"functionCall":{"name":"list_files","args":{"path":"."}}}]}}]})).unwrap();
        let call = &response.tool_calls[0];
        let mut assistant = AgentMessage::text(MessageRole::Assistant, "");
        assistant.tool_calls = response.tool_calls.clone();
        assistant.provider_data = response.provider_data.clone();
        let mut tool_message = AgentMessage::text(MessageRole::Tool, "README.md");
        tool_message.tool_result = Some(ToolResult {
            tool_call_id: call.id.clone(),
            name: call.name.clone(),
            content: "README.md".into(),
            is_error: false,
            duration_ms: 1,
        });
        let messages = vec![assistant, tool_message];
        let (_, payload) = encode(
            &ProviderProtocol::Gemini,
            &ProviderRequest {
                model_id: "gemini-2.5-flash",
                messages: &messages,
                tools: &crate::tools::definitions(),
            },
        )
        .unwrap();
        assert!(payload["contents"][1]["parts"][0]["functionResponse"]
            .get("id")
            .is_none());
    }
}
