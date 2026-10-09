use super::{process::run_program, ToolOutput};
use crate::{
    error::{AppError, AppResult},
    git,
    workspaces::{is_ignored, scoped_path},
};
use serde_json::{json, Value};
use std::{path::Path, time::Duration};

const GIT_OUTPUT_LIMIT: usize = 256 * 1024;

fn success(content: impl Into<String>, data: Value) -> AppResult<ToolOutput> {
    Ok(ToolOutput {
        content: content.into(),
        structured: data,
    })
}

async fn git_command(root: &Path, args: &[String]) -> AppResult<super::process::ProcessOutput> {
    let output = run_program("git", args, root, Duration::from_secs(12), GIT_OUTPUT_LIMIT).await?;
    if output.exit_code != Some(0) {
        return Err(AppError::new(
            "git_error",
            if output.stderr.is_empty() {
                "Git command failed."
            } else {
                &output.stderr
            },
        ));
    }
    Ok(output)
}

fn path_arg(root: &Path, value: &Value) -> AppResult<Option<String>> {
    let Some(relative) = value.as_str() else {
        return Ok(None);
    };
    if is_ignored(root, relative) {
        return Err(AppError::new(
            "permission_denied",
            "This path is excluded by the project's ignore rules.",
        ));
    }
    let path = scoped_path(root, relative)?;
    let relative = path.strip_prefix(root).map_err(|_| {
        AppError::new(
            "permission_denied",
            "Git path is outside the selected project.",
        )
    })?;
    let relative = relative.to_string_lossy().replace('\\', "/");
    Ok((!relative.is_empty()).then_some(relative))
}

fn add_safe_pathspecs(args: &mut Vec<String>, selected: Option<String>) {
    args.push("--".into());
    args.push(selected.unwrap_or_else(|| ".".into()));
    for pattern in [
        ":(exclude,glob).env*",
        ":(exclude,glob)**/.env*",
        ":(exclude,glob).git/**",
        ":(exclude,glob)**/.git/**",
        ":(exclude,glob).aws/**",
        ":(exclude,glob)**/.aws/**",
        ":(exclude,glob).ssh/**",
        ":(exclude,glob)**/.ssh/**",
        ":(exclude,glob).codex/**",
        ":(exclude,glob)**/.codex/**",
        ":(exclude,glob)node_modules/**",
        ":(exclude,glob)**/node_modules/**",
        ":(exclude,glob)target/**",
        ":(exclude,glob)**/target/**",
    ] {
        args.push(pattern.into());
    }
}

fn page(text: &str, args: &Value) -> (String, usize, usize, bool) {
    let lines: Vec<_> = text.lines().collect();
    let offset = args["offset"].as_u64().unwrap_or(0).min(100_000) as usize;
    let limit = args["limit"].as_u64().unwrap_or(200).clamp(1, 400) as usize;
    let result = lines
        .iter()
        .skip(offset)
        .take(limit)
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    let has_more = offset.saturating_add(limit) < lines.len();
    (result, offset, limit, has_more)
}

pub async fn execute(root: &Path, name: &str, args: &Value) -> AppResult<ToolOutput> {
    match name {
        "git_status" => {
            let text = git::status(root).await?;
            success(text.clone(), json!({"status":text}))
        }
        "git_branch" => {
            let current = git::branch(root).await?;
            let branch_output = git_command(
                root,
                &[
                    "--no-optional-locks".into(),
                    "-c".into(),
                    "core.fsmonitor=false".into(),
                    "branch".into(),
                    "--format=%(refname:short)".into(),
                ],
            )
            .await?;
            let mut branches = branch_output
                .stdout
                .lines()
                .map(str::trim)
                .filter(|branch| !branch.is_empty())
                .take(200)
                .map(str::to_owned)
                .collect::<Vec<_>>();
            branches.sort_by_key(|branch| branch.to_ascii_lowercase());
            let data = json!({"current":current,"branches":branches,"truncated":branch_output.truncated || branch_output.stdout.lines().count() > 200});
            success(data.to_string(), data)
        }
        "git_diff" => {
            let mut command = vec![
                "--no-optional-locks".into(),
                "-c".into(),
                "core.fsmonitor=false".into(),
                "-c".into(),
                "core.untrackedCache=false".into(),
                "diff".into(),
                "--no-ext-diff".into(),
                "--no-color".into(),
            ];
            if args["staged"].as_bool().unwrap_or(false) {
                command.push("--cached".into());
            }
            let path = path_arg(root, &args["path"])?;
            add_safe_pathspecs(&mut command, path);
            let response = git_command(root, &command).await?;
            let (diff, offset, limit, has_more) = page(&response.stdout, args);
            success(
                diff.clone(),
                json!({"diff":diff,"offset":offset,"limit":limit,"hasMore":has_more || response.truncated,"truncated":response.truncated}),
            )
        }
        "git_log" => {
            let limit = args["limit"].as_u64().unwrap_or(20).clamp(1, 50);
            let mut command = vec![
                "--no-optional-locks".into(),
                "-c".into(),
                "core.fsmonitor=false".into(),
                "log".into(),
                "--oneline".into(),
                "--decorate".into(),
                format!("--max-count={limit}"),
            ];
            if let Some(path) = path_arg(root, &args["path"])? {
                command.push("--".into());
                command.push(path);
            }
            let response = git_command(root, &command).await?;
            success(
                response.stdout.clone(),
                json!({"commits":response.stdout.lines().collect::<Vec<_>>(),"truncated":response.truncated}),
            )
        }
        "git_show" => {
            let revision = args["revision"].as_str().unwrap_or("HEAD");
            let verified = git_command(
                root,
                &[
                    "rev-parse".into(),
                    "--verify".into(),
                    "--end-of-options".into(),
                    format!("{revision}^{{commit}}"),
                ],
            )
            .await?;
            let commit = verified.stdout.trim();
            if commit.len() < 7
                || !commit
                    .chars()
                    .all(|character| character.is_ascii_hexdigit())
            {
                return Err(AppError::new(
                    "invalid_revision",
                    "Choose a valid commit or ref in this repository.",
                ));
            }
            let mut command = vec![
                "--no-optional-locks".into(),
                "show".into(),
                "--no-ext-diff".into(),
                "--no-color".into(),
                "--format=fuller".into(),
                "--stat".into(),
                "--patch".into(),
                commit.into(),
            ];
            add_safe_pathspecs(&mut command, None);
            let response = git_command(root, &command).await?;
            let (show, offset, limit, has_more) = page(&response.stdout, args);
            success(
                show.clone(),
                json!({"revision":commit,"show":show,"offset":offset,"limit":limit,"hasMore":has_more || response.truncated,"truncated":response.truncated}),
            )
        }
        _ => Err(AppError::new(
            "unknown_tool",
            "The repository tool is not registered.",
        )),
    }
}
