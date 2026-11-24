use super::{branches, git_args, output};
use crate::{
    containment::{collect_output, prepare_command, ProfileKind, SpawnPolicy},
    domain::{
        now, AgentSession, ChangedFile, TaskCheckState, TaskPullRequestCheck,
        TaskPullRequestReviewState, TaskPullRequestState, TaskPullRequestStatus, TaskWorktree,
    },
    error::{AppError, AppResult},
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};

const GIT_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_DIFF_BYTES: usize = 2 * 1024 * 1024;
const GH_TIMEOUT: Duration = Duration::from_secs(45);

async fn gh_output(
    policy: SpawnPolicy,
    root: &Path,
    args: &[String],
) -> AppResult<std::process::Output> {
    if policy.kind != ProfileKind::EditWithNetwork {
        return Err(AppError::new(
            "github_network_permission_required",
            "Creating a draft pull request requires the project's network-enabled edit profile.",
        ));
    }
    let process = prepare_command(policy, "gh")?
        .args(args)
        .current_dir(root)
        .env("GH_PROMPT_DISABLED", "1")
        .spawn()
        .map_err(|_| {
            AppError::new(
                "github_cli_unavailable",
                "GitHub CLI (gh) is unavailable. Install it and sign in with `gh auth login`.",
            )
        })?;
    match tokio::time::timeout(GH_TIMEOUT, collect_output(process)).await {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(_)) => Err(AppError::new(
            "github_cli_unavailable",
            "GitHub CLI could not be started. Install it and sign in with `gh auth login`.",
        )),
        Err(_) => Err(AppError::new(
            "github_cli_timeout",
            "GitHub CLI took too long to respond. Check your connection and try again.",
        )),
    }
}

fn pull_request_url(output: &[u8]) -> Option<String> {
    String::from_utf8_lossy(output).lines().find_map(|line| {
        let candidate = line.trim().trim_matches(['<', '>', '`']);
        let rest = candidate.strip_prefix("https://")?;
        if candidate.chars().any(char::is_control) || rest.contains(char::is_whitespace) {
            return None;
        }
        let segments = rest.split('/').collect::<Vec<_>>();
        let pull_index = segments.iter().rposition(|part| *part == "pull")?;
        if pull_index < 2
            || segments[pull_index + 1..].first().is_none_or(|number| {
                number.is_empty() || !number.chars().all(|c| c.is_ascii_digit())
            })
        {
            return None;
        }
        Some(candidate.to_owned())
    })
}

fn validated_pr_fields(title: &str, body: &str) -> AppResult<(String, String)> {
    let title = title.trim();
    let body = body.trim();
    if title.is_empty() || title.chars().count() > 200 || title.chars().any(char::is_control) {
        return Err(AppError::new(
            "invalid_pull_request_title",
            "A pull request title must contain 1 to 200 characters.",
        ));
    }
    if body.chars().count() > 10_000 || body.contains('\0') {
        return Err(AppError::new(
            "invalid_pull_request_body",
            "A pull request description must be under 10,000 characters and contain valid text.",
        ));
    }
    Ok((title.to_owned(), body.to_owned()))
}

fn gh_pr_view_args(branch: &str) -> [String; 7] {
    [
        "pr".into(),
        "view".into(),
        branch.into(),
        "--json".into(),
        "url".into(),
        "--jq".into(),
        ".url".into(),
    ]
}

fn secure_https_url(value: &str) -> Option<String> {
    let rest = value.strip_prefix("https://")?;
    if rest.is_empty() || rest.chars().any(char::is_control) || rest.contains(char::is_whitespace) {
        return None;
    }
    Some(value.to_owned())
}

fn task_pull_request_status(output: &[u8]) -> AppResult<Option<TaskPullRequestStatus>> {
    let value: serde_json::Value = serde_json::from_slice(output).map_err(|_| {
        AppError::new(
            "github_status_invalid",
            "GitHub returned an unreadable pull request status.",
        )
    })?;
    let Some(pr) = value.as_array().and_then(|items| items.first()) else {
        return Ok(None);
    };
    let url = pr["url"]
        .as_str()
        .and_then(secure_https_url)
        .ok_or_else(|| {
            AppError::new(
                "github_status_invalid",
                "GitHub returned a pull request without a safe URL.",
            )
        })?;
    let state = match pr["state"]
        .as_str()
        .unwrap_or_default()
        .to_ascii_uppercase()
        .as_str()
    {
        "OPEN" => TaskPullRequestState::Open,
        "CLOSED" => TaskPullRequestState::Closed,
        "MERGED" => TaskPullRequestState::Merged,
        _ => {
            return Err(AppError::new(
                "github_status_invalid",
                "GitHub returned an unknown pull request state.",
            ));
        }
    };
    let review_state = match pr["reviewDecision"].as_str().unwrap_or_default() {
        "APPROVED" => Some(TaskPullRequestReviewState::Approved),
        "CHANGES_REQUESTED" => Some(TaskPullRequestReviewState::ChangesRequested),
        "REVIEW_REQUIRED" => Some(TaskPullRequestReviewState::ReviewRequired),
        "" => None,
        _ => Some(TaskPullRequestReviewState::Unknown),
    };
    let checks = pr["statusCheckRollup"]
        .as_array()
        .into_iter()
        .flatten()
        .take(20)
        .map(|check| {
            let name = check["name"]
                .as_str()
                .or_else(|| check["context"].as_str())
                .or_else(|| check["workflowName"].as_str())
                .unwrap_or("Check")
                .chars()
                .take(120)
                .collect();
            let raw_state = check["conclusion"]
                .as_str()
                .or_else(|| check["state"].as_str())
                .or_else(|| check["status"].as_str())
                .unwrap_or_default()
                .to_ascii_uppercase();
            let state = match raw_state.as_str() {
                "SUCCESS" | "SUCCESSFUL" | "PASSED" => TaskCheckState::Passed,
                "FAILURE" | "FAILED" | "ERROR" | "TIMED_OUT" | "ACTION_REQUIRED"
                | "STARTUP_FAILURE" | "STALE" => TaskCheckState::Failed,
                "PENDING" | "QUEUED" | "IN_PROGRESS" | "EXPECTED" | "WAITING" => {
                    TaskCheckState::Pending
                }
                "SKIPPED" | "NEUTRAL" => TaskCheckState::Skipped,
                "CANCELLED" | "CANCELED" => TaskCheckState::Cancelled,
                _ => TaskCheckState::Unknown,
            };
            let url = check["detailsUrl"]
                .as_str()
                .or_else(|| check["targetUrl"].as_str())
                .and_then(secure_https_url);
            TaskPullRequestCheck { name, state, url }
        })
        .collect();
    Ok(Some(TaskPullRequestStatus {
        url,
        state,
        is_draft: pr["isDraft"].as_bool().unwrap_or(false),
        review_state,
        checks,
        refreshed_at: now(),
    }))
}

