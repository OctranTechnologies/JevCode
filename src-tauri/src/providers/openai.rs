use super::{http::HttpProvider, LlmProvider, ProviderRequest, ProviderResponse};
use crate::{
    domain::{Model, Provider, ProviderProtocol},
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

    async fn transport(&self, protocol: &ProviderProtocol) -> AppResult<(HttpProvider, bool)> {
        let credential = self
            .auth
            .request_credential(&self.state.credentials, &self.state.database)
            .await?;
        match credential {
            crate::auth::openai::OpenAiRequestCredential::ApiKey(secret) => Ok((
                HttpProvider::new(self.provider.clone(), Some(secret))?,
                false,
            )),
            crate::auth::openai::OpenAiRequestCredential::ChatGptAccessToken(token) => {
                if protocol != &ProviderProtocol::OpenAiResponses
                    || self.provider.base_url.as_deref() != Some("https://api.openai.com/v1")
                {
                    return Err(AppError::new(
                        "openai_route_unsupported",
                        "ChatGPT sign-in can only be used with OpenAI’s public Responses API host. JevCode will not change the billing method.",
                    ));
                }
                Ok((
                    HttpProvider::new_chatgpt_plan(self.provider.clone(), token)?,
                    true,
                ))
            }
        }
    }

    pub async fn get_available_models(&self) -> AppResult<Vec<Model>> {
        self.auth
            .get_available_models(&self.state.credentials, &self.state.database)
            .await
    }

    fn handle_chatgpt_error<T>(&self, chatgpt_plan: bool, error: AppError) -> AppResult<T> {
        if chatgpt_plan && error.code == "openai_reconnect_required" {
            if let Err(mark_error) = self
                .auth
                .mark_active_needs_reconnect(&self.state.credentials, &self.state.database)
            {
                tracing::warn!(code = %mark_error.code, "Could not mark the ChatGPT account for reconnection");
            }
        }
        Err(error)
    }
}

#[async_trait]
impl LlmProvider for OpenAIProvider {
    async fn complete(&self, request: ProviderRequest<'_>) -> AppResult<ProviderResponse> {
        let (transport, chatgpt_plan) = self.transport(request.protocol).await?;
        match transport.complete(request).await {
            Ok(response) => Ok(response),
            Err(error) => self.handle_chatgpt_error(chatgpt_plan, error),
        }
    }

    async fn complete_stream(
        &self,
        request: ProviderRequest<'_>,
        deltas: tokio::sync::mpsc::UnboundedSender<String>,
    ) -> AppResult<ProviderResponse> {
        let (transport, chatgpt_plan) = self.transport(request.protocol).await?;
        match transport.complete_stream(request, deltas).await {
            Ok(response) => Ok(response),
            Err(error) => self.handle_chatgpt_error(chatgpt_plan, error),
        }
    }
}
