mod http;
mod preview;
mod wire;

use crate::{domain::*, error::AppResult};
use async_trait::async_trait;
use serde_json::Value;

pub struct ProviderRequest<'a> {
    pub model_id: &'a str,
    pub protocol: &'a crate::domain::ProviderProtocol,
    pub messages: &'a [AgentMessage],
    pub tools: &'a [Tool],
}

#[derive(Debug)]
pub struct ProviderResponse {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub provider_data: Option<Value>,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// Runtime depends on this trait, never on a provider's HTTP schema.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn complete(&self, request: ProviderRequest<'_>) -> AppResult<ProviderResponse>;
}

pub fn adapter(provider: &Provider, secret: Option<String>) -> AppResult<Box<dyn LlmProvider>> {
    if provider.protocol == ProviderProtocol::Preview {
        return Ok(Box::new(preview::PreviewProvider));
    }
    Ok(Box::new(http::HttpProvider::new(provider.clone(), secret)?))
}