fn git_text(stdout: &[u8]) -> String {
    String::from_utf8_lossy(stdout).trim().to_owned()
}

fn parse_changed_paths(stdout: &[u8]) -> Vec<String> {
    stdout
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .map(|record| String::from_utf8_lossy(record).replace('\\', "/"))
        .filter(|path| !path.is_empty())
        .take(500)
        .collect()
}

async fn porcelain(policy: SpawnPolicy, root: &Path) -> AppResult<Vec<ChangedFile>> {
    let result = output(
        policy,
        root,
        &git_args(&["status", "--porcelain=v1", "-z", "--untracked-files=all"]),
        Duration::from_secs(15),
    )
    .await?;
    if !result.status.success() {
        return Err(AppError::new(
            "git_error",
            "Could not read the task workspace status.",
        ));
    }
    let mut files = Vec::new();
    let mut records = result
        .stdout
        .split(|byte| *byte == 0)
        .filter(|item| !item.is_empty());
    while let Some(record) = records.next() {
        if record.len() < 4 {
            continue;
        }
        let status = String::from_utf8_lossy(&record[..2]).trim().to_owned();
        let path = String::from_utf8_lossy(&record[3..]).replace('\\', "/");
        files.push(ChangedFile {
            path,
            status: status.clone(),
        });
        if status.contains('R') || status.contains('C') {
            let _ = records.next();
        }
        if files.len() >= 500 {
            break;
        }
    }
    Ok(files)
}

fn conflicts(files: &[ChangedFile]) -> Vec<String> {
    files
        .iter()
        .filter(|file| file.status.contains('U') || matches!(file.status.as_str(), "AA" | "DD"))
        .map(|file| file.path.clone())
        .take(200)
        .collect()
}

async fn common_directory(policy: SpawnPolicy, path: &Path) -> AppResult<PathBuf> {
    let result = output(
        policy,
        path,
        &git_args(&["rev-parse", "--git-common-dir"]),
        Duration::from_secs(5),
    )
    .await?;
    if !result.status.success() {
        return Err(AppError::new(
            "not_git_repository",
            "This task workspace is no longer a Git repository.",
        ));
    }
    let common = PathBuf::from(git_text(&result.stdout));
    let common = if common.is_absolute() {
        common
    } else {
        path.join(common)
    };
    std::fs::canonicalize(common).map_err(|_| {
        AppError::new(
            "worktree_unavailable",
            "The Git repository metadata for this task workspace is unavailable.",
        )
    })
}

/// Validate the persisted path against Git's repository metadata before using
/// it. A task cannot redirect filesystem tools to an unrelated directory.
pub async fn validate(
    policy: SpawnPolicy,
    repository_root: &Path,
    worktree_path: &Path,
) -> AppResult<PathBuf> {
    let repository_root = std::fs::canonicalize(repository_root).map_err(|_| {
        AppError::new(
            "project_unavailable",
            "The project repository is unavailable.",
        )
    })?;
    let worktree_path = std::fs::canonicalize(worktree_path).map_err(|_| {
        AppError::new(
            "worktree_unavailable",
            "The task workspace was moved or removed.",
        )
    })?;
    if !worktree_path.is_dir() || worktree_path == repository_root {
        return Err(AppError::new(
            "invalid_worktree",
            "This task is not using an isolated workspace.",
        ));
    }
    let top = output(
        policy,
        &worktree_path,
        &git_args(&["rev-parse", "--show-toplevel"]),
        Duration::from_secs(5),
    )
    .await?;
    if !top.status.success() {
        return Err(AppError::new(
            "worktree_unavailable",
            "The task workspace is no longer a Git working tree.",
        ));
    }
    let top = std::fs::canonicalize(git_text(&top.stdout)).map_err(|_| {
        AppError::new(
            "worktree_unavailable",
            "The task workspace is no longer a Git working tree.",
        )
    })?;
    if top != worktree_path
        || common_directory(policy, &repository_root).await?
            != common_directory(policy, &worktree_path).await?
    {
        return Err(AppError::new(
            "invalid_worktree",
            "This task workspace does not belong to the selected project repository.",
        ));
    }
    Ok(worktree_path)
}

