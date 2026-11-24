//! Detects the external coding-agent tools installed on this computer. Detection
//! looks for their commands on PATH. Task execution starts only supported adapters;
//! no login, token, or settings file is read by Octran Code. An IDE or desktop app
//! is not a command on PATH, so it is not detected and not assumed to exist.
use crate::{
    domain::{PermissionDecision, PermissionMode},
    error::{AppError, AppResult},
};
use serde::Serialize;
use std::{
    env,
    path::{Path, PathBuf},
};

/// Command-line tools Octran Code recognizes. Each keeps its own sign-in.
const HARNESSES: [(&str, &str, &str, Option<&str>); 5] = [
    (
        "codex",
        "Codex CLI",
        "codex",
        Some("workspace-write sandbox; permission escalations disabled; user config excluded"),
    ),
    (
        "claude_code",
        "Claude Code",
        "claude",
        Some("restricted file tools; shell, MCP, and browser tools are disabled"),
    ),
    ("gemini_cli", "Gemini CLI", "gemini", None),
    ("opencode", "OpenCode", "opencode", None),
    ("antigravity", "Antigravity CLI", "agy", None),
];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessStatus {
    pub id: &'static str,
    pub name: &'static str,
    /// The command Octran Code looks for on PATH.
    pub executable: &'static str,
    pub installed: bool,
    /// Whether Octran Code has a bounded, task-worktree execution adapter.
    pub task_execution_supported: bool,
    /// The exact restrictions used by the task adapter, when supported.
    pub task_permission_summary: Option<&'static str>,
}

pub fn detect() -> Vec<HarnessStatus> {
    let directories: Vec<PathBuf> = env::var_os("PATH")
        .map(|value| env::split_paths(&value).collect())
        .unwrap_or_default();
    detect_in(&directories)
}

