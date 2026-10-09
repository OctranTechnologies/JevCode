//! MCP client and configuration boundary. Server processes and HTTP requests
//! are created only after an explicit trust action; secrets never enter config
//! files, SQLite, tool descriptions, or logs.
use crate::{
    credentials::CredentialStore,
    domain::{
        McpConnectionStatus, McpScope, McpServerConfig, McpServerView, McpTransport, Tool,
        ToolCall, ToolCategory, ToolResult, ToolRiskLevel,
    },
    error::{AppError, AppResult},
    extensions::ToolIntegration,
    persistence::Database,
};
use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::{header, Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::Mutex,
};

const MAX_TOOLS_PER_SERVER: usize = 50;
const MAX_SCHEMA_BYTES: usize = 64 * 1024;
const MAX_SCHEMA_DEPTH: usize = 16;
const MAX_SCHEMA_NODES: usize = 2048;
const MAX_RESULT_BYTES: usize = 64 * 1024;
const MAX_HTTP_BODY_BYTES: usize = 1024 * 1024;
const MCP_PROTOCOL_VERSION: &str = "2026-07-28";

#[derive(Debug, Deserialize, Serialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProjectMcpFile {
    #[serde(default)]
    mcp_servers: Vec<ProjectMcpServer>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProjectMcpServer {
    id: String,
    name: String,
    transport: McpTransport,
    #[serde(default)]
    env_names: Vec<String>,
    #[serde(default)]
    has_auth_token: bool,
    #[serde(default)]
    enabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SaveMcpServerInput {
    pub config: McpServerConfig,
    #[serde(default)]
    pub secrets: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpServerActionInput {
    pub scope: McpScope,
    pub project_id: Option<String>,
    pub server_id: String,
}

#[derive(Clone, Default)]
pub struct McpManager {
    connections: Arc<Mutex<HashMap<String, Arc<Mutex<LiveServer>>>>>,
    errors: Arc<Mutex<HashMap<String, String>>>,
}

struct LiveServer {
    config: McpServerConfig,
    tools: Vec<RemoteTool>,
    connection: McpConnection,
    secrets: Vec<String>,
}

#[derive(Clone)]
struct RemoteTool {
    local_name: String,
    remote_name: String,
    description: String,
    input_schema: Value,
    public_schema: Value,
}

enum McpConnection {
    Stdio(Box<StdioClient>),
    Http(HttpClient),
}

struct StdioClient {
    _child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

struct HttpClient {
    client: Client,
    url: Url,
    bearer: Option<String>,
    session_id: Option<String>,
    next_id: u64,
}

impl McpManager {
    pub async fn list(
        &self,
        database: &Database,
        credentials: &CredentialStore,
        project_root: Option<&Path>,
        project_id: Option<&str>,
    ) -> AppResult<Vec<McpServerView>> {
        let mut configs = database.mcp_servers()?;
        if let (Some(root), Some(project_id)) = (project_root, project_id) {
            configs.extend(read_project_servers(root, project_id)?);
        }
        let connections = self.connections.lock().await;
        let errors = self.errors.lock().await;
        configs
            .into_iter()
            .map(|config| {
                let key = server_key(&config);
                let trusted = database.mcp_is_trusted(&config, &config_fingerprint(&config)?)?;
                let live = connections.get(&key);
                let status = if !config.enabled {
                    McpConnectionStatus::Disabled
                } else if !trusted {
                    McpConnectionStatus::Untrusted
                } else if live.is_some() {
                    McpConnectionStatus::Connected
                } else if errors.contains_key(&key) {
                    McpConnectionStatus::Error
                } else {
                    McpConnectionStatus::Disconnected
                };
                let available_tools = if let Some(live) = live {
                    live.try_lock()
                        .map(|server| {
                            server
                                .tools
                                .iter()
                                .map(|tool| {
                                    let mut name = tool.remote_name.clone();
                                    for secret in &server.secrets {
                                        if !secret.is_empty() {
                                            name = name.replace(secret, "[redacted]");
                                        }
                                    }
                                    name
                                })
                                .collect()
                        })
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                let last_error = errors.get(&key).cloned();
                let _ = credentials; // Presence is intentionally not disclosed by listing.
                Ok(McpServerView {
                    config,
                    status,
                    trusted,
                    available_tools,
                    permission_summary: "External command + network; each call needs approval"
                        .into(),
                    last_error,
                })
            })
            .collect()
    }

    pub async fn save(
        &self,
        database: &Database,
        credentials: &CredentialStore,
        project_root: Option<&Path>,
        input: SaveMcpServerInput,
    ) -> AppResult<McpServerConfig> {
        let mut config = input.config;
        validate_config(&mut config)?;
        let previous = resolve_config(
            database,
            project_root,
            config.project_id.as_deref(),
            &config.scope,
            &config.id,
        )
        .ok();
        if input
            .secrets
            .keys()
            .any(|key| !config.env_names.iter().any(|env| env == key) && key != "__AUTH_TOKEN")
            || input
                .secrets
                .keys()
                .any(|key| !valid_env_name(key) && key != "__AUTH_TOKEN")
        {
            return Err(AppError::new(
                "invalid_mcp_secret",
                "Secret values must match declared environment names.",
            ));
        }
        if input.secrets.contains_key("__AUTH_TOKEN") && !config.has_auth_token {
            return Err(AppError::new(
                "invalid_mcp_secret",
                "Enable HTTP authentication before saving an authorization token.",
            ));
        }
        let public_config = serde_json::to_string(&config)?;
        if input
            .secrets
            .values()
            .any(|secret| secret.len() >= 8 && public_config.contains(secret))
        {
            return Err(AppError::new(
                "invalid_mcp_secret",
                "Keep credential values in the secret fields; do not include them in the server configuration.",
            ));
        }
        for (name, secret) in input.secrets {
            if secret.trim().is_empty() || secret.len() > 16_384 {
                return Err(AppError::new(
                    "invalid_mcp_secret",
                    "MCP credentials must be non-empty and smaller than 16 KiB.",
                ));
            }
            credentials.save(&credential_key(&config, &name), &secret)?;
        }
        if let Some(previous) = previous {
            for name in &previous.env_names {
                if !config.env_names.contains(name) {
                    credentials.delete(&credential_key(&previous, name))?;
                }
            }
            if previous.has_auth_token && !config.has_auth_token {
                credentials.delete(&credential_key(&previous, "__AUTH_TOKEN"))?;
            }
        }
        // Configuration changes must be explicitly re-enabled, re-trusted, and
        // reconnected. This prevents a stale trust decision from carrying over.
        config.enabled = false;
        self.stop(&config).await;
        match &config.scope {
            McpScope::User => database.save_mcp_server(&config)?,
            McpScope::Project => {
                let root = project_root.ok_or_else(|| {
                    AppError::new(
                        "project_required",
                        "Select a project before saving project MCP configuration.",
                    )
                })?;
                write_project_server(
                    root,
                    config.project_id.as_deref().unwrap_or_default(),
                    &config,
                )?;
            }
        }
        database.revoke_mcp_trust(&config)?;
        Ok(config)
    }

    pub async fn set_enabled(
        &self,
        database: &Database,
        root: Option<&Path>,
        project_id: Option<&str>,
        scope: McpScope,
        server_id: &str,
        enabled: bool,
    ) -> AppResult<()> {
        let mut config = resolve_config(database, root, project_id, &scope, server_id)?;
        config.enabled = enabled;
        validate_config(&mut config)?;
        self.stop(&config).await;
        save_config(database, root, &config)?;
        Ok(())
    }

    pub async fn delete(
        &self,
        database: &Database,
        credentials: &CredentialStore,
        root: Option<&Path>,
        project_id: Option<&str>,
        scope: McpScope,
        server_id: &str,
    ) -> AppResult<()> {
        let config = resolve_config(database, root, project_id, &scope, server_id)?;
        self.stop(&config).await;
        for name in &config.env_names {
            credentials.delete(&credential_key(&config, name))?;
        }
        if config.has_auth_token {
            credentials.delete(&credential_key(&config, "__AUTH_TOKEN"))?;
        }
        match &config.scope {
            McpScope::User => database.delete_mcp_server(&config.id)?,
            McpScope::Project => remove_project_server(
                root.ok_or_else(|| {
                    AppError::new(
                        "project_required",
                        "Select a project before editing project MCP configuration.",
                    )
                })?,
                project_id.unwrap_or_default(),
                &config.id,
            )?,
        }
        database.revoke_mcp_trust(&config)
    }

    pub async fn connect(
        &self,
        database: &Database,
        credentials: &CredentialStore,
        root: Option<&Path>,
        action: McpServerActionInput,
        trust: bool,
    ) -> AppResult<McpServerView> {
        let project_id = action.project_id.as_deref();
        let mut config =
            resolve_config(database, root, project_id, &action.scope, &action.server_id)?;
        validate_config(&mut config)?;
        if !config.enabled {
            return Err(AppError::new(
                "mcp_disabled",
                "Enable this MCP server before connecting.",
            ));
        }
        let fingerprint = config_fingerprint(&config)?;
        if trust {
            database.trust_mcp_server(&config, &fingerprint)?;
        } else if !database.mcp_is_trusted(&config, &fingerprint)? {
            return Err(AppError::new(
                "mcp_untrusted",
                "This server configuration is not trusted. Review it, then choose Trust & Connect.",
            ));
        }
        self.stop(&config).await;
        let key = server_key(&config);
        self.errors.lock().await.remove(&key);
        let secrets = load_secrets(credentials, &config)?;
        let secret_values = secrets.values().cloned().collect::<Vec<_>>();
        let serialized_config = serde_json::to_string(&config)?;
        if secret_values
            .iter()
            .any(|secret| secret.len() >= 8 && serialized_config.contains(secret))
        {
            return Err(AppError::new(
                "invalid_mcp_secret",
                "Project MCP configuration contains a credential value. Replace it with an OS-stored secret reference.",
            ));
        }
        let connected = connect_server(config.clone(), secrets, root).await;
        match connected {
            Ok(live) => {
                self.connections
                    .lock()
                    .await
                    .insert(key, Arc::new(Mutex::new(live)));
            }
            Err(error) => {
                let message = redact_secrets(error.message, &secret_values);
                self.errors.lock().await.insert(key, message.clone());
                return Err(AppError::new(&error.code, message));
            }
        }
        let views = self.list(database, credentials, root, project_id).await?;
        views
            .into_iter()
            .find(|view| {
                view.config.scope == action.scope
                    && view.config.project_id.as_deref() == project_id
                    && view.config.id == action.server_id
            })
            .ok_or_else(|| {
                AppError::new(
                    "mcp_not_found",
                    "The MCP server could not be loaded after connecting.",
                )
            })
    }

    pub async fn disconnect(&self, config: &McpServerConfig) {
        self.stop(config).await;
    }

    async fn stop(&self, config: &McpServerConfig) {
        let key = server_key(config);
        if let Some(live) = self.connections.lock().await.remove(&key) {
            let mut server = live.lock().await;
            if let McpConnection::Stdio(client) = &mut server.connection {
                let _ = client._child.kill().await;
                let _ = client._child.wait().await;
            }
            server.secrets.clear();
        }
    }
}

#[async_trait]
impl ToolIntegration for McpManager {
    fn owns(&self, name: &str) -> bool {
        name.starts_with("mcp__")
    }

    async fn definitions(&self, project_id: &str) -> AppResult<Vec<Tool>> {
        let connections = self.connections.lock().await;
        let servers: Vec<_> = connections
            .iter()
            .filter(|(key, _)| {
                key.starts_with(&format!("project|{project_id}|")) || key.starts_with("user||")
            })
            .map(|(_, live)| live.clone())
            .collect();
        drop(connections);
        let mut result = Vec::new();
        for server in servers {
            let server = server.lock().await;
            result.extend(server.tools.iter().map(|tool| {
                Tool {
                    name: tool.local_name.clone(),
                    description: format!(
                        "External MCP tool from {}: {}",
                        server.config.name, tool.description
                    )
                    .chars()
                    .take(2000)
                    .collect(),
                    permission: ToolCategory::Shell,
                    risk_level: ToolRiskLevel::High,
                    timeout_ms: 30_000,
                    parallel_safe: false,
                    input_schema: tool.public_schema.clone(),
                }
            }));
        }
        Ok(result)
    }

    async fn execute(&self, project_id: &str, call: &ToolCall) -> ToolResult {
        let error_result = |message: &str| ToolResult {
            tool_call_id: call.id.clone(),
            name: call.name.clone(),
            content: message.into(),
            is_error: true,
            duration_ms: 0,
            structured_content: None,
        };
        let connections = self.connections.lock().await;
        let selected = connections.values().find_map(|server| {
            let live = server.try_lock().ok()?;
            let in_scope = live.config.scope == McpScope::User
                || live.config.project_id.as_deref() == Some(project_id);
            let owns = live.tools.iter().any(|tool| tool.local_name == call.name);
            (in_scope && owns).then(|| server.clone())
        });
        drop(connections);
        let Some(server) = selected else {
            return error_result(
                "The MCP server is disconnected or its tool is no longer available.",
            );
        };
        let mut server = server.lock().await;
        let Some(tool) = server
            .tools
            .iter()
            .find(|tool| tool.local_name == call.name)
            .cloned()
        else {
            return error_result("The MCP tool is no longer available.");
        };
        if let Err(error) =
            crate::tools::validate_external_schema(&call.arguments, &tool.input_schema)
        {
            return error_result(&error.message);
        }
        let started = std::time::Instant::now();
        let params = json!({ "name": tool.remote_name, "arguments": call.arguments });
        let result = server.connection.request("tools/call", Some(params)).await;
        match result {
            Ok(value) => {
                let failed = value["isError"].as_bool().unwrap_or(false);
                let mut content = remote_content(&value);
                for secret in &server.secrets {
                    if secret.len() >= 4 {
                        content = content.replace(secret, "[redacted]");
                    }
                }
                content = truncate_utf8(content, MAX_RESULT_BYTES);
                let clean_server_name = redact_secrets(server.config.name.clone(), &server.secrets);
                let clean_tool_name = redact_secrets(tool.remote_name.clone(), &server.secrets);
                let prefix = format!(
                    "Untrusted response from MCP server '{}' tool '{}':\n",
                    clean_server_name, clean_tool_name
                );
                ToolResult {
                    tool_call_id: call.id.clone(),
                    name: call.name.clone(),
                    content: format!("{prefix}{content}"),
                    is_error: failed,
                    duration_ms: started.elapsed().as_millis() as u64,
                    structured_content: sanitized_structured(
                        value.get("structuredContent"),
                        &server.secrets,
                    ),
                }
            }
            Err(error) => {
                let message = truncate_utf8(
                    redact_secrets(error.message, &server.secrets),
                    MAX_RESULT_BYTES,
                );
                self.errors
                    .lock()
                    .await
                    .insert(server_key(&server.config), message.clone());
                error_result(&message)
            }
        }
    }
}

impl McpConnection {
    async fn request(&mut self, method: &str, params: Option<Value>) -> AppResult<Value> {
        match self {
            Self::Stdio(client) => client.request(method, params).await,
            Self::Http(client) => client.request(method, params).await,
        }
    }
}

impl StdioClient {
    async fn request(&mut self, method: &str, params: Option<Value>) -> AppResult<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let mut message = json!({"jsonrpc":"2.0","id":id,"method":method});
        if let Some(params) = params {
            message["params"] = params;
        }
        let mut line = serde_json::to_vec(&message)?;
        line.push(b'\n');
        tokio::time::timeout(Duration::from_secs(45), async {
            self.stdin.write_all(&line).await?;
            self.stdin.flush().await?;
            loop {
                let Some(response) = read_bounded_line(&mut self.stdout).await? else {
                    return Err(AppError::new(
                        "mcp_process_exited",
                        "The MCP process closed its protocol stream.",
                    ));
                };
                if response.trim().is_empty() {
                    continue;
                }
                let value: Value = serde_json::from_str(&response).map_err(|_| {
                    AppError::new(
                        "mcp_protocol",
                        "The MCP process returned an invalid protocol message.",
                    )
                })?;
                if value.get("id").and_then(Value::as_u64) == Some(id) {
                    return rpc_result(value);
                }
            }
        })
        .await
        .map_err(|_| {
            AppError::new(
                "mcp_timeout",
                "The MCP server did not respond within 45 seconds.",
            )
        })?
    }

    async fn notify(&mut self, method: &str) -> AppResult<()> {
        let mut line = serde_json::to_vec(&json!({"jsonrpc":"2.0","method":method}))?;
        line.push(b'\n');
        self.stdin.write_all(&line).await?;
        self.stdin.flush().await?;
        Ok(())
    }
}

async fn read_bounded_line(stdout: &mut BufReader<ChildStdout>) -> AppResult<Option<String>> {
    let mut line = Vec::new();
    loop {
        let chunk = stdout.fill_buf().await?;
        if chunk.is_empty() {
            if line.is_empty() {
                return Ok(None);
            }
            break;
        }
        let count = chunk
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|index| index + 1)
            .unwrap_or(chunk.len());
        if line.len() + count > MAX_HTTP_BODY_BYTES {
            return Err(AppError::new(
                "mcp_response_limit",
                "The MCP process message exceeded the 1 MiB limit.",
            ));
        }
        let complete = chunk.get(count.wrapping_sub(1)) == Some(&b'\n');
        line.extend_from_slice(&chunk[..count]);
        stdout.consume(count);
        if complete {
            break;
        }
    }
    String::from_utf8(line).map(Some).map_err(|_| {
        AppError::new(
            "mcp_protocol",
            "The MCP process returned a non-UTF-8 message.",
        )
    })
}

impl HttpClient {
    async fn request(&mut self, method: &str, params: Option<Value>) -> AppResult<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let mut message = json!({"jsonrpc":"2.0","id":id,"method":method});
        if let Some(params) = params {
            message["params"] = params;
        }
        let mut request = self
            .client
            .post(self.url.clone())
            .header(header::ACCEPT, "application/json, text/event-stream")
            .json(&message);
        if let Some(session) = &self.session_id {
            request = request.header("Mcp-Session-Id", session);
        }
        if let Some(bearer) = &self.bearer {
            request = request.bearer_auth(bearer);
        }
        let response = request.send().await.map_err(|_| {
            AppError::new(
                "mcp_network",
                "Could not reach the configured MCP HTTP endpoint.",
            )
        })?;
        let status = response.status();
        if !status.is_success() {
            return Err(AppError::new(
                "mcp_http_error",
                format!("MCP HTTP endpoint returned status {}.", status.as_u16()),
            ));
        }
        if let Some(session) = response
            .headers()
            .get("Mcp-Session-Id")
            .and_then(|value| value.to_str().ok())
        {
            self.session_id = Some(session.to_owned());
        }
        let is_event_stream = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/event-stream"));
        if is_event_stream {
            let mut stream = response.bytes_stream();
            let mut body = Vec::new();
            loop {
                let chunk = tokio::time::timeout(Duration::from_secs(45), stream.next())
                    .await
                    .map_err(|_| {
                        AppError::new(
                            "mcp_timeout",
                            "The MCP HTTP event stream did not return within 45 seconds.",
                        )
                    })?
                    .ok_or_else(|| {
                        AppError::new(
                            "mcp_protocol",
                            "The MCP HTTP event stream closed without a response.",
                        )
                    })?
                    .map_err(|_| {
                        AppError::new("mcp_network", "The MCP HTTP event stream was interrupted.")
                    })?;
                body.extend_from_slice(&chunk);
                if body.len() > MAX_HTTP_BODY_BYTES {
                    return Err(AppError::new(
                        "mcp_response_limit",
                        "The MCP HTTP response exceeded the 1 MiB limit.",
                    ));
                }
                while let Some(line_end) = body.iter().position(|byte| *byte == b'\n') {
                    let line: Vec<_> = body.drain(..=line_end).collect();
                    let line = String::from_utf8_lossy(&line);
                    if let Some(data) = line.trim().strip_prefix("data:") {
                        if let Ok(value) = serde_json::from_str::<Value>(data.trim()) {
                            if value.get("id").and_then(Value::as_u64) == Some(id) {
                                return rpc_result(value);
                            }
                        }
                    }
                }
            }
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|_| AppError::new("mcp_network", "Could not read the MCP HTTP response."))?;
        if bytes.len() > MAX_HTTP_BODY_BYTES {
            return Err(AppError::new(
                "mcp_response_limit",
                "The MCP HTTP response exceeded the 1 MiB limit.",
            ));
        }
        if bytes.is_empty() {
            return Ok(Value::Null);
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| {
            AppError::new(
                "mcp_protocol",
                "The MCP HTTP endpoint returned invalid JSON.",
            )
        })?;
        rpc_result(value)
    }

    async fn notify(&mut self, method: &str) -> AppResult<()> {
        let mut request = self
            .client
            .post(self.url.clone())
            .header(header::ACCEPT, "application/json, text/event-stream")
            .json(&json!({"jsonrpc":"2.0","method":method}));
        if let Some(session) = &self.session_id {
            request = request.header("Mcp-Session-Id", session);
        }
        if let Some(bearer) = &self.bearer {
            request = request.bearer_auth(bearer);
        }
        let response = request.send().await.map_err(|_| {
            AppError::new("mcp_network", "Could not complete MCP HTTP initialization.")
        })?;
        if !response.status().is_success() && response.status().as_u16() != 202 {
            return Err(AppError::new(
                "mcp_http_error",
                format!(
                    "MCP HTTP endpoint returned status {}.",
                    response.status().as_u16()
                ),
            ));
        }
        Ok(())
    }
}

async fn connect_server(
    config: McpServerConfig,
    secrets: BTreeMap<String, String>,
    project_root: Option<&Path>,
) -> AppResult<LiveServer> {
    let secret_values: Vec<_> = secrets.values().cloned().collect();
    let mut connection = match &config.transport {
        McpTransport::Stdio { command, args } => {
            let mut process = Command::new(command);
            process
                .args(args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .env_clear();
            if config.scope == McpScope::Project {
                process.current_dir(project_root.ok_or_else(|| {
                    AppError::new(
                        "project_required",
                        "Select a project before starting its MCP process.",
                    )
                })?);
            }
            for key in [
                "PATH",
                "HOME",
                "LANG",
                "TMPDIR",
                "TEMP",
                "TMP",
                "USERPROFILE",
                "APPDATA",
                "LOCALAPPDATA",
                "SYSTEMROOT",
                "WINDIR",
            ] {
                if let Some(value) = std::env::var_os(key) {
                    process.env(key, value);
                }
            }
            for name in &config.env_names {
                let secret = secrets.get(name).ok_or_else(|| {
                    AppError::new(
                        "mcp_secret_missing",
                        format!(
                            "Add the missing '{}' value in the MCP server settings.",
                            name
                        ),
                    )
                })?;
                process.env(name, secret);
            }
            let mut child = process.spawn().map_err(|_| AppError::new("mcp_process_start", "Could not start the configured MCP process. Check the program path and arguments."))?;
            let stdin = child.stdin.take().ok_or_else(|| {
                AppError::new("mcp_process_start", "Could not open MCP process input.")
            })?;
            let stdout = child.stdout.take().ok_or_else(|| {
                AppError::new("mcp_process_start", "Could not open MCP process output.")
            })?;
            McpConnection::Stdio(Box::new(StdioClient {
                _child: child,
                stdin,
                stdout: BufReader::new(stdout),
                next_id: 1,
            }))
        }
        McpTransport::Http { url } => {
            let url = validate_http_url(url)?;
            if config.has_auth_token && !secrets.contains_key("__AUTH_TOKEN") {
                return Err(AppError::new(
                    "mcp_secret_missing",
                    "Add the bearer token in the MCP server settings.",
                ));
            }
            let client = Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(8))
                .timeout(Duration::from_secs(45))
                .build()
                .map_err(|_| {
                    AppError::new(
                        "mcp_http_client",
                        "Could not initialize the MCP HTTP client.",
                    )
                })?;
            McpConnection::Http(HttpClient {
                client,
                url,
                bearer: secrets.get("__AUTH_TOKEN").cloned(),
                session_id: None,
                next_id: 1,
            })
        }
    };
    let initialize = json!({
        "protocolVersion": MCP_PROTOCOL_VERSION,
        "capabilities": {},
        "clientInfo": {"name":"JevCode","version":env!("CARGO_PKG_VERSION")}
    });
    let initialized = connection.request("initialize", Some(initialize)).await?;
    if initialized["protocolVersion"].as_str() != Some(MCP_PROTOCOL_VERSION) {
        return Err(AppError::new(
            "mcp_protocol_version",
            "The MCP server negotiated an unsupported protocol version.",
        ));
    }
    match &mut connection {
        McpConnection::Stdio(client) => client.notify("notifications/initialized").await?,
        McpConnection::Http(client) => client.notify("notifications/initialized").await?,
    }
    let mut tools = Vec::new();
    let mut cursor: Option<String> = None;
    let mut names = HashSet::new();
    for _ in 0..20 {
        let params = cursor.as_ref().map(|value| json!({"cursor":value}));
        let response = connection.request("tools/list", params).await?;
        let listed = response["tools"].as_array().ok_or_else(|| {
            AppError::new(
                "mcp_protocol",
                "The MCP server returned an invalid tools list.",
            )
        })?;
        for item in listed {
            if tools.len() >= MAX_TOOLS_PER_SERVER {
                break;
            }
            let remote_name = item["name"].as_str().ok_or_else(|| {
                AppError::new("mcp_protocol", "The MCP server returned an unnamed tool.")
            })?;
            let schema = item.get("inputSchema").cloned().unwrap_or_else(
                || json!({"type":"object","properties":{},"additionalProperties":false}),
            );
            if !schema.is_object()
                || schema["type"].as_str() != Some("object")
                || serde_json::to_vec(&schema)?.len() > MAX_SCHEMA_BYTES
                || !schema_within_limits(&schema)
            {
                return Err(AppError::new(
                    "mcp_schema_invalid",
                    "An MCP tool schema is unsupported or exceeds 64 KiB.",
                ));
            }
            if json_contains_secret(&schema, &secret_values) {
                return Err(AppError::new("mcp_secret_exposure", "The MCP server included credential data in its tool schema; the connection was stopped."));
            }
            let local_name = namespaced_name(&config, remote_name)?;
            if !names.insert(local_name.clone()) {
                return Err(AppError::new(
                    "mcp_tool_collision",
                    "Two server tools resolve to the same local name.",
                ));
            }
            tools.push(RemoteTool {
                local_name,
                remote_name: remote_name.chars().take(128).collect(),
                description: redact_secrets(
                    item["description"]
                        .as_str()
                        .unwrap_or("No description provided.")
                        .chars()
                        .take(1800)
                        .collect::<String>(),
                    &secret_values,
                ),
                input_schema: schema.clone(),
                public_schema: schema,
            });
        }
        cursor = response["nextCursor"].as_str().map(str::to_owned);
        if cursor.is_none() || tools.len() >= MAX_TOOLS_PER_SERVER {
            break;
        }
    }
    if tools.is_empty() {
        return Err(AppError::new(
            "mcp_no_tools",
            "The connected MCP server did not provide any tools.",
        ));
    }
    Ok(LiveServer {
        config,
        tools,
        connection,
        secrets: secret_values,
    })
}

fn rpc_result(value: Value) -> AppResult<Value> {
    if let Some(error) = value.get("error") {
        let code = error["code"].as_i64().unwrap_or(-1);
        let message = error["message"].as_str().unwrap_or("MCP request failed.");
        return Err(AppError::new(
            "mcp_remote_error",
            format!(
                "MCP server returned error {code}: {}",
                message.chars().take(300).collect::<String>()
            ),
        ));
    }
    value.get("result").cloned().ok_or_else(|| {
        AppError::new(
            "mcp_protocol",
            "The MCP server response did not include a result.",
        )
    })
}

fn remote_content(value: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(content) = value["content"].as_array() {
        for item in content {
            match item["type"].as_str() {
                Some("text") => {
                    if let Some(text) = item["text"].as_str() {
                        parts.push(text.to_owned());
                    }
                }
                Some("image") => parts.push("[MCP image content omitted]".into()),
                _ => {}
            }
        }
    }
    if parts.is_empty() {
        value
            .get("structuredContent")
            .map(Value::to_string)
            .unwrap_or_else(|| "The MCP server returned no text content.".into())
    } else {
        parts.join("\n")
    }
}

fn validate_config(config: &mut McpServerConfig) -> AppResult<()> {
    if !valid_identifier(&config.id) || config.name.trim().is_empty() || config.name.len() > 120 {
        return Err(AppError::new(
            "invalid_mcp_config",
            "MCP server ID or display name is invalid.",
        ));
    }
    match (&config.scope, &config.project_id) {
        (McpScope::User, None) => {}
        (McpScope::Project, Some(id)) if valid_identifier(id) => {}
        _ => {
            return Err(AppError::new(
                "invalid_mcp_scope",
                "Project MCP configuration must belong to a valid project.",
            ))
        }
    }
    config.env_names.sort();
    config.env_names.dedup();
    if config.env_names.len() > 64 || config.env_names.iter().any(|name| !valid_env_name(name)) {
        return Err(AppError::new(
            "invalid_mcp_env",
            "MCP environment variable names are invalid.",
        ));
    }
    match &config.transport {
        McpTransport::Stdio { command, args } => {
            if config.has_auth_token || looks_like_secret_arguments(args) {
                return Err(AppError::new("invalid_mcp_secret", "Put credentials in the environment secret fields; do not embed them in a command or its arguments."));
            }
            if command.trim().is_empty()
                || command.len() > 2048
                || args.len() > 128
                || args.iter().any(|arg| arg.len() > 4096)
            {
                return Err(AppError::new(
                    "invalid_mcp_command",
                    "MCP process command or argument list is invalid.",
                ));
            }
        }
        McpTransport::Http { url } => {
            validate_http_url(url)?;
            if !config.env_names.is_empty() {
                return Err(AppError::new(
                    "invalid_mcp_env",
                    "Environment variables are supported for stdio servers. Use the HTTP bearer token field for HTTP authentication.",
                ));
            }
        }
    }
    Ok(())
}

fn validate_http_url(value: &str) -> AppResult<Url> {
    let url = Url::parse(value)
        .map_err(|_| AppError::new("invalid_mcp_url", "Enter a valid MCP HTTP URL."))?;
    let local = url.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    if (url.scheme() != "https" && !(url.scheme() == "http" && local))
        || url.username() != ""
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(AppError::new("invalid_mcp_url", "MCP endpoints must use HTTPS (HTTP is allowed only for loopback), without URL credentials, query parameters, or fragments."));
    }
    Ok(url)
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}
fn valid_env_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .enumerate()
            .all(|(i, c)| c == '_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit()))
}

