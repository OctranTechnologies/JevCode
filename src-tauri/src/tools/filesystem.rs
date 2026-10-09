use super::{process::run_program, ToolOutput};
use crate::{
    error::{AppError, AppResult},
    workspaces::{is_ignored, list_directory as list_workspace_directory, scoped_path},
};
use ignore::WalkBuilder;
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::fs;

const MAX_FILE_BYTES: u64 = 65_536;
const MAX_MULTI_READ_BYTES: usize = 256 * 1024;
const MAX_SEARCH_BYTES: usize = 256 * 1024;
const MAX_WALK_FILES: usize = 20_000;
const MAX_PAGE: usize = 100;

fn output(content: impl Into<String>, structured: Value) -> AppResult<ToolOutput> {
    Ok(ToolOutput {
        content: content.into(),
        structured,
    })
}

fn path_value(value: &Value) -> AppResult<&str> {
    value.as_str().ok_or_else(|| {
        AppError::new(
            "invalid_tool_arguments",
            "A workspace-relative path is required.",
        )
    })
}

fn existing_path(root: &Path, value: &Value, external_allowed: bool) -> AppResult<PathBuf> {
    let relative = path_value(value)?;
    if !external_allowed {
        return scoped_path(root, relative);
    }
    let root = std::fs::canonicalize(root)?;
    let input = Path::new(relative);
    let candidate = if input.is_absolute() {
        input.to_path_buf()
    } else {
        root.join(input)
    };
    let target = std::fs::canonicalize(candidate).map_err(|_| {
        AppError::new(
            "invalid_path",
            "The requested file or folder does not exist.",
        )
    })?;
    if restricted(&target) {
        return Err(AppError::new(
            "permission_denied",
            "This path is reserved for credentials or internal service files.",
        ));
    }
    Ok(target)
}

fn reject_ignored(root: &Path, relative: &str) -> AppResult<()> {
    if is_ignored(root, relative) {
        Err(AppError::new(
            "permission_denied",
            "This path is excluded by the project's ignore rules.",
        ))
    } else {
        Ok(())
    }
}

fn reject_symlink(root: &Path, relative: &str, external_allowed: bool) -> AppResult<()> {
    let candidate = Path::new(relative);
    let candidate = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        root.join(candidate)
    };
    if !external_allowed
        && candidate
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(AppError::new(
            "permission_denied",
            "Use a path inside the selected project.",
        ));
    }
    if std::fs::symlink_metadata(candidate).is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        return Err(AppError::new(
            "permission_denied",
            "Editing or moving a symbolic link is not supported.",
        ));
    }
    Ok(())
}

fn create_path(root: &Path, relative: &str, external_allowed: bool) -> AppResult<PathBuf> {
    let relative_path = Path::new(relative);
    if !external_allowed
        && (relative_path.is_absolute()
            || relative_path.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir
                        | std::path::Component::Prefix(_)
                        | std::path::Component::RootDir
                )
            }))
    {
        return Err(AppError::new(
            "permission_denied",
            "Use a path inside the selected project.",
        ));
    }
    let root = std::fs::canonicalize(root)?;
    let filename = relative_path
        .file_name()
        .ok_or_else(|| AppError::new("invalid_path", "Choose a file path inside the project."))?;
    let parent = relative_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let raw_parent = if parent.is_absolute() {
        parent.to_path_buf()
    } else {
        root.join(parent)
    };
    let parent = std::fs::canonicalize(raw_parent)
        .map_err(|_| AppError::new("invalid_path", "The destination folder must already exist."))?;
    let suffix = parent.strip_prefix(&root).ok();
    if !external_allowed && suffix.is_none() {
        return Err(AppError::new(
            "permission_denied",
            "The destination is outside the selected project.",
        ));
    }
    if restricted(&parent) || suffix.is_some_and(restricted) {
        return Err(AppError::new(
            "permission_denied",
            "This path is reserved for internal or credential files.",
        ));
    }
    let target = parent.join(filename);
    if target.exists() {
        let canonical = std::fs::canonicalize(&target)?;
        if (!external_allowed && !canonical.starts_with(&root)) || restricted(&canonical) {
            return Err(AppError::new(
                "permission_denied",
                "The destination is outside the selected project or reserved.",
            ));
        }
    } else if restricted(Path::new(filename)) {
        return Err(AppError::new(
            "permission_denied",
            "This path is reserved for internal or credential files.",
        ));
    }
    Ok(target)
}

