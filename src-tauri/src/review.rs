use crate::{
    domain::{
        now, AgentSession, ChangeKind, ReviewAllAction, ReviewFileAction, SessionChange,
        SessionChanges, SessionFileDiff, ToolCall,
    },
    error::{AppError, AppResult},
    git,
    persistence::{Database, SessionFileCheckpoint, SessionReviewBaseline},
    workspaces,
};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

const MAX_CHECKPOINT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_DIFF_BYTES: usize = 1024 * 1024;
const MAX_COMMAND_SNAPSHOT_BYTES: u64 = 512 * 1024 * 1024;
const MAX_COMMAND_SNAPSHOT_FILES: usize = 50_000;

#[derive(Debug, Clone)]
struct WorkspaceFile {
    path: String,
    hash: String,
    mode: Option<u32>,
    backup: Option<PathBuf>,
}

pub struct WorkspaceSnapshot {
    root: PathBuf,
    temp: tempfile::TempDir,
    before: BTreeMap<String, WorkspaceFile>,
    statuses: BTreeMap<String, String>,
    status_available: bool,
    status_truncated: bool,
}

pub async fn ensure_baseline(
    database: &Database,
    session: &AgentSession,
    project_root: &Path,
) -> AppResult<()> {
    if database.has_review_baseline(&session.id)? {
        return Ok(());
    }
    let root = fs::canonicalize(project_root).map_err(|_| {
        AppError::new(
            "project_unavailable",
            "The task project folder is unavailable.",
        )
    })?;
    let repo_root = workspaces::find_repository_root(&root);
    let mut head = None;
    let mut statuses = BTreeMap::new();
    let mut status_available = false;
    let mut status_truncated = false;
    if let Some(repo_root) = repo_root {
        match git::review_state(&repo_root).await {
            Ok(snapshot) => {
                head = Some(snapshot.head);
                statuses = snapshot.statuses;
                status_available = true;
                status_truncated = snapshot.truncated;
            }
            Err(error) if error.code == "git_unavailable" || error.code == "not_git_repository" => {
            }
            Err(error) => return Err(error),
        }
    }
    database.save_review_baseline(&SessionReviewBaseline {
        session_id: session.id.clone(),
        project_id: session.project_id.clone(),
        root: root.to_string_lossy().to_string(),
        head,
        statuses,
        status_available,
        status_truncated,
        started_at: now(),
    })
}

pub async fn checkpoint_before(
    database: &Database,
    session_id: &str,
    project_root: &Path,
    call: &ToolCall,
) -> AppResult<()> {
    let baseline = database.review_baseline(session_id)?.ok_or_else(|| {
        AppError::new(
            "checkpoint_missing",
            "The task review baseline is unavailable.",
        )
    })?;
    let root = canonical_root(project_root, &baseline)?;
    let (current_status, _, current_truncated) = status_snapshot(&root).await;
    for (label, target) in change_targets(&root, call)? {
        if let Some(existing) = database.file_checkpoint(session_id, &label)? {
            if !live_matches(&existing)? {
                return Err(AppError::new(
                    "file_changed_externally",
                    "This file changed outside the task. Review it before JevCode edits it again.",
                ));
            }
            continue;
        }
        let (existed_before, before_content, before_mode) = read_initial_file(&target)?;
        let expected_hash = before_content.as_deref().map(content_hash);
        let path_key = git::review_path_key(&target.to_string_lossy());
        let preexisting_status = current_status
            .get(&path_key)
            .cloned()
            .or_else(|| current_truncated.then(|| "unknown".to_owned()));
        database.save_file_checkpoint(&SessionFileCheckpoint {
            session_id: session_id.into(),
            path: label,
            target_path: target.to_string_lossy().to_string(),
            existed_before,
            before_content: before_content.clone(),
            before_mode,
            agent_exists: existed_before,
            agent_content: before_content,
            expected_hash,
            preexisting_status,
            reviewed: false,
        })?;
    }
    Ok(())
}

pub fn checkpoint_after(
    database: &Database,
    session_id: &str,
    project_root: &Path,
    call: &ToolCall,
) -> AppResult<()> {
    let baseline = database.review_baseline(session_id)?.ok_or_else(|| {
        AppError::new(
            "checkpoint_missing",
            "The task review baseline is unavailable.",
        )
    })?;
    let root = canonical_root(project_root, &baseline)?;
    for (label, _) in change_targets(&root, call)? {
        let mut checkpoint = database
            .file_checkpoint(session_id, &label)?
            .ok_or_else(|| {
                AppError::new(
                    "checkpoint_missing",
                    "The file review checkpoint is unavailable.",
                )
            })?;
        match read_live(&checkpoint.target_path)? {
            Some((content, _)) => {
                checkpoint.agent_exists = true;
                checkpoint.expected_hash = Some(content_hash(&content));
                checkpoint.agent_content = Some(content);
            }
            None => {
                checkpoint.agent_exists = false;
                checkpoint.expected_hash = None;
                checkpoint.agent_content = None;
            }
        }
        database.update_file_checkpoint_after(&checkpoint)?;
    }
    Ok(())
}

