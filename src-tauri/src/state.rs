use crate::{
    config::AppConfig,
    credentials::CredentialStore,
    error::{AppError, AppResult},
    extensions::ExtensionRegistry,
    mcp::McpManager,
    persistence::Database,
};
use chrono::{DateTime, Utc};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio::sync::watch;
use zeroize::Zeroizing;

struct OpenAiAuthAttempt {
    cancel: watch::Sender<bool>,
    authorization_url: Zeroizing<String>,
    expires_at: DateTime<Utc>,
}

pub struct AppState {
    pub database: Database,
    pub config: AppConfig,
    pub credentials: CredentialStore,
    pub mcp: Arc<McpManager>,
    pub extensions: ExtensionRegistry,
    pub runs: Mutex<HashMap<String, watch::Sender<bool>>>,
    openai_auth_attempts: Mutex<HashMap<String, OpenAiAuthAttempt>>,
    pub google_auth_attempts: Mutex<HashMap<String, watch::Sender<bool>>>,
}

impl AppState {
    pub fn new(database: Database, config: AppConfig, credentials: CredentialStore) -> Self {
        let mcp = Arc::new(McpManager::default());
        let extensions = ExtensionRegistry::default().with(mcp.clone());
        Self {
            database,
            config,
            credentials,
            mcp,
            extensions,
            runs: Mutex::new(Default::default()),
            openai_auth_attempts: Mutex::new(Default::default()),
            google_auth_attempts: Mutex::new(Default::default()),
        }
    }

    pub fn reserve_run(&self, id: &str) -> AppResult<watch::Receiver<bool>> {
        let mut runs = self.runs.lock().map_err(AppError::internal)?;
        if runs.contains_key(id) {
            return Err(AppError::new(
                "session_busy",
                "This session is already running.",
            ));
        }
        let (sender, receiver) = watch::channel(false);
        runs.insert(id.into(), sender);
        Ok(receiver)
    }
    pub fn release_run(&self, id: &str) {
        if let Ok(mut runs) = self.runs.lock() {
            runs.remove(id);
        }
    }

    pub fn register_openai_auth_attempt(
        &self,
        id: &str,
        cancel: watch::Sender<bool>,
        authorization_url: &str,
        expires_at: DateTime<Utc>,
    ) -> AppResult<()> {
        let mut attempts = self
            .openai_auth_attempts
            .lock()
            .map_err(AppError::internal)?;
        attempts.insert(
            id.to_owned(),
            OpenAiAuthAttempt {
                cancel,
                authorization_url: Zeroizing::new(authorization_url.to_owned()),
                expires_at,
            },
        );
        Ok(())
    }

    pub fn cancel_openai_auth_attempt(&self, id: &str) -> AppResult<()> {
        let mut attempts = self
            .openai_auth_attempts
            .lock()
            .map_err(AppError::internal)?;
        let attempt = attempts.remove(id).ok_or_else(|| {
            AppError::new(
                "auth_attempt_not_found",
                "This sign-in attempt is no longer active.",
            )
        })?;
        let _ = attempt.cancel.send(true);
        Ok(())
    }

    /// Copy the active sign-in URL without returning it across the renderer IPC boundary.
    pub fn copy_openai_auth_link(
        &self,
        id: &str,
        copy_to_clipboard: impl FnOnce(&str) -> AppResult<()>,
    ) -> AppResult<()> {
        let attempts = self
            .openai_auth_attempts
            .lock()
            .map_err(AppError::internal)?;
        let attempt = attempts.get(id).ok_or_else(|| {
            AppError::new(
                "auth_attempt_not_found",
                "This sign-in link is no longer available. Start a new sign-in attempt.",
            )
        })?;
        if Utc::now() >= attempt.expires_at {
            return Err(AppError::new(
                "auth_attempt_expired",
                "This sign-in link has expired. Start a new sign-in attempt.",
            ));
        }
        copy_to_clipboard(attempt.authorization_url.as_str())
    }

    pub fn release_openai_auth_attempt(&self, id: &str) {
        if let Ok(mut attempts) = self.openai_auth_attempts.lock() {
            attempts.remove(id);
        }
    }

