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

    async fn complete_stream(
        &self,
        request: ProviderRequest<'_>,
        deltas: tokio::sync::mpsc::UnboundedSender<String>,
    ) -> AppResult<ProviderResponse> {
        let base = self
            .provider
            .base_url
            .as_deref()
            .unwrap_or_default()
            .trim_end_matches('/');
        let (path, payload) = wire::encode_streaming(request.protocol, &request)?;
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
            tracing::warn!(provider_id = %self.provider.id, status = status.as_u16(), "Provider rejected streaming request");
            let body = auth::read_limited(&mut response, 8192).await;
            return Err(auth::provider_response_error(status, &body));
        }

        let mut accumulator = wire::StreamAccumulator::new(request.protocol.clone());
        let mut pending = Vec::new();
        let mut total_bytes = 0usize;
        while let Some(chunk) = response.chunk().await.map_err(|_| auth::network_error())? {
            total_bytes += chunk.len();
            if total_bytes > 4 * 1024 * 1024 {
                return Err(AppError::new(
                    "response_limit",
                    "The provider response exceeded the 4 MiB limit.",
                ));
            }
            pending.extend_from_slice(&chunk);
            for line in take_sse_lines(&mut pending) {
                if let Some(data) = sse_data(&line) {
                    if data == "[DONE]" {
                        return accumulator.finish();
                    }
                    let value = serde_json::from_str::<serde_json::Value>(data).map_err(|_| {
                        AppError::new(
                            "provider_format",
                            "The provider returned an invalid streaming event.",
                        )
                    })?;
                    let delta = wire::stream_delta(request.protocol, &value);
                    if !delta.is_empty() {
                        let _ = deltas.send(delta);
                    }
                    accumulator.push(&value);
                }
            }
        }
        let final_line = String::from_utf8_lossy(&pending);
        if let Some(data) = sse_data(&final_line) {
            if data != "[DONE]" {
                let value = serde_json::from_str::<serde_json::Value>(data).map_err(|_| {
                    AppError::new(
                        "provider_format",
                        "The provider returned an invalid streaming event.",
                    )
                })?;
                let delta = wire::stream_delta(request.protocol, &value);
                if !delta.is_empty() {
                    let _ = deltas.send(delta);
                }
                accumulator.push(&value);
            }
        }
        accumulator.finish()
    }
}

fn sse_data(line: &str) -> Option<&str> {
    line.trim_end_matches(['\r', '\n'])
        .strip_prefix("data:")
        .map(str::trim_start)
}

fn take_sse_lines(pending: &mut Vec<u8>) -> Vec<String> {
    let mut lines = Vec::new();
    while let Some(newline) = pending.iter().position(|byte| *byte == b'\n') {
        let bytes = pending.drain(..=newline).collect::<Vec<_>>();
        lines.push(String::from_utf8_lossy(&bytes).into_owned());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_buffer_preserves_utf8_split_across_transport_chunks() {
        let frame = "data: {\"text\":\"नमस्ते\"}\n".as_bytes();
        let split = frame.iter().position(|byte| *byte >= 0x80).unwrap() + 1;
        let mut pending = frame[..split].to_vec();
        assert!(take_sse_lines(&mut pending).is_empty());
        pending.extend_from_slice(&frame[split..]);
        assert_eq!(
            take_sse_lines(&mut pending),
            ["data: {\"text\":\"नमस्ते\"}\n"]
        );
        assert!(pending.is_empty());
    }
}
