//! MCP servers that other tools keep in project files, imported after a review.
//! Scanning parses each file into a preview and runs nothing. Importing saves each
//! selected server through the same path as a hand-made one, so it starts disabled
//! and untrusted. Literal environment values go straight to the OS credential
//! store, never into the project file, the preview, or a log.
use crate::{
    credentials::CredentialStore,
    domain::{McpScope, McpServerConfig, McpTransport},
    error::{AppError, AppResult},
    mcp::{project_server_ids, valid_env_name, validate_config, McpManager, SaveMcpServerInput},
    persistence::Database,
};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
};

/// Project files that other tools keep MCP servers in, and the key each nests them under.
const SOURCES: [(&str, &str); 4] = [
    (".mcp.json", "mcpServers"),
    (".cursor/mcp.json", "mcpServers"),
    (".vscode/mcp.json", "servers"),
    (".gemini/settings.json", "mcpServers"),
];
const MAX_SOURCE_BYTES: u64 = 256 * 1024;
/// The largest literal value the credential store takes, as saving a server enforces.
const MAX_SECRET_BYTES: usize = 16_384;
/// Fields Octran Code reads. Any other field is listed as not imported.
const KNOWN_FIELDS: [&str; 7] = [
    "type", "command", "args", "env", "url", "headers", "disabled",
];

/// A project file that other tools keep MCP servers in.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpImportSource {
    /// Project-relative path of the file.
    pub path: String,
    /// Why the file could not be read. Its servers are not listed when this is set.
    pub problem: Option<String>,
    pub servers: Vec<McpImportServer>,
}

/// One server in a source file, as an import would save it. Values are never included.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpImportServer {
    /// Names the server in an import: its source file and its name there.
    pub key: String,
    pub name: String,
    /// The ID the server would have in the project's MCP file.
    pub server_id: Option<String>,
    /// How the server starts or where it connects, without credential values.
    pub transport: Option<String>,
    pub env_names: Vec<String>,
    /// Environment variables whose literal values move to the OS credential store.
    pub stored_env_names: Vec<String>,
    pub warnings: Vec<String>,
    /// Why the server cannot be imported. Absent when it can.
    pub blocked: Option<String>,
}

/// A parsed server, with what saving it needs. The values stay on this side of the webview.
struct Parsed {
    server: McpImportServer,
    input: Option<SaveMcpServerInput>,
}

struct Source {
    path: String,
    problem: Option<String>,
    servers: Vec<Parsed>,
}

/// What a server definition would save, before it has an ID.
struct Built {
    config: McpServerConfig,
    secrets: BTreeMap<String, String>,
    transport: String,
}

pub fn scan(project_root: &Path, project_id: &str) -> AppResult<Vec<McpImportSource>> {
    Ok(read_sources(project_root, project_id)?
        .into_iter()
        .map(|source| McpImportSource {
            path: source.path,
            problem: source.problem,
            servers: source
                .servers
                .into_iter()
                .map(|parsed| parsed.server)
                .collect(),
        })
        .collect())
}

/// Saves the selected servers. Each is read from its file again, so a key that no
/// longer names a server is refused. Saving disables the server and revokes any
/// trust, so it must be reviewed and trusted in Octran Code before it runs.
pub async fn import(
    manager: &McpManager,
    database: &Database,
    credentials: &CredentialStore,
    project_root: &Path,
    project_id: &str,
    keys: &[String],
) -> AppResult<Vec<McpServerConfig>> {
    let mut parsed: Vec<Parsed> = read_sources(project_root, project_id)?
        .into_iter()
        .flat_map(|source| source.servers)
        .collect();
    let mut seen = HashSet::new();
    let mut imported = Vec::new();
    for key in keys {
        if !seen.insert(key) {
            continue;
        }
        let index = parsed
            .iter()
            .position(|item| item.server.key == *key)
            .ok_or_else(|| {
                AppError::new(
                    "mcp_import_unavailable",
                    "That server is no longer in its project file. Scan again.",
                )
            })?;
        let item = parsed.swap_remove(index);
        let Some(input) = item.input else {
            return Err(AppError::new(
                "mcp_import_blocked",
                item.server.blocked.unwrap_or_default(),
            ));
        };
        imported.push(
            manager
                .save(database, credentials, Some(project_root), input)
                .await?,
        );
    }
    Ok(imported)
}

