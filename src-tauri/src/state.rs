use crate::{
    config::AppConfig,
    credentials::CredentialStore,
    error::{AppError, AppResult},
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
    pub runs: Mutex<HashMap<String, watch::Sender<bool>>>,
}

impl AppState {
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
