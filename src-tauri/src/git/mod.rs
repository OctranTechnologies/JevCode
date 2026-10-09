use crate::domain::ChangedFile;
use crate::error::{AppError, AppResult};
use std::collections::BTreeMap;
use std::{path::Path, process::Output, time::Duration};
use tokio::process::Command;

pub mod worktree;

async fn output(root: &Path, args: &[String], timeout: Duration) -> AppResult<Output> {
    let mut command = Command::new("git");
    command
        .arg("--no-optional-locks")
        .args(args)
        .current_dir(root)
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    tokio::time::timeout(timeout, command.output())
        .await
        .map_err(|_| AppError::new("timeout", "Git took too long to respond."))?
        .map_err(|_| {
            AppError::new(
                "git_unavailable",
                "Git is unavailable. Install Git and restart JevCode.",
            )
        })
}

fn git_args(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_owned()).collect()
}

#[derive(Debug, Clone)]
pub struct ReviewRepoState {
    pub head: String,
    pub statuses: BTreeMap<String, String>,
    pub truncated: bool,
}

/// Capture the repository revision and exact dirty-path state before the first
/// agent edit in a task. File contents are checkpointed lazily before edits.
pub async fn review_state(root: &Path) -> AppResult<ReviewRepoState> {
    let head = output(
        root,
        &git_args(&["rev-parse", "--verify", "HEAD"]),
        Duration::from_secs(5),
    )
    .await?;
    if !head.status.success() {
        return Err(AppError::new(
            "not_git_repository",
            "This folder has no Git HEAD.",
        ));
    }
    let status = output(
        root,
        &git_args(&["status", "--porcelain=v1", "-z", "--untracked-files=all"]),
        Duration::from_secs(15),
    )
    .await?;
    if !status.status.success() {
        return Err(AppError::new(
            "git_error",
            "Could not capture the initial Git state.",
        ));
    }
    const MAX_STATUS_BYTES: usize = 8 * 1024 * 1024;
    const MAX_STATUS_PATHS: usize = 50_000;
    let bytes = &status.stdout[..status.stdout.len().min(MAX_STATUS_BYTES)];
    let mut truncated = status.stdout.len() > MAX_STATUS_BYTES;
    let mut statuses = BTreeMap::new();
    let mut records = bytes
        .split(|byte| *byte == 0)
        .filter(|item| !item.is_empty());
    while let Some(record) = records.next() {
        if record.len() < 4 {
            continue;
        }
        let status_code = String::from_utf8_lossy(&record[..2]).into_owned();
        let relative = String::from_utf8_lossy(&record[3..]).replace('\\', "/");
        let path = root.join(&relative).to_string_lossy().to_string();
        statuses.insert(review_path_key(&path), status_code.clone());
        if status_code.contains('R') || status_code.contains('C') {
            let _ = records.next();
        }
        if statuses.len() >= MAX_STATUS_PATHS {
            truncated = true;
            break;
        }
    }
    Ok(ReviewRepoState {
        head: String::from_utf8_lossy(&head.stdout).trim().to_owned(),
        statuses,
        truncated,
    })
}

pub fn review_path_key(path: &str) -> String {
    #[cfg(windows)]
    {
        path.to_ascii_lowercase()
    }
    #[cfg(not(windows))]
    {
        path.to_owned()
    }
}

/// Fixed arguments only: no shell interpolation, optional locks or mutations.
pub async fn status(root: &Path) -> AppResult<String> {
    let mut command = Command::new("git");
    command
        .args([
            "--no-optional-locks",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.untrackedCache=false",
            "status",
            "--short",
            "--branch",
            "--untracked-files=no",
        ])
        .current_dir(root)
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let output = tokio::time::timeout(Duration::from_secs(10), command.output())
        .await
        .map_err(|_| AppError::new("timeout", "Git status timed out."))?
        .map_err(|_| {
            AppError::new(
                "git_unavailable",
                "Git is unavailable. Install Git and restart JevCode.",
            )
        })?;
    if !output.status.success() {
        return Err(AppError::new(
            "git_error",
            "Could not read Git status. Make sure the selected folder belongs to a Git repository.",
        ));
    }
    let result = String::from_utf8_lossy(&output.stdout);
    Ok(result.chars().take(65536).collect())
}