fn read_sources(project_root: &Path, project_id: &str) -> AppResult<Vec<Source>> {
    let mut taken: HashSet<String> = project_server_ids(project_root, project_id)?
        .into_iter()
        .collect();
    let mut sources = Vec::new();
    for (path, servers_key) in SOURCES {
        let entries = match read_json(&project_root.join(path)) {
            Ok(None) => continue,
            Err(problem) => {
                sources.push(Source {
                    path: path.to_owned(),
                    problem: Some(problem),
                    servers: Vec::new(),
                });
                continue;
            }
            Ok(Some(value)) => value,
        };
        // A file without a servers section, such as a settings file, is not listed.
        let Some(entries) = entries.get(servers_key).and_then(Value::as_object) else {
            continue;
        };
        let servers = entries
            .iter()
            .map(|(name, entry)| parse_server(project_id, path, name, entry, &mut taken))
            .collect();
        sources.push(Source {
            path: path.to_owned(),
            problem: None,
            servers,
        });
    }
    Ok(sources)
}

/// The parsed JSON in a source file, or `None` when the file does not exist.
fn read_json(path: &Path) -> Result<Option<Value>, String> {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return Ok(None);
    };
    if metadata.file_type().is_symlink() {
        return Err("Symbolic link. Octran Code does not follow links for MCP files.".to_owned());
    }
    if !metadata.is_file() {
        return Err("Not a regular file.".to_owned());
    }
    if metadata.len() > MAX_SOURCE_BYTES {
        return Err("Larger than 256 KiB.".to_owned());
    }
    let bytes = std::fs::read(path).map_err(|_| "Could not be read.".to_owned())?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| "Not valid JSON. Comments are not supported.".to_owned())
}

fn parse_server(
    project_id: &str,
    source: &str,
    name: &str,
    entry: &Value,
    taken: &mut HashSet<String>,
) -> Parsed {
    let mut server = McpImportServer {
        key: format!("{source}#{name}"),
        name: name.to_owned(),
        server_id: None,
        transport: None,
        env_names: Vec::new(),
        stored_env_names: Vec::new(),
        warnings: Vec::new(),
        blocked: None,
    };
    let built = build(project_id, name, entry, &mut server.warnings).and_then(|mut built| {
        let id = allocate_id(name, taken);
        built.config.id = id;
        validate_config(&mut built.config)
            .map_err(|error| error.message)
            .map(|()| built)
    });
    match built.and_then(check_secret_placement) {
        Ok(built) => {
            taken.insert(built.config.id.clone());
            server.server_id = Some(built.config.id.clone());
            server.transport = Some(built.transport);
            server.env_names = built.config.env_names.clone();
            server.stored_env_names = built.secrets.keys().cloned().collect();
            let needs_value: Vec<&String> = built
                .config
                .env_names
                .iter()
                .filter(|name| !built.secrets.contains_key(*name))
                .collect();
            if !needs_value.is_empty() {
                let names: Vec<&str> = needs_value.iter().map(|name| name.as_str()).collect();
                server.warnings.push(format!(
                    "Add values for {} in Octran Code before connecting.",
                    names.join(", ")
                ));
            }
            Parsed {
                server,
                input: Some(SaveMcpServerInput {
                    config: built.config,
                    secrets: built.secrets,
                }),
            }
        }
        Err(reason) => {
            server.blocked = Some(reason);
            Parsed {
                server,
                input: None,
            }
        }
    }
}