pub async fn project_path(
    policy: SpawnPolicy,
    repository_root: &Path,
    project_path: &Path,
    worktree_path: &Path,
) -> AppResult<PathBuf> {
    let repository_root = std::fs::canonicalize(repository_root).map_err(|_| {
        AppError::new(
            "project_unavailable",
            "The project repository is unavailable.",
        )
    })?;
    let project_path = std::fs::canonicalize(project_path)
        .map_err(|_| AppError::new("project_unavailable", "The project folder is unavailable."))?;
    let worktree = validate(policy, &repository_root, worktree_path).await?;
    let relative = project_path.strip_prefix(&repository_root).map_err(|_| {
        AppError::new(
            "invalid_project_root",
            "The selected project folder is outside its Git repository.",
        )
    })?;
    let scoped = std::fs::canonicalize(worktree.join(relative)).map_err(|_| {
        AppError::new(
            "worktree_project_missing",
            "The selected project folder is missing from this task workspace.",
        )
    })?;
    if !scoped.starts_with(&worktree) || !scoped.is_dir() {
        return Err(AppError::new(
            "invalid_worktree_scope",
            "The task workspace does not contain a safe project folder.",
        ));
    }
    Ok(scoped)
}

pub async fn create(
    policy: SpawnPolicy,
    repository_root: &Path,
    destination: &Path,
    base_branch: &str,
    branch: &str,
) -> AppResult<()> {
    if !branches(policy, repository_root)
        .await?
        .iter()
        .any(|item| item == base_branch)
    {
        return Err(AppError::new(
            "unknown_base_branch",
            "Choose an existing local branch as the task's starting point.",
        ));
    }
    let check = output(
        policy,
        repository_root,
        &git_args(&["check-ref-format", "--branch", branch]),
        Duration::from_secs(5),
    )
    .await?;
    if !check.status.success() {
        return Err(AppError::new(
            "invalid_branch_name",
            "A safe task branch name could not be generated.",
        ));
    }
    if destination.exists() {
        return Err(AppError::new(
            "worktree_path_exists",
            "This task workspace path already exists. Start a new task to retry.",
        ));
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let args = vec![
        "worktree".to_owned(),
        "add".to_owned(),
        "--quiet".to_owned(),
        "-b".to_owned(),
        branch.to_owned(),
        destination.to_string_lossy().into_owned(),
        base_branch.to_owned(),
    ];
    let result = output(policy, repository_root, &args, GIT_TIMEOUT).await?;
    if !result.status.success() {
        let _ = std::fs::remove_dir(destination);
        return Err(AppError::new("worktree_create_failed", "Git could not create an isolated workspace from that branch. Check that the project has no conflicting task branch."));
    }
    Ok(())
}

pub async fn inspect(
    policy: SpawnPolicy,
    repository_root: &Path,
    session: &AgentSession,
) -> AppResult<TaskWorktree> {
    let path = session.worktree_path.as_deref().ok_or_else(|| {
        AppError::new(
            "not_isolated",
            "This task uses the project folder directly.",
        )
    })?;
    let base_branch = session.base_branch.as_deref().ok_or_else(|| {
        AppError::new(
            "worktree_metadata_missing",
            "This task is missing its starting branch.",
        )
    })?;
    let path = match validate(policy, repository_root, Path::new(path)).await {
        Ok(path) => path,
        Err(error) if error.code == "worktree_unavailable" => {
            return Ok(TaskWorktree {
                session_id: session.id.clone(),
                title: session.title.clone(),
                branch: session.git_branch.clone().unwrap_or_default(),
                base_branch: base_branch.into(),
                task_status: session.status.clone(),
                available: false,
                dirty: false,
                has_committed_changes: false,
                changed_files: vec![],
                conflicts: vec![],
                additions: 0,
                deletions: 0,
            });
        }
        Err(error) => return Err(error),
    };
    let mut files_by_path = BTreeMap::<String, String>::new();
    let uncommitted = porcelain(policy, &path).await?;
    for file in &uncommitted {
        files_by_path.insert(file.path.clone(), file.status.clone());
    }
    let changed = output(
        policy,
        &path,
        &git_args(&["diff", "--name-only", "-z", base_branch]),
        Duration::from_secs(15),
    )
    .await?;
    if changed.status.success() {
        for file in parse_changed_paths(&changed.stdout) {
            files_by_path.entry(file).or_insert_with(|| "M".into());
        }
    }
    let changed_files = files_by_path
        .into_iter()
        .map(|(path, status)| ChangedFile { path, status })
        .collect::<Vec<_>>();
    let stat = output(
        policy,
        &path,
        &git_args(&["diff", "--numstat", base_branch]),
        Duration::from_secs(15),
    )
    .await?;
    let (mut additions, mut deletions) = (0u64, 0u64);
    if stat.status.success() {
        for line in String::from_utf8_lossy(&stat.stdout).lines() {
            let mut columns = line.split('\t');
            additions = additions.saturating_add(
                columns
                    .next()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(0),
            );
            deletions = deletions.saturating_add(
                columns
                    .next()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(0),
            );
        }
    }
    let ahead = output(
        policy,
        &path,
        &git_args(&["rev-list", "--count", &format!("{base_branch}..HEAD")]),
        Duration::from_secs(10),
    )
    .await?;
    let has_committed_changes =
        ahead.status.success() && git_text(&ahead.stdout).parse::<u64>().unwrap_or(0) > 0;
    Ok(TaskWorktree {
        session_id: session.id.clone(),
        title: session.title.clone(),
        branch: session.git_branch.clone().unwrap_or_default(),
        base_branch: base_branch.into(),
        task_status: session.status.clone(),
        available: true,
        dirty: !uncommitted.is_empty(),
        conflicts: conflicts(&uncommitted),
        changed_files,
        has_committed_changes,
        additions,
        deletions,
    })
}

pub async fn diff(
    policy: SpawnPolicy,
    repository_root: &Path,
    session: &AgentSession,
) -> AppResult<String> {
    let info = inspect(policy, repository_root, session).await?;
    if !info.available {
        return Err(AppError::new(
            "worktree_unavailable",
            "This task workspace was moved or removed.",
        ));
    }
    let worktree_path = session.worktree_path.as_deref().ok_or_else(|| {
        AppError::new(
            "not_isolated",
            "This task uses the project folder directly.",
        )
    })?;
    let worktree_path = validate(policy, repository_root, Path::new(worktree_path)).await?;
    let result = output(
        policy,
        &worktree_path,
        &git_args(&["diff", "--no-ext-diff", "--binary", &info.base_branch]),
        Duration::from_secs(20),
    )
    .await?;
    if !result.status.success() {
        return Err(AppError::new(
            "git_diff_failed",
            "Could not create a diff for this task workspace.",
        ));
    }
    let mut diff_bytes = result.stdout[..result.stdout.len().min(MAX_DIFF_BYTES)].to_vec();
    let mut truncated = result.stdout.len() > MAX_DIFF_BYTES;
    let root = &worktree_path;
    for file in info.changed_files.iter().filter(|file| file.status == "??") {
        let file_path = root.join(&file.path);
        let Ok(canonical_file) = std::fs::canonicalize(&file_path) else {
            continue;
        };
        if !canonical_file.starts_with(root) {
            continue;
        }
        let Ok(metadata) = std::fs::metadata(&canonical_file) else {
            continue;
        };
        let remaining = MAX_DIFF_BYTES.saturating_sub(diff_bytes.len());
        if metadata.len() > remaining as u64 {
            truncated = true;
            break;
        }
        #[cfg(windows)]
        let null_device = "NUL";
        #[cfg(not(windows))]
        let null_device = "/dev/null";
        let args = vec![
            "diff".to_owned(),
            "--no-index".to_owned(),
            "--no-ext-diff".to_owned(),
            "--binary".to_owned(),
            "--".to_owned(),
            null_device.to_owned(),
            file.path.clone(),
        ];
        let added = output(policy, root, &args, Duration::from_secs(10)).await?;
        if added.status.code().is_some_and(|code| code > 1) {
            continue;
        }
        if added.stdout.len() > remaining {
            truncated = true;
            break;
        }
        diff_bytes.extend_from_slice(&added.stdout);
    }
    let mut diff = String::from_utf8_lossy(&diff_bytes).into_owned();
    if truncated {
        diff.push_str("\n… diff truncated at 2 MiB …");
    }
    Ok(diff)
}

pub async fn commit(
    policy: SpawnPolicy,
    repository_root: &Path,
    session: &AgentSession,
) -> AppResult<()> {
    let info = inspect(policy, repository_root, session).await?;
    if !info.available {
        return Err(AppError::new(
            "worktree_unavailable",
            "This task workspace was moved or removed.",
        ));
    }
    if !info.conflicts.is_empty() {
        return Err(AppError::new(
            "worktree_conflicted",
            "Resolve the task workspace conflicts before committing.",
        ));
    }
    if !info.dirty {
        return Err(AppError::new(
            "nothing_to_commit",
            "There are no uncommitted task changes to commit.",
        ));
    }
    let worktree_path = session.worktree_path.as_deref().ok_or_else(|| {
        AppError::new(
            "not_isolated",
            "This task uses the project folder directly.",
        )
    })?;
    let path = validate(policy, repository_root, Path::new(worktree_path)).await?;
    let add = output(policy, &path, &git_args(&["add", "--all"]), GIT_TIMEOUT).await?;
    if !add.status.success() {
        return Err(AppError::new(
            "git_stage_failed",
            "Could not stage task workspace changes.",
        ));
    }
    let staged = output(
        policy,
        &path,
        &git_args(&["diff", "--cached", "--quiet"]),
        Duration::from_secs(10),
    )
    .await?;
    if staged.status.success() {
        return Err(AppError::new(
            "nothing_to_commit",
            "There are no changes to commit.",
        ));
    }
    let message = format!(
        "Octran Code task: {}",
        session
            .title
            .chars()
            .filter(|ch| !ch.is_control())
            .take(120)
            .collect::<String>()
    );
    let args = vec!["commit".to_owned(), "-m".to_owned(), message];
    let result = output(policy, &path, &args, GIT_TIMEOUT).await?;
    if !result.status.success() {
        return Err(AppError::new("git_commit_failed", "Git could not commit the task changes. Check the repository's commit identity and hooks."));
    }
    Ok(())
}

pub async fn create_draft_pull_request(
    policy: SpawnPolicy,
    repository_root: &Path,
    session: &AgentSession,
    title: &str,
    body: &str,
) -> AppResult<String> {
    let (title, body) = validated_pr_fields(title, body)?;
    if session.status != crate::domain::SessionStatus::Completed {
        return Err(AppError::new(
            "task_not_completed",
            "Finish the task before creating a draft pull request.",
        ));
    }
    let info = inspect(policy, repository_root, session).await?;
    if !info.available {
        return Err(AppError::new(
            "worktree_unavailable",
            "This task workspace was moved or removed.",
        ));
    }
    if !info.conflicts.is_empty() {
        return Err(AppError::new(
            "worktree_conflicted",
            "Resolve the task workspace conflicts before creating a pull request.",
        ));
    }
    if info.dirty {
        return Err(AppError::new(
            "worktree_has_changes",
            "Commit the task workspace changes before creating a pull request.",
        ));
    }
    if !info.has_committed_changes {
        return Err(AppError::new(
            "no_task_changes",
            "This task has no committed changes to propose.",
        ));
    }
    let worktree_path = session.worktree_path.as_deref().ok_or_else(|| {
        AppError::new(
            "not_isolated",
            "This task uses the project folder directly.",
        )
    })?;
    let path = validate(policy, repository_root, Path::new(worktree_path)).await?;
    let branch = session.git_branch.as_deref().ok_or_else(|| {
        AppError::new(
            "worktree_metadata_missing",
            "This task is missing its branch.",
        )
    })?;
    let branch_check = output(
        policy,
        &path,
        &git_args(&["check-ref-format", "--branch", branch]),
        Duration::from_secs(5),
    )
    .await?;
    if !branch_check.status.success() {
        return Err(AppError::new(
            "invalid_task_branch",
            "The task branch name is invalid. Refresh the task workspace and try again.",
        ));
    }
    let base_branch_check = output(
        policy,
        &path,
        &git_args(&["check-ref-format", "--branch", &info.base_branch]),
        Duration::from_secs(5),
    )
    .await?;
    if !base_branch_check.status.success() || info.base_branch == branch {
        return Err(AppError::new(
            "invalid_task_base_branch",
            "The task's base branch is invalid. Refresh the task workspace and try again.",
        ));
    }
    let current_branch = output(
        policy,
        &path,
        &git_args(&["rev-parse", "--abbrev-ref", "HEAD"]),
        Duration::from_secs(5),
    )
    .await?;
    if !current_branch.status.success() || git_text(&current_branch.stdout) != branch {
        return Err(AppError::new(
            "task_branch_mismatch",
            "The task workspace is no longer checked out on its recorded branch.",
        ));
    }

    let auth = gh_output(policy, &path, &git_args(&["auth", "status"])).await?;
    if !auth.status.success() {
        return Err(AppError::new(
            "github_not_authenticated",
            "Sign in to GitHub CLI with `gh auth login`, then try again.",
        ));
    }
    let view_args = gh_pr_view_args(branch);
    let existing = gh_output(policy, &path, &view_args).await?;
    if existing.status.success() {
        return pull_request_url(&existing.stdout).ok_or_else(|| {
            AppError::new(
                "github_pr_url_missing",
                "GitHub found a pull request for this branch but did not return its URL.",
            )
        });
    }

    let push_args = [
        "push".into(),
        "--set-upstream".into(),
        "origin".into(),
        branch.into(),
    ];
    let push = output(policy, &path, &push_args, GH_TIMEOUT).await?;
    if !push.status.success() {
        return Err(AppError::new(
            "task_branch_push_failed",
            "Git could not push the task branch to origin. Check the remote and your Git credentials.",
        ));
    }

    let create_args = [
        "pr".into(),
        "create".into(),
        "--draft".into(),
        "--title".into(),
        title,
        "--body".into(),
        body,
        "--base".into(),
        info.base_branch,
        "--head".into(),
        branch.into(),
    ];
    let created = gh_output(policy, &path, &create_args).await?;
    if created.status.success() {
        return pull_request_url(&created.stdout).ok_or_else(|| {
            AppError::new(
                "github_pr_url_missing",
                "GitHub created the draft pull request but did not return its URL.",
            )
        });
    }

    let view_args = gh_pr_view_args(branch);
    let existing = gh_output(policy, &path, &view_args).await?;
    if existing.status.success() {
        return pull_request_url(&existing.stdout).ok_or_else(|| {
            AppError::new(
                "github_pr_url_missing",
                "GitHub found a pull request for this branch but did not return its URL.",
            )
        });
    }
    Err(AppError::new(
        "github_pr_create_failed",
        "GitHub could not create a draft pull request. Check that origin points to a GitHub repository and try again.",
    ))
}

pub async fn pull_request_status(
    policy: SpawnPolicy,
    repository_root: &Path,
    session: &AgentSession,
) -> AppResult<Option<TaskPullRequestStatus>> {
    let info = inspect(policy, repository_root, session).await?;
    if !info.available {
        return Err(AppError::new(
            "worktree_unavailable",
            "This task workspace was moved or removed.",
        ));
    }
    let worktree_path = session.worktree_path.as_deref().ok_or_else(|| {
        AppError::new(
            "not_isolated",
            "This task uses the project folder directly.",
        )
    })?;
    let path = validate(policy, repository_root, Path::new(worktree_path)).await?;
    let branch = session.git_branch.as_deref().ok_or_else(|| {
        AppError::new(
            "worktree_metadata_missing",
            "This task is missing its branch.",
        )
    })?;
    let branch_check = output(
        policy,
        &path,
        &git_args(&["check-ref-format", "--branch", branch]),
        Duration::from_secs(5),
    )
    .await?;
    if !branch_check.status.success() {
        return Err(AppError::new(
            "invalid_task_branch",
            "The task branch name is invalid. Refresh the task workspace and try again.",
        ));
    }
    let auth = gh_output(policy, &path, &git_args(&["auth", "status"])).await?;
    if !auth.status.success() {
        return Err(AppError::new(
            "github_not_authenticated",
            "Sign in to GitHub CLI with `gh auth login`, then try again.",
        ));
    }
    let args = [
        "pr".into(),
        "list".into(),
        "--head".into(),
        branch.into(),
        "--state".into(),
        "all".into(),
        "--limit".into(),
        "1".into(),
        "--json".into(),
        "url,state,isDraft,reviewDecision,statusCheckRollup".into(),
        "--jq".into(),
        ".[:1] | map({url,state,isDraft,reviewDecision,statusCheckRollup:(.statusCheckRollup[:20] // [])})".into(),
    ];
    let result = gh_output(policy, &path, &args).await?;
    if !result.status.success() {
        return Err(AppError::new(
            "github_status_failed",
            "GitHub could not read this branch's pull request status. Check your connection and try again.",
        ));
    }
    task_pull_request_status(&result.stdout)
}

pub async fn apply(
    policy: SpawnPolicy,
    repository_root: &Path,
    session: &AgentSession,
) -> AppResult<Vec<String>> {
    let info = inspect(policy, repository_root, session).await?;
    if !info.available {
        return Err(AppError::new(
            "worktree_unavailable",
            "This task workspace was moved or removed.",
        ));
    }
    if !info.conflicts.is_empty() {
        return Err(AppError::new(
            "worktree_conflicted",
            "Resolve conflicts in the task workspace before applying it.",
        ));
    }
    if info.dirty {
        return Err(AppError::new(
            "uncommitted_task_changes",
            "Commit the task workspace changes before applying them to the project.",
        ));
    }
    if !info.has_committed_changes {
        return Err(AppError::new(
            "no_task_changes",
            "This task has no committed changes to apply.",
        ));
    }
    let project_status = porcelain(policy, repository_root).await?;
    if !project_status.is_empty() {
        return Err(AppError::new("project_has_changes", "The project working tree has local changes. Commit or stash them before applying this task."));
    }
    let current_branch = super::branch(policy, repository_root)
        .await?
        .ok_or_else(|| {
            AppError::new(
                "detached_head",
                "Check out a project branch before applying task changes.",
            )
        })?;
    let branch = session.git_branch.as_deref().ok_or_else(|| {
        AppError::new(
            "worktree_metadata_missing",
            "This task is missing its branch.",
        )
    })?;
    let args = vec![
        "merge".to_owned(),
        "--no-edit".to_owned(),
        "--no-ff".to_owned(),
        branch.to_owned(),
    ];
    let result = output(policy, repository_root, &args, GIT_TIMEOUT).await?;
    if result.status.success() {
        return Ok(vec![]);
    }
    let unresolved = conflicts(&porcelain(policy, repository_root).await?);
    if !unresolved.is_empty() {
        let aborted = output(
            policy,
            repository_root,
            &git_args(&["merge", "--abort"]),
            GIT_TIMEOUT,
        )
        .await?;
        if !aborted.status.success() {
            return Err(AppError::new("merge_abort_failed", "Git found conflicts and could not restore the project checkout. Resolve the merge in the project before continuing."));
        }
        tracing::info!(branch = %branch, target_branch = %current_branch, conflict_count = unresolved.len(), "Task apply found conflicts; merge was safely aborted");
        return Ok(unresolved);
    }
    let _ = output(
        policy,
        repository_root,
        &git_args(&["merge", "--abort"]),
        GIT_TIMEOUT,
    )
    .await;
    Err(AppError::new(
        "git_merge_failed",
        "Git could not apply this task branch to the current project branch.",
    ))
}

pub async fn remove(
    policy: SpawnPolicy,
    repository_root: &Path,
    session: &AgentSession,
) -> AppResult<()> {
    let info = inspect(policy, repository_root, session).await?;
    if !info.available {
        return Err(AppError::new(
            "worktree_unavailable",
            "This task workspace was moved or removed.",
        ));
    }
    if info.dirty {
        return Err(AppError::new("worktree_has_changes", "This task workspace contains uncommitted changes. Commit or apply them before removing its checkout."));
    }
    let worktree_path = session.worktree_path.as_deref().ok_or_else(|| {
        AppError::new(
            "not_isolated",
            "This task uses the project folder directly.",
        )
    })?;
    let worktree_path = validate(policy, repository_root, Path::new(worktree_path)).await?;
    let args = vec![
        "worktree".to_owned(),
        "remove".to_owned(),
        worktree_path.to_string_lossy().into_owned(),
    ];
    let result = output(policy, repository_root, &args, GIT_TIMEOUT).await?;
    if !result.status.success() {
        return Err(AppError::new(
            "worktree_remove_failed",
            "Git could not remove this task checkout. The branch is preserved.",
        ));
    }
    Ok(())
}

pub async fn remove_path(
    policy: SpawnPolicy,
    repository_root: &Path,
    path: &Path,
) -> AppResult<()> {
    let validated = validate(policy, repository_root, path).await?;
    if !porcelain(policy, &validated).await?.is_empty() {
        return Err(AppError::new(
            "worktree_has_changes",
            "This task workspace contains uncommitted changes.",
        ));
    }
    let args = vec![
        "worktree".to_owned(),
        "remove".to_owned(),
        validated.to_string_lossy().into_owned(),
    ];
    let result = output(policy, repository_root, &args, GIT_TIMEOUT).await?;
    if !result.status.success() {
        return Err(AppError::new(
            "worktree_remove_failed",
            "Git could not remove the task workspace.",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{now, PermissionPolicy, SessionStatus, WorkingContext, WorkspaceMode};
    use std::{path::Path, process::Command};
    use tempfile::TempDir;

    fn git(root: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .expect("Git is required for worktree integration tests");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    fn repository(temp: &TempDir) -> (PathBuf, String) {
        let root = temp.path().join("repository");
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "--quiet"]);
        git(
            &root,
            &["config", "user.email", "jevcode-test@example.invalid"],
        );
        git(&root, &["config", "user.name", "Octran Code Test"]);
        std::fs::write(root.join("src.txt"), "base\n").unwrap();
        git(&root, &["add", "src.txt"]);
        git(&root, &["commit", "--quiet", "-m", "initial"]);
        let branch = git(&root, &["branch", "--show-current"]);
        (root, branch)
    }

    fn test_policy() -> SpawnPolicy {
        SpawnPolicy::uncontained_for_tests(crate::containment::ProfileKind::Inspect)
    }

    fn session(path: &Path, base: &str, branch: &str) -> AgentSession {
        AgentSession {
            task_mode: crate::domain::TaskMode::Agent,
            pinned: false,
            queued_follow_ups: Vec::new(),
            id: "integration-session".into(),
            project_id: "integration-project".into(),
            provider_id: "preview".into(),
            model_id: "preview-model".into(),
            title: "Add isolated workspace support".into(),
            status: SessionStatus::Completed,
            messages: vec![],
            permission_policy: PermissionPolicy::default(),
            pending_tool_call: None,
            pending_permission: None,
            session_permission_grants: vec![],
            one_time_permission_grants: vec![],
            pending_user_input: None,
            queued_tool_calls: vec![],
            iterations: 0,
            tool_calls: 0,
            activity_events: vec![],
            created_at: now(),
            updated_at: now(),
            error: None,
            tool_rounds: 0,
            archived_at: None,
            git_branch: Some(branch.into()),
            workspace_mode: WorkspaceMode::Isolated,
            base_branch: Some(base.into()),
            worktree_path: Some(path.to_string_lossy().into_owned()),
            working_context: WorkingContext::default(),
            project_instruction_files: vec![],
            context_sources: vec![],
            context_resolver_version: crate::guidance::RESOLVER_VERSION,
            context_root: crate::domain::GuidanceRoot::Project,
        }
    }

    #[test]
    fn pull_request_fields_are_trimmed_and_bounded() {
        assert_eq!(
            validated_pr_fields("  Add task inbox  ", "  Summary\n\nDetails  ").unwrap(),
            ("Add task inbox".into(), "Summary\n\nDetails".into())
        );
        assert_eq!(
            validated_pr_fields("  ", "body").unwrap_err().code,
            "invalid_pull_request_title"
        );
        assert_eq!(
            validated_pr_fields(&"x".repeat(201), "").unwrap_err().code,
            "invalid_pull_request_title"
        );
        assert_eq!(
            validated_pr_fields("title", &"x".repeat(10_001))
                .unwrap_err()
                .code,
            "invalid_pull_request_body"
        );
        assert_eq!(
            validated_pr_fields("bad\ntitle", "").unwrap_err().code,
            "invalid_pull_request_title"
        );
        assert_eq!(
            validated_pr_fields("title", "bad\0body").unwrap_err().code,
            "invalid_pull_request_body"
        );
    }

    #[test]
    fn pull_request_url_parser_accepts_https_pull_request_links_only() {
        assert_eq!(
            pull_request_url(b"https://github.com/octran/example/pull/42\n"),
            Some("https://github.com/octran/example/pull/42".into())
        );
        assert_eq!(
            pull_request_url(b"https://git.example.com/team/project/pull/7\n"),
            Some("https://git.example.com/team/project/pull/7".into())
        );
        assert_eq!(
            pull_request_url(b"http://github.com/octran/example/pull/42"),
            None
        );
        assert_eq!(
            pull_request_url(b"https://github.com/octran/example/issues/42"),
            None
        );
        assert_eq!(
            pull_request_url(b"https://github.com/octran/example/pull/nope"),
            None
        );
    }

    #[test]
    fn github_pull_request_status_normalizes_review_and_check_states() {
        let status = task_pull_request_status(
            br#"[{"url":"https://github.com/octran/example/pull/42","state":"OPEN","isDraft":true,"reviewDecision":"REVIEW_REQUIRED","statusCheckRollup":[{"name":"CI","status":"COMPLETED","conclusion":"SUCCESS","detailsUrl":"https://github.com/octran/example/actions/runs/1"},{"context":"lint","state":"PENDING","targetUrl":"http://example.invalid/check"}]}]"#,
        )
        .unwrap()
        .unwrap();
        assert_eq!(status.state, TaskPullRequestState::Open);
        assert!(status.is_draft);
        assert_eq!(
            status.review_state,
            Some(TaskPullRequestReviewState::ReviewRequired)
        );
        assert_eq!(status.checks.len(), 2);
        assert_eq!(status.checks[0].name, "CI");
        assert_eq!(status.checks[0].state, TaskCheckState::Passed);
        assert_eq!(
            status.checks[0].url.as_deref(),
            Some("https://github.com/octran/example/actions/runs/1")
        );
        assert_eq!(status.checks[1].name, "lint");
        assert_eq!(status.checks[1].state, TaskCheckState::Pending);
        assert_eq!(status.checks[1].url, None);
    }

    #[test]
    fn github_pull_request_status_handles_no_pr_and_rejects_unsafe_links() {
        assert_eq!(task_pull_request_status(b"[]").unwrap(), None);
        assert_eq!(
            task_pull_request_status(br#"[{"url":"javascript:alert(1)","state":"OPEN"}]"#)
                .unwrap_err()
                .code,
            "github_status_invalid"
        );
    }

    #[tokio::test]
    async fn draft_pull_request_refuses_uncommitted_task_changes_before_network_access() {
        let temp = tempfile::tempdir_in(std::env::current_dir().unwrap().join("target")).unwrap();
        let (repository, base) = repository(&temp);
        let worktree = temp.path().join("draft-pr-dirty-task");
        let branch = "jevcode/draft-pr-dirty-task";
        create(test_policy(), &repository, &worktree, &base, branch)
            .await
            .unwrap();
        std::fs::write(worktree.join("src.txt"), "uncommitted task edit\n").unwrap();
        let task = session(&worktree, &base, branch);
        assert_eq!(
            create_draft_pull_request(
                test_policy(),
                &repository,
                &task,
                "Draft review",
                "Review the task diff.",
            )
            .await
            .unwrap_err()
            .code,
            "worktree_has_changes"
        );
    }

    #[tokio::test]
    async fn task_workspace_is_scoped_reviewable_committable_and_safe_to_remove() {
        let temp = tempfile::tempdir_in(std::env::current_dir().unwrap().join("target")).unwrap();
        let (repository, base) = repository(&temp);
        let worktree = temp.path().join("isolated-task");
        let branch = "jevcode/add-feature-test";
        create(test_policy(), &repository, &worktree, &base, branch)
            .await
            .unwrap();
        let parallel_worktree = temp.path().join("parallel-task");
        let parallel_branch = "jevcode/parallel-task";
        create(
            test_policy(),
            &repository,
            &parallel_worktree,
            &base,
            parallel_branch,
        )
        .await
        .unwrap();
        assert_ne!(worktree, parallel_worktree);

        let scoped = project_path(test_policy(), &repository, &repository, &worktree)
            .await
            .unwrap();
        assert_eq!(scoped, std::fs::canonicalize(&worktree).unwrap());
        std::fs::write(worktree.join("src.txt"), "task edit\n").unwrap();
        std::fs::write(worktree.join("new.txt"), "new task file\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(parallel_worktree.join("src.txt"))
                .unwrap()
                .trim_end(),
            "base"
        );
        let task = session(&worktree, &base, branch);
        let info = inspect(test_policy(), &repository, &task).await.unwrap();
        assert!(info.dirty);
        assert!(info.changed_files.iter().any(|file| file.path == "src.txt"));
        assert!(info.changed_files.iter().any(|file| file.path == "new.txt"));
        let task_diff = diff(test_policy(), &repository, &task).await.unwrap();
        assert!(task_diff.contains("task edit"));
        assert!(task_diff.contains("new task file"));
        assert_eq!(
            remove(test_policy(), &repository, &task)
                .await
                .unwrap_err()
                .code,
            "worktree_has_changes"
        );

        commit(test_policy(), &repository, &task).await.unwrap();
        let committed = inspect(test_policy(), &repository, &task).await.unwrap();
        assert!(!committed.dirty);
        assert!(committed.has_committed_changes);
        assert!(apply(test_policy(), &repository, &task)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(git(&repository, &["show", "HEAD:src.txt"]), "task edit");
        assert_eq!(git(&repository, &["show", "HEAD:new.txt"]), "new task file");
        remove(test_policy(), &repository, &task).await.unwrap();
        assert!(!worktree.exists());
        assert!(git(&repository, &["branch", "--list", branch]).contains(branch));
        remove(
            test_policy(),
            &repository,
            &session(&parallel_worktree, &base, parallel_branch),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn apply_detects_conflicts_and_restores_the_project_checkout() {
        let temp = tempfile::tempdir_in(std::env::current_dir().unwrap().join("target")).unwrap();
        let (repository, base) = repository(&temp);
        let worktree = temp.path().join("conflicting-task");
        let branch = "jevcode/conflicting-task";
        create(test_policy(), &repository, &worktree, &base, branch)
            .await
            .unwrap();
        std::fs::write(worktree.join("src.txt"), "task version\n").unwrap();
        git(&worktree, &["add", "src.txt"]);
        git(&worktree, &["commit", "--quiet", "-m", "task edit"]);
        std::fs::write(repository.join("src.txt"), "project version\n").unwrap();
        git(&repository, &["add", "src.txt"]);
        git(&repository, &["commit", "--quiet", "-m", "project edit"]);

        let task = session(&worktree, &base, branch);
        let conflicts = apply(test_policy(), &repository, &task).await.unwrap();
        assert_eq!(conflicts, vec!["src.txt"]);
        assert_eq!(git(&repository, &["status", "--porcelain"]), "");
        assert_eq!(
            git(&repository, &["show", "HEAD:src.txt"]),
            "project version"
        );
        assert!(worktree.exists());
    }
}
