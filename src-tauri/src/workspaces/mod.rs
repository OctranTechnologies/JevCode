use crate::{
    domain::*,
    error::{AppError, AppResult},
    persistence::Database,
};
use ignore::{gitignore::GitignoreBuilder, Match, WalkBuilder};
use std::path::{Component, Path, PathBuf};

const MAX_DIRECTORY_ENTRIES: usize = 500;
const MAX_SCANNED_FILES: usize = 20_000;
const MAX_INSTRUCTION_BYTES: u64 = 64 * 1024;

/// Load root-level project guidance without giving the webview a filesystem
/// primitive. Symlinks and unexpectedly large instruction files are ignored.
pub fn load_project_instructions(root: &Path) -> AppResult<Vec<(String, String)>> {
    let root = std::fs::canonicalize(root)?;
    let mut loaded = Vec::new();
    for name in ["AGENTS.md", "CLAUDE.md"] {
        let path = root.join(name);
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => continue,
        };
        if metadata.file_type().is_symlink()
            || !metadata.is_file()
            || metadata.len() > MAX_INSTRUCTION_BYTES
        {
            continue;
        }
        let contents = std::fs::read_to_string(&path).map_err(|_| {
            AppError::new(
                "instruction_read_failed",
                format!("Could not read {name} from this project."),
            )
        })?;
        loaded.push((name.to_owned(), contents));
    }
    Ok(loaded)
}

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
    let opened_at = now();
    if let Some(mut project) = database
        .projects()?
        .into_iter()
        .find(|project| Path::new(&project.path) == root)
    {
        project.last_opened_at = opened_at;
        project.is_recent = true;
        project.repository_root =
            find_repository_root(&root).map(|path| path.to_string_lossy().to_string());
        database.save_project(&project)?;
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
        repository_root: find_repository_root(&root).map(|path| path.to_string_lossy().to_string()),
        active_branch: None,
        last_opened_at: opened_at.clone(),
        project_instructions: String::new(),
        preferred_model: None,
        permissions: PermissionPolicy::default(),
        is_recent: true,
        created_at: opened_at,
    };
    database.save_project(&project)?;
    Ok(project)
}

pub fn create_project(database: &Database, name: &str, parent: &str) -> AppResult<Project> {
    let name = name.trim();
    if !valid_project_name(name) {
        return Err(AppError::new(
            "invalid_name",
            "Use a folder name without path separators or reserved characters.",
        ));
    }
    let parent = std::fs::canonicalize(parent).map_err(|_| {
        AppError::new(
            "invalid_path",
            "Choose an existing parent folder for the new project.",
        )
    })?;
    if !parent.is_dir() {
        return Err(AppError::new(
            "invalid_path",
            "Choose an existing parent folder for the new project.",
        ));
    }
    let destination = parent.join(name);
    std::fs::create_dir(&destination).map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            AppError::new("already_exists", "A folder with that name already exists.")
        } else {
            AppError::new("create_failed", "The project folder could not be created.")
        }
    })?;
    let created = std::fs::canonicalize(&destination)?;
    if created.parent() != Some(parent.as_path()) {
        return Err(AppError::new(
            "invalid_path",
            "The new project folder resolved outside its selected parent.",
        ));
    }
    open_project(database, &created.to_string_lossy())
}

fn valid_project_name(name: &str) -> bool {
    if name.is_empty()
        || name.len() > 100
        || name == "."
        || name == ".."
        || name.ends_with(' ')
        || name.ends_with('.')
        || name.chars().any(|character| {
            character.is_control()
                || ['/', '\\', ':', '*', '?', '"', '<', '>', '|'].contains(&character)
        })
    {
        return false;
    }
    #[cfg(windows)]
    {
        let device = name.split('.').next().unwrap_or("").to_ascii_uppercase();
        if matches!(device.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (device.len() == 4
                && (device.starts_with("COM") || device.starts_with("LPT"))
                && device.as_bytes()[3].is_ascii_digit()
                && device.as_bytes()[3] != b'0')
        {
            return false;
        }
    }
    true
}

pub fn find_repository_root(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|candidate| candidate.join(".git").exists())
        .and_then(|candidate| std::fs::canonicalize(candidate).ok())
}