/// Reads a server definition in any of the supported shapes. Anything Octran Code
/// cannot represent is refused, never approximated.
fn build(
    project_id: &str,
    name: &str,
    entry: &Value,
    warnings: &mut Vec<String>,
) -> Result<Built, String> {
    let object = entry
        .as_object()
        .ok_or_else(|| "This entry is not a server definition.".to_owned())?;
    let mut unknown: Vec<&str> = object
        .keys()
        .map(String::as_str)
        .filter(|key| !KNOWN_FIELDS.contains(key))
        .collect();
    unknown.sort_unstable();
    if !unknown.is_empty() {
        warnings.push(format!("Not imported: {}.", unknown.join(", ")));
    }
    let (transport, summary) = match (
        object.get("type").and_then(Value::as_str),
        object.get("command"),
        object.get("url"),
    ) {
        (None | Some("stdio"), Some(Value::String(command)), _) => {
            let args = string_array(object.get("args"))?;
            let summary = std::iter::once(command.as_str())
                .chain(args.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join(" ");
            (
                McpTransport::Stdio {
                    command: command.clone(),
                    args,
                },
                format!("Command: {summary}"),
            )
        }
        (None | Some("http"), _, Some(Value::String(url))) => {
            let has_headers = object
                .get("headers")
                .and_then(Value::as_object)
                .is_some_and(|headers| !headers.is_empty());
            if has_headers {
                warnings.push(
                    "Headers are not imported. Set HTTP authentication in Octran Code before connecting."
                        .to_owned(),
                );
            }
            (
                McpTransport::Http { url: url.clone() },
                format!("URL: {url}"),
            )
        }
        (Some("sse"), _, _) => return Err("SSE servers are not supported yet.".to_owned()),
        _ => return Err("No stdio command or HTTP URL was found.".to_owned()),
    };
    let (env_names, secrets) = match &transport {
        McpTransport::Stdio { .. } => environment(object.get("env"))?,
        McpTransport::Http { .. } => {
            if object
                .get("env")
                .and_then(Value::as_object)
                .is_some_and(|env| !env.is_empty())
            {
                warnings.push(
                    "Environment variables are used by stdio servers only. They were not imported."
                        .to_owned(),
                );
            }
            (Vec::new(), BTreeMap::new())
        }
    };
    Ok(Built {
        config: McpServerConfig {
            id: String::new(),
            name: name.to_owned(),
            scope: McpScope::Project,
            project_id: Some(project_id.to_owned()),
            transport,
            env_names,
            has_auth_token: false,
            enabled: false,
        },
        secrets,
        transport: summary,
    })
}

/// Environment names, and the literal values to store. A value that is empty or
/// refers to another variable is not stored; the user supplies it in Octran Code.
fn environment(value: Option<&Value>) -> Result<(Vec<String>, BTreeMap<String, String>), String> {
    let Some(value) = value else {
        return Ok((Vec::new(), BTreeMap::new()));
    };
    let object = value
        .as_object()
        .ok_or_else(|| "Environment variables must be an object.".to_owned())?;
    let mut names = Vec::new();
    let mut secrets = BTreeMap::new();
    for (name, value) in object {
        if !valid_env_name(name) {
            return Err(format!("Environment variable {name} has an invalid name."));
        }
        let text = value
            .as_str()
            .ok_or_else(|| format!("The value of {name} must be text."))?;
        names.push(name.clone());
        if !text.is_empty() && !is_reference(text) {
            secrets.insert(name.clone(), text.to_owned());
        }
    }
    names.sort();
    Ok((names, secrets))
}

fn string_array(value: Option<&Value>) -> Result<Vec<String>, String> {
    match value {
        None => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| "Arguments must be text.".to_owned())
            })
            .collect(),
        Some(_) => Err("Arguments must be a list of text.".to_owned()),
    }
}

/// `${NAME}` and similar references point at another variable, not at a value.
fn is_reference(text: &str) -> bool {
    text.starts_with("${") && text.ends_with('}')
}

