use super::{
    process::{run_program_streaming, ProcessOutputChunk},
    ToolOutput,
};
use crate::{
    error::{AppError, AppResult},
    workspaces::scoped_path,
};
use serde_json::{json, Value};
use std::{path::Path, time::Duration};

pub async fn run_command(
    root: &Path,
    args: &Value,
    external_allowed: bool,
    events: Option<tokio::sync::mpsc::UnboundedSender<ProcessOutputChunk>>,
) -> AppResult<ToolOutput> {
    let program = args["program"]
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AppError::new("invalid_tool_arguments", "Choose a program to run."))?;
    let command_args = args["args"]
        .as_array()
        .ok_or_else(|| {
            AppError::new(
                "invalid_tool_arguments",
                "Command arguments must be an array.",
            )
        })?
        .iter()
        .map(|value| {
            value.as_str().map(str::to_owned).ok_or_else(|| {
                AppError::new(
                    "invalid_tool_arguments",
                    "Each command argument must be text.",
                )
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    let cwd_relative = args["path"].as_str().unwrap_or(".");
    let cwd = if external_allowed {
        let requested = Path::new(cwd_relative);
        let joined = if requested.is_absolute() {
            requested.to_path_buf()
        } else {
            root.join(requested)
        };
        std::fs::canonicalize(joined).map_err(|_| {
            AppError::new(
                "invalid_path",
                "The command working directory does not exist.",
            )
        })?
    } else {
        scoped_path(root, cwd_relative)?
    };
    if !cwd.is_dir() {
        return Err(AppError::new(
            "not_directory",
            "Command working directory must be a project folder.",
        ));
    }
    let result = run_program_streaming(
        program,
        &command_args,
        &cwd,
        Duration::from_secs(30),
        12 * 1024,
        events,
    )
    .await?;
    let content = format!(
        "exit code: {}\nstdout:\n{}\nstderr:\n{}{}{}",
        result
            .exit_code
            .map_or_else(|| "unknown".into(), |code| code.to_string()),
        result.stdout,
        result.stderr,
        if result.truncated {
            "\n(output truncated)"
        } else {
            ""
        },
        if result.timed_out {
            "\n(command timed out after 30 seconds)"
        } else {
            ""
        }
    );
    let data = json!({"program":program,"args":command_args,"workingDirectory":cwd.to_string_lossy(),"exitCode":result.exit_code,"stdout":result.stdout,"stderr":result.stderr,"truncated":result.truncated,"timedOut":result.timed_out,"interactive":false});
    Ok(ToolOutput {
        content,
        structured: data,
    })
}