/// Snapshot visible project files around an arbitrary command. Git status alone
/// cannot reveal edits made by formatters, tests, or user scripts.
pub async fn command_snapshot_before(
    database: &Database,
    session_id: &str,
    project_root: &Path,
) -> AppResult<WorkspaceSnapshot> {
    let baseline = database.review_baseline(session_id)?.ok_or_else(|| {
        AppError::new(
            "checkpoint_missing",
            "The task review baseline is unavailable.",
        )
    })?;
    let root = canonical_root(project_root, &baseline)?;
    let temp = tempfile::tempdir().map_err(AppError::internal)?;
    let (statuses, status_available, status_truncated) = status_snapshot(&root).await;
    let before = scan_workspace(&root, Some(temp.path()))?;

    // Persist the pre-command bytes of already-dirty files. If the app stops
    // while a command is running, those user edits remain recoverable.
    for file in before.values() {
        let status = statuses.get(&git::review_path_key(&file.path));
        if status.is_none() && !status_truncated {
            continue;
        }
        let label = display_path(&root, Path::new(&file.path));
        if let Some(checkpoint) = database.file_checkpoint(session_id, &label)? {
            if !live_matches(&checkpoint)? {
                return Err(AppError::new(
                    "file_changed_externally",
                    format!(
                        "{} changed outside the task. Review it before running another command.",
                        label
                    ),
                ));
            }
            continue;
        }
        let content = fs::read(file.backup.as_ref().expect("snapshot backup exists"))
            .map_err(AppError::internal)?;
        database.save_file_checkpoint(&SessionFileCheckpoint {
            session_id: session_id.into(),
            path: label,
            target_path: file.path.clone(),
            existed_before: true,
            before_content: Some(content.clone()),
            before_mode: file.mode,
            agent_exists: true,
            agent_content: Some(content),
            expected_hash: Some(file.hash.clone()),
            preexisting_status: status.cloned().or_else(|| Some("unknown".into())),
            reviewed: false,
        })?;
    }
    Ok(WorkspaceSnapshot {
        root,
        temp,
        before,
        statuses,
        status_available,
        status_truncated,
    })
}

