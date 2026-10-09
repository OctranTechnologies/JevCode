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
    pub usage_available: bool,
    /// Cached input tokens are a subset of input_tokens when the API reports them.
    pub cached_input_tokens: Option<u64>,
    pub output_tokens: u64,
}

/// Runtime depends on this trait, never on a provider's HTTP schema.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn complete(&self, request: ProviderRequest<'_>) -> AppResult<ProviderResponse>;

    /// Stream user-visible text while normalizing the final response and tool calls.
    /// Providers without a streaming transport use the buffered fallback.
    async fn complete_stream(
        &self,
        request: ProviderRequest<'_>,
        deltas: tokio::sync::mpsc::UnboundedSender<String>,
    ) -> AppResult<ProviderResponse> {
        let response = self.complete(request).await?;
        if !response.content.is_empty() {
            let _ = deltas.send(response.content.clone());
        }
        Ok(response)
    }
}

pub fn adapter(provider: &Provider, secret: Option<String>) -> AppResult<Box<dyn LlmProvider>> {
    if provider.protocol == ProviderProtocol::Preview {
        return Ok(Box::new(preview::PreviewProvider));
    }
    Ok(Box::new(http::HttpProvider::new(provider.clone(), secret)?))
}
