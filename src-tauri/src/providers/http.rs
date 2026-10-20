use super::{wire, LlmProvider, ProviderRequest, ProviderResponse};
use crate::{
    auth,
    domain::{Provider, ProviderProtocol},
    error::{AppError, AppResult},
};
use async_trait::async_trait;
use std::time::Duration;
use zeroize::Zeroizing;

pub struct HttpProvider {
    provider: Provider,
    secret: Zeroizing<String>,
    client: reqwest::Client,
    chatgpt_plan: bool,
    google_cloud_project: Option<String>,
}

impl HttpProvider {
    pub fn new(provider: Provider, secret: Option<String>) -> AppResult<Self> {
        Self::with_mode(provider, secret, false)
    }

    pub(crate) fn new_chatgpt_plan(provider: Provider, access_token: String) -> AppResult<Self> {
        if provider.id != "openai"
            || provider.protocol != ProviderProtocol::OpenAiResponses
            || provider.base_url.as_deref() != Some("https://api.openai.com/v1")
        {
            return Err(AppError::new(
                "openai_route_unsupported",
                "ChatGPT sign-in is restricted to OpenAI’s public Responses API host.",
            ));
        }
        Self::with_mode(provider, Some(access_token), true)
    }

    pub(crate) fn new_gemini_oauth(
        provider: Provider,
        access_token: String,
        project_id: String,
    ) -> AppResult<Self> {
        let base_url = provider.base_url.as_deref().unwrap_or_default();
        if provider.id != "gemini"
            || provider.protocol != ProviderProtocol::Gemini
            || base_url != "https://generativelanguage.googleapis.com/v1beta"
            || project_id.trim().is_empty()
        {
            return Err(AppError::new(
                "google_gemini_oauth_unavailable",
                "Google Account requests are restricted to the official Gemini API and a configured Cloud project.",
            ));
        }
        let mut transport = Self::with_mode(provider, Some(access_token), false)?;
        transport.google_cloud_project = Some(project_id);
        Ok(transport)
    }

    #[cfg(test)]
    fn new_chatgpt_plan_for_test(provider: Provider, access_token: String) -> AppResult<Self> {
        Self::with_mode(provider, Some(access_token), true)
    }

    #[cfg(test)]
    fn new_gemini_oauth_for_test(
        provider: Provider,
        access_token: String,
        project_id: String,
    ) -> AppResult<Self> {
        if provider.id != "gemini" || provider.protocol != ProviderProtocol::Gemini {
            return Err(AppError::new(
                "provider_configuration",
                "The Gemini OAuth test transport requires the Gemini provider.",
            ));
        }
        let mut transport = Self::with_mode(provider, Some(access_token), false)?;
        transport.google_cloud_project = Some(project_id);
        Ok(transport)
    }

    fn with_mode(
        provider: Provider,
        secret: Option<String>,
        chatgpt_plan: bool,
    ) -> AppResult<Self> {
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
            secret: Zeroizing::new(secret),
            client,
            chatgpt_plan,
            google_cloud_project: None,
        })
    }
}

#[async_trait]
impl LlmProvider for HttpProvider {
    async fn complete(&self, request: ProviderRequest<'_>) -> AppResult<ProviderResponse> {
        if self.chatgpt_plan {
            let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
            drop(receiver);
            return self.complete_stream(request, sender).await;
        }
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
                .header("x-api-key", self.secret.as_str())
                .header("anthropic-version", "2023-06-01"),
            ProviderProtocol::Gemini => match &self.google_cloud_project {
                Some(project_id) => builder
                    .bearer_auth(self.secret.as_str())
                    .header("x-goog-user-project", project_id),
                None => builder.header("x-goog-api-key", self.secret.as_str()),
            },
            _ => builder.bearer_auth(self.secret.as_str()),
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
            return Err(self.response_error(status, &body));
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
        let (path, payload) = if self.chatgpt_plan {
            wire::encode_chatgpt_streaming(&request)?
        } else {
            wire::encode_streaming(request.protocol, &request)?
        };
        let builder = self.client.post(format!("{base}/{path}")).json(&payload);
        let builder = match self.provider.protocol {
            ProviderProtocol::Anthropic => builder
                .header("x-api-key", self.secret.as_str())
                .header("anthropic-version", "2023-06-01"),
            ProviderProtocol::Gemini => match &self.google_cloud_project {
                Some(project_id) => builder
                    .bearer_auth(self.secret.as_str())
                    .header("x-goog-user-project", project_id),
                None => builder.header("x-goog-api-key", self.secret.as_str()),
            },
            _ => builder.bearer_auth(self.secret.as_str()),
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
            return Err(self.response_error(status, &body));
        }

        let mut accumulator = wire::StreamAccumulator::new_with_chatgpt_plan(
            request.protocol.clone(),
            self.chatgpt_plan,
        );
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

impl HttpProvider {
    fn response_error(&self, status: reqwest::StatusCode, body: &[u8]) -> AppError {
        if self.chatgpt_plan {
            crate::auth::openai::chatgpt_response_error(status, body)
        } else {
            auth::provider_response_error(status, body)
        }
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
    use crate::domain::{AgentMessage, MessageRole};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

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

    async fn read_request(stream: &mut tokio::net::TcpStream) -> String {
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 2048];
        let (header_end, content_length) = loop {
            let count = stream.read(&mut chunk).await.unwrap();
            assert!(count > 0, "request ended before its HTTP headers arrived");
            bytes.extend_from_slice(&chunk[..count]);
            if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                let header = String::from_utf8_lossy(&bytes[..end]);
                let content_length = header
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                break (end + 4, content_length);
            }
        };
        while bytes.len() < header_end + content_length {
            let count = stream.read(&mut chunk).await.unwrap();
            assert!(count > 0, "request body ended before Content-Length");
            bytes.extend_from_slice(&chunk[..count]);
        }
        String::from_utf8(bytes).unwrap()
    }

    #[tokio::test]
    async fn chatgpt_plan_transport_uses_bearer_responses_stream_and_no_api_key_header() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_request(&mut stream).await;
            let body = concat!(
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hello\"}\n\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"Hello\"}]}],\"usage\":{\"input_tokens\":2,\"output_tokens\":1}}}\n\n",
                "data: [DONE]\n\n"
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            request
        });

