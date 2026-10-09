use crate::{
    credentials::CredentialStore,
    domain::{
        Model, ModelCapability, ModelStatus, Provider, ProviderAccount, ProviderAccountInfo,
        ProviderAuthMethod, ProviderAuthMethodOption, ProviderAuthState, ProviderProtocol,
    },
    error::{AppError, AppResult},
};
use async_trait::async_trait;
use reqwest::{Client, StatusCode};
use serde_json::Value;
use std::time::Duration;

mod oauth;

#[async_trait]
pub trait ProviderAuthAdapter: Send + Sync {
    async fn connect(
        &self,
        secret: &str,
        credentials: &CredentialStore,
    ) -> AppResult<ProviderAccountInfo>;
    fn disconnect(&self, credentials: &CredentialStore) -> AppResult<()>;
    async fn refresh(&self, secret: &str) -> AppResult<()>;
    async fn validate(&self, secret: &str) -> AppResult<()>;
    fn get_account_info(&self) -> ProviderAccountInfo;
    async fn get_available_models(&self, secret: &str) -> AppResult<Vec<Model>>;
    fn get_auth_status(
        &self,
        credentials: &CredentialStore,
        stored: Option<ProviderAccount>,
    ) -> AppResult<ProviderAccount>;
}

pub struct ApiKeyAuthAdapter {
    provider: Provider,
    client: Client,
}

