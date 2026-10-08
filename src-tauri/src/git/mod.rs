use crate::error::{AppError, AppResult};
use std::{path::Path, time::Duration};
use tokio::process::Command;

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
