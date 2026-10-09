use super::{wire, LlmProvider, ProviderRequest, ProviderResponse};
use crate::{
    auth,
    domain::{Provider, ProviderProtocol},
    error::{AppError, AppResult},
};
use async_trait::async_trait;
use std::time::Duration;

pub struct HttpProvider {
    provider: Provider,
    secret: String,
    client: reqwest::Client,
}

impl HttpProvider {
    pub fn new(provider: Provider, secret: Option<String>) -> AppResult<Self> {
        let secret = secret.ok_or_else(|| {
            AppError::new(
                "credentials_required",
                "Connect this provider with an API key in Providers.",
            )
        })?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .connect_timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(AppError::internal)?;
        Ok(Self {
            provider,
            secret,
            client,
        })
    }
}

#[async_trait]
impl LlmProvider for HttpProvider {
    async fn complete(&self, request: ProviderRequest<'_>) -> AppResult<ProviderResponse> {
        let base = self
            .provider
            .base_url
            .as_deref()
            .unwrap_or_default()
            .trim_end_matches('/');
        let (path, payload) = wire::encode(request.protocol, &request)?;
        let builder = self.client.post(format!("{base}/{path}")).json(&payload);
        let builder = match self.provider.protocol {
            ProviderProtocol::Anthropic => builder
                .header("x-api-key", &self.secret)
                .header("anthropic-version", "2023-06-01"),
            ProviderProtocol::Gemini => builder.header("x-goog-api-key", &self.secret),
            _ => builder.bearer_auth(&self.secret),
        };
        let mut response = builder.send().await.map_err(|error| {
            if error.is_timeout() || error.is_connect() {
                auth::network_error()
            } else {
                auth::provider_outage()
            }
        })?;
        let status = response.status();
        if !status.is_success() {
            tracing::warn!(provider_id = %self.provider.id, status = status.as_u16(), "Provider rejected request");
            let body = auth::read_limited(&mut response, 8192).await;
            return Err(auth::provider_response_error(status, &body));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| auth::network_error())? {
            if bytes.len() + chunk.len() > 4 * 1024 * 1024 {
                return Err(AppError::new(
                    "response_limit",
                    "The provider response exceeded the 4 MiB limit.",
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        let value = serde_json::from_slice(&bytes).map_err(|_| {
            AppError::new(
                "provider_format",
                "The provider returned an invalid JSON response.",
            )
        })?;
        wire::decode(request.protocol, value)
    }
}
