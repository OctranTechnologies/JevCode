use super::{ApiKeyAuthAdapter, ProviderAuthAdapter};
use crate::{
    config::AppConfig,
    credentials::CredentialStore,
    domain::{
        GoogleAccount, GoogleAuthStatus, GoogleAuthType, Model, Provider, ProviderAccount,
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
// OIDC identity scopes support verified account metadata. The remaining scopes
// are the documented Gemini Developer API OAuth scopes, not Gemini Code Assist
// CLI scopes or credentials.
const GOOGLE_SCOPES: &str = "openid email profile https://www.googleapis.com/auth/cloud-platform https://www.googleapis.com/auth/generative-language.retriever";
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
    OAuth {
        access_token: Zeroizing<String>,
        cloud_project_id: String,
    },
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
        let response = self
            .client
            .post(GOOGLE_TOKEN_URL)
            .form(&[
                ("client_id", client_id),
                ("code", code),
                ("code_verifier", verifier),
                ("grant_type", "authorization_code"),
                ("redirect_uri", redirect_uri),
            ])
            .send()
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
}

pub struct GoogleAuthManager {
    provider: Provider,
    api_key: ApiKeyAuthAdapter,
    client_id: Option<String>,
    cloud_project_id: Option<String>,
    oauth: Arc<dyn GoogleOAuthTransport>,
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
        })
    }

    pub fn status(
        &self,
        credentials: &CredentialStore,
        stored: Option<ProviderAccount>,
    ) -> AppResult<GoogleAuthStatus> {
        let account = self
            .load_session(credentials)?
            .map(|session| session.account.clone());
        let api_key_connected = credentials.has("gemini")?;
        let selected_auth_type = match stored.as_ref().and_then(|value| value.auth_method.as_ref())
        {
            Some(ProviderAuthMethod::GoogleOAuth) => Some(GoogleAuthType::GoogleOAuth),
            Some(ProviderAuthMethod::ApiKey) => Some(GoogleAuthType::GeminiApiKey),
            _ if account.is_some() => Some(GoogleAuthType::GoogleOAuth),
            _ if api_key_connected => Some(GoogleAuthType::GeminiApiKey),
            _ => None,
        };
        let oauth_available = self.oauth_configured();
        Ok(GoogleAuthStatus {
            account,
            selected_auth_type,
            api_key_connected,
            oauth_available,
            unavailable_reason: (!oauth_available).then(|| {
                "Google sign-in requires a registered Google Desktop OAuth client ID and a Google Cloud project with the Generative Language API enabled.".into()
            }),
        })
    }

    pub(crate) fn create_authentication_session(
        &self,
        redirect_uri: &str,
    ) -> AppResult<PendingGoogleAuth> {
        if !self.oauth_configured() {
            return Err(AppError::new(
                "google_oauth_not_configured",
                "Google sign-in needs a registered Desktop OAuth client ID and Google Cloud project in the JevCode configuration.",
            ));
        }
        validate_redirect_uri(redirect_uri)?;
        let client_id = self.client_id.clone().expect("configured Google client ID");
        let cloud_project_id = self
            .cloud_project_id
            .clone()
            .expect("configured Google Cloud project");
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
        database.save_provider_account(&ProviderAccount {
            provider_id: self.provider.id.clone(),
            provider_name: self.provider.name.clone(),
            state: ProviderAuthState::Connected,
            auth_method: Some(ProviderAuthMethod::GoogleOAuth),
            account_label: account
                .email
                .clone()
                .or_else(|| Some("Google account".into())),
            connected_at: Some(Utc::now().to_rfc3339()),
            last_validated_at: Some(Utc::now().to_rfc3339()),
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
            let (token, _) = self.access_token(credentials, database, false).await?;
            let session = self
                .load_session(credentials)?
                .ok_or_else(google_reconnect_error)?;
            return Ok(GoogleRequestCredential::OAuth {
                access_token: Zeroizing::new(token),
                cloud_project_id: session.cloud_project_id.clone(),
            });
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
        match self.request_credential(credentials, database).await? {
            GoogleRequestCredential::ApiKey(secret) => {
                self.api_key.get_available_models(&secret).await
            }
            GoogleRequestCredential::OAuth {
                access_token,
                cloud_project_id,
            } => {
                let base = official_gemini_api_base(&self.provider)?;
                let client = Client::builder()
                    .timeout(Duration::from_secs(20))
                    .connect_timeout(Duration::from_secs(8))
                    .redirect(reqwest::redirect::Policy::none())
                    .build()
                    .map_err(|_| super::network_error())?;
                let response = client
                    .get(format!("{base}/models"))
                    .query(&[("pageSize", "1000")])
                    .bearer_auth(access_token.as_str())
                    .header("x-goog-user-project", cloud_project_id)
                    .send()
                    .await
                    .map_err(|error| {
                        if error.is_timeout() || error.is_connect() {
                            super::network_error()
                        } else {
                            super::provider_outage()
                        }
                    })?;
                if !response.status().is_success() {
                    let status = response.status();
                    let mut response = response;
                    let body = super::read_limited(&mut response, 8192).await;
                    return Err(super::provider_response_error(status, &body));
                }
                let mut response = response;
                let body = super::read_limited(&mut response, 2 * 1024 * 1024).await;
                let data: serde_json::Value = serde_json::from_slice(&body).map_err(|_| {
                    AppError::new(
                        "provider_format",
                        "Google returned an unreadable Gemini model catalog.",
                    )
                })?;
                let models = data
                    .get("models")
                    .and_then(serde_json::Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let mut normalized: Vec<_> = models
                    .into_iter()
                    .filter_map(|item| super::normalize_model(&self.provider, item))
                    .collect();
                normalized.sort_by(|left, right| {
                    left.display_name
                        .to_lowercase()
                        .cmp(&right.display_name.to_lowercase())
                });
                normalized.dedup_by(|left, right| left.id == right.id);
                Ok(normalized)
            }
        }
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
        credentials.delete_google_session()?;
        if let Some(mut account) = database.provider_account(&self.provider.id)? {
            if account.auth_method == Some(ProviderAuthMethod::GoogleOAuth) {
                if credentials.has("gemini")? {
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
            && self
                .cloud_project_id
                .as_deref()
                .is_some_and(valid_project_id)
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
                label: "Sign in with Google".into(),
                available: self.oauth_configured(),
                unavailable_reason: (!self.oauth_configured()).then(|| {
                    "Configure a registered Desktop OAuth client ID and Google Cloud project."
                        .into()
                }),
            },
        ]
    }

    async fn access_token(
        &self,
        credentials: &CredentialStore,
        database: &Database,
        force_refresh: bool,
    ) -> AppResult<(String, GoogleAccount)> {
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
        let expires_at = parse_timestamp(&session.expires_at).ok_or_else(google_reconnect_error)?;
        if !force_refresh && expires_at > Utc::now() + REFRESH_SKEW {
            return Ok((session.access_token.clone(), session.account.clone()));
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
                        return Err(google_scope_error());
                    }
                    session.granted_scopes = scopes.clone();
                    session.account.granted_scopes = scopes;
                }
                session.expires_at = new_expiry.to_rfc3339();
                session.account.expires_at = Some(session.expires_at.clone());
                session.account.requires_reconnect = false;
                session.account.connected = true;
                let token = session.access_token.clone();
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
                    session.account.connected = false;
                    session.account.requires_reconnect = true;
                    let account = session.account.clone();
                    self.save_session(credentials, &session)?;
                    if let Some(mut stored) = database.provider_account(&self.provider.id)? {
                        stored.state = ProviderAuthState::NeedsAttention;
                        stored.last_error_code = Some("expired_authentication".into());
                        database.save_provider_account(&stored)?;
                    }
                    let _ = account;
                    return Err(google_reconnect_error());
                }
                Err(error)
            }
        }
    }

    fn load_session(&self, credentials: &CredentialStore) -> AppResult<Option<GoogleAuthSession>> {
        let Some(mut serialized) = credentials.get_google_session()? else {
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
        let result = credentials.save_google_session(&serialized);
        serialized.zeroize();
        result
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
    })
}