impl ApiKeyAuthAdapter {
    pub fn new(provider: Provider) -> AppResult<Self> {
        if provider.protocol == ProviderProtocol::Preview {
            return Err(AppError::new(
                "unsupported_auth_method",
                "Local preview does not need provider authentication.",
            ));
        }
        let client = Client::builder()
            .timeout(Duration::from_secs(20))
            .connect_timeout(Duration::from_secs(8))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| network_error())?;
        Ok(Self { provider, client })
    }

    fn models_url(&self) -> AppResult<String> {
        let base = self.provider.base_url.as_deref().ok_or_else(|| {
            AppError::new(
                "configuration",
                "This provider is missing its documented API base URL.",
            )
        })?;
        Ok(format!("{}/models", base.trim_end_matches('/')))
    }

    fn build_models_request(&self, secret: &str) -> AppResult<reqwest::Request> {
        if secret.trim().is_empty() || secret.len() > 16_384 {
            return Err(AppError::new(
                "invalid_credential",
                "Enter a valid provider API key.",
            ));
        }
        let mut request = self.client.get(self.models_url()?);
        request = match self.provider.protocol {
            ProviderProtocol::Anthropic => request.query(&[("limit", "1000")]),
            ProviderProtocol::Gemini => request.query(&[("pageSize", "1000")]),
            _ => request,
        };
        request = match self.provider.protocol {
            ProviderProtocol::Anthropic => request
                .header("x-api-key", secret)
                .header("anthropic-version", "2023-06-01"),
            ProviderProtocol::Gemini => request.header("x-goog-api-key", secret),
            _ => request.bearer_auth(secret),
        };
        request.build().map_err(|_| {
            AppError::new(
                "configuration",
                "Could not construct the provider model request.",
            )
        })
    }

    async fn request_models(&self, secret: &str) -> AppResult<Vec<Model>> {
        let request = self.build_models_request(secret)?;
        let mut response = self.client.execute(request).await.map_err(|error| {
            if error.is_timeout() || error.is_connect() {
                network_error()
            } else {
                provider_outage()
            }
        })?;
        if !response.status().is_success() {
            let body = read_limited(&mut response, 8192).await;
            return Err(provider_response_error(response.status(), &body));
        }

        let body = read_limited(&mut response, 2 * 1024 * 1024).await;
        let data: Value = serde_json::from_slice(&body).map_err(|_| {
            AppError::new(
                "provider_outage",
                "The provider returned an unreadable model catalog. Try again later.",
            )
        })?;
        let list = data
            .get("data")
            .or_else(|| data.get("models"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        let mut models: Vec<_> = list
            .into_iter()
            .filter_map(|item| normalize_model(&self.provider, item))
            .collect();
        models.sort_by(|left, right| {
            left.display_name
                .to_lowercase()
                .cmp(&right.display_name.to_lowercase())
        });
        models.dedup_by(|left, right| left.id == right.id);
        Ok(models)
    }
}

fn normalize_model(provider: &Provider, item: Value) -> Option<Model> {
    let raw_id = item.get("id").or_else(|| item.get("name"))?.as_str()?;
    let id = raw_id.strip_prefix("models/").unwrap_or(raw_id);
    if id.is_empty() || id.len() > 160 || !is_generating_model(provider, id, &item) {
        return None;
    }

    let configured = provider.models.iter().find(|model| model.id == id);
    let display_name = ["display_name", "displayName", "name"]
        .iter()
        .find_map(|key| item.get(key).and_then(Value::as_str))
        .filter(|name| !name.is_empty() && name.len() <= 160 && !name.starts_with("models/"))
        .map(str::to_owned)
        .or_else(|| configured.map(|model| model.display_name.clone()))
        .unwrap_or_else(|| humanize_model_id(id));

    let supports_tools = read_capability(
        &item,
        &[
            "supportsTools",
            "supports_tools",
            "tool_call",
            "tools",
            "tool_use",
        ],
    )
    .unwrap_or_else(|| {
        configured.is_some_and(|model| model.supports_tools)
            || provider.protocol != ProviderProtocol::Preview
    });
    let supports_vision = read_capability(&item, &["supportsVision", "supports_vision", "vision"])
        .or_else(|| capability_supported(&item, "vision"))
        .unwrap_or_else(|| configured.is_some_and(|model| model.supports_vision));
    let supports_reasoning = read_capability(
        &item,
        &["supportsReasoning", "supports_reasoning", "reasoning"],
    )
    .or_else(|| capability_supported(&item, "reasoning"))
    .unwrap_or_else(|| configured.is_some_and(|model| model.supports_reasoning));
    let supports_streaming = read_capability(
        &item,
        &["supportsStreaming", "supports_streaming", "streaming"],
    )
    .unwrap_or_else(|| configured.is_none_or(|model| model.supports_streaming));

    let mut capabilities = Vec::new();
    if supports_tools {
        capabilities.push(ModelCapability::Tools);
    }
    if supports_vision {
        capabilities.push(ModelCapability::Vision);
    }
    if supports_reasoning {
        capabilities.push(ModelCapability::Reasoning);
    }
    if supports_streaming {
        capabilities.push(ModelCapability::Streaming);
    }

    let status = match item
        .get("lifecycle")
        .or_else(|| item.get("status"))
        .and_then(Value::as_str)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("retired" | "unavailable" | "disabled") => ModelStatus::Unavailable,
        Some("deprecated" | "sunset") => ModelStatus::Deprecated,
        _ => configured
            .map(|model| model.status.clone())
            .unwrap_or_default(),
    };
    let numeric = |keys: &[&str]| {
        keys.iter().find_map(|key| {
            if key.starts_with('/') {
                item.pointer(key).and_then(Value::as_f64)
            } else {
                item.get(*key).and_then(Value::as_f64)
            }
        })
    };
    let input_price = numeric(&["inputPrice", "input_price", "/cost/input"])
        .or_else(|| configured.and_then(|model| model.input_price));
    let output_price = numeric(&["outputPrice", "output_price", "/cost/output"])
        .or_else(|| configured.and_then(|model| model.output_price));
    let context_window = [
        "max_input_tokens",
        "input_token_limit",
        "inputTokenLimit",
        "context_window",
        "contextWindow",
        "limit.context",
    ]
    .iter()
    .find_map(|key| {
        if *key == "limit.context" {
            item.pointer("/limit/context").and_then(Value::as_u64)
        } else {
            item.get(key).and_then(Value::as_u64)
        }
    })
    .or_else(|| configured.and_then(|model| model.context_window));

    Some(Model {
        id: id.to_owned(),
        provider: provider.id.clone(),
        display_name,
        api_protocol: opencode_model_protocol(provider, id),
        capabilities,
        supports_tools,
        supports_vision,
        supports_reasoning,
        supports_streaming,
        context_window,
        input_price,
        output_price,
        status,
    })
}

