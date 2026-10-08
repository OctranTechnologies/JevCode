use crate::error::{AppError, AppResult};

/// Only Rust may retrieve secrets. IPC exposes presence, save, and delete.
pub struct CredentialStore;

impl CredentialStore {
    fn entry(provider_id: &str) -> AppResult<keyring::Entry> {
        keyring::Entry::new("dev.jevcode.desktop", provider_id).map_err(|_| {
            AppError::new(
                "keychain",
                "The OS credential store is unavailable. Unlock your keychain and retry.",
            )
        })
    }
    pub fn get(&self, provider_id: &str) -> AppResult<Option<String>> {
        match Self::entry(provider_id)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(AppError::new(
                "keychain",
                "Could not read the OS credential store. Unlock your keychain and retry.",
            )),
        }
    }
    pub fn save(&self, provider_id: &str, secret: &str) -> AppResult<()> {
        if secret.trim().is_empty() || secret.len() > 16384 {
            return Err(AppError::new("invalid_input", "Enter a valid API key."));
        }
        Self::entry(provider_id)?
            .set_password(secret.trim())
            .map_err(|_| {
                AppError::new(
                    "keychain",
                    "Could not save the API key to the OS credential store.",
                )
            })
    }
    pub fn delete(&self, provider_id: &str) -> AppResult<()> {
        match Self::entry(provider_id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(AppError::new(
                "keychain",
                "Could not remove the API key from the OS credential store.",
            )),
        }
    }
}
