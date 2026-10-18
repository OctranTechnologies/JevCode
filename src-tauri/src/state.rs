use crate::{
    config::AppConfig,
    credentials::CredentialStore,
    error::{AppError, AppResult},
    extensions::ExtensionRegistry,
    mcp::McpManager,
    persistence::Database,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio::sync::watch;

pub struct AppState {
    pub database: Database,
    pub config: AppConfig,
    pub credentials: CredentialStore,
    pub mcp: Arc<McpManager>,
    pub extensions: ExtensionRegistry,
    pub runs: Mutex<HashMap<String, watch::Sender<bool>>>,
    pub openai_auth_attempts: Mutex<HashMap<String, watch::Sender<bool>>>,
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
    ) -> AppResult<()> {
        let mut attempts = self
            .openai_auth_attempts
            .lock()
            .map_err(AppError::internal)?;
        attempts.insert(id.to_owned(), cancel);
        Ok(())
    }

    pub fn cancel_openai_auth_attempt(&self, id: &str) -> AppResult<()> {
        let attempts = self
            .openai_auth_attempts
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