fn is_generating_model(provider: &Provider, id: &str, item: &Value) -> bool {
    // System One uses a structured classification API, not conversational
    // completion, and therefore cannot participate in the agent runtime.
    if matches!(provider.id.as_str(), "opencode-zen" | "opencode-go") && id.starts_with("jev-") {
        return false;
    }
    if provider.protocol == ProviderProtocol::Gemini {
        if let Some(methods) = item
            .get("supportedGenerationMethods")
            .or_else(|| item.get("supportedActions"))
            .and_then(Value::as_array)
        {
            return methods.iter().any(|method| {
                method
                    .as_str()
                    .is_some_and(|name| name.eq_ignore_ascii_case("generateContent"))
            });
        }
    }
    if matches!(
        provider.protocol,
        ProviderProtocol::OpenAiResponses | ProviderProtocol::OpenAiChat
    ) && [
        "embedding",
        "moderation",
        "whisper",
        "transcription",
        "tts",
        "dall-e",
        "image-",
        "realtime",
    ]
    .iter()
    .any(|unsupported| id.to_ascii_lowercase().contains(unsupported))
    {
        return false;
    }
    true
}

/// OpenCode's official `/models` catalog is OpenAI-shaped and omits each model's
/// request route. Its provider docs publish route families, so resolve those
/// families here instead of leaking provider model names into the UI/runtime.
/// Unknown conversational models use the configured OpenAI-compatible default.
fn opencode_model_protocol(provider: &Provider, id: &str) -> Option<ProviderProtocol> {
    if !matches!(provider.id.as_str(), "opencode-zen" | "opencode-go") {
        return None;
    }
    let id = id.to_ascii_lowercase();
    let protocol =
        if id.starts_with("gpt-") || id.starts_with("grok-") || id.starts_with("muse-spark-") {
            ProviderProtocol::OpenAiResponses
        } else if id.starts_with("gemini-") {
            ProviderProtocol::Gemini
        } else if id.starts_with("claude-")
            || (id.starts_with("qwen") && !id.starts_with("qwen3.8-max"))
        {
            ProviderProtocol::Anthropic
        } else {
            ProviderProtocol::OpenAiChat
        };
    Some(protocol)
}

fn read_capability(item: &Value, keys: &[&str]) -> Option<bool> {
    keys.iter().find_map(|key| {
        let value = item.get(key).or_else(|| {
            item.get("capabilities")
                .and_then(|capabilities| capabilities.get(*key))
        });
        value.and_then(|value| {
            value
                .as_bool()
                .or_else(|| value.get("supported").and_then(Value::as_bool))
        })
    })
}

fn capability_supported(item: &Value, capability: &str) -> Option<bool> {
    item.get("capabilities")?
        .get(capability)?
        .get("supported")?
        .as_bool()
}

