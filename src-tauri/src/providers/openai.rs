use super::{http::HttpProvider, LlmProvider, ProviderRequest, ProviderResponse};
use crate::{
    auth,
    credentials::CredentialStore,
    domain::{Provider, ProviderProtocol},
    error::{AppError, AppResult},
};
use async_trait::async_trait;

/// OpenAI transport adapter. Authentication resolution remains in the native
/// auth module, while request/response normalization stays behind `LlmProvider`.
pub struct OpenAIProvider {
    http: HttpProvider,
}

impl OpenAIProvider {
    pub fn new(provider: Provider, credentials: &CredentialStore) -> AppResult<Self> {
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
        let credential = auth::openai_request_credential(&provider, credentials)?;
        Ok(Self {
            http: HttpProvider::new(provider, Some(credential.into_secret()))?,
        })
    }
}

#[async_trait]
impl LlmProvider for OpenAIProvider {
    async fn complete(&self, request: ProviderRequest<'_>) -> AppResult<ProviderResponse> {
        self.http.complete(request).await
    }

    async fn complete_stream(
        &self,
        request: ProviderRequest<'_>,
        deltas: tokio::sync::mpsc::UnboundedSender<String>,
    ) -> AppResult<ProviderResponse> {
        self.http.complete_stream(request, deltas).await
    }
}
