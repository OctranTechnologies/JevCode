use crate::{
    domain::*,
    error::{AppError, AppResult},
};
use rusqlite::{params, Connection, OptionalExtension};
use std::{path::Path, sync::Mutex};

pub struct Database(Mutex<Connection>);

impl Database {
    pub fn open(path: &Path) -> AppResult<Self> {
        let connection = Connection::open(path)?;
        connection.execute_batch(include_str!("schema.sql"))?;
        Self::migrate_projects(&connection)?;
        Self::migrate_model_management(&connection)?;
        let database = Self(Mutex::new(connection));
        database.recover_interrupted()?;
        // Re-serialize legacy JSON rows once so newly added metadata survives the
        // next restart even when the database predates the project workspace UI.
        for project in database.projects()? {
            database.save_project(&project)?;
        }
        Ok(database)
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

    fn connection(&self) -> AppResult<std::sync::MutexGuard<'_, Connection>> {
        self.0.lock().map_err(AppError::internal)
    }

    fn recover_interrupted(&self) -> AppResult<()> {
        for mut session in self.sessions()? {
            if session.status == SessionStatus::Running {
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
        session.status = SessionStatus::Running;
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
