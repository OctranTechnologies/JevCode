use super::{ProviderRequest, ProviderResponse};
use crate::{
    domain::*,
    error::{AppError, AppResult},
};
use serde_json::{json, Value};
use std::collections::BTreeMap;

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

pub fn encode_streaming(
    protocol: &ProviderProtocol,
    request: &ProviderRequest<'_>,
) -> AppResult<(String, Value)> {
    let (mut path, mut payload) = encode(protocol, request)?;
    match protocol {
        ProviderProtocol::OpenAiChat => {
            payload["stream"] = json!(true);
            payload["stream_options"] = json!({"include_usage": true});
        }
        ProviderProtocol::OpenAiResponses | ProviderProtocol::Anthropic => {
            payload["stream"] = json!(true);
        }
        ProviderProtocol::Gemini => {
            path = path.replace(":generateContent", ":streamGenerateContent?alt=sse");
        }
        ProviderProtocol::Preview => {
            return Err(AppError::new(
                "provider_format",
                "The preview provider does not use HTTP.",
            ));
        }
    }
    Ok((path, payload))
}

/// ChatGPT-plan requests use the documented Responses route but must expose
/// client-side functions as one supported namespace. The caller also uses
/// this streaming form for buffered requests so the OAuth flow always streams.
pub fn encode_chatgpt_streaming(request: &ProviderRequest<'_>) -> AppResult<(String, Value)> {
    if request.protocol != &ProviderProtocol::OpenAiResponses {
        return Err(AppError::new(
            "openai_route_unsupported",
            "ChatGPT sign-in requires OpenAI’s Responses API.",
        ));
    }
    let (path, mut payload) = encode_streaming(request.protocol, request)?;
    if !request.tools.is_empty() {
        let functions: Vec<_> = request
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.input_schema,
                    // Native tool schemas preserve omitted optional arguments.
                    // Strict mode requires every property to be required and
                    // nullable when optional, so these schemas must opt out.
                    "strict": false
                })
            })
            .collect();
        payload["tools"] = json!([{
            "type": "namespace",
            "name": "jevcode_workspace",
            "description": "Local project and repository tools executed by JevCode after permission checks.",
            "tools": functions
        }]);
    }
    payload["store"] = json!(false);
    payload["stream"] = json!(true);
    Ok((path, payload))
}

