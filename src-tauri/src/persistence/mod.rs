use crate::{
    domain::*,
    error::{AppError, AppResult},
};
use rusqlite::{params, Connection, OptionalExtension};
use std::{collections::BTreeMap, path::Path, sync::Mutex};

fn mcp_scope_name(scope: &McpScope) -> &'static str {
    match scope {
        McpScope::User => "user",
        McpScope::Project => "project",
    }
}

fn validate_openai_host_id(host_id: String) -> AppResult<String> {
    let valid = host_id
        .strip_prefix("urn:uuid:")
        .and_then(|value| uuid::Uuid::parse_str(value).ok())
        .is_some();
    if valid {
        Ok(host_id)
    } else {
        Err(AppError::new(
            "invalid_installation_identity",
            "The saved OpenAI installation identity is invalid.",
        ))
    }
}

#[derive(Debug, Clone)]
pub struct SessionReviewBaseline {
    pub session_id: String,
    pub project_id: String,
    pub root: String,
    pub head: Option<String>,
    pub statuses: BTreeMap<String, String>,
    pub status_available: bool,
    pub status_truncated: bool,
    pub started_at: String,
}

#[derive(Debug, Clone)]
pub struct SessionFileCheckpoint {
    pub session_id: String,
    pub path: String,
    pub target_path: String,
    pub existed_before: bool,
    pub before_content: Option<Vec<u8>>,
    pub before_mode: Option<u32>,
    pub agent_exists: bool,
    pub agent_content: Option<Vec<u8>>,
    pub expected_hash: Option<String>,
    pub preexisting_status: Option<String>,
    pub reviewed: bool,
}

pub struct Database(Mutex<Connection>);

impl Database {
    pub fn open(path: &Path) -> AppResult<Self> {
        let connection = Connection::open(path)?;
        connection.execute_batch(include_str!("schema.sql"))?;
        Self::migrate_usage_retention(&connection)?;
        Self::migrate_projects(&connection)?;
        Self::migrate_model_management(&connection)?;
        Self::migrate_permissions(&connection)?;
        Self::migrate_code_review(&connection)?;
        Self::migrate_mcp(&connection)?;
        Self::migrate_openai_accounts(&connection)?;
        Self::migrate_openai_installation(&connection)?;
        Self::migrate_openai_account_selection(&connection)?;
        Self::migrate_openai_auth_preference(&connection)?;
        connection.pragma_update(None, "user_version", 14)?;
        let database = Self(Mutex::new(connection));
        database.recover_interrupted()?;
        // Re-serialize legacy JSON rows once so newly added metadata survives the
        // next restart even when the database predates the project workspace UI.
        for project in database.projects()? {
            database.save_project(&project)?;
        }
        Ok(database)
    }

    /// Older versions tied request history to a task with a foreign key. Rebuild
    /// that table once so deleting a task cannot erase local usage telemetry.
    fn migrate_usage_retention(connection: &Connection) -> AppResult<()> {
        let has_session_foreign_key = {
            let mut statement = connection.prepare("PRAGMA foreign_key_list(usage)")?;
            let mut rows = statement.query([])?;
            rows.next()?.is_some()
        };
        if !has_session_foreign_key {
            return Ok(());
        }

        let migration = connection.execute_batch(
            "BEGIN IMMEDIATE;
             CREATE TABLE usage_retained (
                 id TEXT PRIMARY KEY,
                 session_id TEXT NOT NULL,
                 created_at TEXT NOT NULL,
                 data TEXT NOT NULL
             );
             INSERT INTO usage_retained (id, session_id, created_at, data)
                 SELECT id, session_id, created_at, data FROM usage;
             DROP TABLE usage;
             ALTER TABLE usage_retained RENAME TO usage;
             CREATE INDEX IF NOT EXISTS usage_session ON usage(session_id);
             COMMIT;",
        );
        if migration.is_err() {
            let _ = connection.execute_batch("ROLLBACK;");
        }
        migration?;
        Ok(())
    }

    fn migrate_projects(connection: &Connection) -> AppResult<()> {
        let mut statement = connection.prepare("PRAGMA table_info(projects)")?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<std::collections::HashSet<_>, _>>()?;
        if !columns.contains("last_opened_at") {
            connection.execute(
                "ALTER TABLE projects ADD COLUMN last_opened_at TEXT NOT NULL DEFAULT ''",
                [],
            )?;
            connection.execute(
                "UPDATE projects SET last_opened_at = created_at WHERE last_opened_at = ''",
                [],
            )?;
        }
        if !columns.contains("is_recent") {
            connection.execute(
                "ALTER TABLE projects ADD COLUMN is_recent INTEGER NOT NULL DEFAULT 1",
                [],
            )?;
        }
        connection.execute_batch(
            "CREATE INDEX IF NOT EXISTS projects_recent ON projects(is_recent, last_opened_at DESC); PRAGMA user_version = 3;",
        )?;
        Ok(())
    }

    fn migrate_model_management(connection: &Connection) -> AppResult<()> {
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS model_catalogs (provider_id TEXT PRIMARY KEY, data TEXT NOT NULL, fetched_at TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS model_preferences (id INTEGER PRIMARY KEY CHECK (id = 1), data TEXT NOT NULL);
             PRAGMA user_version = 4;",
        )?;
        Ok(())
    }

    fn migrate_permissions(connection: &Connection) -> AppResult<()> {
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS app_preferences (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS permission_rules (id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE, fingerprint TEXT NOT NULL, created_at TEXT NOT NULL, data TEXT NOT NULL, UNIQUE(project_id, fingerprint));
             CREATE INDEX IF NOT EXISTS permission_rules_project ON permission_rules(project_id, created_at DESC);
             INSERT OR IGNORE INTO app_preferences(key, value) VALUES ('permission_mode', 'ask');
             PRAGMA user_version = 6;",
        )?;
        let mut statement = connection.prepare("SELECT id, data FROM projects")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let projects = rows.collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        for (id, data) in projects {
            if let Ok(mut project) = serde_json::from_str::<Project>(&data) {
                let policy = &mut project.permissions;
                if policy.mode == PermissionMode::Ask
                    && policy.read_files == PermissionDecision::Allow
                    && policy.git == PermissionDecision::Ask
                    && policy.write_files == PermissionDecision::Deny
                    && policy.shell == PermissionDecision::Deny
                    && policy.external_files == PermissionDecision::Deny
                    && policy.max_tool_rounds == 8
                {
                    policy.git = PermissionDecision::Allow;
                    policy.write_files = PermissionDecision::Allow;
                    policy.shell = PermissionDecision::Allow;
                    policy.external_files = PermissionDecision::Ask;
                    connection.execute(
                        "UPDATE projects SET data = ?1 WHERE id = ?2",
                        params![serde_json::to_string(&project)?, id],
                    )?;
                }
            }
        }
        Ok(())
    }

