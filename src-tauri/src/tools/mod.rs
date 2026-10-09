//! The only model-facing execution boundary. Tools are described independently
//! from their implementations, validated against JSON Schema, permission-gated,
//! workspace-scoped, and bounded by a per-tool timeout.
mod context;
mod filesystem;
mod process;
mod repository;
mod shell;

use crate::{
    domain::*,
    error::{AppError, AppResult},
};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::{path::Path, time::Instant};

const READ_TIMEOUT_MS: u64 = 10_000;
const EDIT_TIMEOUT_MS: u64 = 10_000;
const GIT_TIMEOUT_MS: u64 = 15_000;
const SHELL_TIMEOUT_MS: u64 = 30_000;

#[derive(Debug)]
struct ToolOutput {
    content: String,
    structured: Value,
}

fn descriptor(
    name: &str,
    description: &str,
    permission: ToolCategory,
    risk_level: ToolRiskLevel,
    timeout_ms: u64,
    parallel_safe: bool,
    input_schema: Value,
) -> Tool {
    Tool {
        name: name.into(),
        description: description.into(),
        permission,
        risk_level,
        timeout_ms,
        parallel_safe,
        input_schema,
    }
}

fn string_property(description: &str) -> Value {
    json!({"type":"string","minLength":1,"maxLength":4096,"description":description})
}

fn optional_path() -> Value {
    string_property("Workspace-relative path. Use . for the project root.")
}

