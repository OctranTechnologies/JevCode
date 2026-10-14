use super::{ApiKeyAuthAdapter, ProviderAuthAdapter};
use crate::{
    credentials::CredentialStore,
    domain::{
        OpenAiAccount, OpenAiAuthStatus, OpenAiChatGptAuthStatus, Provider, ProviderAccount,
        ProviderAccountInfo,
    },
    error::{AppError, AppResult},
    persistence::Database,
};
use async_trait::async_trait;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const OPENAI_AUTHORIZE_URL: &str = "https://auth.openai.com/api/accounts/authorize";
const OPENAI_RESOURCE: &str = "https://api.openai.com/v1";
const OPENAI_SCOPES: &str =
    "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
const OPENAI_AGENT_NAME: &str = "JevCode";

/// Native-only request material for an in-progress sign-in attempt. It is
/// deliberately neither serializable nor debuggable and never enters Tauri IPC.
pub(crate) struct PendingOpenAiAuth {
    pub(crate) attempt_id: String,
    pub(crate) state: String,
    #[allow(dead_code)]
    pub(crate) nonce: String,
    #[allow(dead_code)]
    pub(crate) code_verifier: String,
    #[allow(dead_code)]
    pub(crate) code_challenge: String,
    #[allow(dead_code)]
    pub(crate) redirect_uri: String,
    #[allow(dead_code)]
    pub(crate) authorization_url: Url,
    #[allow(dead_code)]
    pub(crate) expires_at: DateTime<Utc>,
}

/// OpenAI credentials are only resolved inside the native provider layer.
/// ChatGPT credentials are accepted only after the full callback, code exchange,
/// and ID-token validation flow is implemented.
pub(crate) enum OpenAiRequestCredential {
    ApiKey(String),
    #[allow(dead_code)]
    ChatGptAccessToken(String),
}

impl OpenAiRequestCredential {
    pub(crate) fn into_secret(self) -> String {
        match self {
            Self::ApiKey(secret) | Self::ChatGptAccessToken(secret) => secret,
        }
    }
}

/// Native-only OAuth token record. It is serialized only to a keychain item,
/// never returned through Tauri IPC or written to SQLite/logs.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OpenAiSession {
    account_id: String,
    issued_client_id: String,
    ext_agent_host_id: String,
    access_token: String,
    refresh_token: String,
    id_token: String,
    token_expires_at: String,
    granted_scopes: Vec<String>,
}

pub struct OpenAIAuthManager {
    api_key: ApiKeyAuthAdapter,
}

impl OpenAIAuthManager {
    pub fn new(provider: Provider) -> AppResult<Self> {
        if provider.id != "openai" {
            return Err(AppError::new(
                "provider_configuration",
                "The OpenAI auth manager can only be used for the OpenAI provider.",
            ));
        }
        Ok(Self {
            api_key: ApiKeyAuthAdapter::new(provider)?,
        })
    }

    pub fn request_credential(
        &self,
        credentials: &CredentialStore,
    ) -> AppResult<OpenAiRequestCredential> {
        credentials
            .get("openai")?
            .map(OpenAiRequestCredential::ApiKey)
            .ok_or_else(|| {
                AppError::new(
                    "credentials_required",
                    "Connect OpenAI with an API key. Sign in with ChatGPT is not enabled yet.",
                )
            })
    }

    /// Prepare one fresh first-time ChatGPT sign-in attempt using OpenAI's
    /// documented dynamic-client registration flow.
    pub(crate) fn create_authentication_session(
        &self,
        redirect_uri: &str,
        ext_agent_host_id: &str,
    ) -> AppResult<PendingOpenAiAuth> {
        validate_loopback_redirect_uri(redirect_uri)?;
        validate_host_id(ext_agent_host_id)?;

        let attempt_id = uuid::Uuid::new_v4().to_string();
        let state = create_state();
        let nonce = create_state();
        let (code_verifier, code_challenge) = create_pkce_pair();
        let mut authorization_url = Url::parse(OPENAI_AUTHORIZE_URL).map_err(|_| {
            AppError::new(
                "auth_configuration",
                "OpenAI sign-in is not configured correctly.",
            )
        })?;
        authorization_url.query_pairs_mut().extend_pairs([
            ("client_id", "dynamic_agent_client"),
            ("agent_name_hint", OPENAI_AGENT_NAME),
            ("ext_agent_host_id", ext_agent_host_id),
            ("response_type", "code"),
            ("redirect_uri", redirect_uri),
            ("scope", OPENAI_SCOPES),
            ("resource", OPENAI_RESOURCE),
            ("state", state.as_str()),
            ("nonce", nonce.as_str()),
            ("code_challenge_method", "S256"),
            ("code_challenge", code_challenge.as_str()),
        ]);

        Ok(PendingOpenAiAuth {
            attempt_id,
            state,
            nonce,
            code_verifier,
            code_challenge,
            redirect_uri: redirect_uri.to_owned(),
            authorization_url,
            expires_at: Utc::now() + ChronoDuration::minutes(10),
        })
    }