/// Validate a user-relative path against the registered project root. This is
/// the only path resolver exposed to tools; the webview never supplies a root.
pub fn scoped_path(root: &Path, relative: &str) -> AppResult<PathBuf> {
    let relative_path = Path::new(relative);
    if relative_path.is_absolute()
        || relative_path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::Prefix(_)))
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
    if contains_restricted_component(suffix) {
        return Err(AppError::new(
            "permission_denied",
            "This path contains credentials or internal service files.",
        ));
    }
    Ok(target)
}

fn contains_restricted_component(path: &Path) -> bool {
    path.components().any(|part| {
        let name = part.as_os_str().to_string_lossy().to_lowercase();
        matches!(
            name.as_str(),
            ".git" | ".aws" | ".ssh" | ".codex" | "node_modules" | "target"
        ) || name == ".env"
            || name.starts_with(".env.")
    })
}

/// Return only the requested directory's immediate children. WalkerBuilder
/// applies root and nested .gitignore rules without traversing sibling trees.
pub fn list_directory(root: &Path, relative: &str) -> AppResult<Vec<ProjectFileEntry>> {
    let directory = scoped_path(root, relative)?;
    if !directory.is_dir() {
        return Err(AppError::new("not_directory", "Choose a project folder."));
    }
    let root = std::fs::canonicalize(root)?;
    let mut entries = Vec::new();
    let walker = WalkBuilder::new(&directory)
        .max_depth(Some(1))
        .hidden(false)
        .parents(true)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .follow_links(false)
        .require_git(false)
        .build();
    for result in walker {
        let Ok(entry) = result else { continue };
        if entry.depth() == 0 || entries.len() >= MAX_DIRECTORY_ENTRIES {
            continue;
        }
        let path = entry.path();
        let Ok(canonical) = std::fs::canonicalize(path) else {
            continue;
        };
        let Ok(suffix) = canonical.strip_prefix(&root) else {
            continue;
        };
        if contains_restricted_component(suffix) {
            continue;
        }
        let Ok(relative_path) = path.strip_prefix(&root) else {
            continue;
        };
        let Some(name) = path.file_name() else {
            continue;
        };
        let Ok(metadata) = std::fs::symlink_metadata(path) else {
            continue;
        };
        let is_symlink = metadata.file_type().is_symlink();
        let target_metadata = std::fs::metadata(path).ok();
        let kind = if target_metadata.as_ref().is_some_and(|value| value.is_dir()) {
            "directory"
        } else {
            "file"
        };
        entries.push(ProjectFileEntry {
            name: name.to_string_lossy().to_string(),
            path: relative_path.to_string_lossy().replace('\\', "/"),
            kind: kind.into(),
            size_bytes: target_metadata.map(|value| value.len()).unwrap_or(0),
            is_symlink,
        });
    }
    entries.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(entries)
}

pub fn is_ignored(root: &Path, relative: &str) -> bool {
    let Ok(root) = std::fs::canonicalize(root) else {
        return false;
    };
    let input = Path::new(relative);
    let target = if input.is_absolute() {
        input.to_path_buf()
    } else {
        root.join(input)
    };
    let target = lexical_normalize(&target);
    if !target.starts_with(&root) {
        return false;
    }

    let is_dir = target.is_dir();
    let mut directories = Vec::new();
    let mut current = if is_dir {
        target.as_path()
    } else {
        target.parent().unwrap_or(&root)
    };
    loop {
        directories.push(current.to_path_buf());
        if current == root {
            break;
        }
        let Some(parent) = current.parent() else {
            break;
        };
        current = parent;
    }
    directories.reverse();

    let mut ignored = false;
    for directory in directories {
        let mut ignore_files = vec![directory.join(".gitignore")];
        if directory == root {
            ignore_files.push(root.join(".git").join("info").join("exclude"));
        }
        for ignore_file in ignore_files {
            if !ignore_file.is_file() {
                continue;
            }
            let mut builder = GitignoreBuilder::new(&directory);
            if builder.add(&ignore_file).is_some() {
                continue;
            }
            let Ok(matcher) = builder.build() else {
                continue;
            };
            let Ok(candidate) = target.strip_prefix(&directory) else {
                continue;
            };
            match matcher.matched_path_or_any_parents(candidate, is_dir) {
                Match::Ignore(_) => ignored = true,
                Match::Whitelist(_) => ignored = false,
                Match::None => {}
            }
        }
    }
    ignored
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component.as_os_str());
                }
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

#[derive(Debug, Default)]
pub struct ProjectScan {
    pub size_bytes: u64,
    pub limited: bool,
    pub languages: Vec<LanguageCount>,
}

