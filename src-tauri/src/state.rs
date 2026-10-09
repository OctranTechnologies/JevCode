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
}

pub type SharedState = Arc<AppState>;