    pub fn status(
        &self,
        credentials: &CredentialStore,
        database: &Database,
        stored: Option<ProviderAccount>,
    ) -> AppResult<OpenAiAuthStatus> {
        let api_key = self.api_key.get_auth_status(credentials, stored)?;
        let mut connected_accounts = Vec::new();
        for account in database.openai_accounts()? {
            if credentials
                .get_openai_session(&account.id)?
                .and_then(|json| serde_json::from_str::<OpenAiSession>(&json).ok())
                .is_some()
            {
                connected_accounts.push(account);
            }
        }
        Ok(OpenAiAuthStatus {
            api_key,
            chatgpt: OpenAiChatGptAuthStatus {
                available: true,
                connected_accounts,
                unavailable_reason: Some(
                    "Sign-in can be started in the system browser. Account verification and token exchange are not implemented yet.".into(),
                ),
            },
        })
    }

    /// Used by a future native OAuth flow after it has completed OpenAI's
    /// documented token and ID-token validation. This method is not an IPC command.
    #[allow(dead_code)]
    pub(crate) fn store_chatgpt_session(
        &self,
        credentials: &CredentialStore,
        session: &OpenAiSession,
    ) -> AppResult<()> {
        if session.access_token.trim().is_empty()
            || session.refresh_token.trim().is_empty()
            || session.id_token.trim().is_empty()
            || session.issued_client_id.trim().is_empty()
            || session.ext_agent_host_id.trim().is_empty()
            || session.token_expires_at.trim().is_empty()
            || session.granted_scopes.is_empty()
        {
            return Err(AppError::new(
                "invalid_auth_session",
                "The OpenAI sign-in session is incomplete.",
            ));
        }
        let serialized = serde_json::to_string(session)?;
        credentials.save_openai_session(&session.account_id, &serialized)
    }

    #[allow(dead_code)]
    pub(crate) fn remove_chatgpt_session(
        &self,
        credentials: &CredentialStore,
        account: &OpenAiAccount,
    ) -> AppResult<()> {
        credentials.delete_openai_session(&account.id)
    }
}