pub fn command_snapshot_after(
    database: &Database,
    session_id: &str,
    snapshot: WorkspaceSnapshot,
) -> AppResult<()> {
    let _keep_temp_alive = snapshot.temp;
    let after = scan_workspace(&snapshot.root, None)?;
    let keys = snapshot
        .before
        .keys()
        .chain(after.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    for key in keys {
        let before = snapshot.before.get(&key);
        let after = after.get(&key);
        if before.map(|file| file.hash.as_str()) == after.map(|file| file.hash.as_str()) {
            continue;
        }
        let label = display_path(&snapshot.root, Path::new(&key));
        let existing = database.file_checkpoint(session_id, &label)?;
        let (existed_before, before_content, before_mode, preexisting_status) =
            if let Some(checkpoint) = &existing {
                if before.is_some() && !live_matches_at_path(checkpoint, before.unwrap()) {
                    return Err(AppError::new(
                        "file_changed_externally",
                        format!(
                            "{} changed outside the task during command execution.",
                            label
                        ),
                    ));
                }
                (
                    checkpoint.existed_before,
                    checkpoint.before_content.clone(),
                    checkpoint.before_mode,
                    checkpoint.preexisting_status.clone(),
                )
            } else if let Some(file) = before {
                let content = fs::read(file.backup.as_ref().expect("snapshot backup exists"))
                    .map_err(AppError::internal)?;
                let status = snapshot
                    .statuses
                    .get(&git::review_path_key(&file.path))
                    .cloned()
                    .or_else(|| snapshot.status_truncated.then(|| "unknown".into()));
                (true, Some(content), file.mode, status)
            } else {
                let path_status = after.map(|file| git::review_path_key(&file.path));
                let status = path_status
                    .as_ref()
                    .and_then(|path| snapshot.statuses.get(path).cloned());
                (
                    false,
                    None,
                    None,
                    status.or_else(|| snapshot.status_truncated.then(|| "unknown".into())),
                )
            };
        let (agent_exists, agent_content, expected_hash) = if let Some(file) = after {
            let content = fs::read(&file.path).map_err(AppError::internal)?;
            (true, Some(content), Some(file.hash.clone()))
        } else {
            (false, None, None)
        };
        let checkpoint = SessionFileCheckpoint {
            session_id: session_id.into(),
            path: label.clone(),
            target_path: key,
            existed_before,
            before_content,
            before_mode,
            agent_exists,
            agent_content,
            expected_hash,
            preexisting_status,
            reviewed: false,
        };
        if existing.is_some() {
            database.update_file_checkpoint_after(&checkpoint)?;
        } else {
            database.save_file_checkpoint(&checkpoint)?;
        }
    }
    let _ = snapshot.status_available;
    Ok(())
}

fn live_matches_at_path(checkpoint: &SessionFileCheckpoint, file: &WorkspaceFile) -> bool {
    checkpoint.agent_exists && checkpoint.expected_hash.as_deref() == Some(file.hash.as_str())
}

async fn status_snapshot(root: &Path) -> (BTreeMap<String, String>, bool, bool) {
    let Some(repository) = find_git_root(root) else {
        return (BTreeMap::new(), false, false);
    };
    match git::review_state(&repository).await {
        Ok(state) => (state.statuses, true, state.truncated),
        Err(_) => (BTreeMap::new(), false, false),
    }
}

fn scan_workspace(
    root: &Path,
    backup_dir: Option<&Path>,
) -> AppResult<BTreeMap<String, WorkspaceFile>> {
    let mut files = BTreeMap::new();
    let mut total_bytes = 0u64;
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .parents(true)
        .follow_links(false)
        .build();
    for entry in walker {
        let entry = entry.map_err(|_| {
            AppError::new(
                "snapshot_failed",
                "The workspace could not be safely snapshotted before command execution.",
            )
        })?;
        if entry.file_type().is_none_or(|kind| !kind.is_file()) {
            continue;
        }
        let path = fs::canonicalize(entry.path()).map_err(AppError::internal)?;
        if !path.starts_with(root) {
            continue;
        }
        let metadata = fs::metadata(&path).map_err(AppError::internal)?;
        if metadata.len() > MAX_CHECKPOINT_BYTES {
            return Err(AppError::new(
                "snapshot_limit",
                "A workspace file is too large to checkpoint safely; the command was not run.",
            ));
        }
        total_bytes = total_bytes.saturating_add(metadata.len());
        if total_bytes > MAX_COMMAND_SNAPSHOT_BYTES || files.len() >= MAX_COMMAND_SNAPSHOT_FILES {
            return Err(AppError::new(
                "snapshot_limit",
                "This workspace is too large to checkpoint safely before running a command.",
            ));
        }
        let content = fs::read(&path).map_err(AppError::internal)?;
        let hash = content_hash(&content);
        let backup = if let Some(directory) = backup_dir {
            let destination = directory.join(format!("{:08x}.snapshot", files.len()));
            fs::write(&destination, &content).map_err(AppError::internal)?;
            Some(destination)
        } else {
            None
        };
        let key = path.to_string_lossy().to_string();
        files.insert(
            key.clone(),
            WorkspaceFile {
                path: key,
                hash,
                mode: file_mode(&metadata),
                backup,
            },
        );
    }
    Ok(files)
}

fn display_path(root: &Path, target: &Path) -> String {
    let path = target.strip_prefix(root).map_or_else(
        |_| target.to_string_lossy().replace('\\', "/"),
        |relative| relative.to_string_lossy().replace('\\', "/"),
    );
    path.chars().flat_map(char::escape_default).collect()
}

pub async fn list_changes(
    database: &Database,
    session: &AgentSession,
) -> AppResult<SessionChanges> {
    let Some(baseline) = database.review_baseline(&session.id)? else {
        return Ok(SessionChanges {
            session_id: session.id.clone(),
            files: Vec::new(),
            additions: 0,
            deletions: 0,
            tests_summary: test_summary(session),
            working_tree: "clean".into(),
            baseline_head: None,
            baseline_at: session.created_at.clone(),
        });
    };
    let current_status = current_statuses(Path::new(&baseline.root)).await;
    let mut files = Vec::new();
    for checkpoint in database.file_checkpoints(&session.id)? {
        let before_exists = checkpoint.existed_before;
        let after_exists = checkpoint.agent_exists;
        if before_exists == after_exists && checkpoint.before_content == checkpoint.agent_content {
            continue;
        }
        let path_key = git::review_path_key(&checkpoint.target_path);
        let status = current_status
            .as_ref()
            .and_then(|statuses| statuses.get(&path_key));
        let current = live_state_matches(&checkpoint);
        let can_stage = baseline.status_available
            && !baseline.status_truncated
            && checkpoint.preexisting_status.is_none()
            && current
            && Path::new(&checkpoint.target_path).starts_with(
                find_git_root(Path::new(&baseline.root))
                    .as_deref()
                    .unwrap_or(Path::new("")),
            );
        let (additions, deletions) = diff_counts(&checkpoint);
        files.push(SessionChange {
            path: checkpoint.path,
            kind: match (before_exists, after_exists) {
                (false, true) => ChangeKind::Added,
                (true, false) => ChangeKind::Deleted,
                _ => ChangeKind::Modified,
            },
            additions,
            deletions,
            preexisting_status: checkpoint.preexisting_status,
            staged: status.is_some_and(|value| {
                value
                    .chars()
                    .next()
                    .is_some_and(|ch| ch != ' ' && ch != '?')
            }),
            unstaged: status.is_some_and(|value| {
                value.chars().nth(1).is_some_and(|ch| ch != ' ') || value.starts_with("??")
            }),
            conflicted: !current,
            can_stage,
            reviewed: checkpoint.reviewed,
        });
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    let additions = files.iter().map(|file| file.additions).sum();
    let deletions = files.iter().map(|file| file.deletions).sum();
    let working_tree = current_status
        .as_ref()
        .map_or_else(
            || {
                if files.is_empty() {
                    "clean"
                } else {
                    "modified"
                }
            },
            |items| {
                if items.is_empty() {
                    "clean"
                } else {
                    "modified"
                }
            },
        )
        .to_owned();
    Ok(SessionChanges {
        session_id: session.id.clone(),
        files,
        additions,
        deletions,
        tests_summary: test_summary(session),
        working_tree,
        baseline_head: baseline.head,
        baseline_at: baseline.started_at,
    })
}

pub fn file_diff(
    database: &Database,
    session: &AgentSession,
    path: &str,
) -> AppResult<SessionFileDiff> {
    let checkpoint = database
        .file_checkpoint(&session.id, path)?
        .ok_or_else(|| AppError::new("not_found", "This file is no longer in the task review."))?;
    let (diff, binary) = unified_diff(&checkpoint)?;
    let conflicted = !live_state_matches(&checkpoint);
    Ok(SessionFileDiff {
        path: checkpoint.path,
        diff,
        binary,
        conflicted,
    })
}

pub async fn apply_file_action(
    database: &Database,
    session: &AgentSession,
    project_root: &Path,
    path: &str,
    action: ReviewFileAction,
) -> AppResult<()> {
    let baseline = database.review_baseline(&session.id)?.ok_or_else(|| {
        AppError::new("not_found", "There are no changes to review for this task.")
    })?;
    let root = canonical_root(project_root, &baseline)?;
    let checkpoint = database
        .file_checkpoint(&session.id, path)?
        .ok_or_else(|| AppError::new("not_found", "This file is no longer in the task review."))?;
    match action {
        ReviewFileAction::Accept => database.mark_file_checkpoint_reviewed(&session.id, path),
        ReviewFileAction::Revert => {
            unstage_task_changes(&[&checkpoint], &baseline, &root).await?;
            restore_checkpoint(&checkpoint, &root)?;
            database.remove_file_checkpoint(&session.id, path)?;
            database.clear_empty_review(&session.id)
        }
        ReviewFileAction::Stage => stage_checkpoint(&checkpoint, &baseline, &root).await,
        ReviewFileAction::Open => open_checkpoint(&checkpoint),
    }
}

pub async fn apply_all_action(
    database: &Database,
    session: &AgentSession,
    project_root: &Path,
    action: ReviewAllAction,
) -> AppResult<()> {
    let Some(baseline) = database.review_baseline(&session.id)? else {
        return Ok(());
    };
    let root = canonical_root(project_root, &baseline)?;
    let checkpoints = database.file_checkpoints(&session.id)?;
    match action {
        ReviewAllAction::AcceptAll => database.mark_all_file_checkpoints_reviewed(&session.id),
        ReviewAllAction::RevertAll => {
            for checkpoint in &checkpoints {
                if !live_matches(checkpoint)? {
                    return Err(AppError::new(
                        "file_changed_externally",
                        format!(
                            "{} changed outside the task. No files were reverted.",
                            checkpoint.path
                        ),
                    ));
                }
            }
            unstage_task_changes(&checkpoints.iter().collect::<Vec<_>>(), &baseline, &root).await?;
            for checkpoint in &checkpoints {
                restore_checkpoint(checkpoint, &root)?;
            }
            database.clear_review(&session.id)
        }
    }
}

async fn unstage_task_changes(
    checkpoints: &[&SessionFileCheckpoint],
    baseline: &SessionReviewBaseline,
    root: &Path,
) -> AppResult<()> {
    if !baseline.status_available || checkpoints.is_empty() {
        return Ok(());
    }
    let repository = find_git_root(root).ok_or_else(|| {
        AppError::new(
            "not_git_repository",
            "The task Git repository is unavailable.",
        )
    })?;
    let state = git::review_state(&repository).await?;
    let paths = checkpoints
        .iter()
        .filter(|checkpoint| checkpoint.preexisting_status.is_none())
        .filter(|checkpoint| {
            state
                .statuses
                .get(&git::review_path_key(&checkpoint.target_path))
                .and_then(|status| status.chars().next())
                .is_some_and(|index_status| index_status != ' ' && index_status != '?')
        })
        .map(|checkpoint| Path::new(&checkpoint.target_path))
        .collect::<Vec<_>>();
    if paths.is_empty() {
        return Ok(());
    }
    let mut command = tokio::process::Command::new("git");
    command.args(["restore", "--staged", "--"]);
    for path in paths {
        command.arg(path);
    }
    command.current_dir(repository).kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let output = tokio::time::timeout(std::time::Duration::from_secs(15), command.output())
        .await
        .map_err(|_| {
            AppError::new(
                "git_timeout",
                "Git could not safely reset the task's staged paths in time.",
            )
        })?
        .map_err(|_| {
            AppError::new(
                "git_unavailable",
                "Git is unavailable while reverting staged task changes.",
            )
        })?;
    if !output.status.success() {
        return Err(AppError::new(
            "git_unstage_failed",
            "Could not remove this task's staged file changes. The working file was left untouched.",
        ));
    }
    Ok(())
}

fn canonical_root(project_root: &Path, baseline: &SessionReviewBaseline) -> AppResult<PathBuf> {
    let root = fs::canonicalize(project_root).map_err(|_| {
        AppError::new(
            "project_unavailable",
            "The task project folder is unavailable.",
        )
    })?;
    if root != Path::new(&baseline.root) {
        return Err(AppError::new(
            "project_changed",
            "The task project path changed; reopen the project before reviewing files.",
        ));
    }
    Ok(root)
}

fn change_targets(root: &Path, call: &ToolCall) -> AppResult<Vec<(String, PathBuf)>> {
    let paths: Vec<&str> = match call.name.as_str() {
        "apply_patch" | "create_file" | "delete_file" => {
            call.arguments["path"].as_str().into_iter().collect()
        }
        "move_file" => [
            call.arguments["source"].as_str(),
            call.arguments["destination"].as_str(),
        ]
        .into_iter()
        .flatten()
        .collect(),
        _ => return Ok(Vec::new()),
    };
    paths
        .into_iter()
        .map(|value| {
            let input = Path::new(value);
            let unresolved = if input.is_absolute() {
                input.to_path_buf()
            } else {
                root.join(input)
            };
            let target = match fs::symlink_metadata(&unresolved) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(AppError::new(
                        "symlink_blocked",
                        "Agent file edits do not follow symbolic links.",
                    ))
                }
                Ok(metadata) if metadata.is_file() => {
                    fs::canonicalize(&unresolved).map_err(AppError::internal)?
                }
                Ok(_) => {
                    return Err(AppError::new(
                        "not_file",
                        "Agent file changes must target regular files.",
                    ))
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    let parent = unresolved.parent().unwrap_or(root);
                    let parent = fs::canonicalize(parent).map_err(|_| {
                        AppError::new(
                            "invalid_path",
                            "The file's parent directory is unavailable.",
                        )
                    })?;
                    parent.join(unresolved.file_name().ok_or_else(|| {
                        AppError::new("invalid_path", "The file path is invalid.")
                    })?)
                }
                Err(error) => return Err(AppError::internal(error)),
            };
            let label = target.strip_prefix(root).map_or_else(
                |_| target.to_string_lossy().replace('\\', "/"),
                |relative| relative.to_string_lossy().replace('\\', "/"),
            );
            let label: String = label.chars().flat_map(char::escape_default).collect();
            Ok((label, target))
        })
        .collect()
}