fn detect_in(directories: &[PathBuf]) -> Vec<HarnessStatus> {
    HARNESSES
        .iter()
        .map(
            |&(id, name, executable, permission_summary)| HarnessStatus {
                id,
                name,
                executable,
                installed: directories
                    .iter()
                    .any(|directory| on_path(directory, executable)),
                task_execution_supported: permission_summary.is_some(),
                task_permission_summary: permission_summary,
            },
        )
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarnessId {
    Codex,
    ClaudeCode,
}

impl HarnessId {
    pub fn parse(value: &str) -> AppResult<Self> {
        match value {
            "codex" => Ok(Self::Codex),
            "claude_code" => Ok(Self::ClaudeCode),
            _ => Err(AppError::new(
                "unsupported_harness",
                "Choose Codex CLI or Claude Code for task execution.",
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::ClaudeCode => "claude_code",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Codex => "Codex CLI",
            Self::ClaudeCode => "Claude Code",
        }
    }
}

pub fn validate_task_permissions(
    harness: HarnessId,
    mode: PermissionMode,
    read_files: PermissionDecision,
    write_files: PermissionDecision,
    git: PermissionDecision,
    shell: PermissionDecision,
) -> AppResult<()> {
    if read_files != PermissionDecision::Allow {
        return Err(AppError::new(
            "harness_read_permission_required",
            "Allow task file reads before running an external coding tool.",
        ));
    }
    if write_files != PermissionDecision::Allow || mode == PermissionMode::Ask {
        return Err(AppError::new(
            "harness_write_permission_required",
            "Allow task file edits before running an external coding tool.",
        ));
    }
    if harness == HarnessId::Codex
        && (shell != PermissionDecision::Allow
            || git != PermissionDecision::Allow
            || mode != PermissionMode::FullAccess)
    {
        return Err(AppError::new(
            "harness_shell_permission_required",
            "Codex task runs execute commands in the workspace sandbox. Set this task to Full Access and allow task commands and Git operations first.",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessCommand {
    pub executable: &'static str,
    pub args: Vec<String>,
    pub stdin: String,
}

/// Build a fixed, non-interactive command. The prompt and model remain single
/// arguments; neither is interpreted by a shell.
pub fn task_command(
    harness: HarnessId,
    model: Option<&str>,
    prompt: &str,
) -> AppResult<HarnessCommand> {
    let prompt = prompt.trim();
    if prompt.is_empty() || prompt.len() > 65_536 || prompt.contains('\0') {
        return Err(AppError::new(
            "invalid_input",
            "Enter a task prompt under 64 KiB.",
        ));
    }
    let model = model.map(str::trim).filter(|value| !value.is_empty());
    if model.is_some_and(|value| {
        value.len() > 120
            || value.chars().any(char::is_control)
            || value.chars().any(char::is_whitespace)
    }) {
        return Err(AppError::new(
            "invalid_harness_model",
            "Model names must be under 120 characters and contain no whitespace or control characters.",
        ));
    }
    let mut args = match harness {
        HarnessId::Codex => vec![
            "exec".into(),
            "--json".into(),
            "--ignore-user-config".into(),
            "--sandbox".into(),
            "workspace-write".into(),
            "--ask-for-approval".into(),
            "never".into(),
        ],
        HarnessId::ClaudeCode => vec![
            "--restricted".into(),
            "--strict-mcp-config".into(),
            "--no-chrome".into(),
            "--no-session-persistence".into(),
            "-p".into(),
            "--output-format".into(),
            "text".into(),
            "--permission-mode".into(),
            "acceptEdits".into(),
            "--tools".into(),
            "Read,Edit,Glob,Grep".into(),
            "Follow the task request provided on standard input and work only in the current directory.".into(),
        ],
    };
    if let Some(model) = model {
        args.push("--model".into());
        args.push(model.to_owned());
    }
    if harness == HarnessId::Codex {
        // Codex documents `-` as its stdin prompt sentinel. This keeps user text
        // out of argv, where a leading dash could otherwise be parsed as a flag.
        args.push("-".into());
    }
    Ok(HarnessCommand {
        executable: match harness {
            HarnessId::Codex => "codex",
            HarnessId::ClaudeCode => "claude",
        },
        args,
        stdin: prompt.to_owned(),
    })
}

/// Extract the user-facing final answer from Codex CLI JSONL output.
pub fn visible_output(harness: HarnessId, output: &str) -> String {
    if harness == HarnessId::ClaudeCode {
        return output.trim().to_owned();
    }
    let mut messages = Vec::new();
    for line in output.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if event["type"] != "item.completed" || event["item"]["type"] != "agent_message" {
            continue;
        }
        if let Some(text) = event["item"]["text"].as_str() {
            messages.push(text.trim().to_owned());
        }
    }
    if messages.is_empty() {
        String::new()
    } else {
        messages.join("\n\n")
    }
}

/// Windows runs a command through one of its extensions, so each is checked.
fn on_path(directory: &Path, executable: &str) -> bool {
    let extensions: &[&str] = if cfg!(windows) {
        &["", ".exe", ".cmd", ".bat", ".com"]
    } else {
        &[""]
    };
    extensions
        .iter()
        .any(|extension| directory.join(format!("{executable}{extension}")).is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn only_the_tool_whose_command_is_on_the_path_is_installed() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("claude"), "").unwrap();

        let installed: Vec<&str> = detect_in(&[directory.path().to_path_buf()])
            .iter()
            .filter(|harness| harness.installed)
            .map(|harness| harness.id)
            .collect();

        assert_eq!(installed, ["claude_code"]);
        let claude = detect_in(&[directory.path().to_path_buf()])
            .into_iter()
            .find(|harness| harness.id == "claude_code")
            .unwrap();
        assert!(claude.task_execution_supported);
    }

    #[test]
    fn an_empty_path_installs_nothing() {
        assert!(detect_in(&[]).iter().all(|harness| !harness.installed));
    }

    #[test]
    fn codex_arguments_pin_workspace_sandbox_and_keep_prompt_as_one_argument() {
        let prompt = "Fix this; do not run another command";
        let command = task_command(HarnessId::Codex, Some("gpt-5.3-codex"), prompt).unwrap();
        assert_eq!(command.executable, "codex");
        assert!(command
            .args
            .windows(2)
            .any(|pair| pair == ["--sandbox", "workspace-write"]));
        assert!(command
            .args
            .windows(2)
            .any(|pair| pair == ["--ask-for-approval", "never"]));
        assert!(command.args.contains(&"--ignore-user-config".into()));
        assert_eq!(command.args.last().map(String::as_str), Some("-"));
        assert_eq!(command.stdin, prompt);
    }

    #[test]
    fn claude_arguments_restrict_tools_and_disable_custom_mcp_servers() {
        let command = task_command(HarnessId::ClaudeCode, None, "Implement this").unwrap();
        assert!(command.args.contains(&"--restricted".into()));
        assert!(command.args.contains(&"--strict-mcp-config".into()));
        assert!(command
            .args
            .windows(2)
            .any(|pair| pair == ["--tools", "Read,Edit,Glob,Grep"]));
        assert!(command
            .args
            .windows(2)
            .any(|pair| pair == ["--permission-mode", "acceptEdits"]));
        assert_eq!(command.stdin, "Implement this");
        assert_eq!(
            command.args.last().map(String::as_str),
            Some("Follow the task request provided on standard input and work only in the current directory.")
        );
    }

    #[test]
    fn model_and_prompt_are_validated_without_shell_interpolation() {
        assert_eq!(
            task_command(HarnessId::Codex, Some("unsafe model\n--yolo"), "Do it")
                .unwrap_err()
                .code,
            "invalid_harness_model"
        );
        assert_eq!(
            task_command(HarnessId::Codex, None, "  \0  ")
                .unwrap_err()
                .code,
            "invalid_input"
        );
    }

    #[test]
    fn external_adapters_respect_effective_task_permissions() {
        assert!(validate_task_permissions(
            HarnessId::ClaudeCode,
            PermissionMode::Ask,
            PermissionDecision::Allow,
            PermissionDecision::Deny,
            PermissionDecision::Allow,
            PermissionDecision::Deny,
        )
        .is_err());
        assert!(validate_task_permissions(
            HarnessId::ClaudeCode,
            PermissionMode::WorkspaceWrite,
            PermissionDecision::Allow,
            PermissionDecision::Allow,
            PermissionDecision::Deny,
            PermissionDecision::Deny,
        )
        .is_ok());
        assert!(validate_task_permissions(
            HarnessId::ClaudeCode,
            PermissionMode::WorkspaceWrite,
            PermissionDecision::Deny,
            PermissionDecision::Allow,
            PermissionDecision::Allow,
            PermissionDecision::Allow,
        )
        .is_err());
        assert!(validate_task_permissions(
            HarnessId::Codex,
            PermissionMode::WorkspaceWrite,
            PermissionDecision::Allow,
            PermissionDecision::Allow,
            PermissionDecision::Allow,
            PermissionDecision::Allow,
        )
        .is_err());
        assert!(validate_task_permissions(
            HarnessId::Codex,
            PermissionMode::FullAccess,
            PermissionDecision::Allow,
            PermissionDecision::Allow,
            PermissionDecision::Allow,
            PermissionDecision::Allow,
        )
        .is_ok());
        assert!(validate_task_permissions(
            HarnessId::Codex,
            PermissionMode::FullAccess,
            PermissionDecision::Allow,
            PermissionDecision::Allow,
            PermissionDecision::Deny,
            PermissionDecision::Allow,
        )
        .is_err());
    }

    #[test]
    fn codex_jsonl_results_render_only_final_assistant_messages() {
        let output = [
            r#"{"type":"item.completed","item":{"type":"command_execution","command":"secret operation"}}"#,
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"Finished the task."}}"#,
        ]
        .join("\n");
        assert_eq!(
            visible_output(HarnessId::Codex, &output),
            "Finished the task."
        );
        assert_eq!(
            visible_output(HarnessId::Codex, r#"{"type":"command_execution"}"#),
            ""
        );
    }
}
