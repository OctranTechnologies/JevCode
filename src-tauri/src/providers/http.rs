use super::{wire, LlmProvider, ProviderRequest, ProviderResponse};
use crate::{
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
        let (path, payload) = wire::encode(&self.provider.protocol, &request)?;
        let builder = self.client.post(format!("{base}/{path}")).json(&payload);
        let builder = match self.provider.protocol {
            ProviderProtocol::Anthropic => builder
                .header("x-api-key", &self.secret)
                .header("anthropic-version", "2023-06-01"),
            ProviderProtocol::Gemini => builder.header("x-goog-api-key", &self.secret),
            _ => builder.bearer_auth(&self.secret),
        };
        let mut response = builder.send().await.map_err(|error| {
            if error.is_timeout() {
                AppError::new(
                    "provider_timeout",
                    "The provider timed out. Retry or choose another model.",
                )
            } else {
                AppError::new(
                    "provider_network",
                    "Could not reach the provider. Check your connection and configuration.",
                )
            }
        })?;
        let status = response.status();
        if !status.is_success() {
            tracing::warn!(provider_id = %self.provider.id, status = status.as_u16(), "Provider rejected request");
            return Err(match status.as_u16() {
                401 | 403 => AppError::new("provider_auth", "The provider rejected your credentials. Update the API key or check account access."),
                429 => AppError::new("provider_rate_limit", "The provider rate limit or quota was reached. Check your account and retry later."),
                _ => AppError::new("provider_error", format!("The provider returned HTTP {}. Check the configured model and try again.", status.as_u16())),
            });
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| {
            AppError::new("provider_network", "The provider response was interrupted.")
        })? {
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
        wire::decode(&self.provider.protocol, value)
    }
}