/// Return the checked-out branch for toolbar context without mutating the repository.
pub async fn branch(root: &Path) -> AppResult<Option<String>> {
    let output = match output(
        root,
        &git_args(&["rev-parse", "--abbrev-ref", "HEAD"]),
        Duration::from_secs(3),
    )
    .await
    {
        Ok(output) => output,
        Err(error) if error.code == "timeout" || error.code == "git_unavailable" => {
            return Ok(None)
        }
        Err(error) => return Err(error),
    };
    if !output.status.success() {
        return Ok(None);
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if value.is_empty() || value == "HEAD" {
        Ok(None)
    } else {
        Ok(Some(value.chars().take(128).collect()))
    }
}

pub async fn branches(root: &Path) -> AppResult<Vec<String>> {
    let output = output(
        root,
        &git_args(&["branch", "--format=%(refname:short)"]),
        Duration::from_secs(5),
    )
    .await?;
    if !output.status.success() {
        return Err(AppError::new(
            "git_error",
            "Could not list branches for this repository.",
        ));
    }
    let mut branches: Vec<_> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|branch| !branch.is_empty())
        .map(str::to_owned)
        .collect();
    branches.sort_by_key(|branch| branch.to_ascii_lowercase());
    Ok(branches)
}

/// Branch switches are limited to exact existing local names and use Git's
/// normal safety checks, so changes that would be overwritten are preserved.
pub async fn switch_branch(root: &Path, branch_name: &str) -> AppResult<Option<String>> {
    if branch_name.len() > 240 || !branches(root).await?.iter().any(|name| name == branch_name) {
        return Err(AppError::new(
            "unknown_branch",
            "Choose an existing local branch from this repository.",
        ));
    }
    let args = vec![
        "switch".to_owned(),
        "--quiet".to_owned(),
        "--".to_owned(),
        branch_name.to_owned(),
    ];
    let output = output(root, &args, Duration::from_secs(20)).await?;
    if !output.status.success() {
        return Err(AppError::new(
            "git_switch_failed",
            "Git could not switch branches. Resolve any conflicting local changes and try again.",
        ));
    }
    branch(root).await
}

pub async fn changed_files(root: &Path) -> AppResult<Vec<ChangedFile>> {
    let result = output(
        root,
        &git_args(&["status", "--porcelain=v1", "-z", "--untracked-files=all"]),
        Duration::from_secs(10),
    )
    .await?;
    if !result.status.success() {
        return Err(AppError::new(
            "git_error",
            "Could not read Git status for this repository.",
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
        files.push(ChangedFile { path, status });
        if files
            .last()
            .is_some_and(|file| file.status.contains('R') || file.status.contains('C'))
        {
            let _ = records.next(); // Git emits the destination path in a second -z record.
        }
        if files.len() == 500 {
            break;
        }
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn run_git(root: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .expect("git must be installed for repository tests");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    #[tokio::test]
    async fn reports_dirty_state_and_switches_only_to_existing_local_branches() {
        let directory =
            tempfile::tempdir_in(std::env::current_dir().unwrap().join("target")).unwrap();
        let root = directory.path();
        run_git(root, &["init", "--quiet"]);
        run_git(root, &["config", "user.email", "dev@example.test"]);
        run_git(root, &["config", "user.name", "JevCode Test"]);
        std::fs::write(root.join("README.md"), "start\n").unwrap();
        run_git(root, &["add", "README.md"]);
        run_git(root, &["commit", "--quiet", "-m", "initial"]);
        let base = run_git(root, &["branch", "--show-current"]);
        run_git(root, &["switch", "--quiet", "-c", "work/feature"]);

        assert_eq!(branch(root).await.unwrap().as_deref(), Some("work/feature"));
        assert!(branches(root).await.unwrap().contains(&base));
        assert_eq!(
            switch_branch(root, &base).await.unwrap().as_deref(),
            Some(base.as_str())
        );
        std::fs::write(root.join("new.ts"), "export {};\n").unwrap();
        let changes = changed_files(root).await.unwrap();
        assert!(changes
            .iter()
            .any(|file| file.path == "new.ts" && file.status == "??"));
        assert!(switch_branch(root, "work/does-not-exist").await.is_err());
    }
}