fn read_initial_file(target: &Path) -> AppResult<(bool, Option<Vec<u8>>, Option<u32>)> {
    match fs::metadata(target) {
        Ok(metadata) => {
            if !metadata.is_file() {
                return Err(AppError::new(
                    "not_file",
                    "Agent file changes must target regular files.",
                ));
            }
            if metadata.len() > MAX_CHECKPOINT_BYTES {
                return Err(AppError::new(
                    "checkpoint_limit",
                    "This file is too large to checkpoint safely, so the agent left it unchanged.",
                ));
            }
            let content = fs::read(target).map_err(AppError::internal)?;
            Ok((true, Some(content), file_mode(&metadata)))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok((false, None, None)),
        Err(error) => Err(AppError::internal(error)),
    }
}

fn read_live(target: &str) -> AppResult<Option<(Vec<u8>, Option<u32>)>> {
    let path = Path::new(target);
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(AppError::new(
            "file_changed_externally",
            "A file path became a symbolic link after the agent edit.",
        )),
        Ok(metadata) if metadata.is_file() => {
            if metadata.len() > MAX_CHECKPOINT_BYTES {
                return Err(AppError::new(
                    "checkpoint_limit",
                    "The changed file is too large to review safely.",
                ));
            }
            Ok(Some((
                fs::read(path).map_err(AppError::internal)?,
                file_mode(&metadata),
            )))
        }
        Ok(_) => Err(AppError::new(
            "not_file",
            "The changed path is no longer a regular file.",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(AppError::internal(error)),
    }
}

#[cfg(unix)]
fn file_mode(metadata: &fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(metadata.permissions().mode())
}

#[cfg(not(unix))]
fn file_mode(_: &fs::Metadata) -> Option<u32> {
    None
}

fn content_hash(content: &[u8]) -> String {
    let hash = Sha256::digest(content);
    hash.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn live_matches(checkpoint: &SessionFileCheckpoint) -> AppResult<bool> {
    match read_live(&checkpoint.target_path)? {
        Some((content, _)) => Ok(checkpoint.agent_exists
            && checkpoint.expected_hash.as_deref() == Some(content_hash(&content).as_str())),
        None => Ok(!checkpoint.agent_exists && checkpoint.expected_hash.is_none()),
    }
}

fn live_state_matches(checkpoint: &SessionFileCheckpoint) -> bool {
    live_matches(checkpoint).unwrap_or(false)
}

async fn current_statuses(root: &Path) -> Option<BTreeMap<String, String>> {
    let repo_root = find_git_root(root)?;
    git::review_state(&repo_root)
        .await
        .ok()
        .map(|state| state.statuses)
}

fn find_git_root(root: &Path) -> Option<PathBuf> {
    workspaces::find_repository_root(root)
}

fn diff_counts(checkpoint: &SessionFileCheckpoint) -> (u64, u64) {
    let Ok((diff, binary)) = unified_diff(checkpoint) else {
        return (0, 0);
    };
    if binary {
        return (0, 0);
    }
    let additions = diff
        .lines()
        .filter(|line| line.starts_with('+') && !line.starts_with("+++"))
        .count() as u64;
    let deletions = diff
        .lines()
        .filter(|line| line.starts_with('-') && !line.starts_with("---"))
        .count() as u64;
    (additions, deletions)
}

fn unified_diff(checkpoint: &SessionFileCheckpoint) -> AppResult<(String, bool)> {
    let before = checkpoint.before_content.as_deref().unwrap_or_default();
    let after = checkpoint.agent_content.as_deref().unwrap_or_default();
    if before.len().max(after.len()) > MAX_DIFF_BYTES
        || std::str::from_utf8(before).is_err()
        || std::str::from_utf8(after).is_err()
    {
        return Ok((
            "Binary file changed; text diff is unavailable.".into(),
            true,
        ));
    }
    let mut before_file = tempfile::NamedTempFile::new().map_err(AppError::internal)?;
    let mut after_file = tempfile::NamedTempFile::new().map_err(AppError::internal)?;
    before_file.write_all(before).map_err(AppError::internal)?;
    after_file.write_all(after).map_err(AppError::internal)?;
    let mut command = Command::new("git");
    command
        .args([
            "diff",
            "--no-index",
            "--no-ext-diff",
            "--no-color",
            "--unified=4",
            "--",
        ])
        .arg(before_file.path())
        .arg(after_file.path());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let output = command
        .output()
        .map_err(|_| AppError::new("git_unavailable", "Git is unavailable for diff formatting."))?;
    if output.status.code().is_some_and(|code| code > 1) {
        return Err(AppError::new(
            "diff_failed",
            "Could not create a file review diff.",
        ));
    }
    let raw = String::from_utf8_lossy(&output.stdout);
    if raw.contains("Binary files ") {
        return Ok((
            "Binary file changed; text diff is unavailable.".into(),
            true,
        ));
    }
    let mut diff = String::new();
    for line in raw.lines() {
        if line.starts_with("diff --git ") {
            diff.push_str(&format!(
                "diff --git a/{} b/{}\n",
                checkpoint.path, checkpoint.path
            ));
        } else if line.starts_with("index ") {
            continue;
        } else if line.starts_with("--- ") {
            diff.push_str(&format!(
                "--- {}\n",
                if checkpoint.existed_before {
                    format!("a/{}", checkpoint.path)
                } else {
                    "/dev/null".into()
                }
            ));
        } else if line.starts_with("+++ ") {
            diff.push_str(&format!(
                "+++ {}\n",
                if checkpoint.agent_exists {
                    format!("b/{}", checkpoint.path)
                } else {
                    "/dev/null".into()
                }
            ));
        } else {
            diff.push_str(line);
            diff.push('\n');
        }
    }
    if diff.len() > MAX_DIFF_BYTES {
        diff.truncate(MAX_DIFF_BYTES);
        diff.push_str("\n… diff truncated …");
    }
    if diff.is_empty() {
        diff.push_str("No text changes.");
    }
    Ok((diff, false))
}

fn restore_checkpoint(checkpoint: &SessionFileCheckpoint, root: &Path) -> AppResult<()> {
    if !live_matches(checkpoint)? {
        return Err(AppError::new(
            "file_changed_externally",
            format!(
                "{} changed outside the task; it was left untouched.",
                checkpoint.path
            ),
        ));
    }
    let target = Path::new(&checkpoint.target_path);
    if checkpoint.existed_before {
        let parent = target.parent().ok_or_else(|| {
            AppError::new("invalid_path", "The review path has no parent folder.")
        })?;
        if target.starts_with(root) {
            create_parent_within(root, parent)?;
        }
        let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(AppError::internal)?;
        temporary
            .write_all(checkpoint.before_content.as_deref().unwrap_or_default())
            .map_err(AppError::internal)?;
        #[cfg(unix)]
        if let Some(mode) = checkpoint.before_mode {
            use std::os::unix::fs::PermissionsExt;
            temporary
                .as_file()
                .set_permissions(fs::Permissions::from_mode(mode))
                .map_err(AppError::internal)?;
        }
        temporary.persist(target).map_err(AppError::internal)?;
    } else if target.exists() {
        fs::remove_file(target).map_err(AppError::internal)?;
    }
    Ok(())
}

fn create_parent_within(root: &Path, parent: &Path) -> AppResult<()> {
    let relative = parent.strip_prefix(root).map_err(|_| {
        AppError::new(
            "permission_denied",
            "The review directory is outside the project.",
        )
    })?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        if !matches!(component, std::path::Component::Normal(_)) {
            return Err(AppError::new(
                "invalid_path",
                "The review directory path is invalid.",
            ));
        }
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => {
                return Err(AppError::new(
                    "permission_denied",
                    "A review directory resolves through a symbolic link.",
                ))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current).map_err(AppError::internal)?
            }
            Err(error) => return Err(AppError::internal(error)),
        }
    }
    Ok(())
}

