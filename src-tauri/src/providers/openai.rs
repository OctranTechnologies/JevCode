use super::{http::HttpProvider, LlmProvider, ProviderRequest, ProviderResponse};
use crate::{
    domain::{Provider, ProviderProtocol},
    error::{AppError, AppResult},
    state::SharedState,
};
use async_trait::async_trait;

/// OpenAI transport adapter. Authentication resolution remains in the native
/// auth module, while request/response normalization stays behind `LlmProvider`.
pub struct OpenAIProvider {
    provider: Provider,
    state: SharedState,
    auth: crate::auth::openai::OpenAIAuthManager,
}

impl OpenAIProvider {
    pub fn new(provider: Provider, state: SharedState) -> AppResult<Self> {
        if provider.id != "openai"
            || !matches!(
                provider.protocol,
                ProviderProtocol::OpenAiResponses | ProviderProtocol::OpenAiChat
            )
        {
            return Err(AppError::new(
                "provider_configuration",
                "The OpenAI provider requires an OpenAI API protocol configuration.",
            ));
        }
        Ok(Self {
            auth: crate::auth::openai::OpenAIAuthManager::new(provider.clone())?,
            provider,
            state,
        })
    }

    async fn transport(&self, protocol: &ProviderProtocol) -> AppResult<HttpProvider> {
        let credential = self
            .auth
            .request_credential(&self.state.credentials, &self.state.database)
            .await?;
        if matches!(
            &credential,
            crate::auth::openai::OpenAiRequestCredential::ChatGptAccessToken(_)
        ) && (protocol != &ProviderProtocol::OpenAiResponses
            || self.provider.base_url.as_deref() != Some("https://api.openai.com/v1"))
        {
            return Err(AppError::new(
                "openai_route_unsupported",
                "ChatGPT sign-in can only be used with OpenAI’s Responses API at the official API host.",
            ));
        }
        HttpProvider::new(self.provider.clone(), Some(credential.into_secret()))
    }
}

#[async_trait]
impl LlmProvider for OpenAIProvider {
    async fn complete(&self, request: ProviderRequest<'_>) -> AppResult<ProviderResponse> {
        self.transport(request.protocol)
            .await?
            .complete(request)
            .await
    }

    async fn complete_stream(
        &self,
        request: ProviderRequest<'_>,
        deltas: tokio::sync::mpsc::UnboundedSender<String>,
    ) -> AppResult<ProviderResponse> {
        self.transport(request.protocol)
            .await?
            .complete_stream(request, deltas)
            .await
    }
}