fn humanize_model_id(id: &str) -> String {
    id.split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) if first.is_ascii_alphabetic() => {
                    first.to_ascii_uppercase().to_string() + chars.as_str()
                }
                _ => part.to_owned(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[async_trait]
impl ProviderAuthAdapter for ApiKeyAuthAdapter {
    async fn connect(
        &self,
        secret: &str,
        credentials: &CredentialStore,
    ) -> AppResult<ProviderAccountInfo> {
        // Validate before replacing the old key so a typo cannot lock out an existing account.
        self.validate(secret).await?;
        credentials.save(&self.provider.id, secret)?;
        Ok(self.get_account_info())
    }

    fn disconnect(&self, credentials: &CredentialStore) -> AppResult<()> {
        credentials.delete(&self.provider.id)
    }

    async fn refresh(&self, secret: &str) -> AppResult<()> {
        // API keys are not renewable tokens; refresh means revalidating the saved credential.
        self.validate(secret).await
    }

    async fn validate(&self, secret: &str) -> AppResult<()> {
        self.request_models(secret).await.map(|_| ())
    }

    fn get_account_info(&self) -> ProviderAccountInfo {
        ProviderAccountInfo {
            provider_id: self.provider.id.clone(),
            provider_name: self.provider.name.clone(),
            auth_method: ProviderAuthMethod::ApiKey,
            // API keys do not provide a user profile. Never infer or display one.
            account_label: "API key".into(),
        }
    }

    async fn get_available_models(&self, secret: &str) -> AppResult<Vec<Model>> {
        self.request_models(secret).await
    }

    fn get_auth_status(
        &self,
        credentials: &CredentialStore,
        stored: Option<ProviderAccount>,
    ) -> AppResult<ProviderAccount> {
        let has_secret = credentials.get(&self.provider.id)?.is_some();
        let methods = available_methods(&self.provider.id);
        if has_secret {
            let mut account = stored.unwrap_or_else(|| ProviderAccount {
                provider_id: self.provider.id.clone(),
                provider_name: self.provider.name.clone(),
                state: ProviderAuthState::Connected,
                auth_method: Some(ProviderAuthMethod::ApiKey),
                account_label: Some("API key".into()),
                connected_at: None,
                last_validated_at: None,
                last_error_code: None,
                available_methods: methods.clone(),
            });
            account.provider_name.clone_from(&self.provider.name);
            account.state = if account.last_error_code.is_some() {
                ProviderAuthState::NeedsAttention
            } else {
                ProviderAuthState::Connected
            };
            account.auth_method = Some(ProviderAuthMethod::ApiKey);
            account.account_label = Some("API key".into());
            account.available_methods = methods;
            Ok(account)
        } else {
            Ok(ProviderAccount {
                provider_id: self.provider.id.clone(),
                provider_name: self.provider.name.clone(),
                state: ProviderAuthState::NotConnected,
                auth_method: None,
                account_label: None,
                connected_at: None,
                last_validated_at: None,
                last_error_code: None,
                available_methods: methods,
            })
        }
    }
}

pub fn adapter(provider: &Provider) -> AppResult<ApiKeyAuthAdapter> {
    ApiKeyAuthAdapter::new(provider.clone())
}

/// Metadata describes flows JevCode can show without silently enabling unregistered OAuth.
pub fn available_methods(provider_id: &str) -> Vec<ProviderAuthMethodOption> {
    let mut methods = vec![ProviderAuthMethodOption {
        method: ProviderAuthMethod::ApiKey,
        label: match provider_id {
            "anthropic" => "Anthropic Console API key",
            "gemini" => "Gemini API key",
            "opencode-zen" => "OpenCode Zen API key",
            "opencode-go" => "OpenCode Go API key",
            _ => "API key",
        }
        .into(),
        available: true,
        unavailable_reason: None,
    }];
    match provider_id {
        "openai" => methods.push(oauth::chatgpt_method()),
        "gemini" => methods.push(oauth::google_method()),
        _ => {}
    }
    methods
}

pub fn initial_account(provider: &Provider) -> ProviderAccount {
    ProviderAccount {
        provider_id: provider.id.clone(),
        provider_name: provider.name.clone(),
        state: ProviderAuthState::NotConnected,
        auth_method: None,
        account_label: None,
        connected_at: None,
        last_validated_at: None,
        last_error_code: None,
        available_methods: available_methods(&provider.id),
    }
}

pub(crate) async fn read_limited(response: &mut reqwest::Response, limit: usize) -> Vec<u8> {
    let mut body = Vec::new();
    while body.len() < limit {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                let remaining = limit - body.len();
                body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
                if chunk.len() > remaining {
                    break;
                }
            }
            Ok(None) | Err(_) => break,
        }
    }
    body
}

