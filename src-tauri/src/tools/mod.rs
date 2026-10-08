use crate::{
    domain::*,
    error::{AppError, AppResult},
    git,
    workspaces::scoped_path,
};
use serde_json::json;
use std::{path::Path, time::Instant};

pub fn definitions() -> Vec<Tool> {
    vec![
        Tool {
            name: "list_files".into(),
            description: "List one project directory. Internal and credential files are excluded."
                .into(),
            category: ToolCategory::ReadFiles,
            input_schema: json!({"type":"object","properties":{"path":{"type":"string","description":"Relative directory; use . for the project root"}},"required":["path"],"additionalProperties":false}),
        },
        Tool {
            name: "read_file".into(),
            description: "Read a UTF-8 project file up to 64 KiB. Credential files are blocked."
                .into(),
            category: ToolCategory::ReadFiles,
            input_schema: json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}),
        },
        Tool {
            name: "git_status".into(),
            description:
                "Read Git status for the selected project. Does not modify the repository.".into(),
            category: ToolCategory::Git,
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
        },
    ]
}

pub fn definition(name: &str) -> AppResult<Tool> {
    definitions()
        .into_iter()
        .find(|tool| tool.name == name)
        .ok_or_else(|| AppError::new("unknown_tool", "The requested tool is not registered."))
}

/// Every execution, including an approved call, rechecks its policy and scope.
pub async fn execute(
    root: &Path,
    call: &ToolCall,
    policy: &PermissionPolicy,
    approved: bool,
) -> ToolResult {
    let start = Instant::now();
    let outcome = execute_checked(root, call, policy, approved).await;
    ToolResult {
        tool_call_id: call.id.clone(),
        name: call.name.clone(),
        is_error: outcome.is_err(),
        content: outcome.unwrap_or_else(|error| error.message),
        duration_ms: start.elapsed().as_millis() as u64,
    }
}

async fn execute_checked(
    root: &Path,
    call: &ToolCall,
    policy: &PermissionPolicy,
    approved: bool,
) -> AppResult<String> {
    let tool = definition(&call.name)?;
    match policy.decision(&tool.category) {
        PermissionDecision::Deny => {
            return Err(AppError::new(
                "permission_denied",
                "This tool is denied by the session policy.",
            ))
        }
        PermissionDecision::Ask if !approved => {
            return Err(AppError::new(
                "approval_required",
                "This tool needs user approval.",
            ))
        }
        _ => {}
    }
    if tool.category == ToolCategory::Git {
        return git::status(root).await;
    }
    let relative = call
        .arguments
        .get("path")
        .and_then(|value| value.as_str())
        .ok_or_else(|| AppError::new("invalid_input", "The tool requires a relative path."))?;
    let path = scoped_path(root, relative)?;
    match call.name.as_str() {
        "read_file" => {
            let metadata = tokio::fs::metadata(&path).await?;
            if !metadata.is_file() || metadata.len() > 65536 {
                return Err(AppError::new(
                    "file_limit",
                    "Choose a text file smaller than 64 KiB.",
                ));
            }
            let bytes = tokio::fs::read(&path).await?;
            if bytes.len() > 65536 {
                return Err(AppError::new(
                    "file_limit",
                    "The file grew past the 64 KiB limit.",
                ));
            }
            String::from_utf8(bytes)
                .map_err(|_| AppError::new("binary_file", "This tool only reads UTF-8 text files."))
        }
        "list_files" => {
            let mut entries = tokio::fs::read_dir(path).await?;
            let mut names = Vec::new();
            while let Some(entry) = entries.next_entry().await? {
                let name = entry.file_name().to_string_lossy().to_string();
                let child = Path::new(relative).join(&name);
                if scoped_path(root, &child.to_string_lossy()).is_ok() {
                    let suffix = if entry.file_type().await?.is_dir() {
                        "/"
                    } else {
                        ""
                    };
                    names.push(format!("{name}{suffix}"));
                }
                if names.len() >= 250 {
                    names.push("[Listing limited to 250 entries]".into());
                    break;
                }
            }
            names.sort();
            Ok(names.join("\n"))
        }
        _ => Err(AppError::new(
            "unknown_tool",
            "The requested tool is not registered.",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn deny_cannot_be_overridden_by_approval() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("test.txt"), "hello").unwrap();
        let policy = PermissionPolicy {
            read_files: PermissionDecision::Deny,
            ..Default::default()
        };
        let call = ToolCall {
            id: "1".into(),
            name: "read_file".into(),
            arguments: json!({"path":"test.txt"}),
        };
        assert!(execute(root.path(), &call, &policy, true).await.is_error);
        let policy = PermissionPolicy {
            read_files: PermissionDecision::Ask,
            ..Default::default()
        };
        assert!(execute(root.path(), &call, &policy, false).await.is_error);
        assert_eq!(
            execute(root.path(), &call, &policy, true).await.content,
            "hello"
        );
    }
}