/// A bounded, ignore-aware scan for overview totals. The file tree itself is
/// still fetched on demand one directory at a time.
pub fn scan_project(root: &Path) -> AppResult<ProjectScan> {
    let root = std::fs::canonicalize(root)?;
    let mut size_bytes = 0_u64;
    let mut counts = std::collections::HashMap::<String, u32>::new();
    let mut visited = 0usize;
    let mut limited = false;
    let walker = WalkBuilder::new(&root)
        .hidden(false)
        .parents(true)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .follow_links(false)
        .require_git(false)
        .build();
    for result in walker {
        let Ok(entry) = result else { continue };
        if entry.depth() == 0 {
            continue;
        }
        let Ok(relative) = entry.path().strip_prefix(&root) else {
            continue;
        };
        if contains_restricted_component(relative) {
            continue;
        }
        let Ok(metadata) = std::fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            continue;
        }
        visited += 1;
        size_bytes = size_bytes.saturating_add(metadata.len());
        if let Some(language) = language_for_extension(entry.path().extension()) {
            *counts.entry(language.to_string()).or_default() += 1;
        }
        if visited >= MAX_SCANNED_FILES {
            limited = true;
            break;
        }
    }
    let mut languages: Vec<_> = counts
        .into_iter()
        .map(|(name, files)| LanguageCount { name, files })
        .collect();
    languages.sort_by(|a, b| b.files.cmp(&a.files).then_with(|| a.name.cmp(&b.name)));
    languages.truncate(8);
    Ok(ProjectScan {
        size_bytes,
        limited,
        languages,
    })
}