fn builtin_definitions() -> Vec<Tool> {
    let read = ToolCategory::ReadFiles;
    let git = ToolCategory::Git;
    let write = ToolCategory::WriteFiles;
    let shell = ToolCategory::Shell;
    let low = ToolRiskLevel::Low;
    let medium = ToolRiskLevel::Medium;
    let high = ToolRiskLevel::High;
    let critical = ToolRiskLevel::Critical;
    let object = |properties: Value, required: &[&str]| json!({"type":"object","properties":properties,"required":required,"additionalProperties":false});
    let mut tools = vec![
        descriptor("read_file", "Read one UTF-8 file (maximum 64 KiB). Ignored and internal files are excluded.", read.clone(), low, READ_TIMEOUT_MS, true, object(json!({"path":string_property("Workspace-relative file path")}), &["path"])),
        descriptor("read_files", "Read up to 16 UTF-8 files with a combined 256 KiB limit.", read.clone(), low, READ_TIMEOUT_MS, true, object(json!({"paths":{"type":"array","items":string_property("Workspace-relative file path"),"minItems":1,"maxItems":16}}), &["paths"])),
        descriptor("list_directory", "List one project directory. Results are paginated and respect .gitignore.", read.clone(), low, READ_TIMEOUT_MS, true, object(json!({"path":optional_path(),"offset":{"type":"integer","minimum":0,"maximum":100000},"limit":{"type":"integer","minimum":1,"maximum":100}}), &[])),
        descriptor("search_files", "Find file paths by glob or substring using ripgrep when available; respects ignore rules.", read.clone(), low, READ_TIMEOUT_MS, true, object(json!({"query":string_property("File name or glob pattern"),"path":optional_path(),"offset":{"type":"integer","minimum":0,"maximum":100000},"limit":{"type":"integer","minimum":1,"maximum":100}}), &["query"])),
        descriptor("search_text", "Search text in visible project files with bounded matching lines.", read.clone(), low, READ_TIMEOUT_MS, true, object(json!({"query":string_property("Literal search text"),"path":optional_path(),"offset":{"type":"integer","minimum":0,"maximum":100000},"limit":{"type":"integer","minimum":1,"maximum":100}}), &["query"])),
        descriptor("file_metadata", "Return bounded metadata for one visible project path.", read.clone(), low, READ_TIMEOUT_MS, true, object(json!({"path":string_property("Workspace-relative path")}), &["path"])),
        descriptor("git_status", "Read concise Git status for the selected repository.", git.clone(), low, GIT_TIMEOUT_MS, true, object(json!({}), &[])),
        descriptor("git_diff", "Read a bounded Git diff, optionally for one path or the index.", git.clone(), medium, GIT_TIMEOUT_MS, true, object(json!({"path":string_property("Optional workspace-relative path"),"staged":{"type":"boolean"},"offset":{"type":"integer","minimum":0,"maximum":100000},"limit":{"type":"integer","minimum":1,"maximum":400}}), &[])),
        descriptor("git_log", "Read recent commit summaries, optionally scoped to one path.", git.clone(), low, GIT_TIMEOUT_MS, true, object(json!({"path":string_property("Optional workspace-relative path"),"limit":{"type":"integer","minimum":1,"maximum":50}}), &[])),
        descriptor("git_show", "Show a paginated commit summary and patch, excluding protected files.", git.clone(), medium, GIT_TIMEOUT_MS, true, object(json!({"revision":{"type":"string","minLength":1,"maxLength":200,"description":"Commit ID or ref; defaults to HEAD"},"offset":{"type":"integer","minimum":0,"maximum":100000},"limit":{"type":"integer","minimum":1,"maximum":400}}), &[])),
        descriptor("git_branch", "List local branches and report the checked-out branch.", git.clone(), low, GIT_TIMEOUT_MS, true, object(json!({}), &[])),
        descriptor("find_symbol", "Find occurrences of a symbol in visible source files.", read.clone(), low, READ_TIMEOUT_MS, true, object(json!({"symbol":string_property("Symbol name"),"path":optional_path(),"offset":{"type":"integer","minimum":0,"maximum":100000},"limit":{"type":"integer","minimum":1,"maximum":100}}), &["symbol"])),
        descriptor("find_references", "Find text references to a symbol in visible source files.", read.clone(), low, READ_TIMEOUT_MS, true, object(json!({"symbol":string_property("Symbol name"),"path":optional_path(),"offset":{"type":"integer","minimum":0,"maximum":100000},"limit":{"type":"integer","minimum":1,"maximum":100}}), &["symbol"])),
        descriptor("inspect_project", "Summarize project files, repository state, languages, and size with bounded scans.", read.clone(), low, READ_TIMEOUT_MS, true, object(json!({}), &[])),
        descriptor("apply_patch", "Apply exact, unique text edits to a workspace file. Prefer small patches to rewriting whole files.", write.clone(), high, EDIT_TIMEOUT_MS, false, object(json!({"path":string_property("Workspace-relative file path"),"edits":{"type":"array","items":{"type":"object","properties":{"oldText":{"type":"string","minLength":1,"maxLength":65536},"newText":{"type":"string","maxLength":65536}},"required":["oldText","newText"],"additionalProperties":false},"minItems":1,"maxItems":16}}), &["path","edits"])),
        descriptor("create_file", "Create a new UTF-8 project file without replacing an existing file.", write.clone(), high, EDIT_TIMEOUT_MS, false, object(json!({"path":string_property("Workspace-relative file path"),"content":{"type":"string","maxLength":262144}}), &["path","content"])),
        descriptor("delete_file", "Delete one regular project file. Directories cannot be recursively deleted.", write.clone(), high, EDIT_TIMEOUT_MS, false, object(json!({"path":string_property("Workspace-relative file path")}), &["path"])),
        descriptor("move_file", "Move one project file to a new workspace-relative destination without overwriting.", write.clone(), high, EDIT_TIMEOUT_MS, false, object(json!({"source":string_property("Existing workspace-relative file"),"destination":string_property("New workspace-relative destination")}), &["source","destination"])),
        descriptor("run_command", "Run one executable with argument-array semantics in the project. No shell interpolation; requires permission.", shell, critical, SHELL_TIMEOUT_MS, false, object(json!({"program":{"type":"string","minLength":1,"maxLength":256},"args":{"type":"array","items":{"type":"string","maxLength":4096},"maxItems":128},"path":optional_path()}), &["program","args"])),
        descriptor("ask_user", "Ask one concise question and pause until the user answers.", ToolCategory::UserInteraction, low, READ_TIMEOUT_MS, false, object(json!({"question":string_property("A concise question for the user")}), &["question"])),
    ];
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    tools
}

