//! Project memories are user-authored notes with native source verification.
//! This module never sends memory contents to Jev or another remote service.
use crate::{
    domain::{now, ProjectMemory, ProjectMemorySourceStatus},
    error::{AppError, AppResult},
    persistence::Database,
    workspaces,
};
use sha2::{Digest, Sha256};
use std::path::{Component, Path, PathBuf};

const MAX_SOURCE_BYTES: u64 = 64 * 1024;

pub fn normalize_source_path(value: &str) -> AppResult<String> {
    let value = value.trim().replace('\\', "/");
    if value.is_empty()
        || value.len() > 500
        || value.starts_with('/')
        || value.starts_with("//")
        || value
            .as_bytes()
            .get(1)
            .is_some_and(|character| *character == b':')
    {
        return Err(AppError::new(
            "invalid_path",
            "Choose a workspace-relative source file path up to 500 characters.",
        ));
    }
    let mut parts = Vec::new();
    for component in Path::new(&value).components() {
        match component {
            Component::Normal(part) => {
                let name = part.to_string_lossy();
                if name.chars().any(char::is_control) {
                    return Err(AppError::new(
                        "invalid_path",
                        "The source path contains unsupported characters.",
                    ));
                }
                parts.push(name.into_owned());
            }
            Component::CurDir => {}
            _ => {
                return Err(AppError::new(
                    "invalid_path",
                    "Choose a workspace-relative source file without parent traversal.",
                ))
            }
        }
    }
    if parts.is_empty() {
        return Err(AppError::new(
            "invalid_path",
            "Choose a workspace-relative source file.",
        ));
    }
    Ok(parts.join("/"))
}

/// Fingerprint a small, visible project file without retaining or returning its contents.
pub fn source_fingerprint(root: &Path, relative: &str) -> AppResult<String> {
    let relative = normalize_source_path(relative)?;
    if workspaces::is_ignored(root, &relative) {
        return Err(AppError::new(
            "permission_denied",
            "Ignored files cannot be used as memory sources.",
        ));
    }
    let root = std::fs::canonicalize(root)?;
    let path = workspaces::scoped_path(&root, &relative)?;
    reject_symlink_components(&root, &relative)?;
    let metadata = std::fs::metadata(&path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            AppError::new("not_found", "The source file could not be found.")
        } else {
            AppError::new(
                "source_unavailable",
                "The source file could not be checked.",
            )
        }
    })?;
    if !metadata.is_file() || metadata.len() > MAX_SOURCE_BYTES {
        return Err(AppError::new(
            "source_unavailable",
            "Memory sources must be regular files no larger than 64 KiB.",
        ));
    }
    let bytes = std::fs::read(path)
        .map_err(|_| AppError::new("source_unavailable", "The source file could not be read."))?;
    if bytes.len() as u64 > MAX_SOURCE_BYTES {
        return Err(AppError::new(
            "source_unavailable",
            "The source file exceeds the 64 KiB check limit.",
        ));
    }
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn reject_symlink_components(root: &Path, relative: &str) -> AppResult<()> {
    let mut current = PathBuf::from(root);
    for component in Path::new(relative).components() {
        if let Component::Normal(part) = component {
            current.push(part);
            let metadata = std::fs::symlink_metadata(&current)
                .map_err(|_| AppError::new("not_found", "The source file could not be found."))?;
            if metadata.file_type().is_symlink() {
                return Err(AppError::new(
                    "permission_denied",
                    "Symbolic links cannot be used as memory sources.",
                ));
            }
        }
    }
    Ok(())
}

pub fn recheck(database: &Database, root: &Path, memory: &mut ProjectMemory) -> AppResult<()> {
    memory.source_status = match (
        memory.source_path.as_deref(),
        memory.source_fingerprint.as_deref(),
    ) {
        (Some(path), Some(expected)) => match source_fingerprint(root, path) {
            Ok(actual) if actual == expected => ProjectMemorySourceStatus::Current,
            Ok(_) => ProjectMemorySourceStatus::Stale,
            Err(error) if matches!(error.code.as_str(), "not_found" | "invalid_path") => {
                ProjectMemorySourceStatus::Missing
            }
            Err(_) => ProjectMemorySourceStatus::Unavailable,
        },
        _ => ProjectMemorySourceStatus::Unverified,
    };
    memory.source_checked_at = Some(now());
    database.save_project_memory(memory)
}

pub fn recheck_project(
    database: &Database,
    project_id: &str,
    root: &Path,
) -> AppResult<Vec<ProjectMemory>> {
    database
        .project_memories(project_id)?
        .into_iter()
        .map(|mut memory| {
            recheck(database, root, &mut memory)?;
            Ok(memory)
        })
        .collect()
}

pub fn reset_source(database: &Database, root: &Path, memory: &mut ProjectMemory) -> AppResult<()> {
    let relative = memory
        .source_path
        .as_deref()
        .ok_or_else(|| {
            AppError::new(
                "invalid_input",
                "Add a source file before refreshing this memory.",
            )
        })
        .and_then(normalize_source_path)?;
    let fingerprint = source_fingerprint(root, &relative)?;
    memory.source_path = Some(relative);
    memory.source_fingerprint = Some(fingerprint);
    memory.source_status = ProjectMemorySourceStatus::Current;
    memory.source_checked_at = Some(now());
    memory.updated_at = now();
    database.save_project_memory(memory)
}

pub fn to_view(memory: ProjectMemory) -> crate::domain::ProjectMemoryView {
    crate::domain::ProjectMemoryView {
        id: memory.id,
        project_id: memory.project_id,
        title: memory.title,
        category: memory.category,
        content: memory.content,
        source_path: memory.source_path,
        source_status: memory.source_status,
        source_checked_at: memory.source_checked_at,
        pinned: memory.pinned,
        created_at: memory.created_at,
        updated_at: memory.updated_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_paths_are_workspace_relative_and_normalized() {
        assert_eq!(
            normalize_source_path(" src\\auth\\provider.rs ").unwrap(),
            "src/auth/provider.rs"
        );
        for path in [
            "../outside.txt",
            "/outside.txt",
            "C:\\outside.txt",
            "\\\\server\\share\\file",
        ] {
            assert!(normalize_source_path(path).is_err(), "accepted {path}");
        }
    }

    #[test]
    fn source_fingerprint_tracks_file_changes_and_respects_gitignore() {
        let root = tempfile::tempdir().unwrap();
        let tracked = root.path().join("README.md");
        std::fs::write(&tracked, "first version").unwrap();
        let first = source_fingerprint(root.path(), "README.md").unwrap();
        assert_eq!(first, source_fingerprint(root.path(), "README.md").unwrap());

        std::fs::write(&tracked, "second version").unwrap();
        assert_ne!(first, source_fingerprint(root.path(), "README.md").unwrap());

        std::fs::write(root.path().join(".gitignore"), "private.txt\n").unwrap();
        std::fs::write(root.path().join("private.txt"), "private").unwrap();
        let error = source_fingerprint(root.path(), "private.txt").unwrap_err();
        assert_eq!(error.code, "permission_denied");
    }
}
