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
        let database = Self(Mutex::new(connection));
        database.recover_interrupted()?;
        Ok(database)
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
        let mut statement = connection.prepare("SELECT data FROM projects ORDER BY created_at")?;
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
        self.connection()?.execute("INSERT INTO projects (id, path, created_at, data) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(id) DO UPDATE SET data = excluded.data", params![project.id, project.path, project.created_at, serde_json::to_string(project)?])?;
        Ok(())
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
}

#[cfg(test)]
mod tests {
    use super::*;
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
            created_at: now(),
        })
        .unwrap();
        drop(db);
        let reopened = Database::open(&path).unwrap();
        assert_eq!(reopened.project("p").unwrap().name, "test");
        assert_eq!(reopened.project("missing").unwrap_err().code, "not_found");
    }
}