pub(crate) fn provider_response_error(status: StatusCode, body: &[u8]) -> AppError {
    let body_text = String::from_utf8_lossy(body).to_ascii_lowercase();
    let code = error_code(&body_text);
    let category = if body_text.contains("expired") || code.contains("expired") {
        "expired_authentication"
    } else if body_text.contains("quota")
        || body_text.contains("insufficient_quota")
        || body_text.contains("billing_hard_limit")
        || status == StatusCode::TOO_MANY_REQUESTS
    {
        "quota_exhausted"
    } else if status == StatusCode::UNAUTHORIZED {
        "invalid_credential"
    } else if status == StatusCode::FORBIDDEN
        || status == StatusCode::PAYMENT_REQUIRED
        || body_text.contains("subscription")
        || body_text.contains("not_subscribed")
        || body_text.contains("billing")
    {
        "subscription_unavailable"
    } else if status.is_server_error() {
        "provider_outage"
    } else {
        "provider_rejected"
    };
    let message = match category {
        "expired_authentication" => "This provider sign-in expired. Reconnect the provider.",
        "invalid_credential" => "The provider rejected this API key. Check it in the provider console and try again.",
        "quota_exhausted" => "The provider reports that this account reached a quota or usage limit. Check its billing and limits.",
        "subscription_unavailable" => "This account lacks the provider subscription or API access required for this request.",
        "provider_outage" => "The provider service is temporarily unavailable. Try again later.",
        _ => "The provider rejected the connection test. Check the credential and provider account access.",
    };
    AppError::new(category, message)
}

fn error_code(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .pointer("/error/code")
                .or_else(|| value.pointer("/error/type"))
                .or_else(|| value.pointer("/code"))
                .and_then(Value::as_str)
                .map(str::to_ascii_lowercase)
        })
        .unwrap_or_default()
}

pub(crate) fn network_error() -> AppError {
    AppError::new(
        "network_error",
        "Could not reach the provider. Check your network connection and retry.",
    )
}

