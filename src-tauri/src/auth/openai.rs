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
use jsonwebtoken::{
    decode, decode_header,
    jwk::{JwkSet, KeyAlgorithm, KeyOperations, PublicKeyUse},
    Algorithm, DecodingKey, Validation,
};
use reqwest::{Client, StatusCode, Url};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use zeroize::{Zeroize, Zeroizing};

const OPENAI_AUTHORIZE_URL: &str = "https://auth.openai.com/api/accounts/authorize";
const OPENAI_ISSUER: &str = "https://auth.openai.com";
const OPENAI_DISCOVERY_URL: &str = "https://auth.openai.com/.well-known/openid-configuration";
const OPENAI_TOKEN_URL: &str = "https://auth.openai.com/api/accounts/oauth/token";
const OPENAI_JWKS_URL: &str = "https://auth.openai.com/.well-known/jwks.json";
const OPENAI_RESOURCE: &str = "https://api.openai.com/v1";
const OPENAI_SCOPES: &str =
    "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
const OPENAI_AGENT_NAME: &str = "JevCode";

/// Native-only request material for an in-progress sign-in attempt. It is
/// deliberately neither serializable nor debuggable and never enters Tauri IPC.
pub(crate) struct PendingOpenAiAuth {
    pub(crate) attempt_id: String,
    pub(crate) state: String,
    pub(crate) nonce: String,
    pub(crate) code_verifier: String,
    pub(crate) code_challenge: String,
    pub(crate) redirect_uri: String,
    pub(crate) authorization_url: Url,
    pub(crate) expires_at: DateTime<Utc>,
    ext_agent_host_id: String,
    registered_client_id: Option<String>,
    expected_account_id: Option<String>,
    id_token_hint: Option<String>,
}

