use super::{http::HttpProvider, LlmProvider, ProviderRequest, ProviderResponse};
use crate::{
    auth::google::{GoogleAuthManager, GoogleRequestAuthSelection, GoogleRequestCredential},
    domain::{Model, Provider, ProviderProtocol},
    error::{AppError, AppResult},
    state::SharedState,
};
use async_trait::async_trait;

/// Gemini Developer API transport. The selected API-key or Google OAuth route
/// is captured for this provider instance and never falls back to the other.
pub struct GeminiProvider {
    provider: Provider,
    state: SharedState,
    auth: GoogleAuthManager,
    request_selection: GoogleRequestAuthSelection,
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
        // Pin the selected auth route/account for this provider instance. A task
        // keeps using the account it started with even if Settings changes it.
        let request_selection = auth.request_auth_selection(&state.credentials, &state.database)?;
        Ok(Self {
            provider,
            state,
            auth,
            request_selection,
        })
    }

    async fn transport(&self) -> AppResult<HttpProvider> {
        match self
            .auth
            .request_credential_for_selection(
                &self.request_selection,
                &self.state.credentials,
                &self.state.database,
            )
            .await?
        {
            GoogleRequestCredential::ApiKey(key) => {
                HttpProvider::new(self.provider.clone(), Some(key.to_string()))
            }
            GoogleRequestCredential::OAuth {
                access_token,
                project_id,
            } => HttpProvider::new_gemini_oauth(
                self.provider.clone(),
                access_token.to_string(),
                project_id,
            ),
        }
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
