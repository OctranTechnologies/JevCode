use super::{ApiKeyAuthAdapter, ProviderAuthAdapter};
use crate::{
    config::AppConfig,
    credentials::CredentialStore,
    domain::{
        GoogleAccount, GoogleAccountType, GoogleAuthStatus, GoogleAuthType,
        GoogleCodeAssistAvailability, GoogleCodeAssistStatus, Model, Provider, ProviderAccount,
        ProviderAccountInfo, ProviderAuthMethod, ProviderAuthMethodOption, ProviderAuthState,
    },
    error::{AppError, AppResult},
    persistence::Database,
};
use async_trait::async_trait;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use jsonwebtoken::{
    decode, decode_header,
    jwk::{JwkSet, KeyAlgorithm, KeyOperations, PublicKeyUse},
    Algorithm, DecodingKey, Validation,
};
use reqwest::{Client, StatusCode, Url};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex as StdMutex, OnceLock},
    time::Duration,
};
use zeroize::{Zeroize, Zeroizing};

const GOOGLE_AUTHORIZE_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const GOOGLE_DISCOVERY_URL: &str = "https://accounts.google.com/.well-known/openid-configuration";
// Google OAuth is used only for verified account identity. JevCode does not
// send these credentials to the Gemini Developer API or Code Assist backend.
const GOOGLE_SCOPES: &str = "openid email profile";
const REFRESH_SKEW: ChronoDuration = ChronoDuration::minutes(5);
type RefreshLock = Arc<tokio::sync::Mutex<()>>;
static GOOGLE_REFRESH_LOCKS: OnceLock<StdMutex<HashMap<String, RefreshLock>>> = OnceLock::new();

/// Safe account data and OAuth material are intentionally separate types.
/// This record is serialized only into the OS keychain.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleAuthSession {
    pub account: GoogleAccount,
    client_id: String,
    cloud_project_id: String,
    access_token: String,
    refresh_token: String,
    id_token: String,
    expires_at: String,
    granted_scopes: Vec<String>,
}

impl Drop for GoogleAuthSession {
    fn drop(&mut self) {
        self.access_token.zeroize();
        self.refresh_token.zeroize();
        self.id_token.zeroize();
    }
}

/// Per-attempt OAuth material is native-only and deliberately has no Debug or
/// serialization implementation.
pub(crate) struct PendingGoogleAuth {
    pub(crate) attempt_id: String,
    state: String,
    nonce: String,
    code_verifier: String,
    pub(crate) redirect_uri: String,
    pub(crate) authorization_url: Url,
    pub(crate) expires_at: DateTime<Utc>,
    client_id: String,
    cloud_project_id: String,
}

pub(crate) fn parse_authorization_callback(
    pending: &PendingGoogleAuth,
    request_target: &str,
    host_header: &str,
) -> AppResult<Option<Zeroizing<String>>> {
    if Utc::now() >= pending.expires_at {
        return Err(google_callback_error());
    }
    let redirect = Url::parse(&pending.redirect_uri).map_err(|_| google_callback_error())?;
    let expected_host = format!(
        "127.0.0.1:{}",
        redirect.port().ok_or_else(google_callback_error)?
    );
    if !constant_time_equal(host_header.as_bytes(), expected_host.as_bytes())
        || !request_target.starts_with('/')
        || request_target.starts_with("//")
        || request_target.contains('#')
        || request_target.split('?').next() != Some(redirect.path())
        || !request_target
            .bytes()
            .all(|byte| (0x21..=0x7e).contains(&byte))
    {
        return Err(google_callback_error());
    }
    let Some((_, query)) = request_target.split_once('?') else {
        return Err(google_callback_error());
    };
    let mut state: Option<Zeroizing<String>> = None;
    let mut code: Option<Zeroizing<String>> = None;
    let mut error: Option<Zeroizing<String>> = None;
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let key = decode_component(key)?;
        let value = decode_component(value)?;
        match key.as_str() {
            "state" if state.is_none() => state = Some(value),
            "code" if code.is_none() => code = Some(value),
            "error" if error.is_none() => error = Some(value),
            "state" | "code" | "error" => return Err(google_callback_error()),
            _ => {}
        }
    }
    let state = state.ok_or_else(google_callback_error)?;
    if !constant_time_equal(state.as_bytes(), pending.state.as_bytes()) {
        return Err(google_callback_error());
    }
    if error.as_deref().map(String::as_str) == Some("access_denied") && code.is_none() {
        return Ok(None);
    }
    if error.is_some() {
        return Err(google_callback_error());
    }
    let code = code.ok_or_else(google_callback_error)?;
    if code.trim().is_empty() || code.len() > 16_384 || code.chars().any(char::is_control) {
        return Err(google_callback_error());
    }
    Ok(Some(code))
}

impl Drop for PendingGoogleAuth {
    fn drop(&mut self) {
        self.state.zeroize();
        self.nonce.zeroize();
        self.code_verifier.zeroize();
    }
}

pub(crate) enum GoogleRequestCredential {
    ApiKey(Zeroizing<String>),
}

#[async_trait]
pub(crate) trait GoogleOAuthTransport: Send + Sync {
    async fn exchange_code(
        &self,
        client_id: &str,
        redirect_uri: &str,
        code: &str,
        verifier: &str,
    ) -> AppResult<GoogleTokenResponse>;
    async fn refresh_token(
        &self,
        client_id: &str,
        refresh_token: &str,
    ) -> AppResult<GoogleTokenResponse>;
    async fn discovery(&self) -> AppResult<GoogleOidcDiscovery>;
    async fn jwks(&self, url: &str) -> AppResult<JwkSet>;
    async fn revoke_token(&self, token: &str) -> AppResult<()>;
}

/// OAuth sessions are routed to the platform credential store in production.
/// Keeping this boundary explicit lets the exchange and logout path be tested
/// with an in-memory store without reading or modifying a developer's keychain.
pub(crate) trait GoogleSessionStorage: Send + Sync {
    fn save_google_session(&self, session: &str) -> AppResult<()>;
    fn get_google_session(&self) -> AppResult<Option<String>>;
    fn delete_google_session(&self) -> AppResult<()>;
}

impl GoogleSessionStorage for CredentialStore {
    fn save_google_session(&self, session: &str) -> AppResult<()> {
        CredentialStore::save_google_session(self, session)
    }

    fn get_google_session(&self) -> AppResult<Option<String>> {
        CredentialStore::get_google_session(self)
    }

    fn delete_google_session(&self) -> AppResult<()> {
        CredentialStore::delete_google_session(self)
    }
}

struct ReqwestGoogleOAuthTransport {
    client: Client,
}

impl ReqwestGoogleOAuthTransport {
    fn new() -> AppResult<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(20))
            .connect_timeout(Duration::from_secs(8))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| google_network_error())?;
        Ok(Self { client })
    }

    fn build_exchange_request(
        &self,
        client_id: &str,
        redirect_uri: &str,
        code: &str,
        verifier: &str,
    ) -> AppResult<reqwest::Request> {
        self.client
            .post(GOOGLE_TOKEN_URL)
            .form(&[
                ("client_id", client_id),
                ("code", code),
                ("code_verifier", verifier),
                ("grant_type", "authorization_code"),
                ("redirect_uri", redirect_uri),
            ])
            .build()
            .map_err(|_| google_network_error())
    }
}

#[async_trait]
impl GoogleOAuthTransport for ReqwestGoogleOAuthTransport {
    async fn exchange_code(
        &self,
        client_id: &str,
        redirect_uri: &str,
        code: &str,
        verifier: &str,
    ) -> AppResult<GoogleTokenResponse> {
        let request = self.build_exchange_request(client_id, redirect_uri, code, verifier)?;
        let response = self
            .client
            .execute(request)
            .await
            .map_err(|_| google_network_error())?;
        read_google_json(response).await
    }

    async fn refresh_token(
        &self,
        client_id: &str,
        refresh_token: &str,
    ) -> AppResult<GoogleTokenResponse> {
        let response = self
            .client
            .post(GOOGLE_TOKEN_URL)
            .form(&[
                ("client_id", client_id),
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
            ])
            .send()
            .await
            .map_err(|_| google_network_error())?;
        read_google_json(response).await
    }

    async fn discovery(&self) -> AppResult<GoogleOidcDiscovery> {
        let response = self
            .client
            .get(GOOGLE_DISCOVERY_URL)
            .send()
            .await
            .map_err(|_| google_network_error())?;
        read_google_json(response).await
    }