fn language_for_extension(extension: Option<&std::ffi::OsStr>) -> Option<&'static str> {
    let extension = extension?.to_str()?.to_ascii_lowercase();
    Some(match extension.as_str() {
        "rs" => "Rust",
        "ts" | "tsx" | "mts" | "cts" => "TypeScript",
        "js" | "jsx" | "mjs" | "cjs" => "JavaScript",
        "py" | "pyi" => "Python",
        "go" => "Go",
        "java" => "Java",
        "kt" | "kts" => "Kotlin",
        "swift" => "Swift",
        "c" | "h" => "C",
        "cc" | "cpp" | "cxx" | "hpp" => "C++",
        "cs" => "C#",
        "rb" => "Ruby",
        "php" => "PHP",
        "vue" => "Vue",
        "svelte" => "Svelte",
        "html" | "htm" => "HTML",
        "css" | "scss" | "sass" | "less" => "CSS",
        "json" => "JSON",
        "md" | "mdx" => "Markdown",
        "yaml" | "yml" => "YAML",
        "toml" => "TOML",
        "sh" | "bash" | "zsh" => "Shell",
        "sql" => "SQL",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(directory: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(directory)
            .output()
            .expect("git must be installed for repository tests");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn loads_root_project_guidance_files() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("AGENTS.md"), "Keep edits focused.\n").unwrap();
        std::fs::write(root.path().join("CLAUDE.md"), "Run focused tests.\n").unwrap();
        let loaded = load_project_instructions(root.path()).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(
            loaded[0],
            ("AGENTS.md".into(), "Keep edits focused.\n".into())
        );
        assert_eq!(
            loaded[1],
            ("CLAUDE.md".into(), "Run focused tests.\n".into())
        );
    }

    #[cfg(unix)]
    #[test]
    fn ignores_symlinked_instruction_files() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("instructions.md"), "outside guidance").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("instructions.md"),
            root.path().join("AGENTS.md"),
        )
        .unwrap();
        assert!(load_project_instructions(root.path()).unwrap().is_empty());
    }

    #[test]
    fn lazily_lists_children_and_obeys_nested_gitignore_rules() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::create_dir(root.path().join("src/generated")).unwrap();
        std::fs::write(root.path().join(".gitignore"), "*.log\nsrc/generated/\n").unwrap();
        std::fs::write(root.path().join("visible.rs"), "fn main() {}\n").unwrap();
        std::fs::write(root.path().join("hidden.log"), "ignored\n").unwrap();
        std::fs::write(root.path().join("src/nested.ts"), "const x = 1;\n").unwrap();
        std::fs::write(root.path().join("src/generated/bundle.js"), "ignored\n").unwrap();

        let top = list_directory(root.path(), ".").unwrap();
        assert!(top.iter().any(|entry| entry.name == "visible.rs"));
        assert!(top.iter().any(|entry| entry.name == "src"));
        assert!(!top.iter().any(|entry| entry.name == "hidden.log"));
        assert!(!top.iter().any(|entry| entry.name == "generated"));
        let nested = list_directory(root.path(), "src").unwrap();
        assert!(nested.iter().any(|entry| entry.name == "nested.ts"));
        assert!(!nested.iter().any(|entry| entry.name == "generated"));
        assert!(is_ignored(root.path(), "hidden.log"));
    }

    #[test]
    fn multiple_git_projects_reopen_from_sqlite_after_restart() {
        let home = tempfile::tempdir_in(std::env::current_dir().unwrap().join("target")).unwrap();
        let first = tempfile::tempdir_in(home.path()).unwrap();
        let second = tempfile::tempdir_in(home.path()).unwrap();
        for (directory, file, content, branch) in [
            (first.path(), "main.rs", "fn main() {}", "work/first"),
            (second.path(), "app.ts", "export {};", "work/second"),
        ] {
            git(directory, &["init", "--quiet"]);
            git(directory, &["config", "user.email", "dev@example.test"]);
            git(directory, &["config", "user.name", "JevCode Test"]);
            std::fs::write(directory.join(file), content).unwrap();
            git(directory, &["add", file]);
            git(directory, &["commit", "--quiet", "-m", "initial"]);
            git(directory, &["switch", "--quiet", "-c", branch]);
        }
        let database_path = home.path().join("projects.sqlite");
        let first_id;
        {
            let database = Database::open(&database_path).unwrap();
            let first_project = open_project(&database, &first.path().to_string_lossy()).unwrap();
            let second_project = open_project(&database, &second.path().to_string_lossy()).unwrap();
            first_id = first_project.id.clone();
            assert_ne!(first_project.id, second_project.id);
            let expected_root = std::fs::canonicalize(first.path()).unwrap();
            assert_eq!(
                first_project.repository_root.as_deref(),
                Some(expected_root.to_str().unwrap())
            );
            let mut first_project = first_project;
            first_project.project_instructions = "Keep changes minimal.".into();
            first_project.preferred_model = Some("model-a".into());
            first_project.permissions.git = PermissionDecision::Allow;
            database.save_project(&first_project).unwrap();
            database
                .remove_project_from_recents(&second_project.id)
                .unwrap();
        }
        let reopened = Database::open(&database_path).unwrap();
        let projects = reopened.projects().unwrap();
        assert_eq!(projects.len(), 2);
        let first_project = reopened.project(&first_id).unwrap();
        assert_eq!(first_project.project_instructions, "Keep changes minimal.");
        assert_eq!(first_project.preferred_model.as_deref(), Some("model-a"));
        assert_eq!(first_project.permissions.git, PermissionDecision::Allow);
        assert!(first_project.is_recent);
        assert_eq!(
            projects.iter().filter(|project| project.is_recent).count(),
            1
        );

        let first_opened = open_project(&reopened, &first.path().to_string_lossy()).unwrap();
        let second_opened = open_project(&reopened, &second.path().to_string_lossy()).unwrap();
        assert_eq!(first_opened.id, first_id);
        assert_ne!(first_opened.id, second_opened.id);
        assert!(first_opened.is_recent && second_opened.is_recent);
        assert!(first_opened.repository_root.is_some());
        assert!(second_opened.repository_root.is_some());
    }

    #[test]
    fn creates_a_project_as_a_safe_child_of_the_chosen_parent() {
        let parent = tempfile::tempdir_in(std::env::current_dir().unwrap().join("target")).unwrap();
        let database = Database::open(&parent.path().join("projects.sqlite")).unwrap();
        let project =
            create_project(&database, "sample-app", &parent.path().to_string_lossy()).unwrap();
        let canonical_parent = std::fs::canonicalize(parent.path()).unwrap();
        assert_eq!(project.name, "sample-app");
        assert_eq!(
            Path::new(&project.path).parent(),
            Some(canonical_parent.as_path())
        );
        assert!(Path::new(&project.path).is_dir());
        assert!(project.is_recent);
        assert_eq!(
            create_project(&database, "../outside", &parent.path().to_string_lossy())
                .unwrap_err()
                .code,
            "invalid_name"
        );
    }

    #[test]
    fn new_project_names_are_safe_single_folder_names() {
        assert!(valid_project_name("A normal project"));
        assert!(!valid_project_name("../escape"));
        assert!(!valid_project_name("folder/name"));
        assert!(!valid_project_name("CON"));
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