async fn stage_checkpoint(
    checkpoint: &SessionFileCheckpoint,
    baseline: &SessionReviewBaseline,
    root: &Path,
) -> AppResult<()> {
    if !baseline.status_available
        || baseline.status_truncated
        || checkpoint.preexisting_status.is_some()
    {
        return Err(AppError::new("stage_unsafe", "This file already had changes before the task. Stage selected hunks in your Git client to keep them separate."));
    }
    if !live_matches(checkpoint)? {
        return Err(AppError::new(
            "file_changed_externally",
            "This file changed outside the task and cannot be staged safely.",
        ));
    }
    let repository = find_git_root(root)
        .ok_or_else(|| AppError::new("not_git_repository", "Staging requires a Git repository."))?;
    let target = Path::new(&checkpoint.target_path);
    if !target.starts_with(&repository) {
        return Err(AppError::new(
            "stage_outside_repository",
            "Only project files inside the Git repository can be staged.",
        ));
    }
    let mut command = tokio::process::Command::new("git");
    command
        .args(["add", "--"])
        .arg(target)
        .current_dir(repository)
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let output = tokio::time::timeout(std::time::Duration::from_secs(15), command.output())
        .await
        .map_err(|_| AppError::new("git_timeout", "Git staging took too long."))?
        .map_err(|_| AppError::new("git_unavailable", "Git is unavailable for staging."))?;
    if !output.status.success() {
        return Err(AppError::new(
            "git_stage_failed",
            "Git could not stage this file.",
        ));
    }
    Ok(())
}