struct VerifiedGoogleIdentity {
    subject: String,
    email: Option<String>,
    name: Option<String>,
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
    [
        "openid",
        "email",
        "profile",
        "https://www.googleapis.com/auth/cloud-platform",
        "https://www.googleapis.com/auth/generative-language.retriever",
    ]
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
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn official_gemini_api_base(provider: &Provider) -> AppResult<&str> {
    match provider.base_url.as_deref() {
        Some("https://generativelanguage.googleapis.com/v1beta")
            if provider.id == "gemini"
                && provider.protocol == crate::domain::ProviderProtocol::Gemini =>
        {
            Ok("https://generativelanguage.googleapis.com/v1beta")
        }
        _ => Err(AppError::new(
            "google_route_unsupported",
            "Google sign-in is restricted to the official Gemini API host.",
        )),
    }
}

fn validate_redirect_uri(value: &str) -> AppResult<()> {
    let url = Url::parse(value).map_err(|_| google_callback_error())?;
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || url.port().is_none()
        || !matches!(url.path(), "/auth/callback" | "/")
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
    AppError::new("google_scope_denied", "Google did not grant the Gemini API access required by JevCode. Review the requested permissions and try again.")
}

fn google_reconnect_error() -> AppError {
    AppError::new(
        "google_reconnect_required",
        "Your Google connection needs to be renewed. Reconnect Google in Settings.",
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
    fn oauth_is_unavailable_without_a_registered_desktop_client_and_cloud_project() {
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
    }

    #[test]
    fn requested_gemini_scopes_are_verified_as_an_all_or_nothing_set() {
        let scopes = GOOGLE_SCOPES
            .split_ascii_whitespace()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        assert!(has_required_scopes(&scopes));
        assert!(!has_required_scopes(&scopes[..scopes.len() - 1]));
    }

    #[test]
    fn oauth_model_discovery_rejects_non_google_hosts() {
        let mut provider = provider();
        provider.base_url = Some("https://attacker.example/v1beta".into());
        assert_eq!(
            official_gemini_api_base(&provider).unwrap_err().code,
            "google_route_unsupported"
        );
    }

    #[test]
    fn public_google_account_type_contains_no_credential_fields() {
        let account = GoogleAccount {
            id: "subject".into(),
            email: Some("user@example.com".into()),
            display_name: None,
            auth_type: GoogleAuthType::GoogleOAuth,
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
