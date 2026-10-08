use crate::{
    domain::*,
    error::{AppError, AppResult},
    persistence::Database,
};
use std::path::{Path, PathBuf};

pub fn open_project(database: &Database, path: &str) -> AppResult<Project> {
    let root = std::fs::canonicalize(path).map_err(|_| {
        AppError::new(
            "invalid_path",
            "This folder could not be opened. Choose an existing local folder.",
        )
    })?;
    if !root.is_dir() {
        return Err(AppError::new(
            "invalid_path",
            "Choose a folder, rather than a file.",
        ));
    }
    let canonical = root.to_string_lossy().to_string();
    if let Some(project) = database
        .projects()?
        .into_iter()
        .find(|project| project.path == canonical)
    {
        return Ok(project);
    }
    let project = Project {
        id: id(),
        workspace_id: "local".into(),
        name: root
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "Workspace".into()),
        path: canonical,
        created_at: now(),
    };
    database.save_project(&project)?;
    Ok(project)
}

/// Canonicalization closes traversal and symlink/junction escapes. Hidden service
/// directories are blocked even if accessed via an alias inside the project.
pub fn scoped_path(root: &Path, relative: &str) -> AppResult<PathBuf> {
    let relative_path = Path::new(relative);
    if relative_path.is_absolute()
        || relative_path.components().any(|part| {
            matches!(
                part,
                std::path::Component::ParentDir | std::path::Component::Prefix(_)
            )
        })
    {
        return Err(AppError::new(
            "permission_denied",
            "Use a path relative to the selected project.",
        ));
    }
    let canonical_root = std::fs::canonicalize(root)?;
    let target = std::fs::canonicalize(canonical_root.join(relative_path))
        .map_err(|_| AppError::new("invalid_path", "The requested path does not exist."))?;
    let suffix = target.strip_prefix(&canonical_root).map_err(|_| {
        AppError::new(
            "permission_denied",
            "The requested path is outside the selected project.",
        )
    })?;
    if suffix.components().any(|part| {
        let name = part.as_os_str().to_string_lossy().to_lowercase();
        matches!(
            name.as_str(),
            ".git" | ".aws" | ".ssh" | ".codex" | "node_modules" | "target"
        ) || name == ".env"
            || name.starts_with(".env.")
    }) {
        return Err(AppError::new(
            "permission_denied",
            "This path contains credentials or internal service files.",
        ));
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scopes_paths_and_blocks_secrets() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("README.md"), "safe").unwrap();
        std::fs::write(root.path().join(".env"), "secret").unwrap();
        assert!(scoped_path(root.path(), "README.md").is_ok());
        assert!(scoped_path(root.path(), "../outside").is_err());
        assert!(scoped_path(root.path(), ".env").is_err());
        assert!(scoped_path(root.path(), root.path().to_str().unwrap()).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escape() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("escape")).unwrap();
        assert!(scoped_path(root.path(), "escape").is_err());
    }
}