fn create_state() -> String {
    // Two independently generated UUIDv4 values produce a 64-character
    // URL-safe value with 244 bits of OS-CSPRNG entropy.
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

fn create_pkce_pair() -> (String, String) {
    let verifier = create_state();
    let challenge = pkce_challenge(&verifier);
    (verifier, challenge)
}

fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn validate_host_id(host_id: &str) -> AppResult<()> {
    if host_id
        .strip_prefix("urn:uuid:")
        .and_then(|value| uuid::Uuid::parse_str(value).ok())
        .is_some()
    {
        Ok(())
    } else {
        Err(AppError::new(
            "invalid_installation_identity",
            "The saved OpenAI installation identity is invalid.",
        ))
    }
}

fn validate_loopback_redirect_uri(redirect_uri: &str) -> AppResult<()> {
    let valid = Url::parse(redirect_uri).ok().is_some_and(|uri| {
        uri.scheme() == "http"
            && uri.host_str() == Some("127.0.0.1")
            && uri.port().is_some_and(|port| port != 0)
            && uri.path() == "/auth/callback"
            && uri.username().is_empty()
            && uri.password().is_none()
            && uri.query().is_none()
            && uri.fragment().is_none()
    });
    if valid {
        Ok(())
    } else {
        Err(AppError::new(
            "invalid_redirect_uri",
            "OpenAI sign-in requires an HTTP callback on 127.0.0.1/auth/callback.",
        ))
    }
}

#[async_trait]
impl ProviderAuthAdapter for OpenAIAuthManager {
    async fn connect(
        &self,
        secret: &str,
        credentials: &CredentialStore,
    ) -> AppResult<ProviderAccountInfo> {
        self.api_key.connect(secret, credentials).await
    }

    fn disconnect(&self, credentials: &CredentialStore) -> AppResult<()> {
        self.api_key.disconnect(credentials)
    }

    async fn refresh(&self, secret: &str) -> AppResult<()> {
        self.api_key.refresh(secret).await
    }

    async fn validate(&self, secret: &str) -> AppResult<()> {
        self.api_key.validate(secret).await
    }

    fn get_account_info(&self) -> ProviderAccountInfo {
        self.api_key.get_account_info()
    }

    async fn get_available_models(&self, secret: &str) -> AppResult<Vec<crate::domain::Model>> {
        self.api_key.get_available_models(secret).await
    }

    fn get_auth_status(
        &self,
        credentials: &CredentialStore,
        stored: Option<ProviderAccount>,
    ) -> AppResult<ProviderAccount> {
        self.api_key.get_auth_status(credentials, stored)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ProviderAuthMethodOption, ProviderAuthState};

    #[test]
    fn openai_status_and_account_contracts_contain_only_safe_metadata() {
        let account = OpenAiAccount {
            id: "account-id".into(),
            display_name: Some("Example".into()),
            email: Some("user@example.test".into()),
            organization: None,
            client_id: Some("issued-client-id".into()),
            granted_scopes: vec!["openid".into()],
            connected_at: "2026-01-01T00:00:00Z".into(),
            expires_at: None,
        };
        let status = OpenAiAuthStatus {
            api_key: ProviderAccount {
                provider_id: "openai".into(),
                provider_name: "OpenAI".into(),
                state: ProviderAuthState::NotConnected,
                auth_method: None,
                account_label: None,
                connected_at: None,
                last_validated_at: None,
                last_error_code: None,
                available_methods: vec![ProviderAuthMethodOption {
                    method: crate::domain::ProviderAuthMethod::ApiKey,
                    label: "API key".into(),
                    available: true,
                    unavailable_reason: None,
                }],
            },
            chatgpt: OpenAiChatGptAuthStatus {
                available: false,
                connected_accounts: vec![account],
                unavailable_reason: Some("Sign in is not enabled yet.".into()),
            },
        };
        let serialized = serde_json::to_string(&status).unwrap();
        for secret_value in [
            "sk-test-secret",
            "access-token-secret",
            "refresh-token-secret",
        ] {
            assert!(!serialized.contains(secret_value));
        }
        assert!(serialized.contains("issued-client-id"));
    }

    #[test]
    fn pkce_challenge_uses_base64url_sha256_without_padding() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            pkce_challenge(verifier),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn state_values_are_fresh_and_pkce_compatible() {
        let first = create_state();
        let second = create_state();
        assert_eq!(first.len(), 64);
        assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(first, second);

        let (verifier, challenge) = create_pkce_pair();
        assert!((43..=128).contains(&verifier.len()));
        assert!(verifier.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_eq!(challenge, pkce_challenge(&verifier));
        assert!(!challenge.contains('='));
    }

    #[test]
    fn authentication_session_contains_documented_request_and_fresh_values() {
        let provider = Provider {
            id: "openai".into(),
            name: "OpenAI".into(),
            protocol: crate::domain::ProviderProtocol::OpenAiResponses,
            base_url: Some("https://api.openai.com/v1".into()),
            models: vec![],
            connected: false,
        };
        let manager = OpenAIAuthManager::new(provider).unwrap();
        let host_id = format!("urn:uuid:{}", uuid::Uuid::new_v4());
        let first = manager
            .create_authentication_session("http://127.0.0.1:43210/auth/callback", &host_id)
            .unwrap();
        let second = manager
            .create_authentication_session("http://127.0.0.1:43211/auth/callback", &host_id)
            .unwrap();

        assert_ne!(first.attempt_id, second.attempt_id);
        assert_ne!(first.state, second.state);
        assert_ne!(first.nonce, second.nonce);
        assert_ne!(first.code_verifier, second.code_verifier);
        assert_eq!(first.redirect_uri, "http://127.0.0.1:43210/auth/callback");
        assert!(first.expires_at > Utc::now());
        let pairs = first
            .authorization_url
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(
            pairs.get("client_id").map(|value| value.as_ref()),
            Some("dynamic_agent_client")
        );
        assert_eq!(
            pairs.get("agent_name_hint").map(|value| value.as_ref()),
            Some("JevCode")
        );
        assert_eq!(
            pairs.get("ext_agent_host_id").map(|value| value.as_ref()),
            Some(host_id.as_str())
        );
        assert_eq!(
            pairs.get("response_type").map(|value| value.as_ref()),
            Some("code")
        );
        assert_eq!(
            pairs.get("redirect_uri").map(|value| value.as_ref()),
            Some(first.redirect_uri.as_str())
        );
        assert_eq!(
            pairs
                .get("code_challenge_method")
                .map(|value| value.as_ref()),
            Some("S256")
        );
        assert_eq!(
            pairs.get("code_challenge").map(|value| value.as_ref()),
            Some(first.code_challenge.as_str())
        );
        assert_eq!(
            pairs.get("nonce").map(|value| value.as_ref()),
            Some(first.nonce.as_str())
        );
        assert_eq!(
            pairs.get("state").map(|value| value.as_ref()),
            Some(first.state.as_str())
        );
    }

    #[test]
    fn authentication_session_rejects_non_loopback_or_malformed_redirects() {
        let provider = Provider {
            id: "openai".into(),
            name: "OpenAI".into(),
            protocol: crate::domain::ProviderProtocol::OpenAiResponses,
            base_url: Some("https://api.openai.com/v1".into()),
            models: vec![],
            connected: false,
        };
        let manager = OpenAIAuthManager::new(provider).unwrap();
        let host_id = format!("urn:uuid:{}", uuid::Uuid::new_v4());
        for redirect in [
            "http://localhost:43210/auth/callback",
            "https://127.0.0.1:43210/auth/callback",
            "http://127.0.0.1:43210/callback",
            "http://127.0.0.1:43210/auth/callback?code=bad",
        ] {
            assert!(manager
                .create_authentication_session(redirect, &host_id)
                .is_err());
        }
    }
}