        let provider = Provider {
            id: "openai".into(),
            name: "OpenAI".into(),
            protocol: ProviderProtocol::OpenAiResponses,
            base_url: Some(format!("http://{address}/v1")),
            models: vec![],
            connected: true,
        };
        let transport =
            HttpProvider::new_chatgpt_plan_for_test(provider, "test-oauth-access-token".into())
                .unwrap();
        let messages = [AgentMessage::text(MessageRole::User, "Say hello")];
        let (deltas, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let response = transport
            .complete_stream(
                ProviderRequest {
                    model_id: "catalog-slug",
                    protocol: &ProviderProtocol::OpenAiResponses,
                    messages: &messages,
                    tools: &[],
                },
                deltas,
            )
            .await
            .unwrap();
        assert_eq!(response.content, "Hello");
        assert_eq!(receiver.recv().await.as_deref(), Some("Hello"));

        let request = server.await.unwrap();
        let (headers, body) = request.split_once("\r\n\r\n").unwrap();
        assert!(headers.starts_with("POST /v1/responses HTTP/1.1"));
        assert!(headers
            .to_ascii_lowercase()
            .contains("authorization: bearer test-oauth-access-token"));
        assert!(!headers.to_ascii_lowercase().contains("x-api-key"));
        assert!(!headers.to_ascii_lowercase().contains("api-key"));
        let payload: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(payload["model"], "catalog-slug");
        assert_eq!(payload["store"], false);
        assert_eq!(payload["stream"], true);
    }

    #[tokio::test]
    async fn google_account_transport_uses_bearer_and_selected_cloud_project() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_request(&mut stream).await;
            let body = concat!(
                "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Hello\"}],\"role\":\"model\"}}],\"usageMetadata\":{\"promptTokenCount\":2,\"candidatesTokenCount\":1}}\n\n",
                "data: [DONE]\n\n"
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            request
        });

        let provider = Provider {
            id: "gemini".into(),
            name: "Google Gemini".into(),
            protocol: ProviderProtocol::Gemini,
            base_url: Some(format!("http://{address}/v1beta")),
            models: vec![],
            connected: true,
        };
        let transport = HttpProvider::new_gemini_oauth_for_test(
            provider,
            "test-google-access-token".into(),
            "billing-project-123".into(),
        )
        .unwrap();
        let messages = [AgentMessage::text(MessageRole::User, "Say hello")];
        let (deltas, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let response = transport
            .complete_stream(
                ProviderRequest {
                    model_id: "gemini-test-model",
                    protocol: &ProviderProtocol::Gemini,
                    messages: &messages,
                    tools: &[],
                },
                deltas,
            )
            .await
            .unwrap();
        assert_eq!(response.content, "Hello");
        assert_eq!(receiver.recv().await.as_deref(), Some("Hello"));

        let request = server.await.unwrap();
        let (headers, body) = request.split_once("\r\n\r\n").unwrap();
        let normalized_headers = headers.to_ascii_lowercase();
        assert!(headers.starts_with(
            "POST /v1beta/models/gemini-test-model:streamGenerateContent?alt=sse HTTP/1.1"
        ));
        assert!(normalized_headers.contains("authorization: bearer test-google-access-token"));
        assert!(normalized_headers.contains("x-goog-user-project: billing-project-123"));
        assert!(!normalized_headers.contains("x-goog-api-key"));
        let payload: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(payload["contents"][0]["parts"][0]["text"], "Say hello");
    }
}