    fn migrate_code_review(connection: &Connection) -> AppResult<()> {
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS session_review_baselines (
                 session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
                 project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                 root TEXT NOT NULL, head TEXT, statuses TEXT NOT NULL,
                 status_available INTEGER NOT NULL, status_truncated INTEGER NOT NULL,
                 started_at TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS session_file_checkpoints (
                 session_id TEXT NOT NULL REFERENCES session_review_baselines(session_id) ON DELETE CASCADE,
                 path TEXT NOT NULL, target_path TEXT NOT NULL, existed_before INTEGER NOT NULL,
                 before_content BLOB, before_mode INTEGER, agent_exists INTEGER NOT NULL,
                 agent_content BLOB, expected_hash TEXT, preexisting_status TEXT,
                 reviewed INTEGER NOT NULL DEFAULT 0,
                 PRIMARY KEY(session_id, path)
             );
             CREATE INDEX IF NOT EXISTS session_file_checkpoints_session ON session_file_checkpoints(session_id, path);
             PRAGMA user_version = 8;",
        )?;
        let has_reviewed = {
            let mut statement =
                connection.prepare("PRAGMA table_info(session_file_checkpoints)")?;
            let columns = statement.query_map([], |row| row.get::<_, String>(1))?;
            columns
                .collect::<Result<Vec<_>, _>>()?
                .iter()
                .any(|name| name == "reviewed")
        };
        if !has_reviewed {
            connection.execute("ALTER TABLE session_file_checkpoints ADD COLUMN reviewed INTEGER NOT NULL DEFAULT 0", [])?;
        }
        Ok(())
    }

    fn migrate_mcp(connection: &Connection) -> AppResult<()> {
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS mcp_servers (
                 id TEXT PRIMARY KEY, data TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS mcp_trust (
                 scope TEXT NOT NULL, project_id TEXT NOT NULL DEFAULT '',
                 server_id TEXT NOT NULL, fingerprint TEXT NOT NULL,
                 trusted_at TEXT NOT NULL,
                 PRIMARY KEY(scope, project_id, server_id)
             );
             PRAGMA user_version = 9;",
        )?;
        Ok(())
    }