pub(crate) fn provider_outage() -> AppError {
    AppError::new(
        "provider_outage",
        "The provider service is temporarily unavailable. Try again later.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ProviderProtocol;

    fn local_provider(id: &str, protocol: ProviderProtocol, base_url: String) -> Provider {
        Provider {
            id: id.into(),
            name: id.into(),
            protocol,
            base_url: Some(base_url),
            models: Vec::new(),
            connected: false,
        }
    }

    #[test]
    fn connection_errors_are_classified_without_echoing_provider_bodies() {
        let cases = [
            (
                StatusCode::UNAUTHORIZED,
                br#"{"error":{"message":"invalid key sk-test-secret-value"}}"#.as_slice(),
                "invalid_credential",
            ),
            (
                StatusCode::UNAUTHORIZED,
                br#"{"error":{"message":"token expired"}}"#.as_slice(),
                "expired_authentication",
            ),
            (
                StatusCode::TOO_MANY_REQUESTS,
                br#"{"error":{"code":"insufficient_quota"}}"#.as_slice(),
                "quota_exhausted",
            ),
            (
                StatusCode::FORBIDDEN,
                br#"{"error":{"message":"subscription required"}}"#.as_slice(),
                "subscription_unavailable",
            ),
            (StatusCode::SERVICE_UNAVAILABLE, b"", "provider_outage"),
        ];
        for (status, body, expected) in cases {
            let error = provider_response_error(status, body);
            assert_eq!(error.code, expected);
            assert!(!error.to_string().contains("sk-test-secret-value"));
        }
    }

    #[test]
    fn provider_methods_are_gated_by_registered_oauth_capabilities() {
        let openai = available_methods("openai");
        assert!(openai[0].available);
        assert!(!openai[1].available);
        assert_eq!(openai[1].method, ProviderAuthMethod::OpenAiChatGpt);
        let gemini = available_methods("gemini");
        assert!(gemini[0].available);
        assert!(!gemini[1].available);
        assert_eq!(gemini[1].method, ProviderAuthMethod::GoogleOAuth);
    }

    #[test]
    fn model_catalog_request_uses_each_providers_documented_key_header() {
        let cases = [
            (
                "openai",
                ProviderProtocol::OpenAiResponses,
                "authorization: bearer test-secret",
            ),
            (
                "anthropic",
                ProviderProtocol::Anthropic,
                "x-api-key: test-secret",
            ),
            (
                "gemini",
                ProviderProtocol::Gemini,
                "x-goog-api-key: test-secret",
            ),
            (
                "opencode-zen",
                ProviderProtocol::OpenAiChat,
                "authorization: bearer test-secret",
            ),
            (
                "opencode-go",
                ProviderProtocol::OpenAiChat,
                "authorization: bearer test-secret",
            ),
        ];

        for (id, protocol, expected_header) in cases {
            let adapter = ApiKeyAuthAdapter::new(local_provider(
                id,
                protocol,
                "https://provider.test".into(),
            ))
            .expect("construct provider adapter");
            let request = adapter
                .build_models_request("test-secret")
                .expect("build provider request");

            assert_eq!(request.method(), reqwest::Method::GET);
            assert!(request.url().path().ends_with("/models"));
            let (name, value) = expected_header.split_once(": ").unwrap();
            let header = request
                .headers()
                .get(name)
                .expect("provider auth header")
                .to_str()
                .unwrap()
                .to_ascii_lowercase();
            assert_eq!(header, value, "{id} sent an unexpected auth header");
        }
    }

    #[test]
    fn model_catalog_normalizes_capabilities_and_filters_non_generation_models() {
        let provider = local_provider(
            "gemini",
            ProviderProtocol::Gemini,
            "https://example.com".into(),
        );
        let model = normalize_model(&provider, serde_json::json!({
            "name": "models/gemini-3-flash",
            "displayName": "Gemini 3 Flash",
            "inputTokenLimit": 1_000_000,
            "supportedGenerationMethods": ["generateContent"],
            "capabilities": { "vision": { "supported": true }, "tool_use": { "supported": true } }
        })).expect("normalize a generation model");
        assert_eq!(model.id, "gemini-3-flash");
        assert_eq!(model.display_name, "Gemini 3 Flash");
        assert_eq!(model.context_window, Some(1_000_000));
        assert!(model.supports_vision);
        assert!(model.supports_tools);
        assert_eq!(model.api_protocol, None);

        assert!(normalize_model(
            &provider,
            serde_json::json!({
                "name": "models/text-embedding-005",
                "supportedGenerationMethods": ["embedContent"]
            })
        )
        .is_none());
    }

    #[test]
    fn opencode_catalog_assigns_documented_wire_protocols_and_skips_system_one() {
        let provider = local_provider(
            "opencode-zen",
            ProviderProtocol::OpenAiChat,
            "https://example.com".into(),
        );
        let models = [
            ("gpt-6-astra", ProviderProtocol::OpenAiResponses),
            ("claude-opus-5", ProviderProtocol::Anthropic),
            ("gemini-3.8-flash", ProviderProtocol::Gemini),
            ("deepseek-v4-pro", ProviderProtocol::OpenAiChat),
            ("qwen3.8-max", ProviderProtocol::OpenAiChat),
            ("qwen3.8-flash", ProviderProtocol::Anthropic),
        ];
        for (id, expected) in models {
            let model = normalize_model(&provider, serde_json::json!({"id": id}))
                .expect("normalize supported conversational model");
            assert_eq!(model.api_protocol, Some(expected), "{id}");
        }
        assert!(normalize_model(&provider, serde_json::json!({"id":"jev-1.13"})).is_none());
    }

    #[test]
    fn rejected_connection_does_not_echo_a_secret_from_provider_response() {
        let error = provider_response_error(
            StatusCode::UNAUTHORIZED,
            br#"{"error":{"message":"invalid credential test-secret"}}"#,
        );
        assert_eq!(error.code, "invalid_credential");
        assert!(!error.to_string().contains("test-secret"));
    }
}