/// Trait for the registry interface. Concrete implementations remain in Rust;
/// only serializable descriptors and structured results cross IPC/provider APIs.
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    fn definitions(&self) -> Vec<Tool>;
    fn validate(&self, call: &ToolCall) -> AppResult<Tool>;
    async fn execute(
        &self,
        root: &Path,
        call: &ToolCall,
        policy: &PermissionPolicy,
        approved: bool,
    ) -> ToolResult;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ToolRegistry;

#[async_trait]
impl ToolExecutor for ToolRegistry {
    fn definitions(&self) -> Vec<Tool> {
        builtin_definitions()
    }

    fn validate(&self, call: &ToolCall) -> AppResult<Tool> {
        validate_call(call)
    }

    async fn execute(
        &self,
        root: &Path,
        call: &ToolCall,
        policy: &PermissionPolicy,
        approved: bool,
    ) -> ToolResult {
        let started = Instant::now();
        let descriptor = match self.validate(call) {
            Ok(descriptor) => descriptor,
            Err(error) => return tool_error(call, error, started.elapsed().as_millis() as u64),
        };
        if call.name == "ask_user" {
            return tool_error(
                call,
                AppError::new(
                    "runtime_tool",
                    "The user interaction tool must be handled by the agent runtime.",
                ),
                started.elapsed().as_millis() as u64,
            );
        }
        let external = match requires_external_access(root, call) {
            Ok(external) => external,
            Err(error) => return tool_error(call, error, started.elapsed().as_millis() as u64),
        };
        if policy.decision(&descriptor.permission) == PermissionDecision::Deny {
            return tool_error(
                call,
                AppError::new(
                    "permission_denied",
                    "This tool is denied by the session policy.",
                ),
                started.elapsed().as_millis() as u64,
            );
        }
        if external {
            match policy.external_files {
                PermissionDecision::Deny => return tool_error(call, AppError::new("permission_denied", "This path is outside the active project and outside-project access is denied."), started.elapsed().as_millis() as u64),
                PermissionDecision::Ask if !approved => return tool_error(call, AppError::new("approval_required", "This path is outside the active project and needs explicit approval."), started.elapsed().as_millis() as u64),
                _ => {}
            }
        }
        if policy.decision(&descriptor.permission) == PermissionDecision::Ask && !approved {
            return tool_error(
                call,
                AppError::new("approval_required", "This tool needs user approval."),
                started.elapsed().as_millis() as u64,
            );
        }
        let timeout = std::time::Duration::from_millis(descriptor.timeout_ms);
        let output = tokio::time::timeout(
            timeout,
            dispatch(
                root,
                call,
                external && (approved || policy.external_files == PermissionDecision::Allow),
            ),
        )
        .await;
        let elapsed = started.elapsed().as_millis() as u64;
        match output {
            Ok(Ok(output)) => ToolResult {
                tool_call_id: call.id.clone(),
                name: call.name.clone(),
                content: output.content,
                is_error: false,
                duration_ms: elapsed,
                structured_content: Some(output.structured),
            },
            Ok(Err(error)) => tool_error(call, error, elapsed),
            Err(_) => tool_error(
                call,
                AppError::new("tool_timeout", "The tool exceeded its execution timeout."),
                elapsed,
            ),
        }
    }
}

pub fn definitions() -> Vec<Tool> {
    ToolRegistry.definitions()
}

pub fn validate_call(call: &ToolCall) -> AppResult<Tool> {
    let tool = definitions()
        .into_iter()
        .find(|tool| tool.name == call.name)
        .ok_or_else(|| AppError::new("unknown_tool", "The requested tool is not registered."))?;
    validate_schema(&call.arguments, &tool.input_schema, "$")?;
    Ok(tool)
}

