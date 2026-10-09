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
        Self::migrate_permissions(&connection)?;
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
