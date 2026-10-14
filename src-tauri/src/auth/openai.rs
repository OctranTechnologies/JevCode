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
use serde::{Deserialize, Serialize};

/// OpenAI credentials are only resolved inside the native provider layer.
/// OAuth is intentionally not produced until the official sign-in flow is implemented.
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
                available: false,
                connected_accounts,
                unavailable_reason: Some(
                    "The official OpenAI open-source sign-in flow is documented, but JevCode has not implemented its client registration, PKCE callback, or token verification yet.".into(),
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
}