/// Detect requests outside the active project before execution so the runtime
/// can pause and obtain an explicit user approval.
pub fn requires_external_access(root: &Path, call: &ToolCall) -> AppResult<bool> {
    let root = std::fs::canonicalize(root)?;
    let mut paths = Vec::new();
    match call.name.as_str() {
        "read_files" => paths.extend(
            call.arguments["paths"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str),
        ),
        "move_file" => {
            paths.extend(
                [
                    call.arguments["source"].as_str(),
                    call.arguments["destination"].as_str(),
                ]
                .into_iter()
                .flatten(),
            );
        }
        "run_command" | "inspect_project" | "git_status" | "git_branch" | "git_log"
        | "git_diff" | "git_show" | "ask_user" => {}
        _ => paths.extend(call.arguments["path"].as_str()),
    }
    for value in paths {
        let path = Path::new(value);
        let mut candidate = if path.is_absolute() {
            path.to_path_buf()
        } else {
            root.join(path)
        };
        loop {
            if let Ok(canonical) = std::fs::canonicalize(&candidate) {
                if !canonical.starts_with(&root) {
                    return Ok(true);
                }
                break;
            }
            if !candidate.pop() {
                break;
            }
        }
    }
    Ok(false)
}

fn validate_schema(value: &Value, schema: &Value, at: &str) -> AppResult<()> {
    let invalid = || {
        AppError::new(
            "invalid_tool_arguments",
            format!("Tool arguments do not match the registered schema at {at}."),
        )
    };
    if let Some(options) = schema["enum"].as_array() {
        if !options.contains(value) {
            return Err(invalid());
        }
    }
    let type_name = schema["type"].as_str().unwrap_or_default();
    let valid_type = match type_name {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "integer" => value.as_i64().is_some(),
        "number" => value.as_f64().is_some(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        _ => false,
    };
    if !valid_type {
        return Err(invalid());
    }
    match type_name {
        "object" => {
            let object = value.as_object().ok_or_else(invalid)?;
            for required in schema["required"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                if !object.contains_key(required) {
                    return Err(invalid());
                }
            }
            if schema["additionalProperties"] == false
                && object
                    .keys()
                    .any(|key| schema["properties"].get(key).is_none())
            {
                return Err(invalid());
            }
            for (key, child) in object {
                let Some(child_schema) = schema["properties"].get(key) else {
                    continue;
                };
                validate_schema(child, child_schema, &format!("{at}.{key}"))?;
            }
        }
        "array" => {
            let array = value.as_array().ok_or_else(invalid)?;
            if schema["minItems"]
                .as_u64()
                .is_some_and(|minimum| array.len() < minimum as usize)
                || schema["maxItems"]
                    .as_u64()
                    .is_some_and(|maximum| array.len() > maximum as usize)
            {
                return Err(invalid());
            }
            if let Some(item_schema) = schema.get("items") {
                for (index, child) in array.iter().enumerate() {
                    validate_schema(child, item_schema, &format!("{at}[{index}]"))?;
                }
            }
        }
        "string" => {
            let string = value.as_str().ok_or_else(invalid)?;
            if schema["minLength"]
                .as_u64()
                .is_some_and(|minimum| string.chars().count() < minimum as usize)
                || schema["maxLength"]
                    .as_u64()
                    .is_some_and(|maximum| string.chars().count() > maximum as usize)
            {
                return Err(invalid());
            }
        }
        "integer" | "number" => {
            let number = value.as_f64().ok_or_else(invalid)?;
            if schema["minimum"]
                .as_f64()
                .is_some_and(|minimum| number < minimum)
                || schema["maximum"]
                    .as_f64()
                    .is_some_and(|maximum| number > maximum)
            {
                return Err(invalid());
            }
        }
        _ => {}
    }
    Ok(())
}

pub async fn execute(
    root: &Path,
    call: &ToolCall,
    policy: &PermissionPolicy,
    approved: bool,
) -> ToolResult {
    ToolRegistry.execute(root, call, policy, approved).await
}

fn tool_error(call: &ToolCall, error: AppError, duration_ms: u64) -> ToolResult {
    ToolResult {
        tool_call_id: call.id.clone(),
        name: call.name.clone(),
        content: error.message.clone(),
        is_error: true,
        duration_ms,
        structured_content: Some(json!({"error":error.code,"message":error.message})),
    }
}

async fn dispatch(root: &Path, call: &ToolCall, external_allowed: bool) -> AppResult<ToolOutput> {
    match call.name.as_str() {
        "read_file" => filesystem::read_file(root, &call.arguments["path"], external_allowed).await,
        "read_files" => {
            filesystem::read_files(root, &call.arguments["paths"], external_allowed).await
        }
        "list_directory" => {
            filesystem::list_directory(root, &call.arguments, external_allowed).await
        }
        "search_files" => filesystem::search_files(root, &call.arguments, external_allowed).await,
        "search_text" => filesystem::search_text(root, &call.arguments, external_allowed).await,
        "file_metadata" => {
            filesystem::file_metadata(root, &call.arguments["path"], external_allowed).await
        }
        "apply_patch" => filesystem::apply_patch(root, &call.arguments, external_allowed).await,
        "create_file" => filesystem::create_file(root, &call.arguments, external_allowed).await,
        "delete_file" => filesystem::delete_file(root, &call.arguments, external_allowed).await,
        "move_file" => filesystem::move_file(root, &call.arguments, external_allowed).await,
        "git_status" | "git_diff" | "git_log" | "git_show" | "git_branch" => {
            repository::execute(root, call.name.as_str(), &call.arguments).await
        }
        "run_command" => shell::run_command(root, &call.arguments).await,
        "find_symbol" => filesystem::find_symbol(root, &call.arguments, external_allowed).await,
        "find_references" => {
            filesystem::find_references(root, &call.arguments, external_allowed).await
        }
        "inspect_project" => context::inspect_project(root).await,
        _ => Err(AppError::new(
            "unknown_tool",
            "The requested tool is not registered.",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn call(name: &str, arguments: Value) -> ToolCall {
        ToolCall {
            id: "test-call".into(),
            name: name.into(),
            arguments,
        }
    }

    fn git(root: &Path, args: &[&str]) {
        let result = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .expect("start git test command");
        assert!(
            result.status.success(),
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&result.stderr)
        );
    }

    #[test]
    fn catalog_covers_requested_tools_with_explicit_risk_and_permissions() {
        let names: Vec<_> = definitions()
            .into_iter()
            .map(|tool| {
                assert!(tool.timeout_ms > 0);
                assert!(matches!(
                    tool.risk_level,
                    ToolRiskLevel::Low
                        | ToolRiskLevel::Medium
                        | ToolRiskLevel::High
                        | ToolRiskLevel::Critical
                ));
                assert_eq!(tool.input_schema["additionalProperties"], false);
                tool.name
            })
            .collect();
        for name in [
            "read_file",
            "read_files",
            "list_directory",
            "search_files",
            "search_text",
            "file_metadata",
            "apply_patch",
            "create_file",
            "delete_file",
            "move_file",
            "git_status",
            "git_diff",
            "git_log",
            "git_show",
            "git_branch",
            "run_command",
            "find_symbol",
            "find_references",
            "inspect_project",
            "ask_user",
        ] {
            assert!(
                names.iter().any(|registered| registered == name),
                "missing {name}"
            );
        }
    }

    #[test]
    fn schema_validation_rejects_unknown_missing_nested_and_oversized_arguments() {
        let valid = call("read_file", json!({"path":"src/main.rs"}));
        assert!(validate_call(&valid).is_ok());
        assert!(validate_call(&call(
            "read_file",
            json!({"path":"src/main.rs","root":"C:/"})
        ))
        .is_err());
        assert!(validate_call(&call("read_file", json!({}))).is_err());
        assert!(validate_call(&call("read_files", json!({"paths":[]}))).is_err());
        assert!(validate_call(&call(
            "run_command",
            json!({"program":"git","args":[],"extra":true})
        ))
        .is_err());
    }

    #[tokio::test]
    async fn filesystem_tools_reject_parent_traversal_and_symlink_escape() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "private").unwrap();
        let policy = PermissionPolicy {
            read_files: PermissionDecision::Allow,
            external_files: PermissionDecision::Deny,
            ..Default::default()
        };
        let traversal = call("read_file", json!({"path":"../outside/secret.txt"}));
        assert!(requires_external_access(root.path(), &traversal).unwrap());
        assert!(
            execute(root.path(), &traversal, &policy, false)
                .await
                .is_error
        );

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.path(), root.path().join("linked-outside")).unwrap();
            let symlink = call("read_file", json!({"path":"linked-outside/secret.txt"}));
            let result = execute(root.path(), &symlink, &policy, false).await;
            assert!(result.is_error);
            assert_eq!(
                result.structured_content.unwrap()["error"],
                "permission_denied"
            );
        }
    }

    #[tokio::test]
    async fn external_file_access_pauses_until_explicit_approval() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let file = outside.path().join("notes.txt");
        std::fs::write(&file, "approved outside read").unwrap();
        let call = call("read_file", json!({"path":file.to_string_lossy()}));
        let policy = PermissionPolicy {
            read_files: PermissionDecision::Allow,
            external_files: PermissionDecision::Ask,
            ..Default::default()
        };
        let pending = execute(root.path(), &call, &policy, false).await;
        assert!(pending.is_error);
        assert_eq!(
            pending.structured_content.unwrap()["error"],
            "approval_required"
        );
        let approved = execute(root.path(), &call, &policy, true).await;
        assert!(!approved.is_error);
        assert_eq!(approved.content, "approved outside read");
    }

    #[tokio::test]
    async fn edits_require_policy_and_apply_small_patches_without_full_file_rewrites() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("main.rs"),
            "fn main() {\n    println!(\"old\");\n}\n",
        )
        .unwrap();
        let denied = PermissionPolicy {
            write_files: PermissionDecision::Deny,
            ..Default::default()
        };
        let patch = call(
            "apply_patch",
            json!({"path":"main.rs","edits":[{"oldText":"println!(\"old\")","newText":"println!(\"new\")"}]}),
        );
        assert!(execute(root.path(), &patch, &denied, true).await.is_error);
        let allowed = PermissionPolicy {
            write_files: PermissionDecision::Allow,
            ..Default::default()
        };
        let result = execute(root.path(), &patch, &allowed, false).await;
        assert!(!result.is_error);
        assert_eq!(
            std::fs::read_to_string(root.path().join("main.rs")).unwrap(),
            "fn main() {\n    println!(\"new\");\n}\n"
        );
        std::fs::write(
            root.path().join("duplicate.rs"),
            "fn main() {}\nfn main() {}\n",
        )
        .unwrap();
        let ambiguous = call(
            "apply_patch",
            json!({"path":"duplicate.rs","edits":[{"oldText":"fn main","newText":"fn test"}]}),
        );
        let result = execute(root.path(), &ambiguous, &allowed, false).await;
        assert!(result.is_error);
        assert_eq!(
            result.structured_content.unwrap()["error"],
            "patch_context_ambiguous"
        );
    }

    #[tokio::test]
    async fn create_file_allows_new_visible_paths_and_rejects_gitignored_paths() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join(".gitignore"), "ignored.txt\n").unwrap();
        let allowed = PermissionPolicy {
            write_files: PermissionDecision::Allow,
            ..Default::default()
        };
        let visible = call(
            "create_file",
            json!({"path":"created.txt","content":"hello"}),
        );
        let created = execute(root.path(), &visible, &allowed, false).await;
        assert!(!created.is_error, "{}", created.content);
        assert_eq!(
            std::fs::read_to_string(root.path().join("created.txt")).unwrap(),
            "hello"
        );

        let ignored = call(
            "create_file",
            json!({"path":"ignored.txt","content":"secret"}),
        );
        let rejected = execute(root.path(), &ignored, &allowed, false).await;
        assert!(rejected.is_error);
        assert_eq!(
            rejected.structured_content.unwrap()["error"],
            "permission_denied"
        );
    }

    #[tokio::test]
    async fn directory_pages_return_paths_relative_to_the_active_workspace() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::write(root.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        let result = execute(
            root.path(),
            &call("list_directory", json!({"path":"src","limit":1})),
            &PermissionPolicy::default(),
            false,
        )
        .await;
        assert!(!result.is_error, "{}", result.content);
        let data = result.structured_content.unwrap();
        assert_eq!(data["entries"][0]["path"], "src/main.rs");
        assert_eq!(data["hasMore"], false);
    }

    #[tokio::test]
    async fn repository_diffs_and_shows_exclude_protected_file_contents() {
        let root = tempfile::Builder::new()
            .prefix("jevcode-tool-git-")
            .tempdir_in(Path::new(env!("CARGO_MANIFEST_DIR")).join("target"))
            .unwrap();
        git(root.path(), &["init", "--quiet"]);
        git(root.path(), &["config", "user.name", "Tool Test"]);
        git(
            root.path(),
            &["config", "user.email", "tools@example.invalid"],
        );
        std::fs::write(root.path().join("README.md"), "before\n").unwrap();
        std::fs::write(root.path().join(".env.test"), "SECRET_SENTINEL\n").unwrap();
        git(root.path(), &["add", "README.md", ".env.test"]);
        git(root.path(), &["commit", "--quiet", "-m", "initial"]);
        std::fs::write(root.path().join("README.md"), "after\n").unwrap();
        std::fs::write(root.path().join(".env.test"), "ANOTHER_SECRET_SENTINEL\n").unwrap();

        let policy = PermissionPolicy {
            git: PermissionDecision::Allow,
            ..Default::default()
        };
        let diff = execute(
            root.path(),
            &call("git_diff", json!({"limit":100})),
            &policy,
            false,
        )
        .await;
        assert!(!diff.is_error, "{}", diff.content);
        assert!(diff.content.contains("README.md"));
        assert!(!diff.content.contains("SECRET_SENTINEL"));

        let show = execute(
            root.path(),
            &call("git_show", json!({"revision":"HEAD"})),
            &policy,
            false,
        )
        .await;
        assert!(!show.is_error, "{}", show.content);
        assert!(show.content.contains("README.md"));
        assert!(!show.content.contains("SECRET_SENTINEL"));
    }

    #[tokio::test]
    async fn ripgrep_search_obeys_ignore_rules_and_returns_structured_pages() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join(".gitignore"), "ignored.txt\n").unwrap();
        std::fs::write(
            root.path().join("visible.txt"),
            "needle on line one\nsecond\n",
        )
        .unwrap();
        std::fs::write(root.path().join("ignored.txt"), "needle must not leak\n").unwrap();
        let policy = PermissionPolicy::default();
        let result = execute(
            root.path(),
            &call("search_text", json!({"query":"needle","limit":1})),
            &policy,
            false,
        )
        .await;
        assert!(!result.is_error, "{}", result.content);
        assert!(
            result.content.contains("visible.txt:1"),
            "content={:?}; structured={:?}",
            result.content,
            result.structured_content
        );
        assert!(!result.content.contains("ignored.txt"));
        assert_eq!(
            result.structured_content.unwrap()["matches"][0]["path"],
            "visible.txt"
        );
    }
}