fn restricted(path: &Path) -> bool {
    path.components().any(|component| {
        let name = component.as_os_str().to_string_lossy().to_ascii_lowercase();
        matches!(
            name.as_str(),
            ".git" | ".aws" | ".ssh" | ".codex" | "node_modules" | "target"
        ) || name == ".env"
            || name.starts_with(".env.")
    })
}

pub async fn read_file(
    root: &Path,
    value: &Value,
    external_allowed: bool,
) -> AppResult<ToolOutput> {
    let relative = path_value(value)?;
    reject_ignored(root, relative)?;
    let path = existing_path(root, value, external_allowed)?;
    let metadata = fs::metadata(&path).await?;
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
        return Err(AppError::new(
            "file_limit",
            "Choose a UTF-8 file smaller than 64 KiB.",
        ));
    }
    let bytes = fs::read(&path).await?;
    if bytes.len() > MAX_FILE_BYTES as usize {
        return Err(AppError::new(
            "file_limit",
            "The file grew past the 64 KiB read limit.",
        ));
    }
    let content = String::from_utf8(bytes)
        .map_err(|_| AppError::new("binary_file", "This tool only reads UTF-8 text files."))?;
    output(
        content.clone(),
        json!({"path":relative,"content":content,"bytes":metadata.len()}),
    )
}