fn namespaced_name(config: &McpServerConfig, remote: &str) -> AppResult<String> {
    if remote.is_empty() || remote.len() > 128 {
        return Err(AppError::new(
            "mcp_tool_name",
            "The MCP server returned an invalid tool name.",
        ));
    }
    let scope = if config.scope == McpScope::User {
        "u".to_owned()
    } else {
        let project = config.project_id.as_deref().unwrap_or("-");
        let hash = Sha256::digest(project.as_bytes());
        format!(
            "p{}",
            hash[..4]
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        )
    };
    let server_hash = Sha256::digest(config.id.as_bytes());
    let remote_hash = Sha256::digest(remote.as_bytes());
    let server_tag = format!(
        "{}-{}",
        config.id.chars().take(12).collect::<String>(),
        server_hash[..2]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    let remote_tag = remote_hash[..5]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(format!("mcp__{scope}__{server_tag}__{remote_tag}"))
}

fn looks_like_secret_argument(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    [
        "--token=",
        "--api-key=",
        "--apikey=",
        "--password=",
        "--secret=",
        "token=",
        "api_key=",
        "api-key=",
        "password=",
        "secret=",
        "bearer ",
        "sk-",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn looks_like_secret_arguments(args: &[String]) -> bool {
    const FLAGS: &[&str] = &[
        "--token",
        "--auth-token",
        "--api-key",
        "--apikey",
        "--password",
        "--secret",
        "--client-secret",
        "--access-token",
        "--authorization",
    ];
    args.iter().enumerate().any(|(index, value)| {
        let lower = value.to_ascii_lowercase();
        looks_like_secret_argument(value)
            || (FLAGS.contains(&lower.as_str())
                && args
                    .get(index + 1)
                    .is_some_and(|next| !next.starts_with('-')))
    })
}

fn redact_secrets(mut value: String, secrets: &[String]) -> String {
    for secret in secrets.iter().filter(|secret| !secret.is_empty()) {
        value = value.replace(secret, "[redacted]");
    }
    value
}

fn json_contains_secret(value: &Value, secrets: &[String]) -> bool {
    match value {
        Value::String(text) => secrets
            .iter()
            .any(|secret| !secret.is_empty() && text.contains(secret)),
        Value::Array(values) => values
            .iter()
            .any(|value| json_contains_secret(value, secrets)),
        Value::Object(values) => values.iter().any(|(key, value)| {
            secrets
                .iter()
                .any(|secret| !secret.is_empty() && key.contains(secret))
                || json_contains_secret(value, secrets)
        }),
        _ => false,
    }
}

fn truncate_utf8(mut value: String, maximum: usize) -> String {
    if value.len() > maximum {
        let mut boundary = maximum;
        while !value.is_char_boundary(boundary) {
            boundary -= 1;
        }
        value.truncate(boundary);
    }
    value
}

fn sanitized_structured(value: Option<&Value>, secrets: &[String]) -> Option<Value> {
    let raw = serde_json::to_string(value?).ok()?;
    let redacted = redact_secrets(raw, secrets);
    if redacted.len() > MAX_RESULT_BYTES {
        return None;
    }
    serde_json::from_str(&redacted).ok()
}

fn schema_within_limits(schema: &Value) -> bool {
    fn visit(value: &Value, depth: usize, nodes: &mut usize) -> bool {
        *nodes += 1;
        if depth > MAX_SCHEMA_DEPTH || *nodes > MAX_SCHEMA_NODES {
            return false;
        }
        match value {
            Value::Object(map) => map.values().all(|child| visit(child, depth + 1, nodes)),
            Value::Array(items) => items.iter().all(|child| visit(child, depth + 1, nodes)),
            _ => true,
        }
    }
    visit(schema, 0, &mut 0)
}

fn config_fingerprint(config: &McpServerConfig) -> AppResult<String> {
    let mut normalized = config.clone();
    normalized.enabled = false;
    normalized.env_names.sort();
    let bytes = serde_json::to_vec(&normalized)?;
    Ok(Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn server_key(config: &McpServerConfig) -> String {
    format!(
        "{}|{}|{}|",
        if config.scope == McpScope::User {
            "user"
        } else {
            "project"
        },
        config.project_id.as_deref().unwrap_or_default(),
        config.id
    )
}

fn credential_key(config: &McpServerConfig, env_name: &str) -> String {
    let digest = Sha256::digest(
        format!(
            "{}:{}:{}:{}",
            if config.scope == McpScope::User {
                "user"
            } else {
                "project"
            },
            config.project_id.as_deref().unwrap_or_default(),
            config.id,
            env_name
        )
        .as_bytes(),
    );
    format!(
        "mcp-{}",
        digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    )
}

fn load_secrets(
    credentials: &CredentialStore,
    config: &McpServerConfig,
) -> AppResult<BTreeMap<String, String>> {
    let mut values = BTreeMap::new();
    for name in &config.env_names {
        if let Some(secret) = credentials.get(&credential_key(config, name))? {
            values.insert(name.clone(), secret);
        }
    }
    if config.has_auth_token {
        if let Some(secret) = credentials.get(&credential_key(config, "__AUTH_TOKEN"))? {
            values.insert("__AUTH_TOKEN".into(), secret);
        }
    }
    Ok(values)
}

fn project_config_path(root: &Path) -> AppResult<PathBuf> {
    let root = std::fs::canonicalize(root).map_err(|_| {
        AppError::new(
            "project_unavailable",
            "The selected project folder is unavailable.",
        )
    })?;
    let config_dir = root.join(".jevcode");
    if let Ok(metadata) = std::fs::symlink_metadata(&config_dir) {
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(AppError::new(
                "mcp_config_path",
                "The project .jevcode configuration path must be a regular folder.",
            ));
        }
    }
    let path = config_dir.join("mcp.json");
    if let Ok(metadata) = std::fs::symlink_metadata(&path) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(AppError::new(
                "mcp_config_path",
                "Project MCP config must be a regular file, not a symbolic link.",
            ));
        }
    }
    Ok(path)
}

fn read_project_servers(root: &Path, project_id: &str) -> AppResult<Vec<McpServerConfig>> {
    let path = project_config_path(root)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let metadata = std::fs::symlink_metadata(&path)?;
    if metadata.file_type().is_symlink() || metadata.len() > 256 * 1024 {
        return Err(AppError::new(
            "mcp_config_path",
            "Project MCP config must be a regular JSON file smaller than 256 KiB.",
        ));
    }
    let file: ProjectMcpFile = serde_json::from_slice(&std::fs::read(path)?).map_err(|_| {
        AppError::new(
            "mcp_config_invalid",
            "Project .jevcode/mcp.json is invalid JSON or has unsupported fields.",
        )
    })?;
    if file.mcp_servers.len() > 64 {
        return Err(AppError::new(
            "mcp_config_limit",
            "Project MCP config supports up to 64 servers.",
        ));
    }
    file.mcp_servers
        .into_iter()
        .map(|entry| {
            let mut config = McpServerConfig {
                id: entry.id,
                name: entry.name,
                scope: McpScope::Project,
                project_id: Some(project_id.into()),
                transport: entry.transport,
                env_names: entry.env_names,
                has_auth_token: entry.has_auth_token,
                enabled: entry.enabled,
            };
            validate_config(&mut config)?;
            Ok(config)
        })
        .collect()
}

fn write_project_server(root: &Path, project_id: &str, config: &McpServerConfig) -> AppResult<()> {
    if project_id.is_empty() || config.project_id.as_deref() != Some(project_id) {
        return Err(AppError::new(
            "invalid_mcp_scope",
            "The project MCP config does not match the selected project.",
        ));
    }
    let path = project_config_path(root)?;
    if path.exists() && std::fs::metadata(&path)?.len() > 256 * 1024 {
        return Err(AppError::new(
            "mcp_config_limit",
            "Project MCP config exceeds the 256 KiB limit.",
        ));
    }
    let parent = path.parent().ok_or_else(|| {
        AppError::new(
            "mcp_config_path",
            "Project MCP configuration path is invalid.",
        )
    })?;
    std::fs::create_dir_all(parent)?;
    let mut file = if path.exists() {
        serde_json::from_slice::<ProjectMcpFile>(&std::fs::read(&path)?).map_err(|_| {
            AppError::new(
                "mcp_config_invalid",
                "Project .jevcode/mcp.json is invalid JSON.",
            )
        })?
    } else {
        ProjectMcpFile::default()
    };
    let entry = ProjectMcpServer {
        id: config.id.clone(),
        name: config.name.clone(),
        transport: config.transport.clone(),
        env_names: config.env_names.clone(),
        has_auth_token: config.has_auth_token,
        enabled: config.enabled,
    };
    if let Some(existing) = file
        .mcp_servers
        .iter_mut()
        .find(|server| server.id == config.id)
    {
        *existing = entry;
    } else {
        file.mcp_servers.push(entry);
    }
    if file.mcp_servers.len() > 64 {
        return Err(AppError::new(
            "mcp_config_limit",
            "Project MCP config supports up to 64 servers.",
        ));
    }
    file.mcp_servers.sort_by(|a, b| a.id.cmp(&b.id));
    let serialized = serde_json::to_vec_pretty(&file)?;
    if serialized.len() > 256 * 1024 {
        return Err(AppError::new(
            "mcp_config_limit",
            "Project MCP config exceeds the 256 KiB limit.",
        ));
    }
    std::fs::write(path, serialized)?;
    Ok(())
}

fn remove_project_server(root: &Path, project_id: &str, server_id: &str) -> AppResult<()> {
    let path = project_config_path(root)?;
    if !path.exists() {
        return Ok(());
    }
    let mut file: ProjectMcpFile =
        serde_json::from_slice(&std::fs::read(&path)?).map_err(|_| {
            AppError::new(
                "mcp_config_invalid",
                "Project .jevcode/mcp.json is invalid JSON.",
            )
        })?;
    file.mcp_servers.retain(|server| server.id != server_id);
    let _ = project_id;
    std::fs::write(path, serde_json::to_vec_pretty(&file)?)?;
    Ok(())
}

fn resolve_config(
    database: &Database,
    root: Option<&Path>,
    project_id: Option<&str>,
    scope: &McpScope,
    server_id: &str,
) -> AppResult<McpServerConfig> {
    match scope {
        McpScope::User => database
            .mcp_servers()?
            .into_iter()
            .find(|config| config.id == server_id)
            .ok_or_else(|| AppError::new("mcp_not_found", "The user MCP server no longer exists.")),
        McpScope::Project => {
            let project_id = project_id.ok_or_else(|| {
                AppError::new(
                    "project_required",
                    "Select a project before using project MCP servers.",
                )
            })?;
            read_project_servers(
                root.ok_or_else(|| {
                    AppError::new(
                        "project_required",
                        "Select a project before using project MCP servers.",
                    )
                })?,
                project_id,
            )?
            .into_iter()
            .find(|config| config.id == server_id)
            .ok_or_else(|| {
                AppError::new("mcp_not_found", "The project MCP server no longer exists.")
            })
        }
    }
}

fn save_config(
    database: &Database,
    root: Option<&Path>,
    config: &McpServerConfig,
) -> AppResult<()> {
    match &config.scope {
        McpScope::User => database.save_mcp_server(config),
        McpScope::Project => write_project_server(
            root.ok_or_else(|| {
                AppError::new(
                    "project_required",
                    "Select a project before editing project MCP configuration.",
                )
            })?,
            config.project_id.as_deref().unwrap_or_default(),
            config,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stdio(scope: McpScope, id: &str) -> McpServerConfig {
        McpServerConfig {
            id: id.into(),
            name: "Local MCP".into(),
            scope,
            project_id: None,
            transport: McpTransport::Stdio {
                command: "mcp-server".into(),
                args: vec![],
            },
            env_names: vec!["TOKEN".into()],
            has_auth_token: false,
            enabled: false,
        }
    }

    #[test]
    fn http_requires_tls_except_for_loopback_and_rejects_credential_urls() {
        assert!(validate_http_url("https://mcp.example.test/mcp").is_ok());
        assert!(validate_http_url("http://127.0.0.1:3333/mcp").is_ok());
        assert!(validate_http_url("http://mcp.example.test/mcp").is_err());
        assert!(validate_http_url("https://user:pass@mcp.example.test/mcp").is_err());
        assert!(validate_http_url("https://mcp.example.test/mcp?token=secret").is_err());
    }

    #[test]
    fn namespaced_tools_do_not_collide_between_user_and_project_scope() {
        let user = stdio(McpScope::User, "docs");
        let mut project = stdio(McpScope::Project, "docs");
        project.project_id = Some("project-id".into());
        assert_ne!(
            namespaced_name(&user, "search docs").unwrap(),
            namespaced_name(&project, "search docs").unwrap()
        );
    }

    #[test]
    fn trust_fingerprint_tracks_configuration_but_ignores_enable_flag() {
        let mut config = stdio(McpScope::User, "search");
        let first = config_fingerprint(&config).unwrap();
        config.enabled = true;
        assert_eq!(first, config_fingerprint(&config).unwrap());
        config.env_names.push("OTHER".into());
        assert_ne!(first, config_fingerprint(&config).unwrap());
    }

    #[test]
    fn project_mcp_config_has_no_place_for_secret_values_and_rejects_symlink_directory() {
        let root = tempfile::tempdir().unwrap();
        let mut config = stdio(McpScope::Project, "repo-tools");
        config.project_id = Some("project-id".into());
        write_project_server(root.path(), "project-id", &config).unwrap();
        let contents = std::fs::read_to_string(root.path().join(".jevcode/mcp.json")).unwrap();
        assert!(contents.contains("TOKEN"));
        assert!(!contents.contains("secret-value"));
        assert_eq!(
            read_project_servers(root.path(), "project-id")
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn process_environment_is_declared_by_names_not_values_in_config() {
        let config = stdio(McpScope::User, "server");
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("TOKEN"));
        assert!(!json.contains("secret-value"));
    }

    #[test]
    fn server_names_and_argument_values_cannot_smuggle_common_credentials() {
        let mut config = stdio(McpScope::User, "unsafe");
        config.transport = McpTransport::Stdio {
            command: "node".into(),
            args: vec!["--api-key=sk-example-secret".into()],
        };
        assert_eq!(
            validate_config(&mut config).unwrap_err().code,
            "invalid_mcp_secret"
        );
        assert!(looks_like_secret_argument("--token=secret"));
        assert!(looks_like_secret_argument("Bearer abc123"));
        let mut split = stdio(McpScope::User, "split-credential");
        split.transport = McpTransport::Stdio {
            command: "node".into(),
            args: vec!["--api-key".into(), "some-credential".into()],
        };
        assert_eq!(
            validate_config(&mut split).unwrap_err().code,
            "invalid_mcp_secret"
        );
    }

    #[test]
    fn remote_schemas_with_secrets_or_excessive_depth_are_rejected() {
        assert!(json_contains_secret(
            &json!({"type":"object","description":"api-token-123"}),
            &["api-token-123".into()]
        ));
        let mut schema = json!({"type":"object"});
        for _ in 0..MAX_SCHEMA_DEPTH + 2 {
            schema = json!({"type":"object","properties":{"nested":schema}});
        }
        assert!(!schema_within_limits(&schema));
        let structured = sanitized_structured(
            Some(&json!({"message":"api-token-123"})),
            &["api-token-123".into()],
        )
        .unwrap();
        assert!(!structured.to_string().contains("api-token-123"));
        assert_eq!(
            redact_secrets("token=abc".into(), &["abc".into()]),
            "token=[redacted]"
        );
    }

    #[tokio::test]
    async fn saved_servers_are_disabled_untrusted_and_keep_only_nonsecret_configuration() {
        let root = tempfile::tempdir().unwrap();
        let db_path = root.path().join("mcp.sqlite");
        let database = Database::open(&db_path).unwrap();
        let mut config = stdio(McpScope::User, "safe-tools");
        config.name = "Safe tools".into();
        database.save_mcp_server(&config).unwrap();
        let reopened = Database::open(&db_path).unwrap();
        let saved = reopened.mcp_servers().unwrap();
        assert_eq!(saved, vec![config.clone()]);
        assert!(!reopened
            .mcp_is_trusted(&config, &config_fingerprint(&config).unwrap())
            .unwrap());
        let views = McpManager::default()
            .list(&reopened, &CredentialStore, None, None)
            .await
            .unwrap();
        assert_eq!(views[0].status, McpConnectionStatus::Disabled);
        assert!(!views[0].trusted);
        let connection = rusqlite::Connection::open(&db_path).unwrap();
        let data: String = connection
            .query_row(
                "SELECT data FROM mcp_servers WHERE id = 'safe-tools'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(data.contains("TOKEN"));
        assert!(!data.contains("sk-never-persist-this"));
    }

    #[tokio::test]
    async fn stdio_transport_discovers_and_executes_a_tool_through_the_extension_registry() {
        let root = tempfile::tempdir().unwrap();
        let fixture = root.path().join("mcp-fixture.cjs");
        std::fs::write(
            &fixture,
            r#"const readline = require('node:readline');
const input = readline.createInterface({ input: process.stdin });
input.on('line', line => {
  const request = JSON.parse(line);
  if (!('id' in request)) return;
  let result = {};
  if (request.method === 'initialize') result = { protocolVersion: '2026-07-28', capabilities: { tools: {} }, serverInfo: { name: 'fixture', version: '1' } };
  else if (request.method === 'tools/list') result = { tools: [{ name: 'hello', description: 'Say hello', inputSchema: { type: 'object', properties: { who: { type: 'string', minLength: 1 } }, required: ['who'], additionalProperties: false } }] };
  else if (request.method === 'tools/call') result = { content: [{ type: 'text', text: `Hello ${request.params.arguments.who}` }] };
  process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: request.id, result }) + '\n');
});"#,
        )
        .unwrap();
        let config = McpServerConfig {
            id: "fixture".into(),
            name: "Fixture server".into(),
            scope: McpScope::User,
            project_id: None,
            transport: McpTransport::Stdio {
                command: "node".into(),
                args: vec![fixture.display().to_string()],
            },
            env_names: vec![],
            has_auth_token: false,
            enabled: true,
        };
        let live = connect_server(config.clone(), BTreeMap::new(), None)
            .await
            .unwrap();
        let manager = McpManager::default();
        manager
            .connections
            .lock()
            .await
            .insert(server_key(&config), Arc::new(Mutex::new(live)));
        let tools = manager.definitions("project").await.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].permission, ToolCategory::Shell);
        assert!(!tools[0].parallel_safe);
        let result = manager
            .execute(
                "project",
                &ToolCall {
                    id: "call-1".into(),
                    name: tools[0].name.clone(),
                    arguments: json!({"who":"Ada"}),
                },
            )
            .await;
        assert!(!result.is_error);
        assert!(result.content.contains("Hello Ada"));
        assert!(result.content.contains("Untrusted response"));
        manager.stop(&config).await;
    }
}
