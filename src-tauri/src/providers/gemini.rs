use super::{http::HttpProvider, LlmProvider, ProviderRequest, ProviderResponse};
use crate::{
    auth::google::{GoogleAuthManager, GoogleRequestCredential},
    domain::{Model, Provider, ProviderProtocol},
    error::{AppError, AppResult},
    state::SharedState,
};
use async_trait::async_trait;

/// Gemini Developer API transport. Only API keys are accepted here; Google
/// OAuth accounts are managed separately and are never sent to this API.
pub struct GeminiProvider {
    provider: Provider,
    state: SharedState,
    auth: GoogleAuthManager,
}

impl GeminiProvider {
    pub fn new(provider: Provider, state: SharedState) -> AppResult<Self> {
        if provider.id != "gemini" || provider.protocol != ProviderProtocol::Gemini {
            return Err(AppError::new(
                "provider_configuration",
                "The Gemini provider requires a Gemini API protocol configuration.",
            ));
        }
        let auth = GoogleAuthManager::with_app_config(provider.clone(), &state.config)?;
        Ok(Self {
            provider,
            state,
            auth,
        })
    }

    async fn transport(&self) -> AppResult<HttpProvider> {
        let GoogleRequestCredential::ApiKey(key) = self
            .auth
            .request_credential(&self.state.credentials, &self.state.database)
            .await?;
        HttpProvider::new(self.provider.clone(), Some(key.to_string()))
    }

    pub async fn get_available_models(&self) -> AppResult<Vec<Model>> {
        self.auth
            .get_available_models(&self.state.credentials, &self.state.database)
            .await
    }
}

#[async_trait]
impl LlmProvider for GeminiProvider {
    async fn complete(&self, request: ProviderRequest<'_>) -> AppResult<ProviderResponse> {
        self.transport().await?.complete(request).await
    }

    async fn complete_stream(
        &self,
        request: ProviderRequest<'_>,
        deltas: tokio::sync::mpsc::UnboundedSender<String>,
    ) -> AppResult<ProviderResponse> {
        self.transport()
            .await?
            .complete_stream(request, deltas)
            .await
    }
}
