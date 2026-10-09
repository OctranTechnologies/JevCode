PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = 5000;
CREATE TABLE IF NOT EXISTS projects (
    id TEXT PRIMARY KEY,
    path TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL,
    last_opened_at TEXT NOT NULL DEFAULT '',
    is_recent INTEGER NOT NULL DEFAULT 1,
    data TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id),
    updated_at TEXT NOT NULL,
    data TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS usage (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    created_at TEXT NOT NULL,
    data TEXT NOT NULL
);
-- This table contains only non-secret account state. Credential bytes belong in
-- the platform keychain, never in SQLite or serialized application settings.
CREATE TABLE IF NOT EXISTS provider_accounts (
    provider_id TEXT PRIMARY KEY,
    data TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS model_catalogs (
    provider_id TEXT PRIMARY KEY,
    data TEXT NOT NULL,
    fetched_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS model_preferences (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    data TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS app_preferences (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS permission_rules (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    fingerprint TEXT NOT NULL,
    created_at TEXT NOT NULL,
    data TEXT NOT NULL,
    UNIQUE(project_id, fingerprint)
);
CREATE INDEX IF NOT EXISTS permission_rules_project ON permission_rules(project_id, created_at DESC);
CREATE INDEX IF NOT EXISTS sessions_project ON sessions(project_id, updated_at);
CREATE INDEX IF NOT EXISTS usage_session ON usage(session_id);
CREATE TABLE IF NOT EXISTS session_review_baselines (
    session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
    project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    root TEXT NOT NULL,
    head TEXT,
    statuses TEXT NOT NULL,
    status_available INTEGER NOT NULL,
    status_truncated INTEGER NOT NULL,
    started_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS session_file_checkpoints (
    session_id TEXT NOT NULL REFERENCES session_review_baselines(session_id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    target_path TEXT NOT NULL,
    existed_before INTEGER NOT NULL,
    before_content BLOB,
    before_mode INTEGER,
    agent_exists INTEGER NOT NULL,
    agent_content BLOB,
    expected_hash TEXT,
    preexisting_status TEXT,
    reviewed INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY(session_id, path)
);
CREATE INDEX IF NOT EXISTS session_file_checkpoints_session ON session_file_checkpoints(session_id, path);
PRAGMA user_version = 8;