impl Drop for PendingOpenAiAuth {
    fn drop(&mut self) {
        self.state.zeroize();
        self.nonce.zeroize();
        self.code_verifier.zeroize();
        self.code_challenge.zeroize();
        if let Some(id_token_hint) = &mut self.id_token_hint {
            id_token_hint.zeroize();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenAiCallbackOutcome {
    AuthorizationReceived,
    AccessDenied,
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

impl Drop for OpenAiSession {
    fn drop(&mut self) {
        self.access_token.zeroize();
        self.refresh_token.zeroize();
        self.id_token.zeroize();
    }
}

/// OAuth transport contract intentionally lives in the Rust auth layer. Tests
/// substitute this boundary so exchange and identity verification can run
/// without contacting OpenAI or placing credentials in the renderer.
#[async_trait]
pub(crate) trait OpenAiOAuthTransport: Send + Sync {
    async fn discovery(&self) -> AppResult<OpenAiOidcDiscovery>;
    async fn exchange_code(&self, request: OpenAiTokenRequest) -> AppResult<OpenAiTokenResponse>;
    async fn jwks(&self) -> AppResult<JwkSet>;
}

#[derive(Deserialize)]
pub(crate) struct OpenAiOidcDiscovery {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
    id_token_signing_alg_values_supported: Vec<String>,
}

pub(crate) struct OpenAiTokenRequest {
    code: String,
    code_verifier: String,
    redirect_uri: String,
    client_id: String,
}

impl Drop for OpenAiTokenRequest {
    fn drop(&mut self) {
        self.code.zeroize();
        self.code_verifier.zeroize();
    }
}

#[derive(Deserialize)]
pub(crate) struct OpenAiTokenResponse {
    access_token: String,
    refresh_token: String,
    id_token: String,
    token_type: String,
    expires_in: u64,
    scope: String,
}

impl Drop for OpenAiTokenResponse {
    fn drop(&mut self) {
        self.access_token.zeroize();
        self.refresh_token.zeroize();
        self.id_token.zeroize();
    }
}

#[async_trait]
impl OpenAiOAuthTransport for ReqwestOpenAiOAuthTransport {
    async fn discovery(&self) -> AppResult<OpenAiOidcDiscovery> {
        let response = self
            .client
            .get(OPENAI_DISCOVERY_URL)
            .send()
            .await
            .map_err(|_| auth_network_error())?;
        read_json_response(response, "OpenAI sign-in configuration is unavailable.").await
    }

    async fn exchange_code(&self, request: OpenAiTokenRequest) -> AppResult<OpenAiTokenResponse> {
        let response = self
            .client
            .post(OPENAI_TOKEN_URL)
            .form(&[
                ("grant_type", "authorization_code"),
                ("client_id", request.client_id.as_str()),
                ("code", request.code.as_str()),
                ("code_verifier", request.code_verifier.as_str()),
                ("redirect_uri", request.redirect_uri.as_str()),
                ("resource", OPENAI_RESOURCE),
            ])
            .send()
            .await
            .map_err(|_| auth_network_error())?;
        read_json_response(response, "OpenAI could not complete sign-in.").await
    }

    async fn jwks(&self) -> AppResult<JwkSet> {
        let response = self
            .client
            .get(OPENAI_JWKS_URL)
            .send()
            .await
            .map_err(|_| auth_network_error())?;
        read_json_response(response, "OpenAI identity verification is unavailable.").await
    }
}

struct ReqwestOpenAiOAuthTransport {
    client: Client,
}

impl ReqwestOpenAiOAuthTransport {
    fn new() -> AppResult<Self> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| AppError::new("auth_configuration", "OpenAI sign-in is unavailable."))?;
        Ok(Self { client })
    }
}

pub(crate) trait OpenAiSessionStorage: Send + Sync {
    fn save_openai_session(&self, account_id: &str, session: &str) -> AppResult<()>;
    fn get_openai_session(&self, account_id: &str) -> AppResult<Option<String>>;
    fn delete_openai_session(&self, account_id: &str) -> AppResult<()>;
}

impl OpenAiSessionStorage for CredentialStore {
    fn save_openai_session(&self, account_id: &str, session: &str) -> AppResult<()> {
        CredentialStore::save_openai_session(self, account_id, session)
    }

    fn get_openai_session(&self, account_id: &str) -> AppResult<Option<String>> {
        CredentialStore::get_openai_session(self, account_id)
    }

    fn delete_openai_session(&self, account_id: &str) -> AppResult<()> {
        CredentialStore::delete_openai_session(self, account_id)
    }
}

struct AuthorizationCode {
    code: String,
    client_id: String,
}

impl Drop for AuthorizationCode {
    fn drop(&mut self) {
        self.code.zeroize();
    }
}

fn parse_authorization_callback(
    pending: &PendingOpenAiAuth,
    request_target: &str,
    host_header: &str,
) -> AppResult<Option<AuthorizationCode>> {
    if Utc::now() >= pending.expires_at {
        return Err(invalid_callback());
    }
    let expected_redirect = Url::parse(&pending.redirect_uri).map_err(|_| invalid_callback())?;
    let expected_host = format!(
        "{}:{}",
        expected_redirect.host_str().ok_or_else(invalid_callback)?,
        expected_redirect.port().ok_or_else(invalid_callback)?
    );
    if !constant_time_equal(host_header.as_bytes(), expected_host.as_bytes()) {
        return Err(invalid_callback());
    }

    if !request_target.starts_with('/')
        || request_target.starts_with("//")
        || request_target.contains('#')
        || raw_callback_path(request_target) != "/auth/callback"
        || !request_target
            .bytes()
            .all(|byte| (0x21..=0x7e).contains(&byte))
    {
        return Err(invalid_callback());
    }
    let pairs = parse_callback_query(request_target)?;
    let values = |name: &str| {
        pairs
            .iter()
            .filter(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
            .collect::<Vec<_>>()
    };
    let states = values("state");
    if states.len() != 1 || !constant_time_equal(states[0].as_bytes(), pending.state.as_bytes()) {
        return Err(invalid_callback());
    }

    let errors = values("error");
    if errors.len() > 1 {
        return Err(invalid_callback());
    }
    if errors
        .first()
        .is_some_and(|error| *error == "access_denied")
    {
        if !values("code").is_empty() || !values("client_id").is_empty() {
            return Err(invalid_callback());
        }
        return Ok(None);
    }
    if !errors.is_empty() {
        return Err(invalid_callback());
    }

    let codes = values("code");
    let issued_client_ids = values("client_id");
    if codes.len() != 1 || !valid_callback_value(codes[0]) {
        return Err(invalid_callback());
    }
    let client_id = match pending.registered_client_id.as_deref() {
        Some(registered_client_id) => {
            if issued_client_ids.len() > 1
                || issued_client_ids
                    .first()
                    .is_some_and(|received| *received != registered_client_id)
            {
                return Err(invalid_callback());
            }
            registered_client_id
        }
        None => {
            if issued_client_ids.len() != 1 || !valid_callback_value(issued_client_ids[0]) {
                return Err(invalid_callback());
            }
            issued_client_ids[0]
        }
    };
    Ok(Some(AuthorizationCode {
        code: codes[0].to_owned(),
        client_id: client_id.to_owned(),
    }))
}

async fn read_json_response<T: DeserializeOwned>(
    mut response: reqwest::Response,
    rejected_message: &str,
) -> AppResult<T> {
    if !response.status().is_success() {
        return Err(auth_http_error(response.status()));
    }
    const MAX_AUTH_RESPONSE_BYTES: usize = 1024 * 1024;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_AUTH_RESPONSE_BYTES as u64)
    {
        return Err(AppError::new(
            "invalid_auth_response",
            "OpenAI returned an unexpectedly large sign-in response.",
        ));
    }
    let mut body = Zeroizing::new(Vec::new());
    loop {
        let chunk = match response.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(_) => return Err(auth_network_error()),
        };
        if body.len().saturating_add(chunk.len()) > MAX_AUTH_RESPONSE_BYTES {
            return Err(AppError::new(
                "invalid_auth_response",
                "OpenAI returned an unexpectedly large sign-in response.",
            ));
        }
        body.extend_from_slice(&chunk);
    }
    let parsed = serde_json::from_slice(&body);
    parsed.map_err(|_| AppError::new("invalid_auth_response", rejected_message))
}

fn auth_network_error() -> AppError {
    AppError::new(
        "network_error",
        "JevCode could not reach OpenAI. Check your connection and retry.",
    )
}

fn auth_http_error(status: StatusCode) -> AppError {
    if status == StatusCode::TOO_MANY_REQUESTS {
        AppError::new(
            "subscription_unavailable",
            "OpenAI could not authorize this account right now. Try again later.",
        )
    } else if status == StatusCode::UNAUTHORIZED || status == StatusCode::BAD_REQUEST {
        AppError::new(
            "invalid_auth_response",
            "OpenAI could not complete sign-in. Start a new sign-in attempt.",
        )
    } else if status == StatusCode::FORBIDDEN {
        AppError::new(
            "subscription_unavailable",
            "This OpenAI account could not authorize JevCode.",
        )
    } else if status.is_server_error() {
        AppError::new(
            "provider_outage",
            "OpenAI sign-in is temporarily unavailable. Try again later.",
        )
    } else {
        AppError::new(
            "invalid_auth_response",
            "OpenAI returned an unexpected sign-in response.",
        )
    }
}

fn validate_discovery(discovery: &OpenAiOidcDiscovery) -> AppResult<()> {
    if discovery.issuer != OPENAI_ISSUER
        || discovery.authorization_endpoint != OPENAI_AUTHORIZE_URL
        || discovery.token_endpoint != OPENAI_TOKEN_URL
        || discovery.jwks_uri != OPENAI_JWKS_URL
    {
        return Err(identity_verification_error());
    }
    Ok(())
}

#[derive(Deserialize)]
struct OpenAiIdentityClaims {
    iss: String,
    sub: String,
    nonce: String,
    iat: u64,
    aud: ClaimAudience,
    #[serde(default)]
    azp: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    email_verified: bool,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ClaimAudience {
    Single(String),
    Multiple(Vec<String>),
}

struct VerifiedOpenAiIdentity {
    subject: String,
    email: Option<String>,
    email_verified: bool,
    name: Option<String>,
}

fn validate_id_token(
    id_token: &str,
    expected_nonce: &str,
    client_id: &str,
    discovery: &OpenAiOidcDiscovery,
    jwks: &JwkSet,
) -> AppResult<VerifiedOpenAiIdentity> {
    let header = decode_header(id_token).map_err(|_| identity_verification_error())?;
    let algorithm = header.alg;
    if !is_supported_asymmetric_algorithm(algorithm)
        || !discovery
            .id_token_signing_alg_values_supported
            .iter()
            .any(|advertised| supported_algorithm(advertised) == Some(algorithm))
    {
        return Err(identity_verification_error());
    }
    let kid = header.kid.ok_or_else(identity_verification_error)?;
    let mut matches = jwks
        .keys
        .iter()
        .filter(|jwk| jwk.common.key_id.as_deref() == Some(kid.as_str()));
    let jwk = matches.next().ok_or_else(identity_verification_error)?;
    if matches.next().is_some() {
        return Err(identity_verification_error());
    }
    let key_use_valid = jwk
        .common
        .public_key_use
        .as_ref()
        .is_none_or(|key_use| key_use == &PublicKeyUse::Signature)
        && jwk
            .common
            .key_operations
            .as_ref()
            .is_none_or(|operations| operations.contains(&KeyOperations::Verify))
        && jwk
            .common
            .key_algorithm
            .is_none_or(|key_algorithm| key_algorithm_matches(key_algorithm, algorithm));
    if !key_use_valid {
        return Err(identity_verification_error());
    }
    let key = DecodingKey::from_jwk(jwk).map_err(|_| identity_verification_error())?;
    let mut validation = Validation::new(algorithm);
    validation.set_issuer(&[OPENAI_ISSUER]);
    validation.set_audience(&[client_id]);
    validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
    validation.leeway = 5;
    validation.validate_nbf = true;
    let claims = decode::<OpenAiIdentityClaims>(id_token, &key, &validation)
        .map_err(|_| identity_verification_error())?
        .claims;
    let now = Utc::now().timestamp().max(0) as u64;
    let audience_valid = match claims.aud {
        ClaimAudience::Single(audience) => audience == client_id,
        ClaimAudience::Multiple(audiences) => {
            audiences.len() == 1
                && audiences
                    .first()
                    .is_some_and(|audience| audience == client_id)
                || (audiences.len() > 1
                    && audiences.iter().any(|audience| audience == client_id)
                    && claims.azp.as_deref() == Some(client_id))
        }
    };
    let authorized_party_valid = claims
        .azp
        .as_deref()
        .is_none_or(|authorized_party| authorized_party == client_id);
    if claims.iss != OPENAI_ISSUER
        || !audience_valid
        || !authorized_party_valid
        || claims.sub.trim().is_empty()
        || claims.sub.len() > 1024
        || claims.nonce.len() > 1024
        || !constant_time_equal(claims.nonce.as_bytes(), expected_nonce.as_bytes())
        || claims.iat > now.saturating_add(5)
    {
        return Err(identity_verification_error());
    }
    let email = claims
        .email
        .filter(|value| !value.trim().is_empty() && value.len() <= 320);
    let name = claims
        .name
        .filter(|value| !value.trim().is_empty() && value.len() <= 512);
    Ok(VerifiedOpenAiIdentity {
        subject: claims.sub,
        email,
        email_verified: claims.email_verified,
        name,
    })
}

fn key_algorithm_matches(key_algorithm: KeyAlgorithm, algorithm: Algorithm) -> bool {
    matches!(
        (key_algorithm, algorithm),
        (KeyAlgorithm::RS256, Algorithm::RS256)
            | (KeyAlgorithm::RS384, Algorithm::RS384)
            | (KeyAlgorithm::RS512, Algorithm::RS512)
            | (KeyAlgorithm::ES256, Algorithm::ES256)
            | (KeyAlgorithm::ES384, Algorithm::ES384)
            | (KeyAlgorithm::PS256, Algorithm::PS256)
            | (KeyAlgorithm::PS384, Algorithm::PS384)
            | (KeyAlgorithm::PS512, Algorithm::PS512)
    )
}

fn supported_algorithm(value: &str) -> Option<Algorithm> {
    match value {
        "RS256" => Some(Algorithm::RS256),
        "RS384" => Some(Algorithm::RS384),
        "RS512" => Some(Algorithm::RS512),
        "ES256" => Some(Algorithm::ES256),
        "ES384" => Some(Algorithm::ES384),
        "PS256" => Some(Algorithm::PS256),
        "PS384" => Some(Algorithm::PS384),
        "PS512" => Some(Algorithm::PS512),
        _ => None,
    }
}

fn is_supported_asymmetric_algorithm(algorithm: Algorithm) -> bool {
    matches!(
        algorithm,
        Algorithm::RS256
            | Algorithm::RS384
            | Algorithm::RS512
            | Algorithm::ES256
            | Algorithm::ES384
            | Algorithm::PS256
            | Algorithm::PS384
            | Algorithm::PS512
    )
}

fn identity_verification_error() -> AppError {
    AppError::new(
        "invalid_identity_token",
        "OpenAI identity verification failed. Start a new sign-in attempt.",
    )
}

fn valid_scope(scope: &str) -> bool {
    !scope.is_empty()
        && scope.len() <= 256
        && scope.bytes().all(|byte| {
            byte == 0x21 || (0x23..=0x5b).contains(&byte) || (0x5d..=0x7e).contains(&byte)
        })
}

fn validate_token_response(response: &OpenAiTokenResponse) -> AppResult<()> {
    if response.token_type != "Bearer"
        || response.access_token.is_empty()
        || response.access_token.len() > 65_536
        || response.refresh_token.is_empty()
        || response.refresh_token.len() > 65_536
        || response.id_token.is_empty()
        || response.id_token.len() > 65_536
        || response.expires_in == 0
        || response.expires_in > 24 * 60 * 60
    {
        return Err(AppError::new(
            "invalid_auth_response",
            "OpenAI returned an incomplete sign-in response. Start sign-in again.",
        ));
    }
    Ok(())
}

fn openai_account_id(client_id: &str, subject: &str) -> String {
    let profile_key = format!("{OPENAI_ISSUER}\0{client_id}\0{subject}");
    uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, profile_key.as_bytes()).to_string()
}

pub struct OpenAIAuthManager {
    api_key: ApiKeyAuthAdapter,
    oauth: Arc<dyn OpenAiOAuthTransport>,
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
            oauth: Arc::new(ReqwestOpenAiOAuthTransport::new()?),
        })
    }

    #[cfg(test)]
    fn with_oauth_transport(
        provider: Provider,
        oauth: Arc<dyn OpenAiOAuthTransport>,
    ) -> AppResult<Self> {
        if provider.id != "openai" {
            return Err(AppError::new(
                "provider_configuration",
                "The OpenAI auth manager can only be used for the OpenAI provider.",
            ));
        }
        Ok(Self {
            api_key: ApiKeyAuthAdapter::new(provider)?,
            oauth,
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
                    "Connect an OpenAI API key to run agent requests. ChatGPT sign-in currently connects and stores the account securely.",
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
        self.create_authentication_session_for(redirect_uri, ext_agent_host_id, None, None, None)
    }

    fn create_authentication_session_for(
        &self,
        redirect_uri: &str,
        ext_agent_host_id: &str,
        registered_client_id: Option<&str>,
        expected_account_id: Option<&str>,
        id_token_hint: Option<&str>,
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
        let client_id = registered_client_id.unwrap_or("dynamic_agent_client");
        let mut pairs = authorization_url.query_pairs_mut();
        pairs
            .append_pair("client_id", client_id)
            .append_pair("agent_name_hint", OPENAI_AGENT_NAME)
            .append_pair("ext_agent_host_id", ext_agent_host_id)
            .append_pair("response_type", "code")
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("scope", OPENAI_SCOPES)
            .append_pair("resource", OPENAI_RESOURCE)
            .append_pair("state", &state)
            .append_pair("nonce", &nonce)
            .append_pair("code_challenge_method", "S256")
            .append_pair("code_challenge", &code_challenge);
        if let Some(id_token_hint) = id_token_hint {
            pairs.append_pair("id_token_hint", id_token_hint);
        }
        drop(pairs);

        Ok(PendingOpenAiAuth {
            attempt_id,
            state,
            nonce,
            code_verifier,
            code_challenge,
            redirect_uri: redirect_uri.to_owned(),
            authorization_url,
            expires_at: Utc::now() + ChronoDuration::minutes(10),
            ext_agent_host_id: ext_agent_host_id.to_owned(),
            registered_client_id: registered_client_id.map(str::to_owned),
            expected_account_id: expected_account_id.map(str::to_owned),
            id_token_hint: id_token_hint.map(str::to_owned),
        })
    }

    pub(crate) fn create_reauthentication_session(
        &self,
        redirect_uri: &str,
        ext_agent_host_id: &str,
        account_id: &str,
        credentials: &dyn OpenAiSessionStorage,
        database: &Database,
    ) -> AppResult<PendingOpenAiAuth> {
        validate_loopback_redirect_uri(redirect_uri)?;
        validate_host_id(ext_agent_host_id)?;
        let account = database
            .openai_accounts()?
            .into_iter()
            .find(|account| account.id == account_id)
            .ok_or_else(|| {
                AppError::new("account_not_found", "This OpenAI account is unavailable.")
            })?;
        let mut serialized = credentials.get_openai_session(account_id)?.ok_or_else(|| {
            AppError::new("credentials_required", "Reconnect this OpenAI account.")
        })?;
        let session_result = serde_json::from_str::<OpenAiSession>(&serialized);
        serialized.zeroize();
        let session = session_result.map_err(|_| {
            AppError::new(
                "invalid_auth_session",
                "This OpenAI session is invalid. Sign in again.",
            )
        })?;
        if session.account_id != account_id
            || account.client_id.as_deref() != Some(session.issued_client_id.as_str())
            || session.ext_agent_host_id != ext_agent_host_id
        {
            return Err(AppError::new(
                "invalid_auth_session",
                "This OpenAI session does not match its saved account. Sign in again.",
            ));
        }
        self.create_authentication_session_for(
            redirect_uri,
            ext_agent_host_id,
            Some(&session.issued_client_id),
            Some(account_id),
            Some(&session.id_token),
        )
    }

    /// Validate the loopback response, exchange its authorization code, verify
    /// the signed ID token, and persist secrets/profile at their native stores.
    /// Codes, tokens, and provider responses never leave this auth manager.
    pub(crate) async fn receive_authorization_callback(
        &self,
        pending: &PendingOpenAiAuth,
        request_target: &str,
        host_header: &str,
        credentials: &dyn OpenAiSessionStorage,
        database: &Database,
    ) -> AppResult<OpenAiCallbackOutcome> {
        let authorization = parse_authorization_callback(pending, request_target, host_header)?;
        let Some(authorization) = authorization else {
            return Ok(OpenAiCallbackOutcome::AccessDenied);
        };

        self.exchange_and_store(pending, authorization, credentials, database)
            .await?;
        Ok(OpenAiCallbackOutcome::AuthorizationReceived)
    }

    async fn exchange_and_store(
        &self,
        pending: &PendingOpenAiAuth,
        authorization: AuthorizationCode,
        credentials: &dyn OpenAiSessionStorage,
        database: &Database,
    ) -> AppResult<OpenAiAccount> {
        let discovery = self.oauth.discovery().await?;
        validate_discovery(&discovery)?;
        if !discovery
            .id_token_signing_alg_values_supported
            .iter()
            .any(|algorithm| supported_algorithm(algorithm).is_some())
        {
            return Err(identity_verification_error());
        }

        let token_response = self
            .oauth
            .exchange_code(OpenAiTokenRequest {
                code: authorization.code.clone(),
                code_verifier: pending.code_verifier.clone(),
                redirect_uri: pending.redirect_uri.clone(),
                client_id: authorization.client_id.clone(),
            })
            .await?;
        validate_token_response(&token_response)?;
        let jwks = self.oauth.jwks().await?;
        let identity = validate_id_token(
            &token_response.id_token,
            &pending.nonce,
            &authorization.client_id,
            &discovery,
            &jwks,
        )?;

        let account_id = openai_account_id(&authorization.client_id, &identity.subject);
        let existing_account =
            if let Some(expected_account_id) = pending.expected_account_id.as_deref() {
                if account_id != expected_account_id {
                    return Err(AppError::new(
                        "account_mismatch",
                        "OpenAI returned a different account. The saved account was not changed.",
                    ));
                }
                let account = database
                    .openai_accounts()?
                    .into_iter()
                    .find(|account| account.id == expected_account_id)
                    .ok_or_else(|| {
                        AppError::new(
                            "account_not_found",
                            "This OpenAI account is no longer saved.",
                        )
                    })?;
                if account.client_id.as_deref() != Some(authorization.client_id.as_str()) {
                    return Err(identity_verification_error());
                }
                Some(account)
            } else {
                None
            };
        let now = Utc::now();
        let expires_at = now
            .checked_add_signed(ChronoDuration::seconds(token_response.expires_in as i64))
            .ok_or_else(identity_verification_error)?;
        let scopes = token_response
            .scope
            .split_ascii_whitespace()
            .filter(|scope| !scope.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if scopes.is_empty() || scopes.iter().any(|scope| !valid_scope(scope)) {
            return Err(AppError::new(
                "invalid_auth_response",
                "OpenAI returned invalid sign-in permissions. Start sign-in again.",
            ));
        }

        let account = OpenAiAccount {
            id: account_id.clone(),
            connected: true,
            display_name: identity.name,
            email: identity.email_verified.then_some(identity.email).flatten(),
            organization: None,
            client_id: Some(authorization.client_id.clone()),
            granted_scopes: scopes.clone(),
            connected_at: existing_account
                .as_ref()
                .map(|account| account.connected_at.clone())
                .unwrap_or_else(|| now.to_rfc3339()),
            expires_at: Some(expires_at.to_rfc3339()),
        };
        let session = OpenAiSession {
            account_id: account_id.clone(),
            issued_client_id: authorization.client_id.clone(),
            ext_agent_host_id: pending.ext_agent_host_id.clone(),
            access_token: token_response.access_token.clone(),
            refresh_token: token_response.refresh_token.clone(),
            id_token: token_response.id_token.clone(),
            token_expires_at: expires_at.to_rfc3339(),
            granted_scopes: scopes,
        };

        self.store_chatgpt_session(credentials, &session)?;
        if let Err(error) = database.save_openai_account(&account) {
            let _ = credentials.delete_openai_session(&account_id);
            return Err(error);
        }
        Ok(account)
    }

    pub fn logout(
        &self,
        credentials: &dyn OpenAiSessionStorage,
        database: &Database,
        account_id: &str,
    ) -> AppResult<()> {
        let account_id = uuid::Uuid::parse_str(account_id)
            .map_err(|_| AppError::new("invalid_input", "The OpenAI account is invalid."))?
            .to_string();
        if !database
            .openai_accounts()?
            .iter()
            .any(|account| account.id == account_id)
        {
            return Err(AppError::new(
                "account_not_found",
                "This OpenAI account is no longer connected.",
            ));
        }
        credentials.delete_openai_session(&account_id)?;
        database.delete_openai_account(&account_id)?;
        Ok(())
    }

    pub fn status(
        &self,
        credentials: &CredentialStore,
        database: &Database,
        stored: Option<ProviderAccount>,
    ) -> AppResult<OpenAiAuthStatus> {
        let api_key = self.api_key.get_auth_status(credentials, stored)?;
        let mut connected_accounts = Vec::new();
        for mut account in database.openai_accounts()? {
            let session = credentials
                .get_openai_session(&account.id)?
                .and_then(|mut json| {
                    let session = serde_json::from_str::<OpenAiSession>(&json).ok();
                    json.zeroize();
                    session
                });
            if let Some(session) = session.filter(|session| session.account_id == account.id) {
                account.connected = DateTime::parse_from_rfc3339(&session.token_expires_at)
                    .map(|expires_at| expires_at.with_timezone(&Utc) > Utc::now())
                    .unwrap_or(false);
                connected_accounts.push(account);
            }
        }
        Ok(OpenAiAuthStatus {
            api_key,
            chatgpt: OpenAiChatGptAuthStatus {
                available: true,
                connected_accounts,
                unavailable_reason: None,
            },
        })
    }

    fn store_chatgpt_session(
        &self,
        credentials: &dyn OpenAiSessionStorage,
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
        let mut serialized = serde_json::to_string(session)?;
        let result = credentials.save_openai_session(&session.account_id, &serialized);
        serialized.zeroize();
        result
    }
}

fn invalid_callback() -> AppError {
    AppError::new(
        "invalid_auth_callback",
        "The OpenAI sign-in response could not be verified. Start sign-in again.",
    )
}

fn raw_callback_path(target: &str) -> &str {
    let (path, _) = target.split_once('?').unwrap_or((target, ""));
    path
}

fn parse_callback_query(target: &str) -> AppResult<Zeroizing<Vec<(String, String)>>> {
    let Some((_, query)) = target.split_once('?') else {
        return Err(invalid_callback());
    };
    let mut pairs = Zeroizing::new(Vec::new());
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let mut key = Zeroizing::new(decode_form_component(key)?);
        let mut value = Zeroizing::new(decode_form_component(value)?);
        pairs.push((std::mem::take(&mut *key), std::mem::take(&mut *value)));
    }
    Ok(pairs)
}

fn decode_form_component(value: &str) -> AppResult<String> {
    let bytes = value.as_bytes();
    let mut decoded = Zeroizing::new(Vec::with_capacity(bytes.len()));
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            b'%' => {
                let Some(hex) = bytes.get(index + 1..index + 3) else {
                    return Err(invalid_callback());
                };
                if hex.len() != 2 {
                    return Err(invalid_callback());
                }
                let high = (hex_digit(hex[0]).ok_or_else(invalid_callback)?) << 4;
                let low = hex_digit(hex[1]).ok_or_else(invalid_callback)?;
                decoded.push(high | low);
                index += 3;
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    std::str::from_utf8(&decoded)
        .map(str::to_owned)
        .map_err(|_| invalid_callback())
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn valid_callback_value(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= 16_384
        && !value
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
}

fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (a, b)| difference | (a ^ b))
        == 0
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
    use jsonwebtoken::{encode, EncodingKey, Header};
    use rand::thread_rng;
    use rsa::{pkcs1::EncodeRsaPrivateKey, traits::PublicKeyParts, RsaPrivateKey, RsaPublicKey};
    use std::collections::HashMap;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    };

    const TEST_CLIENT_ID: &str = "oaiapp_test_client";
    const TEST_ACCESS_TOKEN: &str = "test-only-access-token";
    const TEST_REFRESH_TOKEN: &str = "test-only-refresh-token";
    const TEST_ID_TOKEN: &str = "test-only-id-token";

    fn test_provider() -> Provider {
        Provider {
            id: "openai".into(),
            name: "OpenAI".into(),
            protocol: crate::domain::ProviderProtocol::OpenAiResponses,
            base_url: Some("https://api.openai.com/v1".into()),
            models: vec![],
            connected: false,
        }
    }

    #[derive(Default)]
    struct MemorySessionStorage {
        sessions: Mutex<HashMap<String, String>>,
    }

    impl OpenAiSessionStorage for MemorySessionStorage {
        fn save_openai_session(&self, account_id: &str, session: &str) -> AppResult<()> {
            self.sessions
                .lock()
                .map_err(AppError::internal)?
                .insert(account_id.to_owned(), session.to_owned());
            Ok(())
        }

        fn get_openai_session(&self, account_id: &str) -> AppResult<Option<String>> {
            Ok(self
                .sessions
                .lock()
                .map_err(AppError::internal)?
                .get(account_id)
                .cloned())
        }

        fn delete_openai_session(&self, account_id: &str) -> AppResult<()> {
            if let Some(mut session) = self
                .sessions
                .lock()
                .map_err(AppError::internal)?
                .remove(account_id)
            {
                session.zeroize();
            }
            Ok(())
        }
    }

    struct MockOAuthTransport {
        jwks: Mutex<Option<JwkSet>>,
        id_token: String,
        expected_verifier: Mutex<Option<String>>,
        exchanged: AtomicBool,
    }

    #[async_trait]
    impl OpenAiOAuthTransport for MockOAuthTransport {
        async fn discovery(&self) -> AppResult<OpenAiOidcDiscovery> {
            Ok(test_discovery())
        }

        async fn exchange_code(
            &self,
            request: OpenAiTokenRequest,
        ) -> AppResult<OpenAiTokenResponse> {
            assert_eq!(request.code, "authorization-code-for-test");
            assert_eq!(request.client_id, TEST_CLIENT_ID);
            assert_eq!(request.redirect_uri, "http://127.0.0.1:43210/auth/callback");
            assert_eq!(
                request.code_verifier,
                self.expected_verifier.lock().unwrap().as_deref().unwrap()
            );
            self.exchanged.store(true, Ordering::SeqCst);
            Ok(OpenAiTokenResponse {
                access_token: TEST_ACCESS_TOKEN.into(),
                refresh_token: TEST_REFRESH_TOKEN.into(),
                id_token: self.id_token.clone(),
                token_type: "Bearer".into(),
                expires_in: 3600,
                scope:
                    "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct"
                        .into(),
            })
        }

        async fn jwks(&self) -> AppResult<JwkSet> {
            self.jwks
                .lock()
                .map_err(AppError::internal)?
                .take()
                .ok_or_else(identity_verification_error)
        }
    }

    fn test_discovery() -> OpenAiOidcDiscovery {
        OpenAiOidcDiscovery {
            issuer: OPENAI_ISSUER.into(),
            authorization_endpoint: OPENAI_AUTHORIZE_URL.into(),
            token_endpoint: OPENAI_TOKEN_URL.into(),
            jwks_uri: OPENAI_JWKS_URL.into(),
            id_token_signing_alg_values_supported: vec!["RS256".into()],
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
                "kid": "jevcode-test-key",
                "use": "sig",
                "alg": "RS256",
                "n": URL_SAFE_NO_PAD.encode(public.n().to_bytes_be()),
                "e": URL_SAFE_NO_PAD.encode(public.e().to_bytes_be())
            }]
        }))
        .unwrap();
        (encoding, jwks)
    }

    fn sign_test_id_token(
        key: &EncodingKey,
        nonce: &str,
        issuer: &str,
        audience: serde_json::Value,
        expires_at: u64,
    ) -> String {
        sign_test_id_token_with_azp(key, nonce, issuer, audience, expires_at, TEST_CLIENT_ID)
    }

    fn sign_test_id_token_with_azp(
        key: &EncodingKey,
        nonce: &str,
        issuer: &str,
        audience: serde_json::Value,
        expires_at: u64,
        authorized_party: &str,
    ) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("jevcode-test-key".into());
        encode(
            &header,
            &serde_json::json!({
                "iss": issuer,
                "sub": "profile-subject-42",
                "aud": audience,
                "azp": authorized_party,
                "nonce": nonce,
                "iat": Utc::now().timestamp() as u64,
                "exp": expires_at,
                "nbf": Utc::now().timestamp() as u64,
                "email": "verified@example.test",
                "email_verified": true,
                "name": "JevCode Test Account"
            }),
            key,
        )
        .unwrap()
    }

    fn manager() -> OpenAIAuthManager {
        OpenAIAuthManager::new(Provider {
            id: "openai".into(),
            name: "OpenAI".into(),
            protocol: crate::domain::ProviderProtocol::OpenAiResponses,
            base_url: Some("https://api.openai.com/v1".into()),
            models: vec![],
            connected: false,
        })
        .unwrap()
    }

    #[test]
    fn openai_status_and_account_contracts_contain_only_safe_metadata() {
        let account = OpenAiAccount {
            id: "account-id".into(),
            connected: true,
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
                available: true,
                connected_accounts: vec![account],
                unavailable_reason: None,
            },
        };
        let serialized = serde_json::to_string(&status).unwrap();
        for sensitive_field in [
            "accessToken",
            "refreshToken",
            "idToken",
            "codeVerifier",
            "authorizationUrl",
            "nonce",
        ] {
            assert!(!serialized.contains(sensitive_field));
        }
        assert!(serialized.contains("issued-client-id"));
    }

    #[test]
    fn reauthentication_reuses_the_saved_client_and_accepts_an_omitted_callback_client_id() {
        let manager = manager();
        let root = tempfile::tempdir().unwrap();
        let database = Database::open(&root.path().join("reauth-test.sqlite")).unwrap();
        let storage = MemorySessionStorage::default();
        let host_id = format!("urn:uuid:{}", uuid::Uuid::new_v4());
        let account_id = openai_account_id(TEST_CLIENT_ID, "profile-subject-42");
        let account = OpenAiAccount {
            id: account_id.clone(),
            connected: false,
            display_name: Some("Saved account".into()),
            email: None,
            organization: None,
            client_id: Some(TEST_CLIENT_ID.into()),
            granted_scopes: vec!["openid".into()],
            connected_at: Utc::now().to_rfc3339(),
            expires_at: None,
        };
        database.save_openai_account(&account).unwrap();
        let session = OpenAiSession {
            account_id: account_id.clone(),
            issued_client_id: TEST_CLIENT_ID.into(),
            ext_agent_host_id: host_id.clone(),
            access_token: TEST_ACCESS_TOKEN.into(),
            refresh_token: TEST_REFRESH_TOKEN.into(),
            id_token: TEST_ID_TOKEN.into(),
            token_expires_at: Utc::now().to_rfc3339(),
            granted_scopes: vec!["openid".into()],
        };
        let mut serialized = serde_json::to_string(&session).unwrap();
        storage
            .save_openai_session(&account_id, &serialized)
            .unwrap();
        serialized.zeroize();

        let pending = manager
            .create_reauthentication_session(
                "http://127.0.0.1:43210/auth/callback",
                &host_id,
                &account_id,
                &storage,
                &database,
            )
            .unwrap();
        let pairs = pending
            .authorization_url
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(
            pairs.get("client_id").map(|value| value.as_ref()),
            Some(TEST_CLIENT_ID)
        );
        assert_eq!(
            pairs.get("id_token_hint").map(|value| value.as_ref()),
            Some(TEST_ID_TOKEN)
        );

        let callback_without_client =
            format!("/auth/callback?code=reauth-code&state={}", pending.state);
        let authorization =
            parse_authorization_callback(&pending, &callback_without_client, "127.0.0.1:43210")
                .unwrap()
                .unwrap();
        assert_eq!(authorization.client_id, TEST_CLIENT_ID);
        let callback_with_wrong_client = format!(
            "/auth/callback?code=reauth-code&state={}&client_id=another-client",
            pending.state
        );
        assert_eq!(
            parse_authorization_callback(&pending, &callback_with_wrong_client, "127.0.0.1:43210")
                .err()
                .unwrap()
                .code,
            "invalid_auth_callback"
        );
    }

    #[tokio::test]
    async fn oauth_exchange_validates_identity_stores_securely_and_logout_clears_it() {
        let (encoding_key, jwks) = test_signing_key();
        let pending = manager()
            .create_authentication_session(
                "http://127.0.0.1:43210/auth/callback",
                &format!("urn:uuid:{}", uuid::Uuid::new_v4()),
            )
            .unwrap();
        let id_token = sign_test_id_token(
            &encoding_key,
            &pending.nonce,
            OPENAI_ISSUER,
            serde_json::json!(TEST_CLIENT_ID),
            Utc::now().timestamp() as u64 + 3600,
        );
        let transport = Arc::new(MockOAuthTransport {
            jwks: Mutex::new(Some(jwks)),
            id_token: id_token.clone(),
            expected_verifier: Mutex::new(None),
            exchanged: AtomicBool::new(false),
        });
        let manager =
            OpenAIAuthManager::with_oauth_transport(test_provider(), transport.clone()).unwrap();
        *transport.expected_verifier.lock().unwrap() = Some(pending.code_verifier.clone());
        let callback = format!(
            "/auth/callback?code=authorization-code-for-test&state={}&client_id={TEST_CLIENT_ID}",
            pending.state
        );
        let storage = MemorySessionStorage::default();
        let root = tempfile::tempdir().unwrap();
        let database = Database::open(&root.path().join("oauth-test.sqlite")).unwrap();

        let outcome = manager
            .receive_authorization_callback(
                &pending,
                &callback,
                "127.0.0.1:43210",
                &storage,
                &database,
            )
            .await
            .unwrap();
        assert_eq!(outcome, OpenAiCallbackOutcome::AuthorizationReceived);
        assert!(transport.exchanged.load(Ordering::SeqCst));
        let accounts = database.openai_accounts().unwrap();
        assert_eq!(accounts.len(), 1);
        let account = &accounts[0];
        assert!(account.connected);
        assert_eq!(account.email.as_deref(), Some("verified@example.test"));
        assert_eq!(
            account.display_name.as_deref(),
            Some("JevCode Test Account")
        );
        assert!(account
            .granted_scopes
            .contains(&"chatgpt.tokens.use.direct".into()));

        let profile_json = serde_json::to_string(account).unwrap();
        let saved_session = storage.get_openai_session(&account.id).unwrap().unwrap();
        assert!(saved_session.contains(TEST_ACCESS_TOKEN));
        assert!(saved_session.contains(TEST_REFRESH_TOKEN));
        assert!(saved_session.contains(&id_token));
        let safe_status = OpenAiAuthStatus {
            api_key: ProviderAccount {
                provider_id: "openai".into(),
                provider_name: "OpenAI".into(),
                state: ProviderAuthState::NotConnected,
                auth_method: None,
                account_label: None,
                connected_at: None,
                last_validated_at: None,
                last_error_code: None,
                available_methods: Vec::new(),
            },
            chatgpt: OpenAiChatGptAuthStatus {
                available: true,
                connected_accounts: accounts.clone(),
                unavailable_reason: None,
            },
        };
        let safe_status_json = serde_json::to_string(&safe_status).unwrap();
        for secret in [TEST_ACCESS_TOKEN, TEST_REFRESH_TOKEN, id_token.as_str()] {
            assert!(!profile_json.contains(secret));
            assert!(!serde_json::to_string(&accounts).unwrap().contains(secret));
            assert!(!safe_status_json.contains(secret));
        }

        manager.logout(&storage, &database, &account.id).unwrap();
        assert!(database.openai_accounts().unwrap().is_empty());
        assert!(storage.get_openai_session(&account.id).unwrap().is_none());
    }

    #[test]
    fn id_token_rejects_bad_signature_issuer_audience_nonce_and_expiry() {
        let (encoding_key, jwks) = test_signing_key();
        let discovery = test_discovery();
        let now = Utc::now().timestamp() as u64;
        let valid = sign_test_id_token(
            &encoding_key,
            "expected-nonce",
            OPENAI_ISSUER,
            serde_json::json!(TEST_CLIENT_ID),
            now + 3600,
        );
        assert!(
            validate_id_token(&valid, "expected-nonce", TEST_CLIENT_ID, &discovery, &jwks).is_ok()
        );

        let invalid_tokens = [
            sign_test_id_token(
                &encoding_key,
                "expected-nonce",
                "https://attacker.example",
                serde_json::json!(TEST_CLIENT_ID),
                now + 3600,
            ),
            sign_test_id_token(
                &encoding_key,
                "expected-nonce",
                OPENAI_ISSUER,
                serde_json::json!("another-client"),
                now + 3600,
            ),
            sign_test_id_token(
                &encoding_key,
                "wrong-nonce",
                OPENAI_ISSUER,
                serde_json::json!(TEST_CLIENT_ID),
                now + 3600,
            ),
            sign_test_id_token(
                &encoding_key,
                "expected-nonce",
                OPENAI_ISSUER,
                serde_json::json!(TEST_CLIENT_ID),
                now.saturating_sub(10),
            ),
            sign_test_id_token_with_azp(
                &encoding_key,
                "expected-nonce",
                OPENAI_ISSUER,
                serde_json::json!(TEST_CLIENT_ID),
                now + 3600,
                "another-client",
            ),
        ];
        for token in invalid_tokens {
            assert_eq!(
                validate_id_token(&token, "expected-nonce", TEST_CLIENT_ID, &discovery, &jwks)
                    .err()
                    .unwrap()
                    .code,
                "invalid_identity_token"
            );
        }

        let parts = valid.split('.').collect::<Vec<_>>();
        let first_signature_byte = if parts[2].starts_with('A') { 'B' } else { 'A' };
        let forged_signature = format!(
            "{}.{}.{}{}",
            parts[0],
            parts[1],
            first_signature_byte,
            &parts[2][1..]
        );
        assert!(validate_id_token(
            &forged_signature,
            "expected-nonce",
            TEST_CLIENT_ID,
            &discovery,
            &jwks
        )
        .is_err());
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

    #[test]
    fn callback_accepts_valid_state_code_and_issued_client_id_without_returning_secrets() {
        let manager = manager();
        let pending = manager
            .create_authentication_session(
                "http://127.0.0.1:43210/auth/callback",
                &format!("urn:uuid:{}", uuid::Uuid::new_v4()),
            )
            .unwrap();
        let target = format!(
            "/auth/callback?code=private-code&state={}&client_id=issued-client",
            pending.state
        );

        let authorization = parse_authorization_callback(&pending, &target, "127.0.0.1:43210")
            .unwrap()
            .unwrap();
        assert_eq!(authorization.code, "private-code");
        assert_eq!(authorization.client_id, "issued-client");
    }

    #[test]
    fn callback_rejects_wrong_state_and_missing_code() {
        let manager = manager();
        let pending = manager
            .create_authentication_session(
                "http://127.0.0.1:43210/auth/callback",
                &format!("urn:uuid:{}", uuid::Uuid::new_v4()),
            )
            .unwrap();

        for target in [
            "/auth/callback?code=private-code&state=wrong&client_id=issued-client".to_owned(),
            format!(
                "/auth/callback?state={}&client_id=issued-client",
                pending.state
            ),
        ] {
            assert_eq!(
                parse_authorization_callback(&pending, &target, "127.0.0.1:43210")
                    .err()
                    .unwrap()
                    .code,
                "invalid_auth_callback"
            );
        }
    }

    #[test]
    fn callback_rejects_malformed_parameters_wrong_origin_and_missing_client_id() {
        let manager = manager();
        let pending = manager
            .create_authentication_session(
                "http://127.0.0.1:43210/auth/callback",
                &format!("urn:uuid:{}", uuid::Uuid::new_v4()),
            )
            .unwrap();
        let valid_query = format!(
            "code=private-code&state={}&client_id=issued-client",
            pending.state
        );

        for (target, host) in [
            ("/auth/callback?state=%Q0".to_owned(), "127.0.0.1:43210"),
            (
                format!("/auth/callback?code=private-code&state={}", pending.state),
                "127.0.0.1:43210",
            ),
            (format!("/auth/callback?{valid_query}"), "127.0.0.1:43211"),
        ] {
            assert_eq!(
                parse_authorization_callback(&pending, &target, host)
                    .err()
                    .unwrap()
                    .code,
                "invalid_auth_callback"
            );
        }
    }

    #[test]
    fn callback_denial_requires_the_matching_state() {
        let manager = manager();
        let pending = manager
            .create_authentication_session(
                "http://127.0.0.1:43210/auth/callback",
                &format!("urn:uuid:{}", uuid::Uuid::new_v4()),
            )
            .unwrap();

        assert!(parse_authorization_callback(
            &pending,
            &format!("/auth/callback?error=access_denied&state={}", pending.state),
            "127.0.0.1:43210",
        )
        .unwrap()
        .is_none());
    }
}