    async fn jwks(&self, url: &str) -> AppResult<JwkSet> {
        let parsed = Url::parse(url).map_err(|_| google_identity_error())?;
        if parsed.scheme() != "https"
            || !matches!(
                parsed.host_str(),
                Some("www.googleapis.com" | "accounts.google.com")
            )
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(google_identity_error());
        }
        let response = self
            .client
            .get(parsed)
            .send()
            .await
            .map_err(|_| google_network_error())?;
        read_google_json(response).await
    }

    async fn revoke_token(&self, token: &str) -> AppResult<()> {
        let response = self
            .client
            .post("https://oauth2.googleapis.com/revoke")
            .form(&[("token", token)])
            .send()
            .await
            .map_err(|_| google_network_error())?;
        if response.status().is_success() || response.status() == StatusCode::BAD_REQUEST {
            Ok(())
        } else {
            Err(google_provider_error(response.status()))
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct GoogleOidcDiscovery {
    issuer: String,
    jwks_uri: String,
    id_token_signing_alg_values_supported: Vec<String>,
}

#[derive(Deserialize)]
pub(crate) struct GoogleTokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
    token_type: String,
    expires_in: u64,
    #[serde(default)]
    scope: Option<String>,
}

impl Drop for GoogleTokenResponse {
    fn drop(&mut self) {
        self.access_token.zeroize();
        if let Some(value) = &mut self.refresh_token {
            value.zeroize();
        }
        if let Some(value) = &mut self.id_token {
            value.zeroize();
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Audience {
    One(String),
    Many(Vec<String>),
}

#[derive(Deserialize)]
struct GoogleIdentityClaims {
    iss: String,
    sub: String,
    aud: Audience,
    iat: u64,
    nonce: String,
    #[serde(default)]
    azp: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    email_verified: Option<bool>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    hd: Option<String>,
}

pub struct GoogleAuthManager {
    provider: Provider,
    api_key: ApiKeyAuthAdapter,
    client_id: Option<String>,
    cloud_project_id: Option<String>,
    oauth: Arc<dyn GoogleOAuthTransport>,
    session_storage: Option<Arc<dyn GoogleSessionStorage>>,
    #[cfg(test)]
    api_key_connected_override: Option<bool>,
}

impl GoogleAuthManager {
    pub fn new(provider: Provider) -> AppResult<Self> {
        Self::with_config(provider, None, None)
    }

    pub fn with_app_config(provider: Provider, config: &AppConfig) -> AppResult<Self> {
        Self::with_config(
            provider,
            config.google_oauth_client_id.clone(),
            config.google_cloud_project_id.clone(),
        )
    }

    fn with_config(
        provider: Provider,
        client_id: Option<String>,
        cloud_project_id: Option<String>,
    ) -> AppResult<Self> {
        if provider.id != "gemini" {
            return Err(AppError::new(
                "provider_configuration",
                "The Google auth manager can only be used for the Gemini provider.",
            ));
        }
        let api_key = ApiKeyAuthAdapter::new(provider.clone())?;
        Ok(Self {
            provider,
            api_key,
            client_id: client_id.filter(|value| !value.trim().is_empty()),
            cloud_project_id: cloud_project_id.filter(|value| !value.trim().is_empty()),
            oauth: Arc::new(ReqwestGoogleOAuthTransport::new()?),
            session_storage: None,
            #[cfg(test)]
            api_key_connected_override: None,
        })
    }

    #[cfg(test)]
    fn with_test_dependencies(
        provider: Provider,
        client_id: Option<String>,
        cloud_project_id: Option<String>,
        oauth: Arc<dyn GoogleOAuthTransport>,
        session_storage: Arc<dyn GoogleSessionStorage>,
    ) -> AppResult<Self> {
        let mut manager = Self::with_config(provider, client_id, cloud_project_id)?;
        manager.oauth = oauth;
        manager.session_storage = Some(session_storage);
        manager.api_key_connected_override = Some(false);
        Ok(manager)
    }

    pub fn status(
        &self,
        credentials: &CredentialStore,
        stored: Option<ProviderAccount>,
    ) -> AppResult<GoogleAuthStatus> {
        let account = self.load_session(credentials)?.map(|session| {
            let mut account = session.account.clone();
            if account.cloud_project_id.is_none() && !session.cloud_project_id.is_empty() {
                account.cloud_project_id = Some(session.cloud_project_id.clone());
            }
            let renewal_needed = parse_timestamp(&session.expires_at)
                .is_none_or(|expires_at| expires_at <= Utc::now() + REFRESH_SKEW);
            if account.requires_reconnect
                || session.access_token.trim().is_empty()
                || (renewal_needed && session.refresh_token.trim().is_empty())
            {
                account.connected = false;
                account.requires_reconnect = true;
            }
            account
        });
        let api_key_connected = self.has_api_key(credentials)?;
        let selected_auth_type = match stored.as_ref().and_then(|value| value.auth_method.as_ref())
        {
            Some(ProviderAuthMethod::GoogleOAuth) => Some(GoogleAuthType::GoogleOAuth),
            Some(ProviderAuthMethod::ApiKey) => Some(GoogleAuthType::GeminiApiKey),
            _ if api_key_connected => Some(GoogleAuthType::GeminiApiKey),
            _ => None,
        };
        let oauth_available = self.oauth_configured();
        let code_assist = code_assist_status(account.as_ref());
        Ok(GoogleAuthStatus {
            account,
            code_assist,
            selected_auth_type,
            api_key_connected,
            oauth_available,
            unavailable_reason: (!oauth_available).then(|| {
                "Google account sign-in requires a Desktop OAuth client ID registered for JevCode.".into()
            }),
        })
    }

    pub(crate) fn set_cloud_project_id(
        &self,
        project_id: &str,
        credentials: &CredentialStore,
    ) -> AppResult<GoogleAccount> {
        let project_id = project_id.trim();
        if !valid_project_id(project_id) {
            return Err(AppError::new(
                "google_invalid_cloud_project",
                "Enter a valid Google Cloud project ID using lowercase letters, numbers, and hyphens.",
            ));
        }
        let mut session = self
            .load_session(credentials)?
            .ok_or_else(google_reconnect_error)?;
        if session.account.account_type != GoogleAccountType::Workspace {
            return Err(AppError::new(
                "google_project_not_required",
                "A Google Cloud project can be configured here only for a Workspace account.",
            ));
        }
        session.cloud_project_id = project_id.to_owned();
        session.account.cloud_project_id = Some(project_id.to_owned());
        self.save_session(credentials, &session)?;
        Ok(session.account.clone())
    }

    pub(crate) fn select_auth_method(
        &self,
        auth_type: GoogleAuthType,
        credentials: &CredentialStore,
        database: &Database,
    ) -> AppResult<GoogleAuthStatus> {
        let current = database.provider_account(&self.provider.id)?;
        let (method, label, state, error_code) = match auth_type {
            GoogleAuthType::GoogleOAuth => {
                return Err(google_code_assist_unsupported_error());
            }
            GoogleAuthType::GeminiApiKey => {
                if !self.has_api_key(credentials)? {
                    return Err(AppError::new(
                        "credentials_required",
                        "Connect a Gemini API key before selecting that sign-in method.",
                    ));
                }
                (
                    ProviderAuthMethod::ApiKey,
                    "Gemini API key".into(),
                    ProviderAuthState::Connected,
                    None,
                )
            }
            GoogleAuthType::VertexAdc => {
                return Err(AppError::new(
                    "google_auth_method_unavailable",
                    "Vertex AI authentication is not available in JevCode yet.",
                ));
            }
        };
        let account = ProviderAccount {
            provider_id: self.provider.id.clone(),
            provider_name: self.provider.name.clone(),
            state,
            auth_method: Some(method),
            account_label: Some(label),
            connected_at: current
                .as_ref()
                .and_then(|account| account.connected_at.clone())
                .or_else(|| Some(Utc::now().to_rfc3339())),
            last_validated_at: current
                .as_ref()
                .and_then(|account| account.last_validated_at.clone()),
            last_error_code: error_code,
            available_methods: self.available_auth_methods(),
        };
        database.save_provider_account(&account)?;
        self.status(credentials, Some(account))
    }

    pub(crate) fn disconnect_api_key(
        &self,
        credentials: &CredentialStore,
        database: &Database,
    ) -> AppResult<()> {
        credentials.delete(&self.provider.id)?;
        if let Some(mut stored) = database.provider_account(&self.provider.id)? {
            if stored.auth_method == Some(ProviderAuthMethod::ApiKey) {
                let status = self.status(credentials, Some(stored.clone()))?;
                if let Some(account) = status.account {
                    stored.state = ProviderAuthState::NotConnected;
                    stored.auth_method = None;
                    stored.account_label = account.email.or_else(|| Some("Google account".into()));
                    stored.last_error_code = Some("google_code_assist_unsupported".into());
                    stored.available_methods = self.available_auth_methods();
                    database.save_provider_account(&stored)?;
                } else {
                    database.delete_provider_account(&self.provider.id)?;
                }
            }
        }
        Ok(())
    }

    pub(crate) fn create_authentication_session(
        &self,
        redirect_uri: &str,
    ) -> AppResult<PendingGoogleAuth> {
        if !self.oauth_configured() {
            return Err(AppError::new(
                "google_oauth_not_configured",
                "Google sign-in needs a Desktop OAuth client ID registered for JevCode.",
            ));
        }
        validate_redirect_uri(redirect_uri)?;
        let client_id = self.client_id.clone().expect("configured Google client ID");
        let cloud_project_id = self.cloud_project_id.clone().unwrap_or_default();
        let state = fresh_random_value();
        let nonce = fresh_random_value();
        let (code_verifier, code_challenge) = create_pkce_pair();
        let expires_at = Utc::now() + ChronoDuration::minutes(10);
        let mut authorization_url = Url::parse(GOOGLE_AUTHORIZE_URL).map_err(AppError::internal)?;
        authorization_url
            .query_pairs_mut()
            .append_pair("client_id", &client_id)
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("response_type", "code")
            .append_pair("scope", GOOGLE_SCOPES)
            .append_pair("state", &state)
            .append_pair("nonce", &nonce)
            .append_pair("code_challenge", &code_challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("access_type", "offline")
            .append_pair("prompt", "select_account");
        Ok(PendingGoogleAuth {
            attempt_id: uuid::Uuid::new_v4().to_string(),
            state,
            nonce,
            code_verifier,
            redirect_uri: redirect_uri.into(),
            authorization_url,
            expires_at,
            client_id,
            cloud_project_id,
        })
    }

    pub(crate) async fn complete_authentication(
        &self,
        pending: &PendingGoogleAuth,
        code: &str,
        credentials: &CredentialStore,
        database: &Database,
    ) -> AppResult<GoogleAccount> {
        if Utc::now() >= pending.expires_at || code.trim().is_empty() || code.len() > 16_384 {
            return Err(google_callback_error());
        }
        let mut response = self
            .oauth
            .exchange_code(
                &pending.client_id,
                &pending.redirect_uri,
                code,
                &pending.code_verifier,
            )
            .await?;
        validate_initial_token_response(&response)?;
        let discovery = self.oauth.discovery().await?;
        if discovery.issuer != "https://accounts.google.com" {
            return Err(google_identity_error());
        }
        let jwks = self.oauth.jwks(&discovery.jwks_uri).await?;
        let id_token = response
            .id_token
            .as_deref()
            .ok_or_else(google_identity_error)?;
        let identity = validate_google_id_token(
            id_token,
            &pending.nonce,
            &pending.client_id,
            &discovery,
            &jwks,
        )?;
        let scopes = response.scope.as_deref().ok_or_else(google_scope_error)?;
        let granted_scopes: Vec<String> =
            scopes.split_ascii_whitespace().map(str::to_owned).collect();
        if !has_required_scopes(&granted_scopes) {
            return Err(google_scope_error());
        }
        let expires_at =
            (Utc::now() + ChronoDuration::seconds(response.expires_in as i64)).to_rfc3339();
        let account = GoogleAccount {
            id: identity.subject,
            email: identity.email,
            display_name: identity.name,
            auth_type: GoogleAuthType::GoogleOAuth,
            account_type: identity.account_type,
            cloud_project_id: (!pending.cloud_project_id.is_empty())
                .then(|| pending.cloud_project_id.clone()),
            connected: true,
            requires_reconnect: false,
            expires_at: Some(expires_at.clone()),
            granted_scopes: granted_scopes.clone(),
        };
        let session = GoogleAuthSession {
            account: account.clone(),
            client_id: pending.client_id.clone(),
            cloud_project_id: pending.cloud_project_id.clone(),
            access_token: std::mem::take(&mut response.access_token),
            refresh_token: response
                .refresh_token
                .take()
                .ok_or_else(google_token_error)?,
            id_token: response.id_token.take().ok_or_else(google_identity_error)?,
            expires_at,
            granted_scopes,
        };
        self.save_session(credentials, &session)?;
        let previous = database.provider_account(&self.provider.id)?;
        let retained_api_key = previous
            .as_ref()
            .is_some_and(|saved| saved.auth_method == Some(ProviderAuthMethod::ApiKey))
            && self.has_api_key(credentials)?;
        database.save_provider_account(&ProviderAccount {
            provider_id: self.provider.id.clone(),
            provider_name: self.provider.name.clone(),
            state: if retained_api_key {
                ProviderAuthState::Connected
            } else {
                ProviderAuthState::NotConnected
            },
            auth_method: retained_api_key.then_some(ProviderAuthMethod::ApiKey),
            account_label: if retained_api_key {
                previous
                    .as_ref()
                    .and_then(|saved| saved.account_label.clone())
            } else {
                account
                    .email
                    .clone()
                    .or_else(|| Some("Google account".into()))
            },
            connected_at: previous
                .as_ref()
                .and_then(|saved| saved.connected_at.clone())
                .or_else(|| Some(Utc::now().to_rfc3339())),
            last_validated_at: if retained_api_key {
                previous
                    .as_ref()
                    .and_then(|saved| saved.last_validated_at.clone())
            } else {
                None
            },
            last_error_code: None,
            available_methods: self.available_auth_methods(),
        })?;
        Ok(account)
    }

    pub(crate) async fn request_credential(
        &self,
        credentials: &CredentialStore,
        database: &Database,
    ) -> AppResult<GoogleRequestCredential> {
        let stored = database.provider_account(&self.provider.id)?;
        let selected = stored
            .as_ref()
            .and_then(|account| account.auth_method.as_ref());
        let saved_session_exists = self.load_session(credentials)?.is_some();
        let wants_oauth = selected == Some(&ProviderAuthMethod::GoogleOAuth)
            || (selected.is_none() && saved_session_exists);
        if wants_oauth {
            return Err(google_code_assist_unsupported_error());
        }
        if selected == Some(&ProviderAuthMethod::ApiKey) || selected.is_none() {
            if let Some(secret) = credentials.get("gemini")? {
                return Ok(GoogleRequestCredential::ApiKey(Zeroizing::new(secret)));
            }
        }
        Err(AppError::new(
            "credentials_required",
            "Connect Google Gemini with a Gemini API key or Google account before running a task.",
        ))
    }

    pub(crate) async fn refresh_account(
        &self,
        credentials: &CredentialStore,
        database: &Database,
    ) -> AppResult<GoogleAccount> {
        let (_, account) = self.access_token(credentials, database, true).await?;
        Ok(account)
    }

    pub(crate) async fn get_available_models(
        &self,
        credentials: &CredentialStore,
        database: &Database,
    ) -> AppResult<Vec<Model>> {
        let GoogleRequestCredential::ApiKey(secret) =
            self.request_credential(credentials, database).await?;
        self.api_key.get_available_models(&secret).await
    }

    pub(crate) async fn logout(
        &self,
        credentials: &CredentialStore,
        database: &Database,
    ) -> AppResult<bool> {
        let mut session = self.load_session(credentials)?;
        let revocation_confirmed = if let Some(session) = session.as_mut() {
            self.oauth
                .revoke_token(&session.refresh_token)
                .await
                .is_ok()
        } else {
            false
        };
        self.delete_session(credentials)?;
        if let Some(mut account) = database.provider_account(&self.provider.id)? {
            if account.auth_method != Some(ProviderAuthMethod::ApiKey) {
                if self.has_api_key(credentials)? {
                    account.state = ProviderAuthState::Connected;
                    account.auth_method = Some(ProviderAuthMethod::ApiKey);
                    account.account_label = Some("API key".into());
                    account.connected_at = None;
                    account.last_validated_at = None;
                    account.last_error_code = None;
                    account.available_methods = self.available_auth_methods();
                    database.save_provider_account(&account)?;
                } else {
                    database.delete_provider_account(&self.provider.id)?;
                }
            }
        }
        Ok(revocation_confirmed)
    }

    fn oauth_configured(&self) -> bool {
        self.client_id.as_deref().is_some_and(valid_client_id)
    }

    fn available_auth_methods(&self) -> Vec<ProviderAuthMethodOption> {
        vec![
            ProviderAuthMethodOption {
                method: ProviderAuthMethod::ApiKey,
                label: "Gemini API key".into(),
                available: true,
                unavailable_reason: None,
            },
            ProviderAuthMethodOption {
                method: ProviderAuthMethod::GoogleOAuth,
                label: "Google account (identity only)".into(),
                available: self.oauth_configured(),
                unavailable_reason: (!self.oauth_configured())
                    .then(|| "Register a Desktop OAuth client ID for JevCode.".into()),
            },
        ]
    }

    async fn access_token(
        &self,
        credentials: &CredentialStore,
        database: &Database,
        force_refresh: bool,
    ) -> AppResult<(Zeroizing<String>, GoogleAccount)> {
        let mut session = self
            .load_session(credentials)?
            .ok_or_else(google_reconnect_error)?;
        let lock = google_refresh_lock(&session.account.id)?;
        let _guard = lock.lock().await;
        // Re-read after acquiring the lock so waiting requests reuse the token
        // written by the first refresh instead of issuing duplicate refreshes.
        session = self
            .load_session(credentials)?
            .ok_or_else(google_reconnect_error)?;
        if session.account.requires_reconnect || session.access_token.trim().is_empty() {
            self.mark_reconnect_required(credentials, database, &mut session)?;
            return Err(google_reconnect_error());
        }
        let Some(expires_at) = parse_timestamp(&session.expires_at) else {
            self.mark_reconnect_required(credentials, database, &mut session)?;
            return Err(google_reconnect_error());
        };
        if !force_refresh && expires_at > Utc::now() + REFRESH_SKEW {
            return Ok((
                Zeroizing::new(session.access_token.clone()),
                session.account.clone(),
            ));
        }
        if session.refresh_token.trim().is_empty() {
            self.mark_reconnect_required(credentials, database, &mut session)?;
            return Err(google_reconnect_error());
        }
        match self
            .oauth
            .refresh_token(&session.client_id, &session.refresh_token)
            .await
        {
            Ok(mut response) => {
                validate_refresh_token_response(&response)?;
                let new_expiry = Utc::now() + ChronoDuration::seconds(response.expires_in as i64);
                session.access_token.zeroize();
                session.access_token = std::mem::take(&mut response.access_token);
                if let Some(mut refresh) = response.refresh_token.take() {
                    session.refresh_token.zeroize();
                    session.refresh_token = std::mem::take(&mut refresh);
                    refresh.zeroize();
                }
                // Google does not require an ID token in refresh responses. A
                // returned value is ignored and zeroized by the response type.
                if let Some(scope) = response.scope.as_deref() {
                    let scopes: Vec<String> =
                        scope.split_ascii_whitespace().map(str::to_owned).collect();
                    if !has_required_scopes(&scopes) {
                        self.mark_reconnect_required(credentials, database, &mut session)?;
                        return Err(google_reconnect_error());
                    }
                    session.granted_scopes = scopes.clone();
                    session.account.granted_scopes = scopes;
                }
                session.expires_at = new_expiry.to_rfc3339();
                session.account.expires_at = Some(session.expires_at.clone());
                session.account.requires_reconnect = false;
                session.account.connected = true;
                let token = Zeroizing::new(session.access_token.clone());
                self.save_session(credentials, &session)?;
                if let Some(mut account) = database.provider_account(&self.provider.id)? {
                    if account.auth_method == Some(ProviderAuthMethod::GoogleOAuth) {
                        account.state = ProviderAuthState::Connected;
                        account.last_validated_at = Some(Utc::now().to_rfc3339());
                        account.last_error_code = None;
                        database.save_provider_account(&account)?;
                    }
                }
                Ok((token, session.account.clone()))
            }
            Err(error) => {
                if is_revoked_grant(&error) {
                    self.mark_reconnect_required(credentials, database, &mut session)?;
                    return Err(google_reconnect_error());
                }
                Err(error)
            }
        }
    }

    fn load_session(&self, credentials: &CredentialStore) -> AppResult<Option<GoogleAuthSession>> {
        let stored = match &self.session_storage {
            Some(storage) => storage.get_google_session()?,
            None => credentials.get_google_session()?,
        };
        let Some(mut serialized) = stored else {
            return Ok(None);
        };
        let parsed = serde_json::from_str(&serialized);
        serialized.zeroize();
        match parsed {
            Ok(session) => Ok(Some(session)),
            Err(_) => Err(google_reconnect_error()),
        }
    }

    fn save_session(
        &self,
        credentials: &CredentialStore,
        session: &GoogleAuthSession,
    ) -> AppResult<()> {
        let mut serialized = serde_json::to_string(session)?;
        let result = match &self.session_storage {
            Some(storage) => storage.save_google_session(&serialized),
            None => credentials.save_google_session(&serialized),
        };
        serialized.zeroize();
        result
    }

    fn delete_session(&self, credentials: &CredentialStore) -> AppResult<()> {
        match &self.session_storage {
            Some(storage) => storage.delete_google_session(),
            None => credentials.delete_google_session(),
        }
    }

    fn mark_reconnect_required(
        &self,
        credentials: &CredentialStore,
        database: &Database,
        session: &mut GoogleAuthSession,
    ) -> AppResult<()> {
        session.account.connected = false;
        session.account.requires_reconnect = true;
        self.save_session(credentials, session)?;
        if let Some(mut stored) = database.provider_account(&self.provider.id)? {
            if stored.auth_method == Some(ProviderAuthMethod::GoogleOAuth) {
                stored.state = ProviderAuthState::NeedsAttention;
                stored.last_error_code = Some("google_reconnect_required".into());
                database.save_provider_account(&stored)?;
            }
        }
        Ok(())
    }

    fn has_api_key(&self, credentials: &CredentialStore) -> AppResult<bool> {
        #[cfg(test)]
        if let Some(connected) = self.api_key_connected_override {
            return Ok(connected);
        }
        credentials.has("gemini")
    }
}

#[async_trait]
impl ProviderAuthAdapter for GoogleAuthManager {
    async fn connect(
        &self,
        secret: &str,
        credentials: &CredentialStore,
    ) -> AppResult<ProviderAccountInfo> {
        self.api_key.connect(secret, credentials).await
    }

    fn disconnect(&self, credentials: &CredentialStore) -> AppResult<()> {
        credentials.delete_google_session()?;
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

    async fn get_available_models(&self, secret: &str) -> AppResult<Vec<Model>> {
        self.api_key.get_available_models(secret).await
    }

    fn get_auth_status(
        &self,
        credentials: &CredentialStore,
        stored: Option<ProviderAccount>,
    ) -> AppResult<ProviderAccount> {
        let google_account = self
            .load_session(credentials)?
            .map(|session| session.account.clone());
        let selected = stored.as_ref().and_then(|value| value.auth_method.as_ref());
        let mut account = if selected == Some(&ProviderAuthMethod::GoogleOAuth) {
            match google_account {
                Some(google_account) => ProviderAccount {
                    provider_id: "gemini".into(),
                    provider_name: self.provider.name.clone(),
                    state: if google_account.requires_reconnect {
                        ProviderAuthState::NeedsAttention
                    } else {
                        ProviderAuthState::Connected
                    },
                    auth_method: Some(ProviderAuthMethod::GoogleOAuth),
                    account_label: google_account
                        .email
                        .or_else(|| Some("Google account".into())),
                    connected_at: stored.as_ref().and_then(|value| value.connected_at.clone()),
                    last_validated_at: stored
                        .as_ref()
                        .and_then(|value| value.last_validated_at.clone()),
                    last_error_code: stored
                        .as_ref()
                        .and_then(|value| value.last_error_code.clone()),
                    available_methods: self.available_auth_methods(),
                },
                None => ProviderAccount {
                    provider_id: "gemini".into(),
                    provider_name: self.provider.name.clone(),
                    state: ProviderAuthState::NeedsAttention,
                    auth_method: Some(ProviderAuthMethod::GoogleOAuth),
                    account_label: Some("Google account".into()),
                    connected_at: stored.as_ref().and_then(|value| value.connected_at.clone()),
                    last_validated_at: stored
                        .as_ref()
                        .and_then(|value| value.last_validated_at.clone()),
                    last_error_code: Some("google_reconnect_required".into()),
                    available_methods: self.available_auth_methods(),
                },
            }
        } else if selected == Some(&ProviderAuthMethod::ApiKey) || google_account.is_none() {
            self.api_key.get_auth_status(credentials, stored)?
        } else if let Some(google_account) = google_account {
            ProviderAccount {
                provider_id: "gemini".into(),
                provider_name: self.provider.name.clone(),
                state: if google_account.requires_reconnect {
                    ProviderAuthState::NeedsAttention
                } else {
                    ProviderAuthState::Connected
                },
                auth_method: Some(ProviderAuthMethod::GoogleOAuth),
                account_label: google_account
                    .email
                    .or_else(|| Some("Google account".into())),
                connected_at: stored.as_ref().and_then(|value| value.connected_at.clone()),
                last_validated_at: stored
                    .as_ref()
                    .and_then(|value| value.last_validated_at.clone()),
                last_error_code: stored
                    .as_ref()
                    .and_then(|value| value.last_error_code.clone()),
                available_methods: self.available_auth_methods(),
            }
        } else {
            self.api_key.get_auth_status(credentials, stored)?
        };
        account.available_methods = self.available_auth_methods();
        Ok(account)
    }
}

fn validate_google_id_token(
    token: &str,
    expected_nonce: &str,
    client_id: &str,
    discovery: &GoogleOidcDiscovery,
    jwks: &JwkSet,
) -> AppResult<VerifiedGoogleIdentity> {
    if discovery.issuer != "https://accounts.google.com"
        || !discovery
            .id_token_signing_alg_values_supported
            .iter()
            .any(|value| value == "RS256")
    {
        return Err(google_identity_error());
    }
    let header = decode_header(token).map_err(|_| google_identity_error())?;
    if header.alg != Algorithm::RS256 {
        return Err(google_identity_error());
    }
    let kid = header.kid.ok_or_else(google_identity_error)?;
    let mut matches = jwks
        .keys
        .iter()
        .filter(|key| key.common.key_id.as_deref() == Some(&kid));
    let jwk = matches.next().ok_or_else(google_identity_error)?;
    if matches.next().is_some()
        || jwk
            .common
            .public_key_use
            .as_ref()
            .is_some_and(|value| value != &PublicKeyUse::Signature)
        || jwk
            .common
            .key_operations
            .as_ref()
            .is_some_and(|value| !value.contains(&KeyOperations::Verify))
        || jwk
            .common
            .key_algorithm
            .is_some_and(|value| value != KeyAlgorithm::RS256)
    {
        return Err(google_identity_error());
    }
    let key = DecodingKey::from_jwk(jwk).map_err(|_| google_identity_error())?;
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&["https://accounts.google.com", "accounts.google.com"]);
    validation.set_audience(&[client_id]);
    validation.set_required_spec_claims(&["exp", "iss", "aud", "sub", "iat"]);
    validation.leeway = 5;
    let claims = decode::<GoogleIdentityClaims>(token, &key, &validation)
        .map_err(|_| google_identity_error())?
        .claims;
    let audience_valid = match claims.aud {
        Audience::One(audience) => audience == client_id,
        Audience::Many(audiences) => {
            audiences.iter().any(|value| value == client_id)
                && (audiences.len() == 1 || claims.azp.as_deref() == Some(client_id))
        }
    };
    let now = Utc::now().timestamp().max(0) as u64;
    if !matches!(
        claims.iss.as_str(),
        "https://accounts.google.com" | "accounts.google.com"
    ) || !audience_valid
        || claims
            .azp
            .as_deref()
            .is_some_and(|value| value != client_id)
        || claims.sub.is_empty()
        || claims.sub.len() > 256
        || claims.iat > now.saturating_add(5)
        || expected_nonce.is_empty()
        || !constant_time_equal(claims.nonce.as_bytes(), expected_nonce.as_bytes())
    {
        return Err(google_identity_error());
    }
    let email = claims.email.filter(|value| {
        claims.email_verified.unwrap_or(false) && value.len() <= 320 && !value.trim().is_empty()
    });
    let name = claims
        .name
        .filter(|value| !value.trim().is_empty() && value.len() <= 512);
    Ok(VerifiedGoogleIdentity {
        subject: claims.sub,
        email,
        name,
        account_type: google_account_type_from_hosted_domain(claims.hd.as_deref())?,
    })
}

struct VerifiedGoogleIdentity {
    subject: String,
    email: Option<String>,
    name: Option<String>,
    account_type: GoogleAccountType,
}

fn valid_google_hosted_domain(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

fn google_account_type_from_hosted_domain(
    hosted_domain: Option<&str>,
) -> AppResult<GoogleAccountType> {
    match hosted_domain {
        Some(value) if valid_google_hosted_domain(value) => Ok(GoogleAccountType::Workspace),
        Some(_) => Err(google_identity_error()),
        None => Ok(GoogleAccountType::Individual),
    }
}

fn code_assist_status(account: Option<&GoogleAccount>) -> GoogleCodeAssistStatus {
    let account_type = account
        .map(|value| value.account_type)
        .unwrap_or(GoogleAccountType::Unknown);
    let cloud_project_id = account.and_then(|value| value.cloud_project_id.clone());
    let availability = match account_type {
        GoogleAccountType::Unknown => GoogleCodeAssistAvailability::NotConnected,
        GoogleAccountType::Individual => GoogleCodeAssistAvailability::ConsumerAccessRetired,
        GoogleAccountType::Workspace if cloud_project_id.is_none() => {
            GoogleCodeAssistAvailability::ProjectRequired
        }
        GoogleAccountType::Workspace => GoogleCodeAssistAvailability::EligibilityUnknown,
    };
    GoogleCodeAssistStatus {
        account_type,
        availability,
        requires_cloud_project: account_type == GoogleAccountType::Workspace
            && cloud_project_id.is_none(),
        cloud_project_id,
        user_tier: None,
        inference_available: false,
    }
}

fn validate_initial_token_response(response: &GoogleTokenResponse) -> AppResult<()> {
    if response.token_type != "Bearer"
        || response.access_token.is_empty()
        || response.access_token.len() > 65_536
        || response
            .refresh_token
            .as_deref()
            .is_none_or(|value| value.is_empty() || value.len() > 65_536)
        || response
            .id_token
            .as_deref()
            .is_none_or(|value| value.is_empty() || value.len() > 65_536)
        || response.expires_in == 0
        || response.expires_in > 24 * 60 * 60
        || response
            .scope
            .as_deref()
            .is_none_or(|value| !valid_scope_list(value))
    {
        return Err(google_token_error());
    }
    Ok(())
}

fn validate_refresh_token_response(response: &GoogleTokenResponse) -> AppResult<()> {
    if response.token_type != "Bearer"
        || response.access_token.is_empty()
        || response.access_token.len() > 65_536
        || response
            .refresh_token
            .as_deref()
            .is_some_and(|value| value.is_empty() || value.len() > 65_536)
        || response
            .id_token
            .as_deref()
            .is_some_and(|value| value.is_empty() || value.len() > 65_536)
        || response.expires_in == 0
        || response.expires_in > 24 * 60 * 60
        || response
            .scope
            .as_deref()
            .is_some_and(|value| !valid_scope_list(value))
    {
        return Err(google_token_error());
    }
    Ok(())
}

fn has_required_scopes(scopes: &[String]) -> bool {
    ["openid", "email", "profile"]
        .iter()
        .all(|required| scopes.iter().any(|scope| scope == required))
}

fn valid_scope_list(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 4096
        && value.split_ascii_whitespace().all(|scope| {
            !scope.is_empty()
                && scope.len() <= 256
                && scope.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
        })
}

fn parse_timestamp(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|date| date.with_timezone(&Utc))
}

fn google_refresh_lock(account_id: &str) -> AppResult<RefreshLock> {
    let mut locks = GOOGLE_REFRESH_LOCKS
        .get_or_init(|| StdMutex::new(HashMap::new()))
        .lock()
        .map_err(AppError::internal)?;
    Ok(locks
        .entry(account_id.to_owned())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone())
}

fn create_pkce_pair() -> (String, String) {
    let verifier = fresh_random_value();
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

fn fresh_random_value() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0_u8, |diff, (a, b)| diff | (a ^ b))
            == 0
}

fn valid_client_id(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 256 && value.ends_with(".apps.googleusercontent.com")
}

fn valid_project_id(value: &str) -> bool {
    (6..=30).contains(&value.len())
        && value.as_bytes()[0].is_ascii_lowercase()
        && value.as_bytes()[value.len() - 1].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn validate_redirect_uri(value: &str) -> AppResult<()> {
    let url = Url::parse(value).map_err(|_| google_callback_error())?;
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || url.port().is_none()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(google_callback_error());
    }
    Ok(())
}

fn is_revoked_grant(error: &AppError) -> bool {
    matches!(
        error.code.as_str(),
        "invalid_credential" | "expired_authentication"
    )
}

async fn read_google_json<T: for<'de> Deserialize<'de>>(
    mut response: reqwest::Response,
) -> AppResult<T> {
    if !response.status().is_success() {
        let status = response.status();
        let body = Zeroizing::new(super::read_limited(&mut response, 8192).await);
        let code = serde_json::from_slice::<serde_json::Value>(&body)
            .ok()
            .and_then(|value| {
                value
                    .get("error")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            });
        if code.as_deref() == Some("invalid_grant") {
            return Err(AppError::new(
                "invalid_credential",
                "Google authorization has expired or was revoked. Reconnect Google in Settings.",
            ));
        }
        if code.as_deref() == Some("invalid_client") {
            return Err(AppError::new(
                "google_oauth_configuration",
                "Google rejected JevCode's OAuth client configuration. Check the registered Desktop client ID.",
            ));
        }
        return Err(google_provider_error(status));
    }
    const MAX: usize = 1024 * 1024;
    if response
        .content_length()
        .is_some_and(|length| length > MAX as u64)
    {
        return Err(google_token_error());
    }
    let mut body = Zeroizing::new(Vec::new());
    loop {
        let chunk = response.chunk().await.map_err(|_| google_network_error())?;
        let Some(chunk) = chunk else { break };
        if body.len().saturating_add(chunk.len()) > MAX {
            return Err(google_token_error());
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| google_token_error())
}

fn google_provider_error(status: StatusCode) -> AppError {
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => AppError::new(
            "invalid_credential",
            "Google rejected this authentication. Reconnect Google or check the API key.",
        ),
        StatusCode::TOO_MANY_REQUESTS => AppError::new(
            "quota_exhausted",
            "Gemini API quota is exhausted. Check the Google Cloud project quota.",
        ),
        value if value.is_server_error() => AppError::new(
            "provider_outage",
            "Google's authentication service is unavailable. Try again later.",
        ),
        _ => AppError::new(
            "oauth_failure",
            "Google could not complete authentication. Check the app's Google OAuth configuration.",
        ),
    }
}

fn google_network_error() -> AppError {
    AppError::new(
        "network_error",
        "Could not connect to Google's authentication service. Check your network and try again.",
    )
}

fn google_identity_error() -> AppError {
    AppError::new(
        "invalid_identity",
        "Google sign-in could not verify the returned account. Start a new sign-in attempt.",
    )
}

fn google_callback_error() -> AppError {
    AppError::new(
        "invalid_auth_callback",
        "The Google sign-in response could not be verified. Start sign-in again.",
    )
}

fn google_token_error() -> AppError {
    AppError::new(
        "invalid_auth_response",
        "Google returned an incomplete authentication response. Start sign-in again.",
    )
}

fn google_scope_error() -> AppError {
    AppError::new("google_scope_denied", "Google did not grant the basic account identity permissions required by JevCode. Review the requested permissions and try again.")
}

fn google_code_assist_unsupported_error() -> AppError {
    AppError::new(
        "google_code_assist_unsupported",
        "Google account sign-in is available for account identification only. Google has not documented a supported third-party Gemini Code Assist inference API, so JevCode will not send this OAuth token to another Gemini service. Use a Gemini API key for model requests.",
    )
}

fn google_reconnect_error() -> AppError {
    AppError::new(
        "google_reconnect_required",
        "Google account needs reconnecting. Reconnect Google in Settings.",
    )
}

fn decode_component(value: &str) -> AppResult<Zeroizing<String>> {
    let bytes = value.as_bytes();
    let mut output = Zeroizing::new(Vec::with_capacity(bytes.len()));
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                output.push(b' ');
                index += 1;
            }
            b'%' => {
                let pair = bytes
                    .get(index + 1..index + 3)
                    .ok_or_else(google_callback_error)?;
                let hex = |byte: u8| match byte {
                    b'0'..=b'9' => Some(byte - b'0'),
                    b'a'..=b'f' => Some(byte - b'a' + 10),
                    b'A'..=b'F' => Some(byte - b'A' + 10),
                    _ => None,
                };
                output.push(
                    (hex(pair[0]).ok_or_else(google_callback_error)? << 4)
                        | hex(pair[1]).ok_or_else(google_callback_error)?,
                );
                index += 3;
            }
            byte => {
                output.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(output.to_vec())
        .map(Zeroizing::new)
        .map_err(|_| google_callback_error())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ProviderProtocol;
    use jsonwebtoken::{encode, EncodingKey, Header};
    use rand::thread_rng;
    use rsa::{pkcs1::EncodeRsaPrivateKey, traits::PublicKeyParts, RsaPrivateKey, RsaPublicKey};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    };

    const TEST_CLIENT_ID: &str = "jevcode-test.apps.googleusercontent.com";
    const TEST_ACCESS_TOKEN: &str = "test-google-access-token";
    const TEST_REFRESH_TOKEN: &str = "test-google-refresh-token";

    fn provider() -> Provider {
        Provider {
            id: "gemini".into(),
            name: "Google Gemini".into(),
            protocol: ProviderProtocol::Gemini,
            base_url: Some("https://generativelanguage.googleapis.com/v1beta".into()),
            models: vec![],
            connected: false,
        }
    }

    #[derive(Default)]
    struct MemoryGoogleSessionStorage {
        serialized: Mutex<Option<String>>,
    }

    impl GoogleSessionStorage for MemoryGoogleSessionStorage {
        fn save_google_session(&self, session: &str) -> AppResult<()> {
            *self.serialized.lock().map_err(AppError::internal)? = Some(session.to_owned());
            Ok(())
        }

        fn get_google_session(&self) -> AppResult<Option<String>> {
            Ok(self.serialized.lock().map_err(AppError::internal)?.clone())
        }

        fn delete_google_session(&self) -> AppResult<()> {
            if let Some(mut serialized) = self.serialized.lock().map_err(AppError::internal)?.take()
            {
                serialized.zeroize();
            }
            Ok(())
        }
    }

    #[derive(Clone, PartialEq, Eq)]
    struct ExchangeArguments {
        client_id: String,
        redirect_uri: String,
        code: String,
        verifier: String,
    }

    struct MockGoogleOAuthTransport {
        jwks: JwkSet,
        id_token: String,
        exchange_arguments: Mutex<Option<ExchangeArguments>>,
        revoke_calls: AtomicUsize,
        refresh_calls: AtomicUsize,
        refresh_behavior: MockRefreshBehavior,
    }

    #[derive(Clone, Copy)]
    enum MockRefreshBehavior {
        Success,
        Revoked,
        Network,
        MissingScopes,
    }

    #[async_trait]
    impl GoogleOAuthTransport for MockGoogleOAuthTransport {
        async fn exchange_code(
            &self,
            client_id: &str,
            redirect_uri: &str,
            code: &str,
            verifier: &str,
        ) -> AppResult<GoogleTokenResponse> {
            *self.exchange_arguments.lock().map_err(AppError::internal)? =
                Some(ExchangeArguments {
                    client_id: client_id.to_owned(),
                    redirect_uri: redirect_uri.to_owned(),
                    code: code.to_owned(),
                    verifier: verifier.to_owned(),
                });
            Ok(GoogleTokenResponse {
                access_token: TEST_ACCESS_TOKEN.into(),
                refresh_token: Some(TEST_REFRESH_TOKEN.into()),
                id_token: Some(self.id_token.clone()),
                token_type: "Bearer".into(),
                expires_in: 3600,
                scope: Some(GOOGLE_SCOPES.into()),
            })
        }

        async fn refresh_token(
            &self,
            _client_id: &str,
            refresh_token: &str,
        ) -> AppResult<GoogleTokenResponse> {
            assert_eq!(refresh_token, TEST_REFRESH_TOKEN);
            self.refresh_calls.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(30)).await;
            match self.refresh_behavior {
                MockRefreshBehavior::Success => Ok(GoogleTokenResponse {
                    access_token: "refreshed-google-access-token".into(),
                    refresh_token: Some("rotated-google-refresh-token".into()),
                    id_token: None,
                    token_type: "Bearer".into(),
                    expires_in: 3600,
                    scope: Some(GOOGLE_SCOPES.into()),
                }),
                MockRefreshBehavior::Revoked => Err(AppError::new(
                    "invalid_credential",
                    "redacted test authentication failure",
                )),
                MockRefreshBehavior::Network => Err(google_network_error()),
                MockRefreshBehavior::MissingScopes => Ok(GoogleTokenResponse {
                    access_token: "refreshed-google-access-token".into(),
                    refresh_token: None,
                    id_token: None,
                    token_type: "Bearer".into(),
                    expires_in: 3600,
                    scope: Some("openid".into()),
                }),
            }
        }

        async fn discovery(&self) -> AppResult<GoogleOidcDiscovery> {
            Ok(GoogleOidcDiscovery {
                issuer: "https://accounts.google.com".into(),
                jwks_uri: "https://www.googleapis.com/oauth2/v3/certs".into(),
                id_token_signing_alg_values_supported: vec!["RS256".into()],
            })
        }

        async fn jwks(&self, url: &str) -> AppResult<JwkSet> {
            assert_eq!(url, "https://www.googleapis.com/oauth2/v3/certs");
            Ok(self.jwks.clone())
        }

        async fn revoke_token(&self, token: &str) -> AppResult<()> {
            assert_eq!(token, TEST_REFRESH_TOKEN);
            self.revoke_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn test_signing_key() -> (EncodingKey, JwkSet) {
        let private = RsaPrivateKey::new(&mut thread_rng(), 2048).unwrap();
        let public = RsaPublicKey::from(&private);
        let private_der = private.to_pkcs1_der().unwrap();
        let encoding = EncodingKey::from_rsa_der(private_der.as_bytes());
        let jwks = serde_json::from_value(serde_json::json!({
            "keys": [{
                "kty": "RSA",
                "kid": "jevcode-google-test-key",
                "use": "sig",
                "alg": "RS256",
                "n": URL_SAFE_NO_PAD.encode(public.n().to_bytes_be()),
                "e": URL_SAFE_NO_PAD.encode(public.e().to_bytes_be())
            }]
        }))
        .unwrap();
        (encoding, jwks)
    }

    fn sign_test_id_token(encoding: &EncodingKey, nonce: &str) -> String {
        sign_test_id_token_with_hosted_domain(encoding, nonce, None)
    }

    fn sign_test_id_token_with_hosted_domain(
        encoding: &EncodingKey,
        nonce: &str,
        hosted_domain: Option<&str>,
    ) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("jevcode-google-test-key".into());
        let mut claims = serde_json::json!({
            "iss": "https://accounts.google.com",
            "sub": "google-subject-42",
            "aud": TEST_CLIENT_ID,
            "iat": Utc::now().timestamp() as u64,
            "exp": (Utc::now() + ChronoDuration::minutes(5)).timestamp() as u64,
            "nonce": nonce,
            "email": "verified@example.test",
            "email_verified": true,
            "name": "JevCode Test Account"
        });
        if let Some(hosted_domain) = hosted_domain {
            claims
                .as_object_mut()
                .expect("test ID token claims are an object")
                .insert("hd".into(), serde_json::Value::String(hosted_domain.into()));
        }
        encode(&header, &claims, encoding).unwrap()
    }

    fn test_manager(
        oauth: Arc<dyn GoogleOAuthTransport>,
        session_storage: Arc<dyn GoogleSessionStorage>,
    ) -> GoogleAuthManager {
        GoogleAuthManager::with_test_dependencies(
            provider(),
            Some(TEST_CLIENT_ID.into()),
            Some("jevcode-test-project".into()),
            oauth,
            session_storage,
        )
        .unwrap()
    }

    fn refresh_mock(behavior: MockRefreshBehavior) -> Arc<MockGoogleOAuthTransport> {
        Arc::new(MockGoogleOAuthTransport {
            jwks: serde_json::from_value(serde_json::json!({ "keys": [] })).unwrap(),
            id_token: String::new(),
            exchange_arguments: Mutex::new(None),
            revoke_calls: AtomicUsize::new(0),
            refresh_calls: AtomicUsize::new(0),
            refresh_behavior: behavior,
        })
    }

    fn save_test_session(
        storage: &MemoryGoogleSessionStorage,
        account_id: &str,
        expires_at: DateTime<Utc>,
        refresh_token: &str,
    ) {
        let expires_at = expires_at.to_rfc3339();
        let scopes = GOOGLE_SCOPES
            .split_ascii_whitespace()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let session = GoogleAuthSession {
            account: GoogleAccount {
                id: account_id.into(),
                email: Some("persistent@example.test".into()),
                display_name: Some("Persistent Test Account".into()),
                auth_type: GoogleAuthType::GoogleOAuth,
                account_type: GoogleAccountType::Individual,
                cloud_project_id: Some("jevcode-test-project".into()),
                connected: true,
                requires_reconnect: false,
                expires_at: Some(expires_at.clone()),
                granted_scopes: scopes.clone(),
            },
            client_id: TEST_CLIENT_ID.into(),
            cloud_project_id: "jevcode-test-project".into(),
            access_token: "existing-google-access-token".into(),
            refresh_token: refresh_token.into(),
            id_token: "test-only-id-token".into(),
            expires_at,
            granted_scopes: scopes,
        };
        storage
            .save_google_session(&serde_json::to_string(&session).unwrap())
            .unwrap();
    }

    fn save_test_provider_account(database: &Database) {
        database
            .save_provider_account(&ProviderAccount {
                provider_id: "gemini".into(),
                provider_name: "Google Gemini".into(),
                state: ProviderAuthState::Connected,
                auth_method: Some(ProviderAuthMethod::GoogleOAuth),
                account_label: Some("persistent@example.test".into()),
                connected_at: Some(Utc::now().to_rfc3339()),
                last_validated_at: Some(Utc::now().to_rfc3339()),
                last_error_code: None,
                available_methods: vec![],
            })
            .unwrap();
    }

    fn test_oauth_manager(
        oauth: Arc<MockGoogleOAuthTransport>,
        storage: Arc<MemoryGoogleSessionStorage>,
    ) -> GoogleAuthManager {
        test_manager(oauth, storage)
    }

    #[tokio::test]
    async fn saved_google_session_is_restored_after_database_and_manager_restart() {
        let root = tempfile::tempdir().unwrap();
        let database_path = root.path().join("restart-google-auth.sqlite");
        let storage = Arc::new(MemoryGoogleSessionStorage::default());
        let account_id = uuid::Uuid::new_v4().to_string();
        save_test_session(
            &storage,
            &account_id,
            Utc::now() + ChronoDuration::minutes(45),
            TEST_REFRESH_TOKEN,
        );
        let oauth = refresh_mock(MockRefreshBehavior::Success);
        {
            let database = Database::open(&database_path).unwrap();
            save_test_provider_account(&database);
            let manager = test_oauth_manager(oauth.clone(), storage.clone());
            let status = manager
                .status(
                    &CredentialStore,
                    database.provider_account("gemini").unwrap(),
                )
                .unwrap();
            let account = status.account.unwrap();
            assert_eq!(account.id, account_id);
            assert_eq!(account.email.as_deref(), Some("persistent@example.test"));
        }

        // A new manager and a reopened SQLite connection model a full app restart.
        let database = Database::open(&database_path).unwrap();
        let manager = test_oauth_manager(oauth.clone(), storage.clone());
        let (access_token, _) = manager
            .access_token(&CredentialStore, &database, false)
            .await
            .unwrap();
        assert_eq!(access_token.as_str(), "existing-google-access-token");
        assert_eq!(oauth.refresh_calls.load(Ordering::SeqCst), 0);
        assert!(storage.get_google_session().unwrap().is_some());
    }

    #[tokio::test]
    async fn expired_access_token_refreshes_and_rotates_secure_credentials() {
        let root = tempfile::tempdir().unwrap();
        let database = Database::open(&root.path().join("expired-google-auth.sqlite")).unwrap();
        save_test_provider_account(&database);
        let storage = Arc::new(MemoryGoogleSessionStorage::default());
        save_test_session(
            &storage,
            &uuid::Uuid::new_v4().to_string(),
            Utc::now() - ChronoDuration::seconds(1),
            TEST_REFRESH_TOKEN,
        );
        let oauth = refresh_mock(MockRefreshBehavior::Success);
        let manager = test_oauth_manager(oauth.clone(), storage.clone());

        let (access_token, _) = manager
            .access_token(&CredentialStore, &database, false)
            .await
            .unwrap();
        assert_eq!(access_token.as_str(), "refreshed-google-access-token");
        assert_eq!(oauth.refresh_calls.load(Ordering::SeqCst), 1);
        let serialized = storage.get_google_session().unwrap().unwrap();
        let session: GoogleAuthSession = serde_json::from_str(&serialized).unwrap();
        assert_eq!(session.access_token, "refreshed-google-access-token");
        assert_eq!(session.refresh_token, "rotated-google-refresh-token");
        assert!(session.account.connected);
        assert!(!session.account.requires_reconnect);
        assert_eq!(
            database.provider_account("gemini").unwrap().unwrap().state,
            ProviderAuthState::Connected
        );
    }

    #[tokio::test]
    async fn concurrent_gemini_requests_share_one_refresh() {
        let root = tempfile::tempdir().unwrap();
        let database = Database::open(&root.path().join("concurrent-google-auth.sqlite")).unwrap();
        save_test_provider_account(&database);
        let storage = Arc::new(MemoryGoogleSessionStorage::default());
        save_test_session(
            &storage,
            &uuid::Uuid::new_v4().to_string(),
            Utc::now() - ChronoDuration::seconds(1),
            TEST_REFRESH_TOKEN,
        );
        let oauth = refresh_mock(MockRefreshBehavior::Success);
        let manager = test_oauth_manager(oauth.clone(), storage);

        let results = futures_util::future::join_all(
            (0..8).map(|_| manager.access_token(&CredentialStore, &database, false)),
        )
        .await;
        for result in results {
            let (access_token, _) = result.unwrap();
            assert_eq!(access_token.as_str(), "refreshed-google-access-token");
        }
        assert_eq!(oauth.refresh_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn revoked_refresh_marks_reconnect_and_does_not_retry_on_later_requests() {
        let root = tempfile::tempdir().unwrap();
        let database = Database::open(&root.path().join("revoked-google-auth.sqlite")).unwrap();
        save_test_provider_account(&database);
        let storage = Arc::new(MemoryGoogleSessionStorage::default());
        save_test_session(
            &storage,
            &uuid::Uuid::new_v4().to_string(),
            Utc::now() - ChronoDuration::seconds(1),
            TEST_REFRESH_TOKEN,
        );
        let oauth = refresh_mock(MockRefreshBehavior::Revoked);
        let manager = test_oauth_manager(oauth.clone(), storage.clone());

        for _ in 0..2 {
            let error = manager
                .access_token(&CredentialStore, &database, false)
                .await
                .expect_err("revoked authorization should require reconnection");
            assert_eq!(error.code, "google_reconnect_required");
            assert!(error
                .message
                .starts_with("Google account needs reconnecting."));
        }
        assert_eq!(oauth.refresh_calls.load(Ordering::SeqCst), 1);
        let serialized = storage.get_google_session().unwrap().unwrap();
        let session: GoogleAuthSession = serde_json::from_str(&serialized).unwrap();
        assert!(!session.account.connected);
        assert!(session.account.requires_reconnect);
        let account = database.provider_account("gemini").unwrap().unwrap();
        assert_eq!(account.state, ProviderAuthState::NeedsAttention);
        assert_eq!(
            account.last_error_code.as_deref(),
            Some("google_reconnect_required")
        );
    }

    #[tokio::test]
    async fn missing_refresh_token_requires_reconnect_only_after_access_token_expires() {
        let root = tempfile::tempdir().unwrap();
        let database =
            Database::open(&root.path().join("missing-refresh-google-auth.sqlite")).unwrap();
        save_test_provider_account(&database);
        let valid_storage = Arc::new(MemoryGoogleSessionStorage::default());
        save_test_session(
            &valid_storage,
            &uuid::Uuid::new_v4().to_string(),
            Utc::now() + ChronoDuration::minutes(15),
            "",
        );
        let oauth = refresh_mock(MockRefreshBehavior::Success);
        let valid_manager = test_oauth_manager(oauth.clone(), valid_storage.clone());
        let (access_token, _) = valid_manager
            .access_token(&CredentialStore, &database, false)
            .await
            .unwrap();
        assert_eq!(access_token.as_str(), "existing-google-access-token");
        assert_eq!(oauth.refresh_calls.load(Ordering::SeqCst), 0);
        let valid_session: GoogleAuthSession =
            serde_json::from_str(&valid_storage.get_google_session().unwrap().unwrap()).unwrap();
        assert!(!valid_session.account.requires_reconnect);

        let storage = Arc::new(MemoryGoogleSessionStorage::default());
        save_test_session(
            &storage,
            &uuid::Uuid::new_v4().to_string(),
            Utc::now() - ChronoDuration::seconds(1),
            "",
        );
        let oauth = refresh_mock(MockRefreshBehavior::Success);
        let manager = test_oauth_manager(oauth.clone(), storage.clone());
        let status = manager
            .status(
                &CredentialStore,
                database.provider_account("gemini").unwrap(),
            )
            .unwrap();
        assert!(status.account.unwrap().requires_reconnect);

        let error = manager
            .access_token(&CredentialStore, &database, false)
            .await
            .expect_err("missing refresh token should require reconnection");
        assert_eq!(error.code, "google_reconnect_required");
        assert_eq!(oauth.refresh_calls.load(Ordering::SeqCst), 0);
        let session: GoogleAuthSession =
            serde_json::from_str(&storage.get_google_session().unwrap().unwrap()).unwrap();
        assert!(session.account.requires_reconnect);
    }

    #[tokio::test]
    async fn network_refresh_failure_preserves_session_for_a_later_user_retry() {
        let root = tempfile::tempdir().unwrap();
        let database = Database::open(&root.path().join("network-google-auth.sqlite")).unwrap();
        save_test_provider_account(&database);
        let storage = Arc::new(MemoryGoogleSessionStorage::default());
        save_test_session(
            &storage,
            &uuid::Uuid::new_v4().to_string(),
            Utc::now() - ChronoDuration::seconds(1),
            TEST_REFRESH_TOKEN,
        );
        let oauth = refresh_mock(MockRefreshBehavior::Network);
        let manager = test_oauth_manager(oauth.clone(), storage.clone());

        let error = manager
            .access_token(&CredentialStore, &database, false)
            .await
            .expect_err("network failure should be reported");
        assert_eq!(error.code, "network_error");
        assert_eq!(oauth.refresh_calls.load(Ordering::SeqCst), 1);
        let session: GoogleAuthSession =
            serde_json::from_str(&storage.get_google_session().unwrap().unwrap()).unwrap();
        assert!(!session.account.requires_reconnect);
        assert_eq!(
            database.provider_account("gemini").unwrap().unwrap().state,
            ProviderAuthState::Connected
        );
    }

    #[tokio::test]
    async fn lost_gemini_permissions_require_google_reconnection() {
        let root = tempfile::tempdir().unwrap();
        let database = Database::open(&root.path().join("scope-google-auth.sqlite")).unwrap();
        save_test_provider_account(&database);
        let storage = Arc::new(MemoryGoogleSessionStorage::default());
        save_test_session(
            &storage,
            &uuid::Uuid::new_v4().to_string(),
            Utc::now() - ChronoDuration::seconds(1),
            TEST_REFRESH_TOKEN,
        );
        let oauth = refresh_mock(MockRefreshBehavior::MissingScopes);
        let manager = test_oauth_manager(oauth.clone(), storage.clone());

        let error = manager
            .access_token(&CredentialStore, &database, false)
            .await
            .expect_err("lost Gemini scopes should require reconnection");
        assert_eq!(error.code, "google_reconnect_required");
        assert_eq!(oauth.refresh_calls.load(Ordering::SeqCst), 1);
        let session: GoogleAuthSession =
            serde_json::from_str(&storage.get_google_session().unwrap().unwrap()).unwrap();
        assert!(session.account.requires_reconnect);
        assert_eq!(
            database.provider_account("gemini").unwrap().unwrap().state,
            ProviderAuthState::NeedsAttention
        );
    }

    #[test]
    fn code_exchange_request_uses_google_token_endpoint_and_exact_pkce_fields() {
        let transport = ReqwestGoogleOAuthTransport::new().unwrap();
        let request = transport
            .build_exchange_request(
                TEST_CLIENT_ID,
                "http://127.0.0.1:43123",
                "test-auth-code",
                "test-code-verifier",
            )
            .unwrap();

        assert_eq!(request.method(), reqwest::Method::POST);
        assert_eq!(request.url().as_str(), GOOGLE_TOKEN_URL);
        assert_eq!(
            request
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .unwrap(),
            "application/x-www-form-urlencoded"
        );
        let form = std::str::from_utf8(
            request
                .body()
                .and_then(reqwest::Body::as_bytes)
                .expect("form body is buffered"),
        )
        .unwrap();
        for field in [
            "client_id=jevcode-test.apps.googleusercontent.com",
            "code=test-auth-code",
            "code_verifier=test-code-verifier",
            "grant_type=authorization_code",
            "redirect_uri=http%3A%2F%2F127.0.0.1%3A43123",
        ] {
            assert!(form.contains(field), "missing {field}");
        }
        assert!(!form.contains("client_secret"));
    }

    #[tokio::test]
    async fn authorization_code_exchange_validates_identity_and_keeps_tokens_native() {
        let root = tempfile::tempdir().unwrap();
        let database = Database::open(&root.path().join("google-auth.sqlite")).unwrap();
        let credentials = CredentialStore;
        let session_creator = GoogleAuthManager::with_config(
            provider(),
            Some(TEST_CLIENT_ID.into()),
            Some("jevcode-test-project".into()),
        )
        .unwrap();
        let redirect_uri = "http://127.0.0.1:43123";
        let pending = session_creator
            .create_authentication_session(redirect_uri)
            .unwrap();
        let (encoding, jwks) = test_signing_key();
        let oauth = Arc::new(MockGoogleOAuthTransport {
            jwks,
            id_token: sign_test_id_token(&encoding, &pending.nonce),
            exchange_arguments: Mutex::new(None),
            revoke_calls: AtomicUsize::new(0),
            refresh_calls: AtomicUsize::new(0),
            refresh_behavior: MockRefreshBehavior::Network,
        });
        let storage = Arc::new(MemoryGoogleSessionStorage::default());
        let manager = test_manager(oauth.clone(), storage.clone());

        let account = manager
            .complete_authentication(
                &pending,
                "authorization-code-for-test",
                &credentials,
                &database,
            )
            .await
            .unwrap();

        assert_eq!(account.id, "google-subject-42");
        assert_eq!(account.email.as_deref(), Some("verified@example.test"));
        assert_eq!(account.account_type, GoogleAccountType::Individual);
        assert_eq!(
            account.display_name.as_deref(),
            Some("JevCode Test Account")
        );
        assert!(account.connected);
        let exchange = oauth
            .exchange_arguments
            .lock()
            .unwrap()
            .clone()
            .expect("authorization code was exchanged");
        assert_eq!(exchange.client_id, TEST_CLIENT_ID);
        assert_eq!(exchange.redirect_uri, redirect_uri);
        assert_eq!(exchange.code, "authorization-code-for-test");
        assert_eq!(exchange.verifier, pending.code_verifier);

        let serialized_session = storage.serialized.lock().unwrap().clone().unwrap();
        assert!(serialized_session.contains(TEST_ACCESS_TOKEN));
        assert!(serialized_session.contains(TEST_REFRESH_TOKEN));
        let stored_account = database.provider_account("gemini").unwrap().unwrap();
        let safe_metadata = serde_json::to_string(&stored_account).unwrap();
        for secret in [TEST_ACCESS_TOKEN, TEST_REFRESH_TOKEN, "id_token"] {
            assert!(!safe_metadata.contains(secret));
        }
        let connection =
            rusqlite::Connection::open(root.path().join("google-auth.sqlite")).unwrap();
        let persisted: String = connection
            .query_row(
                "SELECT data FROM provider_accounts WHERE provider_id = 'gemini'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        for secret in [TEST_ACCESS_TOKEN, TEST_REFRESH_TOKEN, "id_token"] {
            assert!(!persisted.contains(secret));
        }
        let status = manager.status(&credentials, Some(stored_account)).unwrap();
        let safe_status = serde_json::to_string(&status).unwrap();
        for secret in [TEST_ACCESS_TOKEN, TEST_REFRESH_TOKEN, "id_token"] {
            assert!(!safe_status.contains(secret));
        }

        assert!(manager.logout(&credentials, &database).await.unwrap());
        assert_eq!(oauth.revoke_calls.load(Ordering::SeqCst), 1);
        assert!(storage.serialized.lock().unwrap().is_none());
        assert!(database.provider_account("gemini").unwrap().is_none());
        assert!(manager
            .status(&credentials, None)
            .unwrap()
            .account
            .is_none());
    }

    #[test]
    fn malformed_token_responses_fail_with_redacted_errors() {
        let malformed = GoogleTokenResponse {
            access_token: TEST_ACCESS_TOKEN.into(),
            refresh_token: None,
            id_token: Some("test-id-token".into()),
            token_type: "Bearer".into(),
            expires_in: 3600,
            scope: Some(GOOGLE_SCOPES.into()),
        };
        let error = validate_initial_token_response(&malformed).unwrap_err();
        assert_eq!(error.code, "invalid_auth_response");
        assert!(!error.message.contains(TEST_ACCESS_TOKEN));
        assert!(!error.message.contains("test-id-token"));
    }

    #[test]
    fn pkce_uses_s256_and_fresh_crypto_random_values() {
        let (verifier_a, challenge_a) = create_pkce_pair();
        let (verifier_b, challenge_b) = create_pkce_pair();
        assert_eq!(verifier_a.len(), 64);
        assert_eq!(
            challenge_a,
            URL_SAFE_NO_PAD.encode(Sha256::digest(verifier_a.as_bytes()))
        );
        assert_ne!(verifier_a, verifier_b);
        assert_ne!(challenge_a, challenge_b);
    }

    #[test]
    fn oauth_is_unavailable_without_a_registered_desktop_client() {
        let manager = GoogleAuthManager::new(provider()).unwrap();
        match manager.create_authentication_session("http://127.0.0.1:43123") {
            Err(error) => assert_eq!(error.code, "google_oauth_not_configured"),
            Ok(_) => panic!("OAuth must remain disabled without registered Google configuration"),
        }
    }

    #[test]
    fn desktop_oauth_request_uses_fresh_state_nonce_and_s256_pkce() {
        let manager = GoogleAuthManager::with_config(
            provider(),
            Some("jevcode-client.apps.googleusercontent.com".into()),
            Some("jevcode-project-123".into()),
        )
        .unwrap();
        let first = manager
            .create_authentication_session("http://127.0.0.1:43123")
            .unwrap();
        let second = manager
            .create_authentication_session("http://127.0.0.1:43124")
            .unwrap();
        let query: HashMap<_, _> = first.authorization_url.query_pairs().into_owned().collect();
        assert_eq!(
            first.authorization_url.host_str(),
            Some("accounts.google.com")
        );
        assert_eq!(query.get("response_type").map(String::as_str), Some("code"));
        assert_eq!(
            query.get("redirect_uri").map(String::as_str),
            Some(first.redirect_uri.as_str())
        );
        assert_eq!(first.redirect_uri, "http://127.0.0.1:43123");
        assert_eq!(
            query.get("client_id").map(String::as_str),
            Some("jevcode-client.apps.googleusercontent.com")
        );
        assert!(!query.contains_key("client_secret"));
        assert_eq!(
            query.get("access_type").map(String::as_str),
            Some("offline")
        );
        assert_eq!(
            query.get("prompt").map(String::as_str),
            Some("select_account")
        );
        assert_eq!(query.get("scope").map(String::as_str), Some(GOOGLE_SCOPES));
        assert_eq!(
            query.get("code_challenge_method").map(String::as_str),
            Some("S256")
        );
        assert_eq!(
            query.get("code_challenge").map(String::as_str),
            Some(
                URL_SAFE_NO_PAD
                    .encode(Sha256::digest(first.code_verifier.as_bytes()))
                    .as_str()
            )
        );
        assert_eq!(
            query.get("state").map(String::as_str),
            Some(first.state.as_str())
        );
        assert_eq!(
            query.get("nonce").map(String::as_str),
            Some(first.nonce.as_str())
        );
        assert_ne!(first.state, second.state);
        assert_ne!(first.nonce, second.nonce);
        assert_ne!(first.code_verifier, second.code_verifier);
        assert_eq!(first.state.len(), 64);
        assert_eq!(first.code_verifier.len(), 64);
        assert_eq!(query.get("code_challenge").unwrap().len(), 43);
    }

    #[test]
    fn callback_accepts_only_one_code_with_matching_state_and_loopback_host() {
        let manager = GoogleAuthManager::with_config(
            provider(),
            Some("jevcode-client.apps.googleusercontent.com".into()),
            Some("jevcode-project-123".into()),
        )
        .unwrap();
        let pending = manager
            .create_authentication_session("http://127.0.0.1:43123")
            .unwrap();
        let valid = format!("/?code=auth-code-123&state={}", pending.state);
        let parsed_code = parse_authorization_callback(&pending, &valid, "127.0.0.1:43123")
            .unwrap()
            .unwrap();
        assert_eq!(parsed_code.as_str(), "auth-code-123");
        assert!(parse_authorization_callback(
            &pending,
            &valid.replacen('/', "/auth/callback", 1),
            "127.0.0.1:43123"
        )
        .is_err());
        let wrong_state = "/?code=auth-code-123&state=wrong";
        assert!(parse_authorization_callback(&pending, wrong_state, "127.0.0.1:43123").is_err());
        let missing_code = format!("/?state={}", pending.state);
        assert!(parse_authorization_callback(&pending, &missing_code, "127.0.0.1:43123").is_err());
        assert!(parse_authorization_callback(&pending, &valid, "localhost:43123").is_err());
        assert!(parse_authorization_callback(&pending, &valid, "127.0.0.1:43124").is_err());
        assert!(manager
            .create_authentication_session("http://127.0.0.1:43123/oauth2callback")
            .is_err());
    }

    #[test]
    fn oauth_errors_require_the_matching_state_and_are_not_treated_as_codes() {
        let manager = GoogleAuthManager::with_config(
            provider(),
            Some("jevcode-client.apps.googleusercontent.com".into()),
            Some("jevcode-project-123".into()),
        )
        .unwrap();
        let pending = manager
            .create_authentication_session("http://127.0.0.1:43123")
            .unwrap();

        let denied = format!("/?error=access_denied&state={}", pending.state);
        assert!(
            parse_authorization_callback(&pending, &denied, "127.0.0.1:43123")
                .unwrap()
                .is_none()
        );

        let provider_error = format!("/?error=server_error&state={}", pending.state);
        assert!(
            parse_authorization_callback(&pending, &provider_error, "127.0.0.1:43123").is_err()
        );
        assert!(parse_authorization_callback(
            &pending,
            "/?error=access_denied&state=wrong",
            "127.0.0.1:43123"
        )
        .is_err());
        assert!(parse_authorization_callback(
            &pending,
            &format!(
                "/?error=access_denied&state={}&code=unexpected",
                pending.state
            ),
            "127.0.0.1:43123"
        )
        .is_err());
    }

    #[test]
    fn requested_google_identity_scopes_are_verified_as_an_all_or_nothing_set() {
        let scopes = GOOGLE_SCOPES
            .split_ascii_whitespace()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        assert!(has_required_scopes(&scopes));
        assert!(!has_required_scopes(&scopes[..scopes.len() - 1]));
    }

    #[test]
    fn oauth_identity_classifies_workspace_accounts_and_keeps_consumer_status_current() {
        assert_eq!(
            google_account_type_from_hosted_domain(None).unwrap(),
            GoogleAccountType::Individual
        );
        assert_eq!(
            google_account_type_from_hosted_domain(Some("example.com")).unwrap(),
            GoogleAccountType::Workspace
        );
        assert!(google_account_type_from_hosted_domain(Some("bad..domain")).is_err());

        let individual = GoogleAccount {
            id: "individual".into(),
            email: Some("user@gmail.com".into()),
            display_name: None,
            auth_type: GoogleAuthType::GoogleOAuth,
            account_type: GoogleAccountType::Individual,
            cloud_project_id: None,
            connected: true,
            requires_reconnect: false,
            expires_at: None,
            granted_scopes: vec![],
        };
        let personal_status = code_assist_status(Some(&individual));
        assert_eq!(
            personal_status.availability,
            GoogleCodeAssistAvailability::ConsumerAccessRetired
        );
        assert_eq!(personal_status.user_tier, None);
        assert!(!personal_status.inference_available);

        let mut workspace = individual;
        workspace.account_type = GoogleAccountType::Workspace;
        let required = code_assist_status(Some(&workspace));
        assert_eq!(
            required.availability,
            GoogleCodeAssistAvailability::ProjectRequired
        );
        assert!(required.requires_cloud_project);
        workspace.cloud_project_id = Some("team-project-123".into());
        let configured = code_assist_status(Some(&workspace));
        assert_eq!(
            configured.availability,
            GoogleCodeAssistAvailability::EligibilityUnknown
        );
        assert_eq!(
            configured.cloud_project_id.as_deref(),
            Some("team-project-123")
        );
        assert!(!configured.requires_cloud_project);
    }

    #[test]
    fn verified_workspace_id_token_is_classified_without_claiming_entitlement() {
        let (encoding, jwks) = test_signing_key();
        let nonce = "verified-workspace-nonce";
        let token = sign_test_id_token_with_hosted_domain(&encoding, nonce, Some("example.com"));
        let discovery = GoogleOidcDiscovery {
            issuer: "https://accounts.google.com".into(),
            jwks_uri: "https://www.googleapis.com/oauth2/v3/certs".into(),
            id_token_signing_alg_values_supported: vec!["RS256".into()],
        };
        let identity =
            validate_google_id_token(&token, nonce, TEST_CLIENT_ID, &discovery, &jwks).unwrap();
        assert_eq!(identity.account_type, GoogleAccountType::Workspace);
        let account = GoogleAccount {
            id: identity.subject,
            email: identity.email,
            display_name: identity.name,
            auth_type: GoogleAuthType::GoogleOAuth,
            account_type: identity.account_type,
            cloud_project_id: None,
            connected: true,
            requires_reconnect: false,
            expires_at: None,
            granted_scopes: vec![],
        };
        let status = code_assist_status(Some(&account));
        assert_eq!(
            status.availability,
            GoogleCodeAssistAvailability::ProjectRequired
        );
        assert_eq!(status.user_tier, None);
        assert!(!status.inference_available);
    }

    #[test]
    fn workspace_project_is_validated_and_stays_in_the_native_session() {
        let storage = Arc::new(MemoryGoogleSessionStorage::default());
        save_test_session(
            &storage,
            "workspace-subject",
            Utc::now() + ChronoDuration::minutes(10),
            TEST_REFRESH_TOKEN,
        );
        let mut serialized = storage.get_google_session().unwrap().unwrap();
        let mut session: GoogleAuthSession = serde_json::from_str(&serialized).unwrap();
        serialized.zeroize();
        let individual_manager =
            test_oauth_manager(refresh_mock(MockRefreshBehavior::Success), storage.clone());
        let error = individual_manager
            .set_cloud_project_id("team-project-123", &CredentialStore)
            .unwrap_err();
        assert_eq!(error.code, "google_project_not_required");

        session.account.account_type = GoogleAccountType::Workspace;
        let mut serialized = serde_json::to_string(&session).unwrap();
        storage.save_google_session(&serialized).unwrap();
        serialized.zeroize();

        let manager =
            test_oauth_manager(refresh_mock(MockRefreshBehavior::Success), storage.clone());
        assert!(manager
            .set_cloud_project_id("bad", &CredentialStore)
            .is_err());
        let account = manager
            .set_cloud_project_id("team-project-123", &CredentialStore)
            .unwrap();
        assert_eq!(
            account.cloud_project_id.as_deref(),
            Some("team-project-123")
        );
        let status = manager.status(&CredentialStore, None).unwrap();
        assert_eq!(
            status.code_assist.availability,
            GoogleCodeAssistAvailability::EligibilityUnknown
        );
        assert!(!status.code_assist.requires_cloud_project);
        let mut status_json = serde_json::to_string(&status).unwrap();
        assert!(!status_json.contains("access_token"));
        assert!(!status_json.contains("refresh_token"));
        status_json.zeroize();
        let mut saved = storage.get_google_session().unwrap().unwrap();
        assert!(saved.contains("team-project-123"));
        saved.zeroize();
    }

    #[tokio::test]
    async fn google_oauth_identity_is_never_promoted_to_a_gemini_model_credential() {
        let root = tempfile::tempdir().unwrap();
        let database = Database::open(&root.path().join("google-identity-route.sqlite")).unwrap();
        save_test_provider_account(&database);
        let storage = Arc::new(MemoryGoogleSessionStorage::default());
        save_test_session(
            &storage,
            "google-subject-identity-only",
            Utc::now() + ChronoDuration::minutes(10),
            TEST_REFRESH_TOKEN,
        );
        let oauth = refresh_mock(MockRefreshBehavior::Success);
        let manager = test_oauth_manager(oauth.clone(), storage);

        let error = match manager
            .request_credential(&CredentialStore, &database)
            .await
        {
            Err(error) => error,
            Ok(_) => panic!("OAuth identity must not be accepted as Gemini inference auth"),
        };
        assert_eq!(error.code, "google_code_assist_unsupported");
        assert_eq!(oauth.refresh_calls.load(Ordering::SeqCst), 0);
        assert!(!error.message.contains(TEST_ACCESS_TOKEN));
        assert!(!error.message.contains(TEST_REFRESH_TOKEN));
    }

    #[test]
    fn public_google_account_type_contains_no_credential_fields() {
        let account = GoogleAccount {
            id: "subject".into(),
            email: Some("user@example.com".into()),
            display_name: None,
            auth_type: GoogleAuthType::GoogleOAuth,
            account_type: GoogleAccountType::Individual,
            cloud_project_id: None,
            connected: true,
            requires_reconnect: false,
            expires_at: None,
            granted_scopes: Vec::new(),
        };
        let json = serde_json::to_string(&account).unwrap();
        for secret in ["access_token", "refresh_token", "id_token"] {
            assert!(!json.contains(secret));
        }
    }
}
