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

    /// OpenAI OAuth records are stored as one keychain item per local account.
    /// This method is native-only; no Tauri command accepts or returns sessions.
    #[allow(dead_code)]
    pub(crate) fn save_openai_session(&self, account_id: &str, session: &str) -> AppResult<()> {
        if session.trim().is_empty() || session.len() > 65_536 {
            return Err(AppError::new(
                "invalid_input",
                "The OpenAI session record is invalid or too large.",
            ));
        }
        Self::openai_session_entry(account_id)?
            .set_password(session)
            .map_err(|_| {
                AppError::new(
                    "keychain",
                    "Could not save the OpenAI session to the OS credential store.",
                )
            })
    }

    pub(crate) fn get_openai_session(&self, account_id: &str) -> AppResult<Option<String>> {
        match Self::openai_session_entry(account_id)?.get_password() {
            Ok(session) => Ok(Some(session)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(AppError::new(
                "keychain",
                "Could not read the OpenAI session from the OS credential store.",
            )),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn delete_openai_session(&self, account_id: &str) -> AppResult<()> {
        match Self::openai_session_entry(account_id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(AppError::new(
                "keychain",
                "Could not remove the OpenAI session from the OS credential store.",
            )),
        }
    }

    fn openai_session_entry(account_id: &str) -> AppResult<keyring::Entry> {
        let account_id = uuid::Uuid::parse_str(account_id).map_err(|_| {
            AppError::new("invalid_input", "The OpenAI account identifier is invalid.")
        })?;
        keyring::Entry::new(
            "dev.jevcode.desktop.openai",
            &format!("chatgpt.{}", account_id.hyphenated()),
        )
        .map_err(|_| {
            AppError::new(
                "keychain",
                "The OS credential store is unavailable. Unlock your keychain and retry.",
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::CredentialStore;

    #[test]
    fn openai_session_keychain_names_are_scoped_to_uuid_accounts() {
        assert!(CredentialStore::openai_session_entry("../../other-service").is_err());
        assert!(CredentialStore::openai_session_entry("not-a-uuid").is_err());
        assert!(
            CredentialStore::openai_session_entry("550e8400-e29b-41d4-a716-446655440000").is_ok()
        );
    }
}