pub fn stream_delta(protocol: &ProviderProtocol, value: &Value) -> String {
    match protocol {
        ProviderProtocol::OpenAiChat => value["choices"][0]["delta"]["content"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        ProviderProtocol::OpenAiResponses if value["type"] == "response.output_text.delta" => {
            value["delta"].as_str().unwrap_or_default().to_owned()
        }
        ProviderProtocol::Anthropic if value["type"] == "content_block_delta" => {
            if value["delta"]["type"] == "text_delta" {
                value["delta"]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned()
            } else {
                String::new()
            }
        }
        ProviderProtocol::Gemini => value["candidates"][0]["content"]["parts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|part| part["thought"] != true)
            .filter_map(|part| part["text"].as_str())
            .collect(),
        _ => String::new(),
    }
}

#[derive(Default)]
struct PartialChatTool {
    id: String,
    name: String,
    arguments: String,
}

/// Collect protocol-specific SSE frames into the same response shape used by
/// buffered adapters. Only visible text deltas leave this collector.
pub struct StreamAccumulator {
    protocol: ProviderProtocol,
    chatgpt_plan: bool,
    chat_tools: BTreeMap<u64, PartialChatTool>,
    anthropic_blocks: BTreeMap<u64, Value>,
    anthropic_arguments: BTreeMap<u64, String>,
    visible_text: String,
    gemini_parts: Vec<Value>,
    response_output_items: BTreeMap<u64, Value>,
    final_response: Option<Value>,
    response_failure: Option<String>,
    incomplete_reason: Option<String>,
    input_tokens: u64,
    usage_available: bool,
    cached_input_tokens: Option<u64>,
    output_tokens: u64,
}

fn response_stream_error(
    chatgpt_plan: bool,
    code: Option<&str>,
    incomplete_reason: Option<&str>,
) -> AppError {
    if chatgpt_plan {
        return crate::auth::openai::chatgpt_stream_error(code, incomplete_reason);
    }
    match code.unwrap_or_default() {
        "rate_limit_exceeded" | "insufficient_quota" => AppError::new(
            "quota_exhausted",
            "OpenAI reports that this account reached a request or usage limit. Check its usage and retry later.",
        ),
        "model_not_found" | "model_unavailable" => AppError::new(
            "unknown_model",
            "OpenAI no longer has this model available. Refresh the model list and choose another model.",
        ),
        _ if incomplete_reason.is_some() => AppError::new(
            "openai_response_incomplete",
            "OpenAI ended the response without completing it. Retry the request.",
        ),
        _ => AppError::new(
            "provider_outage",
            "OpenAI could not complete this response. Retry the request.",
        ),
    }
}

impl StreamAccumulator {
    #[cfg(test)]
    pub fn new(protocol: ProviderProtocol) -> Self {
        Self::new_with_chatgpt_plan(protocol, false)
    }

    pub fn new_with_chatgpt_plan(protocol: ProviderProtocol, chatgpt_plan: bool) -> Self {
        Self {
            protocol,
            chatgpt_plan,
            chat_tools: BTreeMap::new(),
            anthropic_blocks: BTreeMap::new(),
            anthropic_arguments: BTreeMap::new(),
            visible_text: String::new(),
            gemini_parts: Vec::new(),
            response_output_items: BTreeMap::new(),
            final_response: None,
            response_failure: None,
            incomplete_reason: None,
            input_tokens: 0,
            usage_available: false,
            cached_input_tokens: None,
            output_tokens: 0,
        }
    }

    pub fn push(&mut self, value: &Value) {
        if matches!(
            self.protocol,
            ProviderProtocol::OpenAiChat | ProviderProtocol::OpenAiResponses
        ) {
            self.visible_text
                .push_str(&stream_delta(&self.protocol, value));
        }
        match self.protocol {
            ProviderProtocol::OpenAiChat => {
                self.usage_available |= value["usage"]["prompt_tokens"].as_u64().is_some()
                    || value["usage"]["completion_tokens"].as_u64().is_some();
                self.input_tokens = value["usage"]["prompt_tokens"]
                    .as_u64()
                    .unwrap_or(self.input_tokens);
                self.output_tokens = value["usage"]["completion_tokens"]
                    .as_u64()
                    .unwrap_or(self.output_tokens);
                self.cached_input_tokens = value["usage"]["prompt_tokens_details"]["cached_tokens"]
                    .as_u64()
                    .or(self.cached_input_tokens);
                if let Some(calls) = value["choices"][0]["delta"]["tool_calls"].as_array() {
                    for call in calls {
                        let index = call["index"].as_u64().unwrap_or(0);
                        let partial = self.chat_tools.entry(index).or_default();
                        partial.id.push_str(call["id"].as_str().unwrap_or_default());
                        partial
                            .name
                            .push_str(call["function"]["name"].as_str().unwrap_or_default());
                        partial
                            .arguments
                            .push_str(call["function"]["arguments"].as_str().unwrap_or_default());
                    }
                }
            }
            ProviderProtocol::OpenAiResponses => {
                if value["type"] == "response.completed" {
                    let response = value.get("response").cloned();
                    self.response_failure = response
                        .as_ref()
                        .and_then(|response| response["error"]["code"].as_str())
                        .filter(|code| code.len() <= 128)
                        .map(str::to_owned)
                        .or(self.response_failure.take());
                    self.final_response = response;
                } else if value["type"] == "response.failed" || value["type"] == "error" {
                    self.response_failure = value["response"]["error"]["code"]
                        .as_str()
                        .or_else(|| value["error"]["code"].as_str())
                        .or_else(|| value["code"].as_str())
                        .filter(|code| code.len() <= 128)
                        .map(str::to_owned);
                } else if value["type"] == "response.incomplete" {
                    self.incomplete_reason = value["response"]["incomplete_details"]["reason"]
                        .as_str()
                        .filter(|reason| reason.len() <= 128)
                        .map(str::to_owned);
                }
                if value["type"] == "response.output_item.done" {
                    if let (Some(index), Some(item)) = (
                        value["output_index"].as_u64(),
                        value.get("item").filter(|item| item.is_object()),
                    ) {
                        // Keep only user-visible messages and actionable calls;
                        // reasoning items must never enter retained tool state.
                        if has_visible_response_output(std::slice::from_ref(item)) {
                            self.response_output_items.insert(index, item.clone());
                        }
                    }
                }
            }
            ProviderProtocol::Anthropic => match value["type"].as_str().unwrap_or_default() {
                "message_start" => {
                    let usage = &value["message"]["usage"];
                    self.usage_available |= usage["input_tokens"].as_u64().is_some();
                    let cache_read = usage["cache_read_input_tokens"].as_u64().unwrap_or(0);
                    let cache_write = usage["cache_creation_input_tokens"].as_u64().unwrap_or(0);
                    self.input_tokens = usage["input_tokens"].as_u64().unwrap_or(0) + cache_write;
                    self.cached_input_tokens = (cache_read > 0).then_some(cache_read);
                }
                "content_block_start" => {
                    if let Some(index) = value["index"].as_u64() {
                        self.anthropic_blocks
                            .insert(index, value["content_block"].clone());
                    }
                }
                "content_block_delta" => {
                    if let Some(index) = value["index"].as_u64() {
                        let delta = &value["delta"];
                        if delta["type"] == "text_delta" {
                            if let Some(block) = self.anthropic_blocks.get_mut(&index) {
                                let text = delta["text"].as_str().unwrap_or_default();
                                let current = block["text"].as_str().unwrap_or_default();
                                block["text"] = json!(format!("{current}{text}"));
                            }
                        } else if delta["type"] == "input_json_delta" {
                            self.anthropic_arguments
                                .entry(index)
                                .or_default()
                                .push_str(delta["partial_json"].as_str().unwrap_or_default());
                        }
                    }
                }
                "message_delta" => {
                    self.usage_available |= value["usage"]["output_tokens"].as_u64().is_some();
                    self.output_tokens = value["usage"]["output_tokens"]
                        .as_u64()
                        .unwrap_or(self.output_tokens);
                }
                _ => {}
            },
            ProviderProtocol::Gemini => {
                if let Some(parts) = value["candidates"][0]["content"]["parts"].as_array() {
                    for part in parts {
                        if part["thought"] == true {
                            // Never retain or display hidden reasoning text.
                            continue;
                        }
                        if let Some(text) = part["text"].as_str() {
                            self.visible_text.push_str(text);
                        } else if part.is_object() {
                            self.gemini_parts.push(part.clone());
                        }
                    }
                }
                self.usage_available |= value["usageMetadata"]["promptTokenCount"]
                    .as_u64()
                    .is_some()
                    || value["usageMetadata"]["candidatesTokenCount"]
                        .as_u64()
                        .is_some();
                self.input_tokens = value["usageMetadata"]["promptTokenCount"]
                    .as_u64()
                    .unwrap_or(self.input_tokens);
                self.output_tokens = value["usageMetadata"]["candidatesTokenCount"]
                    .as_u64()
                    .unwrap_or(self.output_tokens)
                    + value["usageMetadata"]["thoughtsTokenCount"]
                        .as_u64()
                        .unwrap_or(0);
            }
            ProviderProtocol::Preview => {}
        }
    }

    pub fn finish(mut self) -> AppResult<ProviderResponse> {
        match self.protocol {
            ProviderProtocol::OpenAiResponses => {
                if let Some(code) = self.response_failure.as_deref() {
                    return Err(response_stream_error(self.chatgpt_plan, Some(code), None));
                }
                if let Some(reason) = self.incomplete_reason.as_deref() {
                    return Err(response_stream_error(self.chatgpt_plan, None, Some(reason)));
                }
                let mut response = self
                    .final_response
                    .ok_or_else(|| response_stream_error(self.chatgpt_plan, None, None))?;
                let output = response["output"].as_array().cloned().unwrap_or_default();
                if !has_visible_response_output(&output) {
                    let mut recovered: Vec<Value> =
                        self.response_output_items.into_values().collect();
                    if !has_visible_response_output(&recovered) && !self.visible_text.is_empty() {
                        recovered.push(json!({
                            "type": "message",
                            "role": "assistant",
                            "content": [{"type": "output_text", "text": self.visible_text}]
                        }));
                    }
                    if has_visible_response_output(&recovered) {
                        response["output"] = json!(recovered);
                    } else if self.chatgpt_plan {
                        return Err(AppError::new(
                            "openai_empty_response",
                            "OpenAI completed this ChatGPT response without visible text or a supported tool call. Refresh the available models and retry, or choose another model.",
                        ));
                    }
                }
                decode(&self.protocol, response)
            }
            ProviderProtocol::OpenAiChat => {
                let calls: Vec<_> = self
                    .chat_tools
                    .into_values()
                    .map(|partial| {
                        json!({"id":partial.id,"type":"function","function":{"name":partial.name,"arguments":partial.arguments}})
                    })
                    .collect();
                decode(
                    &self.protocol,
                    json!({"choices":[{"message":{"role":"assistant","content":if self.visible_text.is_empty() { Value::Null } else { json!(self.visible_text) },"tool_calls":calls}}],"usage":{"prompt_tokens":self.usage_available.then_some(self.input_tokens),"prompt_tokens_details":{"cached_tokens":self.cached_input_tokens},"completion_tokens":self.usage_available.then_some(self.output_tokens)}}),
                )
            }
            ProviderProtocol::Anthropic => {
                for (index, arguments) in self.anthropic_arguments {
                    if let Some(block) = self.anthropic_blocks.get_mut(&index) {
                        block["input"] = serde_json::from_str(&arguments).unwrap_or(Value::Null);
                    }
                }
                let content: Vec<_> = self.anthropic_blocks.into_values().collect();
                decode(
                    &self.protocol,
                    json!({"content":content,"usage":{"input_tokens":self.usage_available.then_some(self.input_tokens),"cache_read_input_tokens":self.cached_input_tokens,"output_tokens":self.usage_available.then_some(self.output_tokens)}}),
                )
            }
            ProviderProtocol::Gemini => {
                let mut parts = Vec::new();
                if !self.visible_text.is_empty() {
                    parts.push(json!({"text":self.visible_text}));
                }
                parts.append(&mut self.gemini_parts);
                decode(
                    &self.protocol,
                    json!({"candidates":[{"content":{"role":"model","parts":parts}}],"usageMetadata":{"promptTokenCount":self.usage_available.then_some(self.input_tokens),"cachedContentTokenCount":self.cached_input_tokens,"candidatesTokenCount":self.usage_available.then_some(self.output_tokens)}}),
                )
            }
            ProviderProtocol::Preview => Err(AppError::new(
                "provider_format",
                "The preview provider does not use HTTP.",
            )),
        }
    }
}

fn has_visible_response_output(output: &[Value]) -> bool {
    output.iter().any(|item| {
        item["type"] == "function_call"
            || (item["type"] == "message"
                && item["content"].as_array().is_some_and(|content| {
                    content.iter().any(|part| {
                        part["text"].as_str().is_some_and(|text| !text.is_empty())
                            || part["refusal"]
                                .as_str()
                                .is_some_and(|refusal| !refusal.is_empty())
                    })
                }))
    })
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
            let definitions: Vec<_> = tools.iter().map(|tool| json!({"type":"function","name":tool.name,"description":tool.description,"parameters":tool.input_schema,"strict":false})).collect();
            Ok((
                "responses".into(),
                json!({"model":model,"instructions":system(request.messages),"input":input,"tools":definitions,"store":false}),
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
        usage_available: false,
        cached_input_tokens: None,
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
            let mut safe_message = message.clone();
            if let Some(object) = safe_message.as_object_mut() {
                for private_field in [
                    "reasoning",
                    "reasoning_content",
                    "reasoning_details",
                    "analysis",
                    "thinking",
                ] {
                    object.remove(private_field);
                }
            }
            response.provider_data = Some(safe_message);
            response.input_tokens = value["usage"]["prompt_tokens"].as_u64().unwrap_or(0);
            response.usage_available = value["usage"]["prompt_tokens"].as_u64().is_some()
                || value["usage"]["completion_tokens"].as_u64().is_some();
            response.cached_input_tokens =
                value["usage"]["prompt_tokens_details"]["cached_tokens"].as_u64();
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
            response.provider_data = Some(Value::Array(
                output
                    .iter()
                    .filter(|item| item["type"] != "reasoning")
                    .cloned()
                    .collect(),
            ));
            response.input_tokens = value["usage"]["input_tokens"].as_u64().unwrap_or(0);
            response.usage_available = value["usage"]["input_tokens"].as_u64().is_some()
                || value["usage"]["output_tokens"].as_u64().is_some();
            response.cached_input_tokens =
                value["usage"]["input_tokens_details"]["cached_tokens"].as_u64();
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
            response.provider_data = Some(Value::Array(
                content
                    .iter()
                    .filter(|part| {
                        part["type"] != "thinking" && part["type"] != "redacted_thinking"
                    })
                    .cloned()
                    .collect(),
            ));
            let usage = &value["usage"];
            let input = usage["input_tokens"].as_u64().unwrap_or(0);
            let cache_read = usage["cache_read_input_tokens"].as_u64().unwrap_or(0);
            let cache_write = usage["cache_creation_input_tokens"].as_u64().unwrap_or(0);
            response.input_tokens = input + cache_read + cache_write;
            response.usage_available = usage["input_tokens"].as_u64().is_some()
                || usage["output_tokens"].as_u64().is_some();
            response.cached_input_tokens = (cache_read > 0).then_some(cache_read);
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
            response.provider_data = Some(Value::Array(
                parts
                    .iter()
                    .filter(|part| part["thought"] != true)
                    .cloned()
                    .collect(),
            ));
            response.input_tokens = value["usageMetadata"]["promptTokenCount"]
                .as_u64()
                .unwrap_or(0);
            response.usage_available = value["usageMetadata"]["promptTokenCount"]
                .as_u64()
                .is_some()
                || value["usageMetadata"]["candidatesTokenCount"]
                    .as_u64()
                    .is_some();
            response.cached_input_tokens =
                value["usageMetadata"]["cachedContentTokenCount"].as_u64();
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
                json!({"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call","function":{"name":"list_directory","arguments":"{\"path\":\".\"}"}}]}}]}),
            ),
            (
                ProviderProtocol::OpenAiResponses,
                json!({"output":[{"type":"function_call","call_id":"call","name":"list_directory","arguments":"{\"path\":\".\"}"}]}),
            ),
            (
                ProviderProtocol::Anthropic,
                json!({"content":[{"type":"tool_use","id":"call","name":"list_directory","input":{"path":"."}}]}),
            ),
            (
                ProviderProtocol::Gemini,
                json!({"candidates":[{"content":{"parts":[{"functionCall":{"id":"call","name":"list_directory","args":{"path":"."}},"thoughtSignature":"opaque"}]}}]}),
            ),
        ];
        for (protocol, fixture) in fixtures {
            let response = decode(&protocol, fixture).unwrap();
            assert_eq!(response.tool_calls[0].name, "list_directory");
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
                    protocol: &protocol,
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
    fn hidden_reasoning_is_neither_returned_nor_retained() {
        let fixtures = [
            (
                ProviderProtocol::OpenAiChat,
                json!({"choices":[{"message":{"role":"assistant","content":"Visible","reasoning":"private thought","reasoning_content":"private thought","reasoning_details":[{"text":"private thought"}],"analysis":"private thought","thinking":"private thought"}}]}),
            ),
            (
                ProviderProtocol::OpenAiResponses,
                json!({"output":[{"type":"reasoning","summary":[{"text":"private thought"}]},{"type":"message","content":[{"type":"output_text","text":"Visible"}]}]}),
            ),
            (
                ProviderProtocol::Anthropic,
                json!({"content":[{"type":"thinking","thinking":"private thought"},{"type":"text","text":"Visible"}]}),
            ),
            (
                ProviderProtocol::Gemini,
                json!({"candidates":[{"content":{"parts":[{"text":"Visible"},{"text":"private thought","thought":true}]}}]}),
            ),
        ];
        for (protocol, fixture) in fixtures {
            let response = decode(&protocol, fixture).unwrap();
            assert_eq!(response.content, "Visible");
            let retained = serde_json::to_string(&response.provider_data).unwrap();
            assert!(!retained.contains("private thought"));
        }
    }

    #[test]
    fn normalizes_cached_input_counts_across_provider_protocols() {
        let open_ai = decode(
            &ProviderProtocol::OpenAiChat,
            json!({"choices":[{"message":{"role":"assistant","content":"ok"}}],"usage":{"prompt_tokens":100,"prompt_tokens_details":{"cached_tokens":40},"completion_tokens":10}}),
        ).unwrap();
        assert_eq!(open_ai.input_tokens, 100);
        assert_eq!(open_ai.cached_input_tokens, Some(40));

        let responses = decode(
            &ProviderProtocol::OpenAiResponses,
            json!({"output":[{"type":"message","content":[{"type":"output_text","text":"ok"}]}],"usage":{"input_tokens":100,"input_tokens_details":{"cached_tokens":50},"output_tokens":10}}),
        ).unwrap();
        assert_eq!(responses.input_tokens, 100);
        assert_eq!(responses.cached_input_tokens, Some(50));

        let anthropic = decode(
            &ProviderProtocol::Anthropic,
            json!({"content":[{"type":"text","text":"ok"}],"usage":{"input_tokens":35,"cache_read_input_tokens":55,"cache_creation_input_tokens":10,"output_tokens":10}}),
        ).unwrap();
        assert_eq!(anthropic.input_tokens, 100);
        assert_eq!(anthropic.cached_input_tokens, Some(55));

        let gemini = decode(
            &ProviderProtocol::Gemini,
            json!({"candidates":[{"content":{"parts":[{"text":"ok"}]}}],"usageMetadata":{"promptTokenCount":100,"cachedContentTokenCount":60,"candidatesTokenCount":10}}),
        ).unwrap();
        assert_eq!(gemini.input_tokens, 100);
        assert_eq!(gemini.cached_input_tokens, Some(60));

        let no_usage = decode(
            &ProviderProtocol::OpenAiChat,
            json!({"choices":[{"message":{"role":"assistant","content":"ok"}}]}),
        )
        .unwrap();
        assert!(!no_usage.usage_available);
    }

    #[test]
    fn streaming_collectors_keep_visible_text_and_tool_calls() {
        let protocol = ProviderProtocol::OpenAiChat;
        let mut stream = StreamAccumulator::new(protocol.clone());
        stream.push(&json!({"choices":[{"delta":{"content":"Hello "}}]}));
        stream.push(
            &json!({"choices":[{"delta":{"content":"world","reasoning_content":"private"}}]}),
        );
        stream.push(&json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call","function":{"name":"list_directory","arguments":json!({"path":"."}).to_string()}}]}}]}));
        let response = stream.finish().unwrap();
        assert_eq!(response.content, "Hello world");
        assert_eq!(response.tool_calls[0].name, "list_directory");
        assert_eq!(response.tool_calls[0].arguments["path"], ".");
    }

    #[test]
    fn chatgpt_plan_payload_uses_streaming_namespaced_local_tools_and_no_storage() {
        let protocol = ProviderProtocol::OpenAiResponses;
        let messages = vec![
            AgentMessage::text(MessageRole::System, "Inspect safely."),
            AgentMessage::text(MessageRole::User, "Read the project README."),
        ];
        let tools = crate::tools::definitions();
        let (_, payload) = encode_chatgpt_streaming(&ProviderRequest {
            model_id: "account-provided-slug",
            protocol: &protocol,
            messages: &messages,
            tools: &tools,
        })
        .unwrap();

        assert_eq!(payload["model"], "account-provided-slug");
        assert_eq!(payload["store"], false);
        assert_eq!(payload["stream"], true);
        assert_eq!(payload["instructions"], "Inspect safely.");
        assert_eq!(payload["input"][0]["role"], "user");
        assert_eq!(payload["tools"][0]["type"], "namespace");
        assert_eq!(payload["tools"][0]["name"], "jevcode_workspace");
        assert_eq!(payload["tools"][0]["tools"][0]["type"], "function");
        assert_eq!(payload["tools"][0]["tools"][0]["name"], tools[0].name);
        assert!(payload["tools"][0]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .all(|tool| tool["strict"] == false));
        assert!(payload.get("max_output_tokens").is_none());
        assert!(payload.get("previous_response_id").is_none());
    }

    #[test]
    fn openai_responses_tool_schemas_preserve_optional_properties_without_strict_mode() {
        let protocol = ProviderProtocol::OpenAiResponses;
        let messages = [AgentMessage::text(MessageRole::User, "List this directory")];
        let tools = crate::tools::definitions();
        let (_, payload) = encode(
            &protocol,
            &ProviderRequest {
                model_id: "account-provided-slug",
                protocol: &protocol,
                messages: &messages,
                tools: &tools,
            },
        )
        .unwrap();

        let list_directory = payload["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "list_directory")
            .unwrap();
        assert_eq!(list_directory["strict"], false);
        assert_eq!(list_directory["parameters"]["required"], json!([]));
        assert_eq!(
            list_directory["parameters"]["properties"]["path"]["type"],
            "string"
        );
    }

    #[test]
    fn chatgpt_plan_stream_round_trips_function_calls_and_usage() {
        let mut stream =
            StreamAccumulator::new_with_chatgpt_plan(ProviderProtocol::OpenAiResponses, true);
        stream
            .push(&json!({"type":"response.output_text.delta","delta":"I’ll inspect the files."}));
        stream.push(&json!({"type":"response.completed","response":{"output":[
            {"type":"message","content":[{"type":"output_text","text":"I’ll inspect the files."}]},
            {"type":"function_call","call_id":"call_123","name":"read_file","arguments":"{\"path\":\"README.md\"}"}
        ],"usage":{"input_tokens":21,"input_tokens_details":{"cached_tokens":5},"output_tokens":8}}}));
        let response = stream.finish().unwrap();
        assert_eq!(response.content, "I’ll inspect the files.");
        assert_eq!(response.tool_calls[0].id, "call_123");
        assert_eq!(response.tool_calls[0].name, "read_file");
        assert_eq!(response.tool_calls[0].arguments["path"], "README.md");
        assert_eq!(response.input_tokens, 21);
        assert_eq!(response.cached_input_tokens, Some(5));
        assert_eq!(response.output_tokens, 8);

        let mut assistant = AgentMessage::text(MessageRole::Assistant, response.content);
        assistant.provider_data = response.provider_data;
        assistant.tool_calls = response.tool_calls;
        let mut tool_result = AgentMessage::text(MessageRole::Tool, "README contents");
        tool_result.tool_result = Some(ToolResult {
            tool_call_id: "call_123".into(),
            name: "read_file".into(),
            content: "README contents".into(),
            is_error: false,
            duration_ms: 3,
            structured_content: None,
        });
        let messages = vec![assistant, tool_result];
        let tools = crate::tools::definitions();
        let (_, payload) = encode_chatgpt_streaming(&ProviderRequest {
            model_id: "account-provided-slug",
            protocol: &ProviderProtocol::OpenAiResponses,
            messages: &messages,
            tools: &tools,
        })
        .unwrap();
        assert_eq!(payload["input"][1]["type"], "function_call");
        assert_eq!(payload["input"][2]["type"], "function_call_output");
        assert_eq!(payload["input"][2]["call_id"], "call_123");
        assert_eq!(payload["input"][2]["output"], "README contents");
    }

    #[test]
    fn chatgpt_plan_stream_surfaces_usage_limits_and_incomplete_responses() {
        let mut limited =
            StreamAccumulator::new_with_chatgpt_plan(ProviderProtocol::OpenAiResponses, true);
        limited.push(&json!({"type":"response.failed","response":{"error":{"code":"subscription_sharing_usage_limit_exceeded","message":"do not expose provider body"}}}));
        let error = limited.finish().unwrap_err();
        assert_eq!(error.code, "chatgpt_plan_usage_limit");
        assert!(!error.message.contains("provider body"));

        let mut incomplete =
            StreamAccumulator::new_with_chatgpt_plan(ProviderProtocol::OpenAiResponses, true);
        incomplete.push(&json!({"type":"response.incomplete","response":{"incomplete_details":{"reason":"max_output_tokens"}}}));
        assert_eq!(
            incomplete.finish().unwrap_err().code,
            "openai_response_incomplete"
        );
    }

    #[test]
    fn responses_stream_recovers_final_output_items_when_completion_has_empty_output() {
        let mut stream = StreamAccumulator::new(ProviderProtocol::OpenAiResponses);
        stream.push(&json!({
            "type": "response.output_item.done",
            "output_index": 0,
            "item": {
                "id": "rs_123",
                "type": "reasoning",
                "summary": [{"type": "summary_text", "text": "private reasoning"}]
            }
        }));
        stream.push(&json!({
            "type": "response.output_item.done",
            "output_index": 1,
            "item": {
                "id": "msg_123",
                "status": "completed",
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "print('hello world')"}]
            }
        }));
        stream.push(&json!({
            "type": "response.output_item.done",
            "output_index": 2,
            "item": {
                "id": "fc_123",
                "status": "completed",
                "type": "function_call",
                "call_id": "call_123",
                "name": "create_file",
                "arguments": "{\"path\":\"hello.py\",\"content\":\"print('hello world')\"}"
            }
        }));
        stream.push(&json!({
            "type": "response.completed",
            "response": {"output": [], "usage": {"input_tokens": 3, "output_tokens": 4}}
        }));

        let response = stream.finish().unwrap();
        assert_eq!(response.content, "print('hello world')");
        assert_eq!(response.tool_calls[0].id, "call_123");
        assert_eq!(response.tool_calls[0].name, "create_file");
        assert_eq!(response.input_tokens, 3);
        assert_eq!(response.output_tokens, 4);
        assert!(!response
            .provider_data
            .unwrap_or_default()
            .to_string()
            .contains("private reasoning"));
    }

    #[test]
    fn responses_stream_recovers_visible_deltas_when_completion_has_no_output_items() {
        let mut stream = StreamAccumulator::new(ProviderProtocol::OpenAiResponses);
        stream.push(&json!({
            "type": "response.output_text.delta",
            "delta": "The response text arrived in the stream."
        }));
        stream.push(&json!({"type":"response.completed","response":{"output":[]}}));

        assert_eq!(
            stream.finish().unwrap().content,
            "The response text arrived in the stream."
        );
    }

    #[test]
    fn chatgpt_plan_stream_maps_top_level_error_events() {
        let mut stream =
            StreamAccumulator::new_with_chatgpt_plan(ProviderProtocol::OpenAiResponses, true);
        stream.push(&json!({
            "type": "error",
            "code": "subscription_sharing_usage_limit_exceeded",
            "message": "This provider message must not be surfaced."
        }));

        let error = stream.finish().unwrap_err();
        assert_eq!(error.code, "chatgpt_plan_usage_limit");
        assert!(!error.message.contains("provider message"));
    }

    #[test]
    fn chatgpt_plan_empty_completion_has_an_actionable_error() {
        let mut stream =
            StreamAccumulator::new_with_chatgpt_plan(ProviderProtocol::OpenAiResponses, true);
        stream.push(&json!({"type":"response.completed","response":{"output":[]}}));

        let error = stream.finish().unwrap_err();
        assert_eq!(error.code, "openai_empty_response");
        assert!(error.message.contains("Refresh the available models"));
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
                    protocol: &protocol,
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
        let response = decode(&ProviderProtocol::Gemini, json!({"candidates":[{"content":{"parts":[{"functionCall":{"name":"list_directory","args":{"path":"."}}}]}}]})).unwrap();
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
            structured_content: None,
        });
        let messages = vec![assistant, tool_message];
        let (_, payload) = encode(
            &ProviderProtocol::Gemini,
            &ProviderRequest {
                model_id: "gemini-2.5-flash",
                protocol: &ProviderProtocol::Gemini,
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
