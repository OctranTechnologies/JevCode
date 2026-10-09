use crate::{
    domain::*,
    error::{AppError, AppResult},
    git,
    workspaces::{is_ignored, list_directory, scoped_path},
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
            parallel_safe: true,
            input_schema: json!({"type":"object","properties":{"path":{"type":"string","description":"Relative directory; use . for the project root"}},"required":["path"],"additionalProperties":false}),
        },
        Tool {
            name: "read_file".into(),
            description: "Read a UTF-8 project file up to 64 KiB. Credential files are blocked."
                .into(),
            category: ToolCategory::ReadFiles,
            parallel_safe: true,
            input_schema: json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}),
        },
        Tool {
            name: "git_status".into(),
            description:
                "Read Git status for the selected project. Does not modify the repository.".into(),
            category: ToolCategory::Git,
            parallel_safe: true,
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
        },
        Tool {
            name: "ask_user".into(),
            description: "Ask the user one concise question when required information is missing. Pause until they answer.".into(),
            category: ToolCategory::UserInteraction,
            parallel_safe: false,
            input_schema: json!({"type":"object","properties":{"question":{"type":"string","description":"A concise question for the user"}},"required":["question"],"additionalProperties":false}),
        },
    ]
}

pub fn definition(name: &str) -> AppResult<Tool> {
    definitions()
        .into_iter()
        .find(|tool| tool.name == name)
        .ok_or_else(|| AppError::new("unknown_tool", "The requested tool is not registered."))
}

pub fn validate_call(call: &ToolCall) -> AppResult<Tool> {
    let tool = definition(&call.name)?;
    let arguments = call.arguments.as_object().ok_or_else(|| {
        AppError::new(
            "invalid_tool_arguments",
            "Tool arguments must be a JSON object.",
        )
    })?;
    let properties = tool.input_schema["properties"].as_object().ok_or_else(|| {
        AppError::new(
            "tool_schema",
            "The registered tool has an invalid input schema.",
        )
    })?;
    for required in tool.input_schema["required"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let key = required.as_str().unwrap_or_default();
        if !arguments.contains_key(key) {
            return Err(AppError::new(
                "invalid_tool_arguments",
                "The model omitted a required tool argument.",
            ));
        }
    }
    if arguments.keys().any(|key| !properties.contains_key(key)) {
        return Err(AppError::new(
            "invalid_tool_arguments",
            "The model supplied an unsupported tool argument.",
        ));
    }
    for (key, value) in arguments {
        let expected = properties[key]["type"].as_str().unwrap_or_default();
        let valid = match expected {
            "string" => value
                .as_str()
                .is_some_and(|value| !value.is_empty() && value.len() <= 4096),
            "boolean" => value.is_boolean(),
            "integer" => value.as_i64().is_some(),
            "number" => value.as_f64().is_some(),
            "object" => value.is_object(),
            "array" => value.is_array(),
            _ => false,
        };
        if !valid {
            return Err(AppError::new(
                "invalid_tool_arguments",
                "A tool argument did not match the registered input schema.",
            ));
        }
    }
    Ok(tool)
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
    let tool = validate_call(call)?;
    if call.name == "ask_user" {
        return Err(AppError::new(
            "runtime_tool",
            "The user interaction tool must be handled by the agent runtime.",
        ));
    }
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
            if is_ignored(root, relative) {
                return Err(AppError::new(
                    "permission_denied",
                    "This file is excluded by the project's ignore rules.",
                ));
            }
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
        "list_files" => Ok(list_directory(root, relative)?
            .into_iter()
            .map(|entry| {
                if entry.kind == "directory" {
                    format!("{}/", entry.name)
                } else {
                    entry.name
                }
            })
            .collect::<Vec<_>>()
            .join("\n")),
        _ => Err(AppError::new(
            "unknown_tool",
            "The requested tool is not registered.",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_requests_must_match_registered_schemas() {
        let valid = ToolCall {
            id: "1".into(),
            name: "read_file".into(),
            arguments: json!({"path":"README.md"}),
        };
        assert!(validate_call(&valid).is_ok());

        let extra = ToolCall {
            arguments: json!({"path":"README.md","outsideWorkspace":true}),
            ..valid.clone()
        };
        assert_eq!(
            validate_call(&extra).unwrap_err().code,
            "invalid_tool_arguments"
        );

        let missing = ToolCall {
            arguments: json!({}),
            ..valid
        };
        assert_eq!(
            validate_call(&missing).unwrap_err().code,
            "invalid_tool_arguments"
        );
    }

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
