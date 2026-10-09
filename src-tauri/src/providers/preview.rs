use super::{LlmProvider, ProviderRequest, ProviderResponse};
use crate::{domain::*, error::AppResult};
use async_trait::async_trait;
use serde_json::json;

pub struct PreviewProvider;

/// A deterministic adapter exercising real project tools. Never claims to be an LLM.
#[async_trait]
impl LlmProvider for PreviewProvider {
    async fn complete(&self, request: ProviderRequest<'_>) -> AppResult<ProviderResponse> {
        let last = request.messages.last();
        let (content, tool_calls) = if last.is_some_and(|message| message.role == MessageRole::User)
        {
            let prompt = last.unwrap().content.to_lowercase();
            let (name, arguments) = if prompt.contains("git") {
                ("git_status", json!({}))
            } else if prompt.contains("architecture") || prompt.contains("readme") {
                ("read_file", json!({"path":"README.md"}))
            } else {
                ("list_directory", json!({"path":".","limit":50}))
            };
            (
                "I’ll inspect the selected project using a local read-only tool.".into(),
                vec![ToolCall {
                    id: id(),
                    name: name.into(),
                    arguments,
                }],
            )
        } else {
            let result = last.and_then(|message| message.tool_result.as_ref());
            let text = match result {
                Some(result) if result.is_error => format!("Local preview could not finish: {}", result.content),
                Some(result) => format!("Local preview inspected your project with {}.\n\n{}\n\nConnect a provider in Providers for reasoning and coding assistance. This preview is deterministic and runs entirely on your computer.", result.name, result.content),
                None => "Local preview is ready. Ask to explore the project, read its README, or review Git status.".into(),
            };
            (text, vec![])
        };
        Ok(ProviderResponse {
            content,
            tool_calls,
            provider_data: None,
            input_tokens: 0,
            output_tokens: 0,
        })
    }
}