    fn migrate_openai_accounts(connection: &Connection) -> AppResult<()> {
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS openai_accounts (
                 id TEXT PRIMARY KEY, data TEXT NOT NULL
             );
             PRAGMA user_version = 10;",
        )?;
        Ok(())
    }

    fn migrate_openai_installation(connection: &Connection) -> AppResult<()> {
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS openai_installation (
                 singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
                 ext_agent_host_id TEXT NOT NULL
             );
             PRAGMA user_version = 11;",
        )?;
        Ok(())
    }

    fn migrate_openai_account_selection(connection: &Connection) -> AppResult<()> {
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS openai_account_selection (
                 singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
                 account_id TEXT NOT NULL
             );
             PRAGMA user_version = 12;",
        )?;
        Ok(())
    }

    fn migrate_openai_auth_preference(connection: &Connection) -> AppResult<()> {
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS openai_auth_preference (
                singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                method TEXT NOT NULL CHECK (method IN ('api_key', 'openai_chatgpt'))
             );
             PRAGMA user_version = 13;",
        )?;
        Ok(())
    }

    /// Stable, non-secret identifier for this installation's OpenAI agent host.
    /// It is persisted locally because OpenAI requires the same value on later
    /// sign-ins from this host.
    pub fn openai_ext_agent_host_id(&self) -> AppResult<String> {
        let connection = self.connection()?;
        if let Some(host_id) = connection
            .query_row(
                "SELECT ext_agent_host_id FROM openai_installation WHERE singleton = 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        {
            return validate_openai_host_id(host_id);
        }

        let host_id = format!("urn:uuid:{}", uuid::Uuid::new_v4());
        connection.execute(
            "INSERT OR IGNORE INTO openai_installation(singleton, ext_agent_host_id) VALUES (1, ?1)",
            [&host_id],
        )?;
        let stored = connection.query_row(
            "SELECT ext_agent_host_id FROM openai_installation WHERE singleton = 1",
            [],
            |row| row.get::<_, String>(0),
        )?;
        validate_openai_host_id(stored)
    }

    /// Persist only the safe OpenAI account profile. Token material has a
    /// separate keychain-only path in the native authentication module.
    pub fn openai_accounts(&self) -> AppResult<Vec<OpenAiAccount>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare("SELECT data FROM openai_accounts ORDER BY id")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }

    pub fn selected_openai_account_id(&self) -> AppResult<Option<String>> {
        let selected = self
            .connection()?
            .query_row(
                "SELECT account_id FROM openai_account_selection WHERE singleton = 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        selected
            .map(|id| {
                uuid::Uuid::parse_str(&id)
                    .map(|id| id.to_string())
                    .map_err(|_| {
                        AppError::new(
                            "persistence",
                            "The saved OpenAI account selection is invalid.",
                        )
                    })
            })
            .transpose()
    }

    pub fn openai_auth_preference(&self) -> AppResult<Option<ProviderAuthMethod>> {
        let method = self
            .connection()?
            .query_row(
                "SELECT method FROM openai_auth_preference WHERE singleton = 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        method
            .map(|method| match method.as_str() {
                "api_key" => Ok(ProviderAuthMethod::ApiKey),
                "openai_chatgpt" => Ok(ProviderAuthMethod::OpenAiChatGpt),
                _ => Err(AppError::new(
                    "persistence",
                    "The saved OpenAI authentication preference is invalid.",
                )),
            })
            .transpose()
    }

    pub fn set_openai_auth_preference(&self, method: ProviderAuthMethod) -> AppResult<()> {
        let method = match method {
            ProviderAuthMethod::ApiKey => "api_key",
            ProviderAuthMethod::OpenAiChatGpt => "openai_chatgpt",
            ProviderAuthMethod::GoogleOAuth => {
                return Err(AppError::new(
                    "invalid_input",
                    "Choose an available OpenAI authentication method.",
                ));
            }
        };
        let connection = self.connection()?;
        connection.execute(
            "INSERT INTO openai_auth_preference(singleton, method) VALUES (1, ?1) ON CONFLICT(singleton) DO UPDATE SET method = excluded.method",
            [method],
        )?;
        connection.execute(
            "DELETE FROM model_catalogs WHERE provider_id = 'openai'",
            [],
        )?;
        Ok(())
    }

    pub fn ensure_selected_openai_account(&self) -> AppResult<Option<String>> {
        let accounts = self.openai_accounts()?;
        if accounts.is_empty() {
            self.connection()?.execute(
                "DELETE FROM openai_account_selection WHERE singleton = 1",
                [],
            )?;
            return Ok(None);
        }
        if let Some(selected) = self.selected_openai_account_id()? {
            if accounts.iter().any(|account| account.id == selected) {
                return Ok(Some(selected));
            }
        }
        let fallback = accounts
            .iter()
            .max_by(|left, right| left.connected_at.cmp(&right.connected_at))
            .map(|account| account.id.as_str())
            .ok_or_else(|| AppError::new("persistence", "No OpenAI account is available."))?;
        self.select_openai_account(fallback)?;
        Ok(Some(fallback.to_owned()))
    }

    pub fn select_openai_account_if_unset(&self, account_id: &str) -> AppResult<()> {
        let selected = self.ensure_selected_openai_account()?;
        if selected.is_none() {
            self.select_openai_account(account_id)?;
        }
        Ok(())
    }

    pub fn select_openai_account(&self, account_id: &str) -> AppResult<()> {
        let account_id = uuid::Uuid::parse_str(account_id)
            .map_err(|_| AppError::new("invalid_input", "Choose a valid OpenAI account."))?
            .to_string();
        if !self
            .openai_accounts()?
            .iter()
            .any(|account| account.id == account_id)
        {
            return Err(AppError::new(
                "account_not_found",
                "This OpenAI account is no longer saved.",
            ));
        }
        let connection = self.connection()?;
        connection.execute(
            "INSERT INTO openai_account_selection(singleton, account_id) VALUES (1, ?1) ON CONFLICT(singleton) DO UPDATE SET account_id = excluded.account_id",
            [&account_id],
        )?;
        // The catalog belongs to the selected account. Never leave the prior
        // account's models available while the new catalog is loading.
        connection.execute(
            "DELETE FROM model_catalogs WHERE provider_id = 'openai'",
            [],
        )?;
        drop(connection);
        self.set_openai_auth_preference(ProviderAuthMethod::OpenAiChatGpt)?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn save_openai_account(&self, account: &OpenAiAccount) -> AppResult<()> {
        self.connection()?.execute(
            "INSERT INTO openai_accounts(id, data) VALUES (?1, ?2) ON CONFLICT(id) DO UPDATE SET data = excluded.data",
            params![account.id, serde_json::to_string(account)?],
        )?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn delete_openai_account(&self, id: &str) -> AppResult<()> {
        let id = uuid::Uuid::parse_str(id)
            .map_err(|_| {
                AppError::new("invalid_input", "The OpenAI account identifier is invalid.")
            })?
            .to_string();
        {
            let connection = self.connection()?;
            connection.execute("DELETE FROM openai_accounts WHERE id = ?1", [&id])?;
            connection.execute(
                "DELETE FROM openai_account_selection WHERE singleton = 1 AND account_id = ?1",
                [&id],
            )?;
            connection.execute(
                "DELETE FROM model_catalogs WHERE provider_id = 'openai'",
                [],
            )?;
        }
        // Removing the active profile picks a safe local fallback without
        // switching to a different credential type.
        self.ensure_selected_openai_account()?;
        Ok(())
    }

    pub fn mcp_servers(&self) -> AppResult<Vec<McpServerConfig>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare("SELECT data FROM mcp_servers ORDER BY id")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }

    pub fn save_mcp_server(&self, server: &McpServerConfig) -> AppResult<()> {
        if server.scope != McpScope::User || server.project_id.is_some() {
            return Err(AppError::new(
                "invalid_mcp_scope",
                "Only user-scoped MCP servers can be stored in application settings.",
            ));
        }
        self.connection()?.execute(
            "INSERT INTO mcp_servers(id, data) VALUES (?1, ?2) ON CONFLICT(id) DO UPDATE SET data = excluded.data",
            params![server.id, serde_json::to_string(server)?],
        )?;
        Ok(())
    }

    pub fn delete_mcp_server(&self, id: &str) -> AppResult<()> {
        let connection = self.connection()?;
        connection.execute("DELETE FROM mcp_servers WHERE id = ?1", [id])?;
        connection.execute(
            "DELETE FROM mcp_trust WHERE scope = 'user' AND project_id = '' AND server_id = ?1",
            [id],
        )?;
        Ok(())
    }

    pub fn mcp_is_trusted(&self, server: &McpServerConfig, fingerprint: &str) -> AppResult<bool> {
        let project_id = server.project_id.as_deref().unwrap_or_default();
        self.connection()?.query_row(
            "SELECT EXISTS(SELECT 1 FROM mcp_trust WHERE scope = ?1 AND project_id = ?2 AND server_id = ?3 AND fingerprint = ?4)",
            params![mcp_scope_name(&server.scope), project_id, server.id, fingerprint],
            |row| row.get(0),
        ).map_err(Into::into)
    }

    pub fn trust_mcp_server(&self, server: &McpServerConfig, fingerprint: &str) -> AppResult<()> {
        let project_id = server.project_id.as_deref().unwrap_or_default();
        self.connection()?.execute(
            "INSERT INTO mcp_trust(scope, project_id, server_id, fingerprint, trusted_at) VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(scope, project_id, server_id) DO UPDATE SET fingerprint = excluded.fingerprint, trusted_at = excluded.trusted_at",
            params![mcp_scope_name(&server.scope), project_id, server.id, fingerprint, now()],
        )?;
        Ok(())
    }

    pub fn revoke_mcp_trust(&self, server: &McpServerConfig) -> AppResult<()> {
        self.connection()?.execute(
            "DELETE FROM mcp_trust WHERE scope = ?1 AND project_id = ?2 AND server_id = ?3",
            params![
                mcp_scope_name(&server.scope),
                server.project_id.as_deref().unwrap_or_default(),
                server.id
            ],
        )?;
        Ok(())
    }

    pub fn has_review_baseline(&self, session_id: &str) -> AppResult<bool> {
        self.connection()?
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM session_review_baselines WHERE session_id = ?1)",
                [session_id],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    pub fn save_review_baseline(&self, baseline: &SessionReviewBaseline) -> AppResult<()> {
        self.connection()?.execute(
            "INSERT OR IGNORE INTO session_review_baselines(session_id, project_id, root, head, statuses, status_available, status_truncated, started_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                baseline.session_id,
                baseline.project_id,
                baseline.root,
                baseline.head,
                serde_json::to_string(&baseline.statuses)?,
                baseline.status_available,
                baseline.status_truncated,
                baseline.started_at,
            ],
        )?;
        Ok(())
    }

    pub fn review_baseline(&self, session_id: &str) -> AppResult<Option<SessionReviewBaseline>> {
        let row = self.connection()?.query_row(
            "SELECT project_id, root, head, statuses, status_available, status_truncated, started_at FROM session_review_baselines WHERE session_id = ?1",
            [session_id],
            |row| Ok((
                row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?, row.get::<_, bool>(4)?, row.get::<_, bool>(5)?, row.get::<_, String>(6)?,
            )),
        ).optional()?;
        row.map(
            |(project_id, root, head, statuses, status_available, status_truncated, started_at)| {
                Ok(SessionReviewBaseline {
                    session_id: session_id.into(),
                    project_id,
                    root,
                    head,
                    statuses: serde_json::from_str(&statuses)?,
                    status_available,
                    status_truncated,
                    started_at,
                })
            },
        )
        .transpose()
    }

    pub fn save_file_checkpoint(&self, checkpoint: &SessionFileCheckpoint) -> AppResult<()> {
        self.connection()?.execute(
            "INSERT OR IGNORE INTO session_file_checkpoints(session_id, path, target_path, existed_before, before_content, before_mode, agent_exists, agent_content, expected_hash, preexisting_status, reviewed) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                checkpoint.session_id, checkpoint.path, checkpoint.target_path,
                checkpoint.existed_before, checkpoint.before_content, checkpoint.before_mode.map(i64::from),
                checkpoint.agent_exists, checkpoint.agent_content, checkpoint.expected_hash, checkpoint.preexisting_status, checkpoint.reviewed,
            ],
        )?;
        Ok(())
    }

    pub fn file_checkpoint(
        &self,
        session_id: &str,
        path: &str,
    ) -> AppResult<Option<SessionFileCheckpoint>> {
        self.connection()?.query_row(
            "SELECT target_path, existed_before, before_content, before_mode, agent_exists, agent_content, expected_hash, preexisting_status, reviewed FROM session_file_checkpoints WHERE session_id = ?1 AND path = ?2",
            params![session_id, path],
            |row| Ok(SessionFileCheckpoint {
                session_id: session_id.into(), path: path.into(), target_path: row.get(0)?,
                existed_before: row.get(1)?, before_content: row.get(2)?,
                before_mode: row.get::<_, Option<i64>>(3)?.and_then(|mode| u32::try_from(mode).ok()),
                agent_exists: row.get(4)?, agent_content: row.get(5)?, expected_hash: row.get(6)?, preexisting_status: row.get(7)?,
                reviewed: row.get(8)?,
            }),
        ).optional().map_err(Into::into)
    }

    pub fn file_checkpoints(&self, session_id: &str) -> AppResult<Vec<SessionFileCheckpoint>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT path, target_path, existed_before, before_content, before_mode, agent_exists, agent_content, expected_hash, preexisting_status, reviewed FROM session_file_checkpoints WHERE session_id = ?1 ORDER BY path",
        )?;
        let checkpoints = statement
            .query_map([session_id], |row| {
                Ok(SessionFileCheckpoint {
                    session_id: session_id.into(),
                    path: row.get(0)?,
                    target_path: row.get(1)?,
                    existed_before: row.get(2)?,
                    before_content: row.get(3)?,
                    before_mode: row
                        .get::<_, Option<i64>>(4)?
                        .and_then(|mode| u32::try_from(mode).ok()),
                    agent_exists: row.get(5)?,
                    agent_content: row.get(6)?,
                    expected_hash: row.get(7)?,
                    preexisting_status: row.get(8)?,
                    reviewed: row.get(9)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(AppError::internal)?;
        Ok(checkpoints)
    }

    pub fn update_file_checkpoint_after(
        &self,
        checkpoint: &SessionFileCheckpoint,
    ) -> AppResult<()> {
        let changed = self.connection()?.execute(
            "UPDATE session_file_checkpoints SET agent_exists = ?3, agent_content = ?4, expected_hash = ?5, reviewed = 0 WHERE session_id = ?1 AND path = ?2",
            params![checkpoint.session_id, checkpoint.path, checkpoint.agent_exists, checkpoint.agent_content, checkpoint.expected_hash],
        )?;
        if changed == 0 {
            return Err(AppError::new(
                "checkpoint_missing",
                "The file review checkpoint is unavailable.",
            ));
        }
        Ok(())
    }

    pub fn remove_file_checkpoint(&self, session_id: &str, path: &str) -> AppResult<()> {
        self.connection()?.execute(
            "DELETE FROM session_file_checkpoints WHERE session_id = ?1 AND path = ?2",
            params![session_id, path],
        )?;
        Ok(())
    }

    pub fn mark_file_checkpoint_reviewed(&self, session_id: &str, path: &str) -> AppResult<()> {
        self.connection()?.execute(
            "UPDATE session_file_checkpoints SET reviewed = 1 WHERE session_id = ?1 AND path = ?2",
            params![session_id, path],
        )?;
        Ok(())
    }

    pub fn mark_all_file_checkpoints_reviewed(&self, session_id: &str) -> AppResult<()> {
        self.connection()?.execute(
            "UPDATE session_file_checkpoints SET reviewed = 1 WHERE session_id = ?1",
            [session_id],
        )?;
        Ok(())
    }

    pub fn clear_review(&self, session_id: &str) -> AppResult<()> {
        self.connection()?.execute(
            "DELETE FROM session_review_baselines WHERE session_id = ?1",
            [session_id],
        )?;
        Ok(())
    }

    pub fn clear_empty_review(&self, session_id: &str) -> AppResult<()> {
        self.connection()?.execute(
            "DELETE FROM session_review_baselines WHERE session_id = ?1 AND NOT EXISTS(SELECT 1 FROM session_file_checkpoints WHERE session_id = ?1)",
            [session_id],
        )?;
        Ok(())
    }

    pub fn permission_mode(&self) -> AppResult<PermissionMode> {
        let value: Option<String> = self
            .connection()?
            .query_row(
                "SELECT value FROM app_preferences WHERE key = 'permission_mode'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        Ok(value
            .and_then(|value| serde_json::from_str(&format!("\"{value}\"")).ok())
            .unwrap_or_default())
    }

    pub fn set_permission_mode(&self, mode: PermissionMode) -> AppResult<()> {
        let value = serde_json::to_value(mode)?;
        self.connection()?.execute(
            "INSERT INTO app_preferences(key, value) VALUES ('permission_mode', ?1) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [value.as_str().unwrap_or("ask")],
        )?;
        Ok(())
    }

    pub fn permission_rules(&self) -> AppResult<Vec<PermissionRule>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare("SELECT fingerprint, data FROM permission_rules ORDER BY created_at DESC")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.map(|row| {
            let (fingerprint, data) = row?;
            let mut rule: PermissionRule = serde_json::from_str(&data)?;
            rule.fingerprint = fingerprint;
            Ok(rule)
        })
        .collect()
    }

    pub fn has_permission_rule(&self, project_id: &str, fingerprint: &str) -> AppResult<bool> {
        let exists: bool = self.connection()?.query_row(
            "SELECT EXISTS(SELECT 1 FROM permission_rules WHERE project_id = ?1 AND fingerprint = ?2)",
            params![project_id, fingerprint],
            |row| row.get(0),
        )?;
        Ok(exists)
    }

    pub fn save_permission_rule(
        &self,
        project_id: &str,
        categories: Vec<PermissionCategory>,
        summary: &str,
        fingerprint: &str,
    ) -> AppResult<PermissionRule> {
        let rule = PermissionRule {
            id: id(),
            project_id: project_id.into(),
            categories,
            summary: summary.chars().take(420).collect(),
            created_at: now(),
            fingerprint: fingerprint.into(),
        };
        self.connection()?.execute(
            "INSERT INTO permission_rules(id, project_id, fingerprint, created_at, data) VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(project_id, fingerprint) DO UPDATE SET id = excluded.id, created_at = excluded.created_at, data = excluded.data",
            params![rule.id, rule.project_id, rule.fingerprint, rule.created_at, serde_json::to_string(&rule)?],
        )?;
        Ok(rule)
    }

    pub fn revoke_permission_rule(&self, id: &str) -> AppResult<()> {
        let changed = self
            .connection()?
            .execute("DELETE FROM permission_rules WHERE id = ?1", [id])?;
        if changed == 0 {
            return Err(AppError::new(
                "not_found",
                "Permission rule no longer exists.",
            ));
        }
        Ok(())
    }

    fn connection(&self) -> AppResult<std::sync::MutexGuard<'_, Connection>> {
        self.0.lock().map_err(AppError::internal)
    }

    fn recover_interrupted(&self) -> AppResult<()> {
        for mut session in self.sessions()? {
            if matches!(
                session.status,
                SessionStatus::Queued | SessionStatus::Planning | SessionStatus::Working
            ) {
                session.status = SessionStatus::Failed;
                session.error =
                    Some("The app closed during this run. Send a new message to continue.".into());
                session.close_pending_tools("The app closed during this tool call.");
                session.updated_at = now();
                self.save_session(&session)?;
            }
        }
        Ok(())
    }

    pub fn projects(&self) -> AppResult<Vec<Project>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare("SELECT data FROM projects ORDER BY is_recent DESC, last_opened_at DESC")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }

    pub fn project(&self, id: &str) -> AppResult<Project> {
        let json: Option<String> = self
            .connection()?
            .query_row("SELECT data FROM projects WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .optional()?;
        json.map(|data| serde_json::from_str(&data).map_err(Into::into))
            .unwrap_or_else(|| {
                Err(AppError::new(
                    "not_found",
                    "Project not found. Open the folder again.",
                ))
            })
    }

    pub fn save_project(&self, project: &Project) -> AppResult<()> {
        self.connection()?.execute("INSERT INTO projects (id, path, created_at, last_opened_at, is_recent, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT(id) DO UPDATE SET path = excluded.path, last_opened_at = excluded.last_opened_at, is_recent = excluded.is_recent, data = excluded.data", params![project.id, project.path, project.created_at, project.last_opened_at, project.is_recent, serde_json::to_string(project)?])?;
        Ok(())
    }

    pub fn remove_project_from_recents(&self, id: &str) -> AppResult<Project> {
        let mut project = self.project(id)?;
        project.is_recent = false;
        self.save_project(&project)?;
        Ok(project)
    }

    pub fn sessions(&self) -> AppResult<Vec<AgentSession>> {
        let connection = self.connection()?;
        let mut statement =
            connection.prepare("SELECT data FROM sessions ORDER BY updated_at DESC")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }

    pub fn session(&self, id: &str) -> AppResult<AgentSession> {
        let json: Option<String> = self
            .connection()?
            .query_row("SELECT data FROM sessions WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .optional()?;
        json.map(|data| serde_json::from_str(&data).map_err(Into::into))
            .unwrap_or_else(|| {
                Err(AppError::new(
                    "not_found",
                    "Session not found. Start a new session.",
                ))
            })
    }

    pub fn save_session(&self, session: &AgentSession) -> AppResult<()> {
        self.connection()?.execute("INSERT INTO sessions (id, project_id, updated_at, data) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(id) DO UPDATE SET updated_at = excluded.updated_at, data = excluded.data", params![session.id, session.project_id, session.updated_at, serde_json::to_string(session)?])?;
        Ok(())
    }

    pub fn delete_session(&self, id: &str) -> AppResult<()> {
        let connection = self.connection()?;
        let removed = connection.execute("DELETE FROM sessions WHERE id = ?1", [id])?;
        if removed == 0 {
            return Err(AppError::new(
                "not_found",
                "This task has already been deleted.",
            ));
        }
        Ok(())
    }

    pub fn save_usage(&self, record: &UsageRecord) -> AppResult<()> {
        self.connection()?.execute(
            "INSERT INTO usage (id, session_id, created_at, data) VALUES (?1, ?2, ?3, ?4)",
            params![
                record.id,
                record.session_id,
                record.created_at,
                serde_json::to_string(record)?
            ],
        )?;
        Ok(())
    }

    pub fn usage(&self) -> AppResult<Vec<UsageRecord>> {
        let connection = self.connection()?;
        let mut statement =
            connection.prepare("SELECT data FROM usage ORDER BY created_at DESC")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }

    pub fn provider_account(&self, provider_id: &str) -> AppResult<Option<ProviderAccount>> {
        let json: Option<String> = self
            .connection()?
            .query_row(
                "SELECT data FROM provider_accounts WHERE provider_id = ?1",
                [provider_id],
                |row| row.get(0),
            )
            .optional()?;
        json.map(|data| serde_json::from_str(&data).map_err(Into::into))
            .transpose()
    }

    pub fn save_provider_account(&self, account: &ProviderAccount) -> AppResult<()> {
        self.connection()?.execute(
            "INSERT INTO provider_accounts (provider_id, data) VALUES (?1, ?2) ON CONFLICT(provider_id) DO UPDATE SET data = excluded.data",
            params![account.provider_id, serde_json::to_string(account)?],
        )?;
        Ok(())
    }

    pub fn delete_provider_account(&self, provider_id: &str) -> AppResult<()> {
        self.connection()?.execute(
            "DELETE FROM provider_accounts WHERE provider_id = ?1",
            [provider_id],
        )?;
        Ok(())
    }

    pub fn model_catalog(&self, provider_id: &str) -> AppResult<Option<Vec<Model>>> {
        let json: Option<String> = self
            .connection()?
            .query_row(
                "SELECT data FROM model_catalogs WHERE provider_id = ?1",
                [provider_id],
                |row| row.get(0),
            )
            .optional()?;
        json.map(|data| serde_json::from_str(&data).map_err(Into::into))
            .transpose()
    }

    pub fn save_model_catalog(&self, provider_id: &str, models: &[Model]) -> AppResult<()> {
        self.connection()?.execute(
            "INSERT INTO model_catalogs (provider_id, data, fetched_at) VALUES (?1, ?2, ?3) ON CONFLICT(provider_id) DO UPDATE SET data = excluded.data, fetched_at = excluded.fetched_at",
            params![provider_id, serde_json::to_string(models)?, now()],
        )?;
        Ok(())
    }

    pub fn model_preferences(&self) -> AppResult<ModelPreferences> {
        let json: Option<String> = self
            .connection()?
            .query_row(
                "SELECT data FROM model_preferences WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        json.map(|data| serde_json::from_str(&data).map_err(Into::into))
            .transpose()
            .map(|preferences| preferences.unwrap_or_default())
    }

    pub fn save_model_preferences(&self, preferences: &ModelPreferences) -> AppResult<()> {
        self.connection()?.execute(
            "INSERT INTO model_preferences (id, data) VALUES (1, ?1) ON CONFLICT(id) DO UPDATE SET data = excluded.data",
            [serde_json::to_string(preferences)?],
        )?;
        Ok(())
    }

    pub fn record_model_used(&self, model: &ModelReference) -> AppResult<ModelPreferences> {
        let mut preferences = self.model_preferences()?;
        preferences.recent.retain(|item| item != model);
        preferences.recent.insert(0, model.clone());
        preferences.recent.truncate(8);
        self.save_model_preferences(&preferences)?;
        Ok(preferences)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_catalog_and_preferences_survive_database_reopen() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("models.sqlite");
        let model: Model = serde_json::from_value(serde_json::json!({
            "id": "catalog-model",
            "provider": "opencode-zen",
            "displayName": "Catalog model",
            "capabilities": ["tools", "streaming"],
            "supportsTools": true,
            "supportsVision": false,
            "supportsReasoning": false,
            "supportsStreaming": true,
            "contextWindow": 128000,
            "inputPrice": null,
            "outputPrice": null,
            "status": "available"
        }))
        .unwrap();
        let selection = ModelReference {
            provider_id: "opencode-zen".into(),
            model_id: "catalog-model".into(),
        };
        {
            let database = Database::open(&path).unwrap();
            database
                .save_model_catalog("opencode-zen", std::slice::from_ref(&model))
                .unwrap();
            database
                .save_model_preferences(&ModelPreferences {
                    default_model: Some(selection.clone()),
                    favorites: vec![selection.clone()],
                    recent: vec![],
                })
                .unwrap();
            let preferences = database.record_model_used(&selection).unwrap();
            assert_eq!(preferences.recent, vec![selection.clone()]);
        }
        let database = Database::open(&path).unwrap();
        assert_eq!(
            database.model_catalog("opencode-zen").unwrap().unwrap(),
            vec![model]
        );
        let preferences = database.model_preferences().unwrap();
        assert_eq!(preferences.default_model, Some(selection.clone()));
        assert_eq!(preferences.favorites, vec![selection.clone()]);
        assert_eq!(preferences.recent, vec![selection]);
    }

    #[test]
    fn openai_host_identity_survives_database_reopen() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("openai-host.sqlite");
        let first = Database::open(&path)
            .unwrap()
            .openai_ext_agent_host_id()
            .unwrap();
        assert!(first.starts_with("urn:uuid:"));
        {
            let reopened = Database::open(&path).unwrap();
            assert_eq!(reopened.openai_ext_agent_host_id().unwrap(), first);
        }
    }

    #[test]
    fn openai_profile_metadata_survives_reopen_without_oauth_tokens() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("openai-auth.sqlite");
        let account = OpenAiAccount {
            id: "safe-account-id".into(),
            connected: true,
            access_token_valid: true,
            requires_reconnect: false,
            display_name: Some("Example".into()),
            email: Some("user@example.test".into()),
            organization: None,
            client_id: Some("issued-client-id".into()),
            granted_scopes: vec!["openid".into(), "profile".into()],
            connected_at: "2026-01-01T00:00:00Z".into(),
            expires_at: None,
        };
        {
            let database = Database::open(&path).unwrap();
            database.save_openai_account(&account).unwrap();
        }
        let database = Database::open(&path).unwrap();
        assert_eq!(database.openai_accounts().unwrap(), vec![account]);
        let connection = Connection::open(&path).unwrap();
        let persisted: String = connection
            .query_row("SELECT data FROM openai_accounts", [], |row| row.get(0))
            .unwrap();
        let persisted: serde_json::Value = serde_json::from_str(&persisted).unwrap();
        let account = persisted.as_object().unwrap();
        for token_field in ["access_token", "refresh_token", "id_token"] {
            assert!(!account.contains_key(token_field));
        }
    }

    #[test]
    fn openai_account_selection_survives_reopen_and_is_removed_with_its_catalog() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("openai-selection.sqlite");
        let make_account = |id: String, opened: &str| OpenAiAccount {
            id,
            connected: true,
            access_token_valid: true,
            requires_reconnect: false,
            display_name: None,
            email: Some("same@example.test".into()),
            organization: None,
            client_id: Some("issued-client-id".into()),
            granted_scopes: vec!["openid".into()],
            connected_at: opened.into(),
            expires_at: None,
        };
        let first = make_account(uuid::Uuid::new_v4().to_string(), "2026-10-12T10:00:00Z");
        let second = make_account(uuid::Uuid::new_v4().to_string(), "2026-10-13T10:00:00Z");
        let model: Model = serde_json::from_value(serde_json::json!({
            "id": "account-model",
            "provider": "openai",
            "displayName": "Account model",
            "capabilities": ["tools"],
            "supportsTools": true,
            "supportsVision": false,
            "supportsReasoning": false,
            "supportsStreaming": true,
            "contextWindow": null,
            "inputPrice": null,
            "outputPrice": null,
            "status": "available"
        }))
        .unwrap();
        {
            let database = Database::open(&path).unwrap();
            database.save_openai_account(&first).unwrap();
            database.save_openai_account(&second).unwrap();
            database.select_openai_account(&first.id).unwrap();
            database
                .set_openai_auth_preference(ProviderAuthMethod::ApiKey)
                .unwrap();
            database.save_model_catalog("openai", &[model]).unwrap();
        }

        {
            let database = Database::open(&path).unwrap();
            assert_eq!(
                database
                    .ensure_selected_openai_account()
                    .unwrap()
                    .as_deref(),
                Some(first.id.as_str())
            );
            assert_eq!(
                database.openai_auth_preference().unwrap(),
                Some(ProviderAuthMethod::ApiKey)
            );
            database.select_openai_account(&second.id).unwrap();
            assert!(database.model_catalog("openai").unwrap().is_none());
        }

        let database = Database::open(&path).unwrap();
        assert_eq!(
            database
                .ensure_selected_openai_account()
                .unwrap()
                .as_deref(),
            Some(second.id.as_str())
        );
        assert_eq!(
            database.openai_auth_preference().unwrap(),
            Some(ProviderAuthMethod::OpenAiChatGpt)
        );
        database.delete_openai_account(&second.id).unwrap();
        assert_eq!(
            database
                .ensure_selected_openai_account()
                .unwrap()
                .as_deref(),
            Some(first.id.as_str())
        );
    }

    #[test]
    fn permission_modes_and_revocable_project_rules_survive_reopening() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("permissions.sqlite");
        let fingerprint = "0f8a4d6d71a2".to_owned();
        let rule_id;
        {
            let database = Database::open(&path).unwrap();
            let project = Project {
                id: "permission-project".into(),
                workspace_id: "local".into(),
                name: "Permission sample".into(),
                path: root.path().display().to_string(),
                repository_root: None,
                active_branch: None,
                last_opened_at: now(),
                project_instructions: String::new(),
                preferred_model: None,
                permissions: PermissionPolicy::default(),
                is_recent: true,
                created_at: now(),
            };
            database.save_project(&project).unwrap();
            database
                .set_permission_mode(PermissionMode::WorkspaceWrite)
                .unwrap();
            let rule = database
                .save_permission_rule(
                    &project.id,
                    vec![PermissionCategory::Command],
                    "npm test",
                    &fingerprint,
                )
                .unwrap();
            rule_id = rule.id.clone();
            assert!(!serde_json::to_string(&rule).unwrap().contains(&fingerprint));
        }
        let reopened = Database::open(&path).unwrap();
        assert_eq!(
            reopened.permission_mode().unwrap(),
            PermissionMode::WorkspaceWrite
        );
        assert!(reopened
            .has_permission_rule("permission-project", &fingerprint)
            .unwrap());
        let rules = reopened.permission_rules().unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].summary, "npm test");
        reopened.revoke_permission_rule(&rule_id).unwrap();
        assert!(!reopened
            .has_permission_rule("permission-project", &fingerprint)
            .unwrap());
    }

    #[test]
    fn interrupted_runs_close_tool_calls_before_retry() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("test.sqlite");
        let database = Database::open(&path).unwrap();
        database
            .save_project(&Project {
                id: "test-project".into(),
                workspace_id: "local".into(),
                name: "test".into(),
                path: root.path().display().to_string(),
                repository_root: None,
                active_branch: None,
                last_opened_at: now(),
                project_instructions: String::new(),
                preferred_model: None,
                permissions: PermissionPolicy::default(),
                is_recent: true,
                created_at: now(),
            })
            .unwrap();
        let mut session: AgentSession =
            serde_json::from_str(include_str!("../../../tests/fixtures/session.json")).unwrap();
        session.status = SessionStatus::Working;
        database.save_session(&session).unwrap();
        drop(database);
        let reopened = Database::open(&path).unwrap();
        let recovered = reopened.session(&session.id).unwrap();
        assert_eq!(recovered.status, SessionStatus::Failed);
        assert!(recovered.pending_tool_call.is_none());
        assert_eq!(
            recovered
                .messages
                .last()
                .unwrap()
                .tool_result
                .as_ref()
                .unwrap()
                .tool_call_id,
            "call"
        );
        assert!(
            recovered
                .messages
                .last()
                .unwrap()
                .tool_result
                .as_ref()
                .unwrap()
                .is_error
        );
    }

    #[test]
    fn task_context_and_usage_roundtrip_and_usage_survives_task_deletion() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("tasks.sqlite");
        let mut session: AgentSession =
            serde_json::from_str(include_str!("../../../tests/fixtures/session.json")).unwrap();
        session.archived_at = Some(now());
        session.git_branch = Some("feature/task-history".into());
        session.workspace_mode = crate::domain::WorkspaceMode::Isolated;
        session.base_branch = Some("main".into());
        session.worktree_path = Some(root.path().join("task-worktree").display().to_string());
        session.project_instruction_files = vec!["AGENTS.md".into()];
        session.working_context = WorkingContext {
            objective: "Preserve this task objective.".into(),
            protected_instructions: vec!["Keep the API stable.".into()],
            compacted_turns: 3,
            ..WorkingContext::default()
        };
        {
            let database = Database::open(&path).unwrap();
            database
                .save_project(&Project {
                    id: session.project_id.clone(),
                    workspace_id: "local".into(),
                    name: "tasks".into(),
                    path: root.path().display().to_string(),
                    repository_root: None,
                    active_branch: None,
                    last_opened_at: now(),
                    project_instructions: String::new(),
                    preferred_model: None,
                    permissions: PermissionPolicy::default(),
                    is_recent: true,
                    created_at: now(),
                })
                .unwrap();
            database.save_session(&session).unwrap();
            database
                .save_usage(&UsageRecord {
                    id: "usage-test".into(),
                    session_id: session.id.clone(),
                    session_title: "Persist usage task".into(),
                    project_id: session.project_id.clone(),
                    provider_id: session.provider_id.clone(),
                    model_id: session.model_id.clone(),
                    input_tokens: 3,
                    usage_available: true,
                    cached_input_tokens: None,
                    output_tokens: 2,
                    cost_usd: None,
                    duration_ms: 10,
                    success: true,
                    failure_code: None,
                    created_at: now(),
                })
                .unwrap();
        }
        let database = Database::open(&path).unwrap();
        let reopened = database.session(&session.id).unwrap();
        assert_eq!(
            reopened.working_context.protected_instructions,
            vec!["Keep the API stable."]
        );
        assert_eq!(reopened.working_context.compacted_turns, 3);
        assert_eq!(reopened.archived_at, session.archived_at);
        assert_eq!(reopened.git_branch.as_deref(), Some("feature/task-history"));
        assert_eq!(
            reopened.workspace_mode,
            crate::domain::WorkspaceMode::Isolated
        );
        assert_eq!(reopened.base_branch.as_deref(), Some("main"));
        assert_eq!(reopened.worktree_path, session.worktree_path);
        assert_eq!(reopened.project_instruction_files, vec!["AGENTS.md"]);
        let stored_usage = database.usage().unwrap();
        assert_eq!(stored_usage.len(), 1);
        assert_eq!(stored_usage[0].session_title, "Persist usage task");
        database.delete_session(&session.id).unwrap();
        assert!(database.sessions().unwrap().is_empty());
        let retained_usage = database.usage().unwrap();
        assert_eq!(retained_usage.len(), 1);
        assert_eq!(retained_usage[0].id, "usage-test");
    }

    #[test]
    fn legacy_usage_table_migrates_without_losing_history() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("legacy-usage.sqlite");
        let session: AgentSession =
            serde_json::from_str(include_str!("../../../tests/fixtures/session.json")).unwrap();
        let project = Project {
            id: session.project_id.clone(),
            workspace_id: "local".into(),
            name: "legacy usage".into(),
            path: root.path().display().to_string(),
            repository_root: None,
            active_branch: None,
            last_opened_at: now(),
            project_instructions: String::new(),
            preferred_model: None,
            permissions: PermissionPolicy::default(),
            is_recent: true,
            created_at: now(),
        };
        let record = UsageRecord {
            id: "legacy-usage-record".into(),
            session_id: session.id.clone(),
            session_title: session.title.clone(),
            project_id: session.project_id.clone(),
            provider_id: session.provider_id.clone(),
            model_id: session.model_id.clone(),
            input_tokens: 12,
            usage_available: true,
            cached_input_tokens: None,
            output_tokens: 5,
            cost_usd: None,
            duration_ms: 42,
            success: true,
            failure_code: None,
            created_at: now(),
        };
        {
            let connection = Connection::open(&path).unwrap();
            connection
                .execute_batch(
                    "PRAGMA foreign_keys = ON;
                     CREATE TABLE projects (id TEXT PRIMARY KEY, path TEXT NOT NULL UNIQUE, created_at TEXT NOT NULL, last_opened_at TEXT NOT NULL DEFAULT '', is_recent INTEGER NOT NULL DEFAULT 1, data TEXT NOT NULL);
                     CREATE TABLE sessions (id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES projects(id), updated_at TEXT NOT NULL, data TEXT NOT NULL);
                     CREATE TABLE usage (id TEXT PRIMARY KEY, session_id TEXT NOT NULL REFERENCES sessions(id), created_at TEXT NOT NULL, data TEXT NOT NULL);
                     CREATE INDEX usage_session ON usage(session_id);",
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO projects (id, path, created_at, last_opened_at, is_recent, data) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![project.id, project.path, project.created_at, project.last_opened_at, project.is_recent, serde_json::to_string(&project).unwrap()],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO sessions (id, project_id, updated_at, data) VALUES (?1, ?2, ?3, ?4)",
                    params![session.id, session.project_id, session.updated_at, serde_json::to_string(&session).unwrap()],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO usage (id, session_id, created_at, data) VALUES (?1, ?2, ?3, ?4)",
                    params![
                        record.id,
                        record.session_id,
                        record.created_at,
                        serde_json::to_string(&record).unwrap()
                    ],
                )
                .unwrap();
        }

        let database = Database::open(&path).unwrap();
        let migrated_usage = database.usage().unwrap();
        assert_eq!(migrated_usage.len(), 1);
        assert_eq!(migrated_usage[0].id, record.id);
        assert_eq!(migrated_usage[0].session_title, record.session_title);
        let connection = database.connection().unwrap();
        let mut statement = connection
            .prepare("PRAGMA foreign_key_list(usage)")
            .unwrap();
        assert!(statement.query([]).unwrap().next().unwrap().is_none());
    }

    #[test]
    fn projects_survive_reopening_and_unknown_ids_fail() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.sqlite");
        let db = Database::open(&path).unwrap();
        db.save_project(&Project {
            id: "p".into(),
            workspace_id: "local".into(),
            name: "test".into(),
            path: dir.path().display().to_string(),
            repository_root: None,
            active_branch: None,
            last_opened_at: now(),
            project_instructions: String::new(),
            preferred_model: None,
            permissions: PermissionPolicy::default(),
            is_recent: true,
            created_at: now(),
        })
        .unwrap();
        drop(db);
        let reopened = Database::open(&path).unwrap();
        assert_eq!(reopened.project("p").unwrap().name, "test");
        assert_eq!(reopened.project("missing").unwrap_err().code, "not_found");
    }

    #[test]
    fn provider_account_metadata_survives_reopening_without_secret_fields() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("accounts.sqlite");
        let db = Database::open(&path).unwrap();
        let account = ProviderAccount {
            provider_id: "openai".into(),
            provider_name: "OpenAI".into(),
            state: ProviderAuthState::Connected,
            auth_method: Some(ProviderAuthMethod::ApiKey),
            account_label: Some("API key".into()),
            connected_at: Some(now()),
            last_validated_at: Some(now()),
            last_error_code: None,
            available_methods: Vec::new(),
        };
        db.save_provider_account(&account).unwrap();
        drop(db);

        let reopened = Database::open(&path).unwrap();
        assert_eq!(
            reopened
                .provider_account("openai")
                .unwrap()
                .unwrap()
                .provider_id,
            "openai"
        );
        let stored: String = reopened
            .connection()
            .unwrap()
            .query_row(
                "SELECT data FROM provider_accounts WHERE provider_id = 'openai'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!stored.to_lowercase().contains("secret"));
    }

    #[test]
    fn legacy_project_rows_migrate_to_the_new_metadata_shape() {
        let root = tempfile::tempdir_in(std::env::current_dir().unwrap().join("target")).unwrap();
        let path = root.path().join("legacy.sqlite");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch("CREATE TABLE projects (id TEXT PRIMARY KEY, path TEXT NOT NULL UNIQUE, created_at TEXT NOT NULL, data TEXT NOT NULL);")
            .unwrap();
        let created_at = now();
        let legacy_data = serde_json::json!({
            "id": "legacy", "workspaceId": "local", "name": "Legacy", "path": "C:/legacy", "createdAt": created_at
        });
        connection
            .execute(
                "INSERT INTO projects (id, path, created_at, data) VALUES (?1, ?2, ?3, ?4)",
                params!["legacy", "C:/legacy", created_at, legacy_data.to_string()],
            )
            .unwrap();
        drop(connection);

        let reopened = Database::open(&path).unwrap();
        let project = reopened.project("legacy").unwrap();
        assert_eq!(project.name, "Legacy");
        assert!(project.last_opened_at.len() > 10);
        assert!(project.is_recent);
        assert!(project.repository_root.is_none());
        assert_eq!(project.permissions.read_files, PermissionDecision::Allow);
    }
}