fn open_checkpoint(checkpoint: &SessionFileCheckpoint) -> AppResult<()> {
    if !checkpoint.agent_exists || !Path::new(&checkpoint.target_path).is_file() {
        return Err(AppError::new(
            "file_unavailable",
            "This file no longer exists in the project.",
        ));
    }
    #[cfg(windows)]
    let mut command = {
        use std::os::windows::process::CommandExt;
        let mut command = Command::new("explorer.exe");
        command.creation_flags(0x08000000);
        command
    };
    #[cfg(target_os = "macos")]
    let mut command = Command::new("open");
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = Command::new("xdg-open");
    command
        .arg(&checkpoint.target_path)
        .spawn()
        .map_err(|_| AppError::new("open_failed", "The file could not be opened by the system."))?;
    Ok(())
}

fn test_summary(session: &AgentSession) -> Option<String> {
    let mut best = None;
    for message in &session.messages {
        let Some(result) = &message.tool_result else {
            continue;
        };
        if result.name != "run_command" || result.is_error {
            continue;
        }
        let output = result.structured_content.as_ref()?;
        let args = output["args"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
            .collect::<Vec<_>>();
        if !args
            .iter()
            .any(|arg| matches!(*arg, "test" | "tests" | "check"))
        {
            continue;
        }
        let text = format!(
            "{}\n{}",
            output["stdout"].as_str().unwrap_or_default(),
            output["stderr"].as_str().unwrap_or_default()
        );
        let words = text.split_whitespace().collect::<Vec<_>>();
        let count = words.windows(2).find_map(|pair| {
            if pair[1].to_ascii_lowercase().starts_with("passed") {
                pair[0].parse::<u64>().ok()
            } else {
                None
            }
        });
        best = Some(count.map_or_else(
            || "Tests passed".into(),
            |value| format!("{value} tests passed"),
        ));
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domain::{AgentMessage, MessageRole, Project, SessionStatus},
        persistence::Database,
    };

    fn session(database: &Database, root: &Path) -> AgentSession {
        let project = Project {
            id: "review-project".into(),
            workspace_id: "local".into(),
            name: "review".into(),
            path: root.to_string_lossy().to_string(),
            repository_root: None,
            active_branch: None,
            last_opened_at: now(),
            project_instructions: String::new(),
            preferred_model: None,
            permissions: Default::default(),
            is_recent: true,
            created_at: now(),
        };
        database.save_project(&project).unwrap();
        let session = AgentSession {
            id: "review-session".into(),
            project_id: project.id,
            provider_id: "preview".into(),
            model_id: "mock".into(),
            title: "review".into(),
            status: SessionStatus::Working,
            messages: vec![AgentMessage::text(MessageRole::User, "edit")],
            permission_policy: Default::default(),
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
        };
        database.save_session(&session).unwrap();
        session
    }

    #[tokio::test]
    async fn review_diff_and_revert_preserve_preexisting_user_edits() {
        let temp = tempfile::tempdir_in(std::env::current_dir().unwrap().join("target")).unwrap();
        let root = temp.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        let path = root.join("src/lib.rs");
        std::fs::write(&path, "committed\n").unwrap();
        for args in [
            vec!["init"],
            vec!["config", "user.email", "review@example.test"],
            vec!["config", "user.name", "review-test"],
            vec!["add", "src/lib.rs"],
            vec!["commit", "-m", "initial"],
        ] {
            let output = Command::new("git")
                .args(&args)
                .current_dir(root)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::fs::write(&path, "committed\nuser edit\n").unwrap();
        let database_dir = tempfile::tempdir().unwrap();
        let database = Database::open(&database_dir.path().join("review.sqlite")).unwrap();
        let session = session(&database, root);
        ensure_baseline(&database, &session, root).await.unwrap();
        let preexisting = git::review_state(root).await.unwrap().statuses;
        assert!(preexisting.contains_key(&git::review_path_key(&path.to_string_lossy())));

        let call = ToolCall {
            id: "edit".into(),
            name: "apply_patch".into(),
            arguments: serde_json::json!({"path":"src/lib.rs"}),
        };
        checkpoint_before(&database, &session.id, root, &call)
            .await
            .unwrap();
        std::fs::write(&path, "committed\nuser edit\nagent edit\n").unwrap();
        checkpoint_after(&database, &session.id, root, &call).unwrap();
        let changes = list_changes(&database, &session).await.unwrap();
        assert_eq!(changes.files.len(), 1);
        assert!(changes.files[0].preexisting_status.is_some());
        assert!(!changes.files[0].can_stage);
        let diff = file_diff(&database, &session, "src/lib.rs").unwrap();
        assert!(diff.diff.contains("+agent edit"));
        assert!(!diff.diff.contains("-user edit"));

        apply_all_action(&database, &session, root, ReviewAllAction::RevertAll)
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "committed\nuser edit\n"
        );
        assert!(database.review_baseline(&session.id).unwrap().is_none());
    }

    #[tokio::test]
    async fn accepted_changes_remain_visible_and_can_still_be_reverted() {
        let root = tempfile::tempdir_in(std::env::current_dir().unwrap().join("target")).unwrap();
        let path = root.path().join("new-file.ts");
        let database_dir =
            tempfile::tempdir_in(std::env::current_dir().unwrap().join("target")).unwrap();
        let database_path = database_dir.path().join("review.sqlite");
        let database = Database::open(&database_path).unwrap();
        let session = session(&database, root.path());
        ensure_baseline(&database, &session, root.path())
            .await
            .unwrap();
        let call = ToolCall {
            id: "create".into(),
            name: "create_file".into(),
            arguments: serde_json::json!({"path":"new-file.ts"}),
        };
        checkpoint_before(&database, &session.id, root.path(), &call)
            .await
            .unwrap();
        std::fs::write(&path, "export const taskChange = true;\n").unwrap();
        checkpoint_after(&database, &session.id, root.path(), &call).unwrap();

        apply_file_action(
            &database,
            &session,
            root.path(),
            "new-file.ts",
            ReviewFileAction::Accept,
        )
        .await
        .unwrap();
        let accepted = list_changes(&database, &session).await.unwrap();
        assert_eq!(
            accepted.files.len(),
            1,
            "accepting should mark the change reviewed without hiding it"
        );
        assert!(accepted.files[0].reviewed);
        drop(database);
        let database = Database::open(&database_path).unwrap();
        assert!(list_changes(&database, &session).await.unwrap().files[0].reviewed);

        apply_file_action(
            &database,
            &session,
            root.path(),
            "new-file.ts",
            ReviewFileAction::Revert,
        )
        .await
        .unwrap();
        assert!(!path.exists());
        assert!(list_changes(&database, &session)
            .await
            .unwrap()
            .files
            .is_empty());
    }

    #[tokio::test]
    async fn command_snapshot_tracks_formatter_edits_and_preserves_user_baseline() {
        let root = tempfile::tempdir_in(std::env::current_dir().unwrap().join("target")).unwrap();
        let project = root.path();
        std::fs::write(project.join("source.ts"), "committed\n").unwrap();
        std::fs::create_dir_all(project.join("nested")).unwrap();
        std::fs::write(project.join("nested/remove.ts"), "remove me\n").unwrap();
        for args in [
            vec!["init", "--quiet"],
            vec!["config", "user.email", "review@example.test"],
            vec!["config", "user.name", "review-test"],
            vec!["add", "source.ts", "nested/remove.ts"],
            vec!["commit", "--quiet", "-m", "initial"],
        ] {
            let output = Command::new("git")
                .args(&args)
                .current_dir(project)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::fs::write(project.join("source.ts"), "user edit before task\n").unwrap();
        let database_dir =
            tempfile::tempdir_in(std::env::current_dir().unwrap().join("target")).unwrap();
        let database = Database::open(&database_dir.path().join("review.sqlite")).unwrap();
        let session = session(&database, project);
        ensure_baseline(&database, &session, project).await.unwrap();
        let snapshot = command_snapshot_before(&database, &session.id, project)
            .await
            .unwrap();
        std::fs::write(
            project.join("source.ts"),
            "user edit before task\nformatter edit\n",
        )
        .unwrap();
        std::fs::remove_dir_all(project.join("nested")).unwrap();
        std::fs::write(project.join("created.ts"), "generated by command\n").unwrap();
        command_snapshot_after(&database, &session.id, snapshot).unwrap();

        let changes = list_changes(&database, &session).await.unwrap();
        assert_eq!(changes.files.len(), 3);
        assert!(changes
            .files
            .iter()
            .any(|file| file.path == "source.ts" && file.preexisting_status.is_some()));
        let diff = file_diff(&database, &session, "source.ts").unwrap();
        assert!(diff.diff.contains("+formatter edit"));
        assert!(!diff.diff.contains("-user edit before task"));

        apply_all_action(&database, &session, project, ReviewAllAction::RevertAll)
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(project.join("source.ts")).unwrap(),
            "user edit before task\n"
        );
        assert_eq!(
            std::fs::read_to_string(project.join("nested/remove.ts")).unwrap(),
            "remove me\n"
        );
        assert!(!project.join("created.ts").exists());
    }

    #[tokio::test]
    async fn revert_refuses_to_overwrite_a_file_changed_after_agent_edit() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("file.txt"), "before\n").unwrap();
        let database_dir = tempfile::tempdir().unwrap();
        let database = Database::open(&database_dir.path().join("review.sqlite")).unwrap();
        let session = session(&database, root.path());
        ensure_baseline(&database, &session, root.path())
            .await
            .unwrap();
        let call = ToolCall {
            id: "edit".into(),
            name: "apply_patch".into(),
            arguments: serde_json::json!({"path":"file.txt"}),
        };
        checkpoint_before(&database, &session.id, root.path(), &call)
            .await
            .unwrap();
        std::fs::write(root.path().join("file.txt"), "agent\n").unwrap();
        checkpoint_after(&database, &session.id, root.path(), &call).unwrap();
        std::fs::write(root.path().join("file.txt"), "user after edit\n").unwrap();
        let diff = file_diff(&database, &session, "file.txt").unwrap();
        assert!(diff.conflicted);
        assert!(
            apply_all_action(&database, &session, root.path(), ReviewAllAction::RevertAll)
                .await
                .is_err()
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("file.txt")).unwrap(),
            "user after edit\n"
        );
    }

    #[tokio::test]
    async fn staged_task_file_can_be_reverted_without_leaving_index_changes() {
        let root = tempfile::tempdir_in(std::env::current_dir().unwrap().join("target")).unwrap();
        let project = root.path();
        let path = project.join("file.ts");
        std::fs::write(&path, "before\n").unwrap();
        for args in [
            vec!["init", "--quiet"],
            vec!["config", "user.email", "review@example.test"],
            vec!["config", "user.name", "review-test"],
            vec!["add", "file.ts"],
            vec!["commit", "--quiet", "-m", "initial"],
        ] {
            let output = Command::new("git")
                .args(&args)
                .current_dir(project)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let database_dir =
            tempfile::tempdir_in(std::env::current_dir().unwrap().join("target")).unwrap();
        let database = Database::open(&database_dir.path().join("review.sqlite")).unwrap();
        let session = session(&database, project);
        ensure_baseline(&database, &session, project).await.unwrap();
        let call = ToolCall {
            id: "edit".into(),
            name: "apply_patch".into(),
            arguments: serde_json::json!({"path":"file.ts"}),
        };
        checkpoint_before(&database, &session.id, project, &call)
            .await
            .unwrap();
        std::fs::write(&path, "after\n").unwrap();
        checkpoint_after(&database, &session.id, project, &call).unwrap();
        assert!(list_changes(&database, &session).await.unwrap().files[0].can_stage);
        apply_file_action(
            &database,
            &session,
            project,
            "file.ts",
            ReviewFileAction::Stage,
        )
        .await
        .unwrap();
        assert!(list_changes(&database, &session).await.unwrap().files[0].staged);
        apply_file_action(
            &database,
            &session,
            project,
            "file.ts",
            ReviewFileAction::Revert,
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), "before\n");
        assert!(git::review_state(project)
            .await
            .unwrap()
            .statuses
            .is_empty());
    }
}