pub async fn read_files(
    root: &Path,
    paths: &Value,
    external_allowed: bool,
) -> AppResult<ToolOutput> {
    let paths = paths
        .as_array()
        .ok_or_else(|| AppError::new("invalid_tool_arguments", "paths must be an array."))?;
    let mut entries = Vec::new();
    let mut total_bytes = 0usize;
    let mut truncated = false;
    for (index, value) in paths.iter().enumerate() {
        let relative = path_value(value)?;
        reject_ignored(root, relative)?;
        let path = existing_path(root, value, external_allowed)?;
        let metadata = fs::metadata(&path).await?;
        if !metadata.is_file() {
            return Err(AppError::new(
                "not_file",
                "Every read_files path must be a file.",
            ));
        }
        let available = MAX_MULTI_READ_BYTES.saturating_sub(total_bytes);
        let read_limit = available.min(MAX_FILE_BYTES as usize);
        if read_limit == 0 {
            truncated = index < paths.len();
            break;
        }
        let bytes = fs::read(&path).await?;
        if bytes.len() > read_limit {
            return Err(AppError::new(
                "file_limit",
                "Combined read_files output is limited to 256 KiB.",
            ));
        }
        let content = String::from_utf8(bytes).map_err(|_| {
            AppError::new("binary_file", "read_files accepts UTF-8 text files only.")
        })?;
        total_bytes += content.len();
        entries.push(json!({"path":relative,"content":content}));
    }
    let content = entries
        .iter()
        .map(|entry| {
            format!(
                "--- {} ---\n{}",
                entry["path"].as_str().unwrap_or(""),
                entry["content"].as_str().unwrap_or("")
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    output(
        content,
        json!({"files":entries,"bytes":total_bytes,"truncated":truncated}),
    )
}

pub async fn list_directory(
    root: &Path,
    args: &Value,
    external_allowed: bool,
) -> AppResult<ToolOutput> {
    let relative = args["path"].as_str().unwrap_or(".");
    reject_ignored(root, relative)?;
    let directory = existing_path(root, &Value::String(relative.into()), external_allowed)?;
    if !directory.is_dir() {
        return Err(AppError::new("not_directory", "Choose a project folder."));
    }
    let mut entries = list_workspace_directory(&directory, ".")?;
    for entry in &mut entries {
        entry.path = display_match_path(relative, &entry.path);
    }
    let total = entries.len();
    let offset = args["offset"].as_u64().unwrap_or(0).min(100_000) as usize;
    let limit = args["limit"]
        .as_u64()
        .unwrap_or(100)
        .clamp(1, MAX_PAGE as u64) as usize;
    let page: Vec<_> = entries.into_iter().skip(offset).take(limit).collect();
    let has_more = offset + page.len() < total;
    let content = page
        .iter()
        .map(|entry| {
            format!(
                "{}{}",
                entry.path,
                if entry.kind == "directory" { "/" } else { "" }
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    output(
        content,
        json!({"path":relative,"entries":page,"offset":offset,"limit":limit,"total":total,"hasMore":has_more}),
    )
}

pub async fn file_metadata(
    root: &Path,
    value: &Value,
    external_allowed: bool,
) -> AppResult<ToolOutput> {
    let relative = path_value(value)?;
    reject_ignored(root, relative)?;
    let path = existing_path(root, value, external_allowed)?;
    let metadata = fs::metadata(&path).await?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs());
    let result = json!({"path":relative,"kind":if metadata.is_dir(){"directory"}else if metadata.is_file(){"file"}else{"other"},"sizeBytes":metadata.len(),"readonly":metadata.permissions().readonly(),"modifiedUnixSeconds":modified});
    output(result.to_string(), result)
}

fn optional_root(
    root: &Path,
    args: &Value,
    external_allowed: bool,
) -> AppResult<(PathBuf, String)> {
    let relative = args["path"].as_str().unwrap_or(".");
    reject_ignored(root, relative)?;
    let path = existing_path(root, &Value::String(relative.into()), external_allowed)?;
    if !path.is_dir() {
        return Err(AppError::new(
            "not_directory",
            "Choose a project directory.",
        ));
    }
    Ok((path, relative.to_owned()))
}

fn internal_ignore_file() -> AppResult<tempfile::NamedTempFile> {
    use std::io::Write;
    let mut file = tempfile::NamedTempFile::new()
        .map_err(|_| AppError::new("search_failed", "Could not prepare safe search rules."))?;
    file.write_all(
        b".git/\n**/.git/\n.aws/\n**/.aws/\n.ssh/\n**/.ssh/\n.codex/\n**/.codex/\nnode_modules/\n**/node_modules/\ntarget/\n**/target/\n.env*\n**/.env*\n",
    )
    .map_err(|_| AppError::new("search_failed", "Could not prepare safe search rules."))?;
    Ok(file)
}

fn page_strings(items: &[String], args: &Value) -> (Vec<String>, usize, bool) {
    let offset = args["offset"].as_u64().unwrap_or(0).min(100_000) as usize;
    let limit = args["limit"]
        .as_u64()
        .unwrap_or(50)
        .clamp(1, MAX_PAGE as u64) as usize;
    let page = items
        .iter()
        .skip(offset)
        .take(limit)
        .cloned()
        .collect::<Vec<_>>();
    let more = offset + page.len() < items.len();
    (page, offset, more)
}

fn display_match_path(base: &str, found: &str) -> String {
    let normalized = found.replace('\\', "/");
    let found = normalized.strip_prefix("./").unwrap_or(&normalized);
    let normalized_base = base.replace('\\', "/");
    let base = normalized_base
        .strip_prefix("./")
        .unwrap_or(&normalized_base);
    if base == "." || base.is_empty() {
        return found.to_owned();
    }
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        found.trim_start_matches(['/', '\\'])
    )
}

pub async fn search_files(
    root: &Path,
    args: &Value,
    external_allowed: bool,
) -> AppResult<ToolOutput> {
    let query = args["query"].as_str().unwrap_or_default();
    let (directory, relative) = optional_root(root, args, external_allowed)?;
    let glob = if query
        .chars()
        .any(|character| ['*', '?', '[', '{'].contains(&character))
    {
        query.to_owned()
    } else {
        format!("*{query}*")
    };
    let mut command_args = vec!["--no-require-git".into(), "--files".into()];
    let ignore_file = internal_ignore_file()?;
    command_args.extend([
        "--ignore-file".into(),
        ignore_file.path().to_string_lossy().into_owned(),
    ]);
    command_args.extend(["--glob".into(), glob, "--".into(), ".".into()]);
    let rg = run_program(
        "rg",
        &command_args,
        &directory,
        Duration::from_secs(8),
        MAX_SEARCH_BYTES,
    )
    .await;
    let (mut items, engine, truncated) = match rg {
        Ok(output) if output.exit_code == Some(0) || output.exit_code == Some(1) => (
            output
                .stdout
                .lines()
                .filter(|line| !line.is_empty() && !restricted(Path::new(line)))
                .map(|line| display_match_path(&relative, line))
                .collect::<Vec<_>>(),
            "ripgrep",
            output.truncated,
        ),
        Err(error) if error.code == "program_unavailable" => {
            let mut files = Vec::new();
            let walker = WalkBuilder::new(&directory)
                .hidden(false)
                .git_ignore(true)
                .git_exclude(true)
                .git_global(true)
                .follow_links(false)
                .require_git(false)
                .filter_entry(|entry| !restricted(entry.path()))
                .build();
            for entry in walker.filter_map(Result::ok).filter(|entry| {
                entry.depth() > 0 && entry.file_type().is_some_and(|kind| kind.is_file())
            }) {
                if files.len() >= MAX_WALK_FILES {
                    break;
                }
                let path = entry.path();
                let Ok(path) = path.strip_prefix(&directory) else {
                    continue;
                };
                if restricted(path) {
                    continue;
                }
                let text = path.to_string_lossy().replace('\\', "/");
                let file_name = path.file_name().unwrap_or_default().to_string_lossy();
                let needle = query.replace(['*', '?', '[', ']'], "");
                if file_name
                    .to_ascii_lowercase()
                    .contains(&needle.to_ascii_lowercase())
                {
                    files.push(display_match_path(&relative, &text));
                }
            }
            (files, "ignore-walk", false)
        }
        Err(error) => return Err(error),
        Ok(output) => return Err(AppError::new("search_failed", output.stderr)),
    };
    items.sort();
    let total = items.len();
    let (page, offset, has_more) = page_strings(&items, args);
    output(
        page.join("\n"),
        json!({"query":query,"path":relative,"paths":page,"offset":offset,"limit":args["limit"].as_u64().unwrap_or(50),"total":total,"hasMore":has_more,"truncated":truncated,"engine":engine}),
    )
}

pub async fn search_text(
    root: &Path,
    args: &Value,
    external_allowed: bool,
) -> AppResult<ToolOutput> {
    search_text_query(
        root,
        args["query"].as_str().unwrap_or_default(),
        args,
        external_allowed,
    )
    .await
}

pub async fn find_symbol(
    root: &Path,
    args: &Value,
    external_allowed: bool,
) -> AppResult<ToolOutput> {
    search_text_query(
        root,
        args["symbol"].as_str().unwrap_or_default(),
        args,
        external_allowed,
    )
    .await
}

pub async fn find_references(
    root: &Path,
    args: &Value,
    external_allowed: bool,
) -> AppResult<ToolOutput> {
    search_text_query(
        root,
        args["symbol"].as_str().unwrap_or_default(),
        args,
        external_allowed,
    )
    .await
}

async fn search_text_query(
    root: &Path,
    query: &str,
    args: &Value,
    external_allowed: bool,
) -> AppResult<ToolOutput> {
    let (directory, relative) = optional_root(root, args, external_allowed)?;
    let mut rg_args = vec![
        "--no-require-git".into(),
        "--json".into(),
        "--line-number".into(),
        "--column".into(),
        "--max-columns".into(),
        "500".into(),
        "--max-columns-preview".into(),
        "--max-count".into(),
        "20".into(),
        "--fixed-strings".into(),
    ];
    let ignore_file = internal_ignore_file()?;
    rg_args.extend([
        "--ignore-file".into(),
        ignore_file.path().to_string_lossy().into_owned(),
    ]);
    rg_args.extend(["--".into(), query.into(), ".".into()]);
    let rg = run_program(
        "rg",
        &rg_args,
        &directory,
        Duration::from_secs(10),
        MAX_SEARCH_BYTES,
    )
    .await;
    let (matches, engine, truncated) = match rg {
        Ok(result) if result.exit_code == Some(0) || result.exit_code == Some(1) => {
            let matches = result.stdout.lines().filter_map(|line| serde_json::from_str::<Value>(line).ok()).filter(|record| record["type"] == "match").filter_map(|record| {
                let path = record["data"]["path"]["text"].as_str()?;
                if restricted(Path::new(path)) { return None; }
                let line_number = record["data"]["line_number"].as_u64()?;
                let text = record["data"]["lines"]["text"].as_str()?.trim_end_matches(['\n','\r']);
                Some(json!({"path":display_match_path(&relative, path),"line":line_number,"text":text}))
            }).take(500).collect::<Vec<_>>();
            (matches, "ripgrep", result.truncated)
        }
        Err(error) if error.code == "program_unavailable" => {
            let mut matches = Vec::new();
            let walker = WalkBuilder::new(&directory)
                .hidden(false)
                .git_ignore(true)
                .git_exclude(true)
                .git_global(true)
                .follow_links(false)
                .require_git(false)
                .filter_entry(|entry| !restricted(entry.path()))
                .build();
            let mut scanned = 0;
            'files: for entry in walker.filter_map(Result::ok).filter(|entry| {
                entry.depth() > 0 && entry.file_type().is_some_and(|kind| kind.is_file())
            }) {
                if scanned >= MAX_WALK_FILES {
                    break;
                }
                let path = entry.path();
                let Ok(relative_path) = path.strip_prefix(&directory) else {
                    continue;
                };
                if restricted(relative_path) {
                    continue;
                }
                let Ok(metadata) = std::fs::metadata(path) else {
                    continue;
                };
                if metadata.len() > 2 * 1024 * 1024 {
                    continue;
                }
                let Ok(content) = std::fs::read_to_string(path) else {
                    continue;
                };
                scanned += 1;
                for (index, line) in content.lines().enumerate() {
                    if line.contains(query) {
                        matches.push(json!({"path":display_match_path(&relative, &relative_path.to_string_lossy().replace('\\', "/")),"line":index + 1,"text":line.chars().take(500).collect::<String>()}));
                        if matches.len() >= 500 {
                            break 'files;
                        }
                    }
                }
            }
            (matches, "ignore-walk", false)
        }
        Err(error) => return Err(error),
        Ok(result) => return Err(AppError::new("search_failed", result.stderr)),
    };
    let total = matches.len();
    let offset = args["offset"].as_u64().unwrap_or(0).min(100_000) as usize;
    let limit = args["limit"]
        .as_u64()
        .unwrap_or(50)
        .clamp(1, MAX_PAGE as u64) as usize;
    let page: Vec<_> = matches.into_iter().skip(offset).take(limit).collect();
    let has_more = offset + page.len() < total || truncated;
    let content = page
        .iter()
        .map(|item| {
            format!(
                "{}:{}:{}",
                item["path"].as_str().unwrap_or(""),
                item["line"],
                item["text"].as_str().unwrap_or("")
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    output(
        content,
        json!({"query":query,"path":relative,"matches":page,"offset":offset,"limit":limit,"totalAtMost":total,"hasMore":has_more,"truncated":truncated,"engine":engine}),
    )
}

pub async fn apply_patch(
    root: &Path,
    args: &Value,
    external_allowed: bool,
) -> AppResult<ToolOutput> {
    let relative = path_value(&args["path"])?;
    reject_ignored(root, relative)?;
    reject_symlink(root, relative, external_allowed)?;
    let path = existing_path(root, &args["path"], external_allowed)?;
    let metadata = fs::metadata(&path).await?;
    if !metadata.is_file() || metadata.len() > 1024 * 1024 {
        return Err(AppError::new(
            "file_limit",
            "Patches are limited to regular files up to 1 MiB.",
        ));
    }
    let mut content = fs::read_to_string(&path)
        .await
        .map_err(|_| AppError::new("binary_file", "Patches apply to UTF-8 text files only."))?;
    let edits = args["edits"]
        .as_array()
        .ok_or_else(|| AppError::new("invalid_tool_arguments", "Patch edits must be an array."))?;
    for edit in edits {
        let old = edit["oldText"].as_str().unwrap_or_default();
        let new = edit["newText"].as_str().unwrap_or_default();
        let mut occurrences = content.match_indices(old);
        let Some((start, matched)) = occurrences.next() else {
            return Err(AppError::new(
                "patch_context_missing",
                "Patch context was not found; reread the file and retry with a smaller patch.",
            ));
        };
        if occurrences.next().is_some() {
            return Err(AppError::new(
                "patch_context_ambiguous",
                "Patch context matched more than once; include more surrounding lines.",
            ));
        }
        content.replace_range(start..start + matched.len(), new);
        if content.len() > 1024 * 1024 {
            return Err(AppError::new(
                "file_limit",
                "Patched file would exceed 1 MiB.",
            ));
        }
    }
    fs::write(&path, &content).await?;
    output(
        format!("Applied {} patch edit(s) to {relative}.", edits.len()),
        json!({"path":relative,"edits":edits.len(),"bytes":content.len()}),
    )
}

pub async fn create_file(
    root: &Path,
    args: &Value,
    external_allowed: bool,
) -> AppResult<ToolOutput> {
    let relative = path_value(&args["path"])?;
    reject_ignored(root, relative)?;
    let path = create_path(root, relative, external_allowed)?;
    let content = args["content"].as_str().unwrap_or_default();
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .await
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                AppError::new("already_exists", "That project file already exists.")
            } else {
                AppError::new("create_failed", "Could not create the project file.")
            }
        })?;
    use tokio::io::AsyncWriteExt;
    file.write_all(content.as_bytes()).await?;
    output(
        format!("Created {relative} ({} bytes).", content.len()),
        json!({"path":relative,"bytes":content.len()}),
    )
}

pub async fn delete_file(
    root: &Path,
    args: &Value,
    external_allowed: bool,
) -> AppResult<ToolOutput> {
    let relative = path_value(&args["path"])?;
    reject_ignored(root, relative)?;
    reject_symlink(root, relative, external_allowed)?;
    let path = existing_path(root, &args["path"], external_allowed)?;
    let metadata = fs::metadata(&path).await?;
    if !metadata.is_file() {
        return Err(AppError::new(
            "not_file",
            "delete_file only removes regular files; directories are never recursive.",
        ));
    }
    fs::remove_file(&path).await?;
    output(
        format!("Deleted {relative}."),
        json!({"path":relative,"deleted":true}),
    )
}

pub async fn move_file(root: &Path, args: &Value, external_allowed: bool) -> AppResult<ToolOutput> {
    let source_relative = path_value(&args["source"])?;
    let destination_relative = path_value(&args["destination"])?;
    reject_ignored(root, source_relative)?;
    reject_ignored(root, destination_relative)?;
    reject_symlink(root, source_relative, external_allowed)?;
    let source = existing_path(root, &args["source"], external_allowed)?;
    let metadata = fs::metadata(&source).await?;
    if !metadata.is_file() {
        return Err(AppError::new(
            "not_file",
            "move_file only moves regular files.",
        ));
    }
    let destination = create_path(root, destination_relative, external_allowed)?;
    if destination.exists() {
        return Err(AppError::new(
            "already_exists",
            "Move destination already exists; files are never overwritten.",
        ));
    }
    fs::rename(&source, &destination).await.map_err(|_| {
        AppError::new(
            "move_failed",
            "Could not move the file within the selected project.",
        )
    })?;
    output(
        format!("Moved {source_relative} to {destination_relative}."),
        json!({"source":source_relative,"destination":destination_relative}),
    )
}