    pub fn register_google_auth_attempt(
        &self,
        id: &str,
        cancel: watch::Sender<bool>,
    ) -> AppResult<()> {
        self.google_auth_attempts
            .lock()
            .map_err(AppError::internal)?
            .insert(id.to_owned(), cancel);
        Ok(())
    }

    pub fn cancel_google_auth_attempt(&self, id: &str) -> AppResult<()> {
        let attempts = self
            .google_auth_attempts
            .lock()
            .map_err(AppError::internal)?;
        let cancel = attempts.get(id).ok_or_else(|| {
            AppError::new(
                "auth_attempt_not_found",
                "This sign-in attempt is no longer active.",
            )
        })?;
        let _ = cancel.send(true);
        Ok(())
    }

    pub fn release_google_auth_attempt(&self, id: &str) {
        if let Ok(mut attempts) = self.google_auth_attempts.lock() {
            attempts.remove(id);
        }
    }
}

pub type SharedState = Arc<AppState>;

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> AppState {
        AppState::new(
            crate::persistence::Database::open(std::path::Path::new(":memory:"))
                .expect("in-memory database"),
            crate::config::AppConfig::load(std::path::Path::new("."))
                .expect("default application config"),
            crate::credentials::CredentialStore,
        )
    }

    #[test]
    fn copies_active_sign_in_link_without_returning_it() {
        let state = state();
        let (cancel, _) = watch::channel(false);
        state
            .register_openai_auth_attempt(
                "attempt",
                cancel,
                "https://auth.openai.com/authorize?state=secret-state&code_challenge=secret-challenge",
                Utc::now() + chrono::Duration::minutes(1),
            )
            .expect("register attempt");

        let mut copied = String::new();
        state
            .copy_openai_auth_link("attempt", |url| {
                copied = url.to_owned();
                Ok(())
            })
            .expect("copy active link");
        assert_eq!(
            copied,
            "https://auth.openai.com/authorize?state=secret-state&code_challenge=secret-challenge"
        );
    }

    #[test]
    fn refuses_unknown_and_expired_sign_in_links() {
        let state = state();
        assert_eq!(
            state
                .copy_openai_auth_link("missing", |_| Ok(()))
                .unwrap_err()
                .code,
            "auth_attempt_not_found"
        );

        let (cancel, _) = watch::channel(false);
        state
            .register_openai_auth_attempt(
                "expired",
                cancel,
                "https://auth.openai.com/authorize?state=secret-state",
                Utc::now() - chrono::Duration::seconds(1),
            )
            .expect("register expired attempt");
        assert_eq!(
            state
                .copy_openai_auth_link("expired", |_| Ok(()))
                .unwrap_err()
                .code,
            "auth_attempt_expired"
        );
    }

    #[test]
    fn releasing_attempt_removes_its_link() {
        let state = state();
        let (cancel, _) = watch::channel(false);
        state
            .register_openai_auth_attempt(
                "attempt",
                cancel,
                "https://auth.openai.com/authorize?state=secret-state",
                Utc::now() + chrono::Duration::minutes(1),
            )
            .expect("register attempt");
        state.release_openai_auth_attempt("attempt");
        assert_eq!(
            state
                .copy_openai_auth_link("attempt", |_| Ok(()))
                .unwrap_err()
                .code,
            "auth_attempt_not_found"
        );
    }

    #[test]
    fn clipboard_failure_does_not_include_sign_in_material() {
        let state = state();
        let (cancel, _) = watch::channel(false);
        state
            .register_openai_auth_attempt(
                "attempt",
                cancel,
                "https://auth.openai.com/authorize?state=must-not-leak",
                Utc::now() + chrono::Duration::minutes(1),
            )
            .expect("register attempt");
        let error = state
            .copy_openai_auth_link("attempt", |_| {
                Err(AppError::new(
                    "clipboard_unavailable",
                    "Clipboard access is unavailable.",
                ))
            })
            .unwrap_err();
        assert_eq!(error.code, "clipboard_unavailable");
        assert!(!error.message.contains("must-not-leak"));
    }
}
