use crate::error::{AppError, AppResult};
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

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
    pub fn has(&self, provider_id: &str) -> AppResult<bool> {
        match Self::entry(provider_id)?.get_password() {
            Ok(mut secret) => {
                secret.zeroize();
                Ok(true)
            }
            Err(keyring::Error::NoEntry) => Ok(false),
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

    /// OpenAI OAuth records are stored only in the OS credential store. Large
    /// records are split into bounded items because Windows Credential Manager
    /// limits each credential blob to 2,560 bytes. This remains native-only;
    /// no Tauri command accepts or returns sessions.
    #[allow(dead_code)]
    pub(crate) fn save_openai_session(&self, account_id: &str, session: &str) -> AppResult<()> {
        save_openai_session_with(&OsSessionCredentialBackend, account_id, session)
    }

    pub(crate) fn get_openai_session(&self, account_id: &str) -> AppResult<Option<String>> {
        get_openai_session_with(&OsSessionCredentialBackend, account_id)
    }

    #[allow(dead_code)]
    pub(crate) fn delete_openai_session(&self, account_id: &str) -> AppResult<()> {
        delete_openai_session_with(&OsSessionCredentialBackend, account_id)
    }

    /// Store a Google OAuth session as one native credential. The serialized
    /// value is never returned by a Tauri command or written to SQLite.
    pub(crate) fn save_google_session(&self, account_id: &str, session: &str) -> AppResult<()> {
        if session.trim().is_empty() || session.len() > 65_536 {
            return Err(AppError::new(
                "invalid_input",
                "The Google session record is invalid or too large.",
            ));
        }
        Self::google_session_entry(account_id)?
            .set_password(session)
            .map_err(|_| {
                AppError::new(
                    "keychain",
                    "Could not save the Google session to the OS credential store.",
                )
            })
    }

    pub(crate) fn get_google_session(&self, account_id: &str) -> AppResult<Option<String>> {
        match Self::google_session_entry(account_id)?.get_password() {
            Ok(session) => Ok(Some(session)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(AppError::new(
                "keychain",
                "Could not read the Google session from the OS credential store.",
            )),
        }
    }

    pub(crate) fn delete_google_session(&self, account_id: &str) -> AppResult<()> {
        match Self::google_session_entry(account_id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(AppError::new(
                "keychain",
                "Could not remove the Google session from the OS credential store.",
            )),
        }
    }

    pub(crate) fn get_legacy_google_session(&self) -> AppResult<Option<String>> {
        match Self::legacy_google_session_entry()?.get_password() {
            Ok(session) => Ok(Some(session)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(AppError::new(
                "keychain",
                "Could not read the Google session from the OS credential store.",
            )),
        }
    }

    pub(crate) fn delete_legacy_google_session(&self) -> AppResult<()> {
        match Self::legacy_google_session_entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(AppError::new(
                "keychain",
                "Could not remove the legacy Google session from the OS credential store.",
            )),
        }
    }

    fn google_session_entry(account_id: &str) -> AppResult<keyring::Entry> {
        let account_key = google_session_account_key(account_id)?;
        keyring::Entry::new("dev.jevcode.desktop.google", &account_key).map_err(|_| {
            AppError::new(
                "keychain",
                "The OS credential store is unavailable. Unlock your keychain and retry.",
            )
        })
    }

    fn legacy_google_session_entry() -> AppResult<keyring::Entry> {
        keyring::Entry::new("dev.jevcode.desktop.google", "gemini.oauth.session").map_err(|_| {
            AppError::new(
                "keychain",
                "The OS credential store is unavailable. Unlock your keychain and retry.",
            )
        })
    }

    #[cfg(test)]
    fn openai_session_entry(account_id: &str) -> AppResult<keyring::Entry> {
        let account_id = parse_account_id(account_id)?;
        openai_entry_for_key(&session_root_key(&account_id))
    }
}

const OPENAI_SESSION_SERVICE: &str = "dev.jevcode.desktop.openai";
const OPENAI_SESSION_MANIFEST_PREFIX: &str = "jevcode-openai-session-v1:";
const OPENAI_SESSION_CHUNK_BYTES: usize = 1024;
const MAX_OPENAI_SESSION_BYTES: usize = 65_536;
// Allow for the few bytes lost when a UTF-8 chunk ends near a multi-byte char.
const MAX_OPENAI_SESSION_CHUNKS: usize = MAX_OPENAI_SESSION_BYTES / OPENAI_SESSION_CHUNK_BYTES + 2;

trait SessionCredentialBackend {
    fn get(&self, key: &str) -> AppResult<Option<String>>;
    fn set(&self, key: &str, value: &str) -> AppResult<()>;
    fn delete(&self, key: &str) -> AppResult<()>;
}

struct OsSessionCredentialBackend;

impl SessionCredentialBackend for OsSessionCredentialBackend {
    fn get(&self, key: &str) -> AppResult<Option<String>> {
        match openai_entry_for_key(key)?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(AppError::new(
                "keychain",
                "Could not read the OpenAI session from the OS credential store.",
            )),
        }
    }

    fn set(&self, key: &str, value: &str) -> AppResult<()> {
        openai_entry_for_key(key)?.set_password(value).map_err(|_| {
            AppError::new(
                "keychain",
                "Could not save the OpenAI session to the OS credential store.",
            )
        })
    }

    fn delete(&self, key: &str) -> AppResult<()> {
        match openai_entry_for_key(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(AppError::new(
                "keychain",
                "Could not remove the OpenAI session from the OS credential store.",
            )),
        }
    }
}

fn openai_entry_for_key(key: &str) -> AppResult<keyring::Entry> {
    validate_session_key(key)?;
    keyring::Entry::new(OPENAI_SESSION_SERVICE, key).map_err(|_| {
        AppError::new(
            "keychain",
            "The OS credential store is unavailable. Unlock your keychain and retry.",
        )
    })
}

fn parse_account_id(account_id: &str) -> AppResult<uuid::Uuid> {
    uuid::Uuid::parse_str(account_id)
        .map_err(|_| AppError::new("invalid_input", "The OpenAI account identifier is invalid."))
}

fn session_root_key(account_id: &uuid::Uuid) -> String {
    format!("chatgpt.{}", account_id.hyphenated())
}

fn session_chunk_key(account_id: &uuid::Uuid, generation: &uuid::Uuid, index: usize) -> String {
    format!(
        "{}.session.{}.{}",
        session_root_key(account_id),
        generation.hyphenated(),
        index
    )
}

fn validate_session_key(key: &str) -> AppResult<()> {
    let parts = key.split('.').collect::<Vec<_>>();
    let valid_root =
        parts.len() == 2 && parts[0] == "chatgpt" && uuid::Uuid::parse_str(parts[1]).is_ok();
    let valid_chunk = parts.len() == 5
        && parts[0] == "chatgpt"
        && uuid::Uuid::parse_str(parts[1]).is_ok()
        && parts[2] == "session"
        && uuid::Uuid::parse_str(parts[3]).is_ok()
        && parts[4]
            .parse::<usize>()
            .is_ok_and(|index| index < MAX_OPENAI_SESSION_CHUNKS);
    if valid_root || valid_chunk {
        Ok(())
    } else {
        Err(AppError::new(
            "invalid_input",
            "The OpenAI credential identifier is invalid.",
        ))
    }
}

#[derive(Debug, PartialEq, Eq)]
enum SessionPointer {
    Legacy,
    Chunked {
        generation: uuid::Uuid,
        count: usize,
    },
}

fn parse_session_pointer(value: &str) -> AppResult<SessionPointer> {
    let Some(encoded) = value.strip_prefix(OPENAI_SESSION_MANIFEST_PREFIX) else {
        return Ok(SessionPointer::Legacy);
    };
    let mut parts = encoded.split(':');
    let generation = parts
        .next()
        .and_then(|part| uuid::Uuid::parse_str(part).ok());
    let count = parts.next().and_then(|part| part.parse::<usize>().ok());
    if parts.next().is_some() {
        return Err(corrupt_session_error());
    }
    match (generation, count) {
        (Some(generation), Some(count)) if (1..=MAX_OPENAI_SESSION_CHUNKS).contains(&count) => {
            Ok(SessionPointer::Chunked { generation, count })
        }
        _ => Err(corrupt_session_error()),
    }
}

fn session_manifest(generation: &uuid::Uuid, count: usize) -> String {
    format!(
        "{OPENAI_SESSION_MANIFEST_PREFIX}{}:{count}",
        generation.hyphenated()
    )
}

fn corrupt_session_error() -> AppError {
    AppError::new(
        "keychain",
        "The saved OpenAI session could not be read. Remove the account and reconnect.",
    )
}

fn split_session(session: &str) -> Vec<&str> {
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < session.len() {
        let mut end = (start + OPENAI_SESSION_CHUNK_BYTES).min(session.len());
        while !session.is_char_boundary(end) {
            end -= 1;
        }
        chunks.push(&session[start..end]);
        start = end;
    }
    chunks
}

fn save_openai_session_with<B: SessionCredentialBackend>(
    backend: &B,
    account_id: &str,
    session: &str,
) -> AppResult<()> {
    let account_id = parse_account_id(account_id)?;
    if session.trim().is_empty() || session.len() > MAX_OPENAI_SESSION_BYTES {
        return Err(AppError::new(
            "invalid_input",
            "The OpenAI session record is invalid or too large.",
        ));
    }
    let root = session_root_key(&account_id);
    let old_pointer = backend.get(&root)?;
    let old_generation = old_pointer
        .as_deref()
        .map(parse_session_pointer)
        .transpose()?;
    if session.len() <= OPENAI_SESSION_CHUNK_BYTES {
        backend.set(&root, session)?;
        if let Some(SessionPointer::Chunked {
            generation: old_generation,
            count,
        }) = old_generation
        {
            for index in 0..count {
                let _ = backend.delete(&session_chunk_key(&account_id, &old_generation, index));
            }
        }
        return Ok(());
    }
    let generation = uuid::Uuid::new_v4();
    let chunks = split_session(session);
    let mut written_keys: Vec<String> = Vec::with_capacity(chunks.len());
    for (index, chunk) in chunks.iter().enumerate() {
        let key = session_chunk_key(&account_id, &generation, index);
        if let Err(error) = backend.set(&key, chunk) {
            for written_key in written_keys {
                let _ = backend.delete(&written_key);
            }
            return Err(error);
        }
        written_keys.push(key);
    }
    if let Err(error) = backend.set(&root, &session_manifest(&generation, chunks.len())) {
        for written_key in written_keys {
            let _ = backend.delete(&written_key);
        }
        return Err(error);
    }
    if let Some(SessionPointer::Chunked {
        generation: old_generation,
        count,
    }) = old_generation
    {
        for index in 0..count {
            let _ = backend.delete(&session_chunk_key(&account_id, &old_generation, index));
        }
    }
    Ok(())
}

fn get_openai_session_with<B: SessionCredentialBackend>(
    backend: &B,
    account_id: &str,
) -> AppResult<Option<String>> {
    let account_id = parse_account_id(account_id)?;
    let root = session_root_key(&account_id);
    let Some(pointer) = backend.get(&root)? else {
        return Ok(None);
    };
    match parse_session_pointer(&pointer)? {
        SessionPointer::Legacy => Ok(Some(pointer)),
        SessionPointer::Chunked { generation, count } => {
            let mut session = String::new();
            for index in 0..count {
                let chunk_key = session_chunk_key(&account_id, &generation, index);
                let chunk = backend.get(&chunk_key)?.ok_or_else(corrupt_session_error)?;
                if chunk.len() > OPENAI_SESSION_CHUNK_BYTES {
                    return Err(corrupt_session_error());
                }
                session.push_str(&chunk);
                if session.len() > MAX_OPENAI_SESSION_BYTES {
                    return Err(corrupt_session_error());
                }
            }
            Ok(Some(session))
        }
    }
}

fn delete_openai_session_with<B: SessionCredentialBackend>(
    backend: &B,
    account_id: &str,
) -> AppResult<()> {
    let account_id = parse_account_id(account_id)?;
    let root = session_root_key(&account_id);
    if let Some(pointer) = backend.get(&root)? {
        if let SessionPointer::Chunked { generation, count } = parse_session_pointer(&pointer)? {
            for index in 0..count {
                backend.delete(&session_chunk_key(&account_id, &generation, index))?;
            }
        }
        backend.delete(&root)?;
    }
    Ok(())
}

#[cfg(test)]
mod session_storage_tests {
    use super::{
        delete_openai_session_with, get_openai_session_with, parse_session_pointer,
        save_openai_session_with, session_chunk_key, session_root_key, split_session,
        validate_session_key, SessionCredentialBackend, SessionPointer, MAX_OPENAI_SESSION_CHUNKS,
        OPENAI_SESSION_CHUNK_BYTES,
    };
    use crate::error::{AppError, AppResult};
    use std::{collections::HashMap, sync::Mutex};

    #[derive(Default)]
    struct MemoryCredentialBackend {
        values: Mutex<HashMap<String, String>>,
        fail_on_set: Mutex<Option<usize>>,
        set_count: Mutex<usize>,
    }

    impl SessionCredentialBackend for MemoryCredentialBackend {
        fn get(&self, key: &str) -> AppResult<Option<String>> {
            Ok(self.values.lock().unwrap().get(key).cloned())
        }

        fn set(&self, key: &str, value: &str) -> AppResult<()> {
            let mut count = self.set_count.lock().unwrap();
            *count += 1;
            if *self.fail_on_set.lock().unwrap() == Some(*count) {
                return Err(AppError::new("keychain", "mock write failure"));
            }
            self.values
                .lock()
                .unwrap()
                .insert(key.to_owned(), value.to_owned());
            Ok(())
        }

        fn delete(&self, key: &str) -> AppResult<()> {
            self.values.lock().unwrap().remove(key);
            Ok(())
        }
    }

    const ACCOUNT_ID: &str = "550e8400-e29b-41d4-a716-446655440000";

    #[test]
    fn stores_large_sessions_in_bounded_chunks_and_reads_them_back() {
        let backend = MemoryCredentialBackend::default();
        let session = format!("{{\"token\":\"{}\"}}", "x".repeat(12_000));
        save_openai_session_with(&backend, ACCOUNT_ID, &session).unwrap();
        assert_eq!(
            get_openai_session_with(&backend, ACCOUNT_ID).unwrap(),
            Some(session)
        );

        let root = session_root_key(&uuid::Uuid::parse_str(ACCOUNT_ID).unwrap());
        let pointer = backend.values.lock().unwrap().get(&root).unwrap().clone();
        let SessionPointer::Chunked { generation, count } =
            parse_session_pointer(&pointer).unwrap()
        else {
            panic!("expected chunk manifest");
        };
        assert!(count > 1);
        assert!(count <= MAX_OPENAI_SESSION_CHUNKS);
        for index in 0..count {
            let key = session_chunk_key(
                &uuid::Uuid::parse_str(ACCOUNT_ID).unwrap(),
                &generation,
                index,
            );
            assert!(backend.values.lock().unwrap()[&key].len() <= OPENAI_SESSION_CHUNK_BYTES);
        }
    }

    #[test]
    fn chunks_unicode_only_at_utf8_character_boundaries() {
        let session = "a".repeat(1023) + "🧑‍💻" + &"z".repeat(2048);
        let chunks = split_session(&session);
        assert!(chunks
            .iter()
            .all(|chunk| chunk.len() <= OPENAI_SESSION_CHUNK_BYTES));
        assert_eq!(chunks.concat(), session);
    }

    #[test]
    fn reads_legacy_single_item_sessions() {
        let backend = MemoryCredentialBackend::default();
        let root = session_root_key(&uuid::Uuid::parse_str(ACCOUNT_ID).unwrap());
        backend
            .values
            .lock()
            .unwrap()
            .insert(root, "legacy-json-session".into());
        assert_eq!(
            get_openai_session_with(&backend, ACCOUNT_ID).unwrap(),
            Some("legacy-json-session".into())
        );
    }

    #[test]
    fn failed_manifest_write_cleans_up_new_chunks_and_keeps_old_session() {
        let backend = MemoryCredentialBackend::default();
        let old = "old-token-session";
        save_openai_session_with(&backend, ACCOUNT_ID, old).unwrap();
        let previous_records = backend.values.lock().unwrap().clone();
        // Three chunk writes succeed; fail the fourth write, the manifest switch.
        *backend.fail_on_set.lock().unwrap() = Some(*backend.set_count.lock().unwrap() + 4);
        let result = save_openai_session_with(&backend, ACCOUNT_ID, &"new".repeat(1024));
        assert!(result.is_err());
        assert_eq!(
            get_openai_session_with(&backend, ACCOUNT_ID).unwrap(),
            Some(old.into())
        );
        let values = backend.values.lock().unwrap();
        assert_eq!(*values, previous_records);
    }

    #[test]
    fn deleting_chunked_session_removes_manifest_and_all_chunks() {
        let backend = MemoryCredentialBackend::default();
        save_openai_session_with(&backend, ACCOUNT_ID, &"secret".repeat(1000)).unwrap();
        delete_openai_session_with(&backend, ACCOUNT_ID).unwrap();
        assert!(get_openai_session_with(&backend, ACCOUNT_ID)
            .unwrap()
            .is_none());
        assert!(backend.values.lock().unwrap().is_empty());
    }

    #[test]
    fn credential_keys_reject_untrusted_names() {
        assert!(validate_session_key(&format!("chatgpt.{ACCOUNT_ID}")).is_ok());
        assert!(validate_session_key("chatgpt...\0").is_err());
        assert!(parse_session_pointer("jevcode-openai-session-v1:../../:999").is_err());
    }
}

fn google_session_account_key(account_id: &str) -> AppResult<String> {
    if account_id.trim().is_empty()
        || account_id.len() > 512
        || account_id.chars().any(char::is_control)
    {
        return Err(AppError::new(
            "invalid_input",
            "The Google account identifier is invalid.",
        ));
    }
    // OIDC subject values do not need to appear in the user's keychain labels.
    Ok(format!(
        "gemini.oauth.{}",
        format_digest(Sha256::digest(account_id.as_bytes()).as_slice())
    ))
}

fn format_digest(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
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

    #[test]
    fn google_oauth_session_keychain_entry_is_separate_from_api_keys() {
        assert!(CredentialStore::google_session_entry("google-subject").is_ok());
        assert!(CredentialStore::google_session_entry("").is_err());
        let first = super::google_session_account_key("subject-one").unwrap();
        let second = super::google_session_account_key("subject-two").unwrap();
        assert_ne!(first, second);
        assert!(!first.contains("subject-one"));
        assert!(CredentialStore::entry("gemini").is_ok());
    }
}