/// The server ID for a name: lowercase letters, digits, and hyphens, made unique
/// against the project's servers with a numeric suffix.
fn allocate_id(name: &str, taken: &HashSet<String>) -> String {
    let cleaned: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let trimmed: String = cleaned.trim_matches('-').chars().take(56).collect();
    let base = if trimmed.is_empty() {
        "server".to_owned()
    } else {
        trimmed
    };
    let mut id = base.clone();
    let mut number = 2;
    while taken.contains(&id) {
        id = format!("{base}-{number}");
        number += 1;
    }
    id
}

/// The checks saving applies to credential values, repeated here so a server that
/// cannot be saved is blocked in the preview rather than failing an import midway.
fn check_secret_placement(built: Built) -> Result<Built, String> {
    let public =
        serde_json::to_string(&built.config).map_err(|_| "Could not be read.".to_owned())?;
    if built
        .secrets
        .values()
        .any(|secret| secret.len() > MAX_SECRET_BYTES)
    {
        return Err("A value is larger than 16 KiB.".to_owned());
    }
    if built
        .secrets
        .values()
        .any(|secret| secret.len() >= 8 && public.contains(secret.as_str()))
    {
        return Err(
            "A credential value also appears in the command or URL. Move it to the environment fields."
                .to_owned(),
        );
    }
    Ok(built)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    const LITERAL: &str = "ghp_literalvalue0123456789";

    fn write(root: &Path, relative: &str, text: &str) {
        let path = relative
            .split('/')
            .fold(root.to_path_buf(), |path, part| path.join(part));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn project() -> TempDir {
        tempfile::tempdir().unwrap()
    }

    fn server<'a>(sources: &'a [McpImportSource], key: &str) -> &'a McpImportServer {
        sources
            .iter()
            .flat_map(|source| &source.servers)
            .find(|server| server.key == key)
            .unwrap_or_else(|| panic!("{key} was not listed"))
    }

    #[test]
    fn scan_previews_each_server_without_its_values() {
        let root = project();
        write(
            root.path(),
            ".mcp.json",
            &format!(
                r#"{{"mcpServers": {{
                    "github": {{"command": "npx", "args": ["-y", "@example/github"],
                        "env": {{"GITHUB_TOKEN": "{LITERAL}", "DEBUG": "${{DEBUG}}"}}}},
                    "docs": {{"type": "http", "url": "https://mcp.example.test/mcp",
                        "headers": {{"Authorization": "Bearer ${{DOCS_TOKEN}}"}}}}
                }}}}"#
            ),
        );

        let sources = scan(root.path(), "project-1").unwrap();
        let rendered = serde_json::to_string(&sources).unwrap();

        assert!(
            !rendered.contains(LITERAL),
            "a literal value reached the preview"
        );
        let github = server(&sources, ".mcp.json#github");
        assert_eq!(github.server_id.as_deref(), Some("github"));
        assert_eq!(
            github.transport.as_deref(),
            Some("Command: npx -y @example/github")
        );
        assert_eq!(github.env_names, ["DEBUG", "GITHUB_TOKEN"]);
        assert_eq!(github.stored_env_names, ["GITHUB_TOKEN"]);
        assert!(github
            .warnings
            .iter()
            .any(|warning| warning.contains("Add values for DEBUG")));
        assert!(github.blocked.is_none());
        let docs = server(&sources, ".mcp.json#docs");
        assert_eq!(
            docs.transport.as_deref(),
            Some("URL: https://mcp.example.test/mcp")
        );
        assert!(docs
            .warnings
            .iter()
            .any(|warning| warning.contains("Headers are not imported")));
    }

    #[test]
    fn unsupported_shapes_are_blocked_and_unknown_fields_are_listed() {
        let root = project();
        write(
            root.path(),
            ".cursor/mcp.json",
            r#"{"mcpServers": {
                "legacy": {"type": "sse", "url": "https://mcp.example.test/sse"},
                "tokened": {"command": "tool", "args": ["--api-key=sk-abcdefghijklmnopqrstuvwx"]},
                "tuned": {"command": "tool", "timeout": 30}
            }}"#,
        );

        let sources = scan(root.path(), "project-1").unwrap();

        assert!(server(&sources, ".cursor/mcp.json#legacy")
            .blocked
            .as_deref()
            .is_some_and(|reason| reason.contains("SSE")));
        assert!(server(&sources, ".cursor/mcp.json#tokened")
            .blocked
            .as_deref()
            .is_some_and(|reason| reason.contains("credentials")));
        let tuned = server(&sources, ".cursor/mcp.json#tuned");
        assert!(tuned.blocked.is_none());
        assert!(tuned
            .warnings
            .iter()
            .any(|warning| warning == "Not imported: timeout."));
    }

    #[test]
    fn imported_ids_never_reuse_a_server_already_in_the_project() {
        let root = project();
        write(
            root.path(),
            ".jevcode/mcp.json",
            r#"{"mcpServers": [{"id": "github", "name": "GitHub", "transport": {"type": "stdio", "command": "tool", "args": []}, "envNames": [], "hasAuthToken": false, "enabled": false}]}"#,
        );
        write(
            root.path(),
            ".mcp.json",
            r#"{"mcpServers": {"GitHub": {"command": "tool"}, "Docs Server": {"command": "tool"}}}"#,
        );

        let sources = scan(root.path(), "project-1").unwrap();

        assert_eq!(
            server(&sources, ".mcp.json#GitHub").server_id.as_deref(),
            Some("github-2")
        );
        assert_eq!(
            server(&sources, ".mcp.json#Docs Server")
                .server_id
                .as_deref(),
            Some("docs-server")
        );
    }

    #[test]
    fn unreadable_sources_are_reported_as_problems() {
        let root = project();
        write(root.path(), ".vscode/mcp.json", "{ not json");

        let sources = scan(root.path(), "project-1").unwrap();
        let vscode = sources
            .iter()
            .find(|source| source.path == ".vscode/mcp.json")
            .unwrap();

        assert!(vscode.servers.is_empty());
        assert!(vscode
            .problem
            .as_deref()
            .is_some_and(|reason| reason.contains("Not valid JSON")));
    }

    #[test]
    fn files_without_a_servers_section_are_not_listed() {
        let root = project();
        write(root.path(), ".gemini/settings.json", r#"{"theme": "dark"}"#);

        assert!(scan(root.path(), "project-1").unwrap().is_empty());
    }

    #[tokio::test]
    async fn import_refuses_keys_that_are_unknown_or_blocked() {
        let root = project();
        write(
            root.path(),
            ".cursor/mcp.json",
            r#"{"mcpServers": {"legacy": {"type": "sse", "url": "https://mcp.example.test/sse"}}}"#,
        );
        let database = Database::open(&root.path().join("jevcode.sqlite")).unwrap();
        let manager = McpManager::default();
        let credentials = CredentialStore;

        let unknown = import(
            &manager,
            &database,
            &credentials,
            root.path(),
            "project-1",
            &[".mcp.json#missing".to_owned()],
        )
        .await
        .unwrap_err();
        assert_eq!(unknown.code, "mcp_import_unavailable");

        let blocked = import(
            &manager,
            &database,
            &credentials,
            root.path(),
            "project-1",
            &[".cursor/mcp.json#legacy".to_owned()],
        )
        .await
        .unwrap_err();
        assert_eq!(blocked.code, "mcp_import_blocked");
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_mcp_files_are_reported_and_never_read() {
        let root = project();
        let outside = project();
        write(
            outside.path(),
            "servers.json",
            r#"{"mcpServers": {"x": {"command": "tool"}}}"#,
        );
        std::os::unix::fs::symlink(
            outside.path().join("servers.json"),
            root.path().join(".mcp.json"),
        )
        .unwrap();

        let sources = scan(root.path(), "project-1").unwrap();

        assert!(sources[0]
            .problem
            .as_deref()
            .is_some_and(|reason| reason.contains("Symbolic link")));
        assert!(sources[0].servers.is_empty());
    }
}
