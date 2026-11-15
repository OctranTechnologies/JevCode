//! Resolves the guidance a new task starts with. The order is stable: project
//! settings first, then `AGENTS.md` and `CLAUDE.md` in each folder between the
//! repository root and the opened folder (outermost first), then the same
//! files in the opened folder itself, so the most specific guidance comes last.
//! Folders outside the repository are never read, links are reported but never
//! followed, and unreadable or oversized files stay visible as skipped. The
//! result is captured with the task, so later edits affect new tasks only.
use crate::{
    context_migration::{preview, secret_line_numbers},
    domain::{
        EffectiveContext, EffectiveContextBasis, EffectiveContextSource, SourceChange,
        TaskContextScope, TaskContextSource, TaskContextStatus,
    },
    error::{AppError, AppResult},
    workspaces::{LOADED_INSTRUCTION_FILES, MAX_INSTRUCTION_BYTES},
};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Guidance text one task may carry, counting settings and every file.
pub const MAX_TASK_GUIDANCE_BYTES: u64 = 256 * 1024;

pub struct ResolvedGuidance {
    pub sources: Vec<TaskContextSource>,
    /// Sections for included sources, in the order they are sent to the model.
    pub sections: Vec<GuidanceSection>,
}

pub struct GuidanceSection {
    /// The source path this section came from, matching `TaskContextSource::path`.
    pub path: String,
    pub header: String,
    pub text: String,
}

/// `project_root` is the opened folder. `repository_root`, when it contains the
/// opened folder, bounds the ancestor lookup; without it only the opened folder
/// is read.
pub fn resolve(
    project_root: &Path,
    repository_root: Option<&Path>,
    settings: &str,
) -> AppResult<ResolvedGuidance> {
    let mut resolved = ResolvedGuidance {
        sources: Vec::new(),
        sections: Vec::new(),
    };
    let mut budget = 0u64;

    if !settings.trim().is_empty() {
        let bytes = settings.len() as u64;
        budget += bytes;
        let secrets = secret_line_numbers(settings);
        resolved.sources.push(source(
            "Project settings",
            TaskContextScope::Settings,
            TaskContextStatus::Included,
            included_summary(&secrets, "Included from this project's settings."),
            bytes,
            secrets,
            Some(fingerprint(settings.as_bytes())),
        ));
        resolved.sections.push(GuidanceSection {
            path: "Project settings".into(),
            header: "Project instructions from settings:".into(),
            text: settings.to_owned(),
        });
    }

    let project = canonical(project_root)?;
    let boundary = repository_root
        .and_then(|root| std::fs::canonicalize(root).ok())
        .filter(|root| project.starts_with(root));
    let mut folders = Vec::new();
    if let Some(boundary) = &boundary {
        let mut ancestors = Vec::new();
        let mut current = project.parent();
        while let Some(dir) = current.filter(|dir| dir.starts_with(boundary)) {
            ancestors.push(dir.to_path_buf());
            if dir == boundary.as_path() {
                break;
            }
            current = dir.parent();
        }
        ancestors.reverse();
        folders.extend(
            ancestors
                .into_iter()
                .map(|dir| (dir, TaskContextScope::Ancestor)),
        );
    }
    folders.push((project.clone(), TaskContextScope::Folder));

    let repository = boundary.unwrap_or_else(|| project.clone());
    for (folder, scope) in folders {
        for name in LOADED_INSTRUCTION_FILES {
            let path = folder.join(name);
            let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            let label = display_path(&repository, &path);
            match read_guidance(&path, &metadata) {
                Ok(text) => {
                    let bytes = text.len() as u64;
                    if budget + bytes > MAX_TASK_GUIDANCE_BYTES {
                        resolved.sources.push(source(
                            label,
                            scope,
                            TaskContextStatus::Skipped,
                            "Over the 256 KiB guidance budget for one task.".to_owned(),
                            bytes,
                            Vec::new(),
                            None,
                        ));
                        continue;
                    }
                    budget += bytes;
                    let secrets = secret_line_numbers(&text);
                    resolved.sources.push(source(
                        label.clone(),
                        scope,
                        TaskContextStatus::Included,
                        included_summary(&secrets, "Included at task start."),
                        bytes,
                        secrets,
                        Some(fingerprint(text.as_bytes())),
                    ));
                    resolved.sections.push(GuidanceSection {
                        path: label.clone(),
                        header: format!("Project guidance from {label}:"),
                        text,
                    });
                }
                Err(reason) => {
                    let bytes = if metadata.is_file() {
                        metadata.len()
                    } else {
                        0
                    };
                    resolved.sources.push(source(
                        label,
                        scope,
                        TaskContextStatus::Skipped,
                        reason,
                        bytes,
                        Vec::new(),
                        None,
                    ));
                }
            }
        }
    }
    Ok(resolved)
}

/// What a new task would send right now, with secret lines masked in the previews.
pub fn preview_for_new_task(resolved: &ResolvedGuidance) -> EffectiveContext {
    let sources = resolved
        .sources
        .iter()
        .map(|source| {
            let section = (source.status == TaskContextStatus::Included)
                .then(|| {
                    resolved
                        .sections
                        .iter()
                        .find(|section| section.path == source.path)
                })
                .flatten();
            EffectiveContextSource {
                source: source.clone(),
                change: None,
                preview: section
                    .map(|section| preview(&section.text, &secret_line_numbers(&section.text))),
            }
        })
        .collect();
    EffectiveContext {
        basis: EffectiveContextBasis::Preview,
        budget_bytes: MAX_TASK_GUIDANCE_BYTES,
        used_bytes: included_bytes(&resolved.sources),
        sources,
    }
}

/// Compares what a task recorded when it started with the files as they are now.
/// Included sources that changed, disappeared, or stopped being readable are
/// reported; guidance that appeared since the task started is listed as added.
pub fn against_task(
    recorded: &[TaskContextSource],
    current: &ResolvedGuidance,
) -> EffectiveContext {
    let mut sources: Vec<EffectiveContextSource> = recorded
        .iter()
        .map(|source| {
            let change = (source.status == TaskContextStatus::Included).then(|| {
                match current.sources.iter().find(|now| {
                    now.path == source.path && now.status == TaskContextStatus::Included
                }) {
                    Some(now) if now.fingerprint == source.fingerprint => SourceChange::Unchanged,
                    Some(_) => SourceChange::Changed,
                    None => SourceChange::Removed,
                }
            });
            EffectiveContextSource {
                source: source.clone(),
                change,
                preview: None,
            }
        })
        .collect();
    for now in current
        .sources
        .iter()
        .filter(|now| now.status == TaskContextStatus::Included)
    {
        let recorded_included = recorded
            .iter()
            .any(|source| source.path == now.path && source.status == TaskContextStatus::Included);
        if !recorded_included {
            sources.push(EffectiveContextSource {
                source: now.clone(),
                change: Some(SourceChange::Added),
                preview: None,
            });
        }
    }
    EffectiveContext {
        basis: EffectiveContextBasis::Task,
        budget_bytes: MAX_TASK_GUIDANCE_BYTES,
        used_bytes: included_bytes(recorded),
        sources,
    }
}

fn included_bytes(sources: &[TaskContextSource]) -> u64 {
    sources
        .iter()
        .filter(|source| source.status == TaskContextStatus::Included)
        .map(|source| source.bytes)
        .sum()
}

fn read_guidance(path: &Path, metadata: &std::fs::Metadata) -> Result<String, String> {
    if metadata.file_type().is_symlink() {
        return Err("Symbolic link. JevCode does not follow links for guidance.".into());
    }
    if !metadata.is_file() {
        return Err("Not a regular file.".into());
    }
    if metadata.len() > MAX_INSTRUCTION_BYTES {
        return Err("Larger than 64 KiB.".into());
    }
    let bytes = std::fs::read(path).map_err(|_| "Could not be read.".to_owned())?;
    if bytes.len() as u64 > MAX_INSTRUCTION_BYTES {
        return Err("Larger than 64 KiB.".into());
    }
    String::from_utf8(bytes).map_err(|_| "Not UTF-8 text.".to_owned())
}

fn included_summary(secrets: &[u32], base: &str) -> String {
    match secrets.first() {
        None => base.to_owned(),
        Some(line) => {
            format!("{base} Possible secret on line {line}: review it before starting tasks.")
        }
    }
}

fn source(
    path: impl Into<String>,
    scope: TaskContextScope,
    status: TaskContextStatus,
    summary: impl Into<String>,
    bytes: u64,
    secret_lines: Vec<u32>,
    fingerprint: Option<String>,
) -> TaskContextSource {
    TaskContextSource {
        path: path.into(),
        scope,
        status,
        summary: summary.into(),
        bytes,
        secret_lines,
        fingerprint,
    }
}

/// SHA-256 of the exact bytes, so a later read can tell whether a source changed.
pub fn fingerprint(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn canonical(root: &Path) -> AppResult<PathBuf> {
    std::fs::canonicalize(root).map_err(|_| {
        AppError::new(
            "project_unavailable",
            "This project folder is unavailable. Reopen it from its current location.",
        )
    })
}

/// Forward-slash path relative to the repository (or the opened folder when
/// there is no repository), so the same file reads the same in every view.
fn display_path(base: &Path, path: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write(root: &Path, relative: &str, bytes: &[u8]) {
        let path = relative
            .split('/')
            .fold(root.to_path_buf(), |path, part| path.join(part));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn monorepo() -> TempDir {
        let repo = tempfile::tempdir().unwrap();
        write(repo.path(), "AGENTS.md", b"repository");
        write(repo.path(), "packages/AGENTS.md", b"packages");
        write(repo.path(), "packages/app/AGENTS.md", b"app");
        write(repo.path(), "packages/app/CLAUDE.md", b"app claude");
        write(repo.path(), "packages/app/sub/AGENTS.md", b"nested");
        write(repo.path(), "packages/other/AGENTS.md", b"sibling");
        repo
    }

    fn included_paths(resolved: &ResolvedGuidance) -> Vec<&str> {
        resolved
            .sources
            .iter()
            .filter(|source| source.status == TaskContextStatus::Included)
            .map(|source| source.path.as_str())
            .collect()
    }

    fn texts(resolved: &ResolvedGuidance) -> Vec<&str> {
        resolved
            .sections
            .iter()
            .map(|section| section.text.as_str())
            .collect()
    }

    fn reported<'a>(resolved: &'a ResolvedGuidance, path: &str) -> &'a TaskContextSource {
        resolved
            .sources
            .iter()
            .find(|source| source.path == path)
            .unwrap_or_else(|| panic!("{path} was not reported"))
    }

    #[test]
    fn opened_subfolder_resolves_ancestors_outermost_first() {
        let repo = monorepo();
        let app = repo.path().join("packages/app");
        let resolved = resolve(&app, Some(repo.path()), "").unwrap();

        assert_eq!(
            included_paths(&resolved),
            [
                "AGENTS.md",
                "packages/AGENTS.md",
                "packages/app/AGENTS.md",
                "packages/app/CLAUDE.md",
            ]
        );
        assert_eq!(
            texts(&resolved),
            ["repository", "packages", "app", "app claude"]
        );
    }

    #[test]
    fn siblings_and_nested_folders_are_not_read_at_task_start() {
        let repo = monorepo();
        let app = repo.path().join("packages/app");
        let resolved = resolve(&app, Some(repo.path()), "").unwrap();
        let joined = texts(&resolved).join("\n");

        assert!(!joined.contains("sibling"));
        assert!(!joined.contains("nested"));
    }

    #[test]
    fn without_a_repository_only_the_opened_folder_is_read() {
        let repo = monorepo();
        let app = repo.path().join("packages/app");
        let resolved = resolve(&app, None, "").unwrap();

        assert_eq!(included_paths(&resolved), ["AGENTS.md", "CLAUDE.md"]);
        assert_eq!(texts(&resolved), ["app", "app claude"]);
    }

    #[test]
    fn settings_come_first_and_are_recorded_as_a_source() {
        let repo = monorepo();
        let app = repo.path().join("packages/app");
        let resolved = resolve(&app, Some(repo.path()), "Prefer small diffs.").unwrap();

        assert_eq!(resolved.sources[0].scope, TaskContextScope::Settings);
        assert_eq!(resolved.sections[0].text, "Prefer small diffs.");
        assert_eq!(
            resolved.sections[0].header,
            "Project instructions from settings:"
        );
    }

    #[test]
    fn included_sources_carry_a_fingerprint_of_their_exact_bytes() {
        let repo = monorepo();
        let app = repo.path().join("packages/app");
        let resolved = resolve(&app, Some(repo.path()), "").unwrap();
        let root = reported(&resolved, "AGENTS.md");

        assert_eq!(
            root.fingerprint.as_deref(),
            Some(fingerprint(b"repository").as_str())
        );
        assert_eq!(root.bytes, "repository".len() as u64);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_guidance_is_reported_and_never_read() {
        let repo = monorepo();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("notes.md"), "outside guidance").unwrap();
        let app = repo.path().join("packages/app");
        fs::remove_file(app.join("CLAUDE.md")).unwrap();
        std::os::unix::fs::symlink(outside.path().join("notes.md"), app.join("CLAUDE.md")).unwrap();

        let resolved = resolve(&app, Some(repo.path()), "").unwrap();
        let link = reported(&resolved, "packages/app/CLAUDE.md");

        assert_eq!(link.status, TaskContextStatus::Skipped);
        assert!(link.summary.contains("Symbolic link"));
        assert!(!texts(&resolved).join("\n").contains("outside guidance"));
    }

    #[test]
    fn non_utf8_guidance_is_skipped_without_failing_the_task() {
        let repo = monorepo();
        write(repo.path(), "packages/app/CLAUDE.md", &[0xff, 0xfe, 0x00]);
        let app = repo.path().join("packages/app");

        let resolved = resolve(&app, Some(repo.path()), "").unwrap();
        let entry = reported(&resolved, "packages/app/CLAUDE.md");

        assert_eq!(entry.status, TaskContextStatus::Skipped);
        assert_eq!(entry.summary, "Not UTF-8 text.");
        assert!(included_paths(&resolved).contains(&"packages/app/AGENTS.md"));
    }

    #[test]
    fn guidance_beyond_the_task_budget_is_skipped_and_visible() {
        let repo = monorepo();
        // Leave 4 KiB of the budget; the opened folder's 5 KiB guidance cannot fit in it.
        write(repo.path(), "packages/app/AGENTS.md", &vec![b'a'; 5 * 1024]);
        let app = repo.path().join("packages/app");
        let settings = "s".repeat(MAX_TASK_GUIDANCE_BYTES as usize - 4 * 1024);

        let resolved = resolve(&app, Some(repo.path()), &settings).unwrap();
        let skipped = reported(&resolved, "packages/app/AGENTS.md");

        assert_eq!(skipped.status, TaskContextStatus::Skipped);
        assert!(skipped.summary.contains("guidance budget"));
    }

    #[test]
    fn a_likely_secret_is_included_with_a_visible_warning() {
        let repo = monorepo();
        write(
            repo.path(),
            "AGENTS.md",
            b"Use OPENAI_API_KEY = \"sk-abcdefghijklmnopqrstuvwx\".\n",
        );
        let app = repo.path().join("packages/app");

        let resolved = resolve(&app, Some(repo.path()), "").unwrap();
        let root = reported(&resolved, "AGENTS.md");

        assert_eq!(root.status, TaskContextStatus::Included);
        assert_eq!(root.secret_lines, vec![1]);
        assert!(root.summary.contains("Possible secret on line 1"));
    }

    #[test]
    fn a_new_task_preview_masks_secret_lines() {
        let repo = monorepo();
        write(
            repo.path(),
            "AGENTS.md",
            b"Use OPENAI_API_KEY = \"sk-abcdefghijklmnopqrstuvwx\".\nRun tests.\n",
        );
        let app = repo.path().join("packages/app");
        let resolved = resolve(&app, Some(repo.path()), "").unwrap();

        let view = preview_for_new_task(&resolved);
        let root = view
            .sources
            .iter()
            .find(|item| item.source.path == "AGENTS.md")
            .unwrap();
        let text = root.preview.as_deref().unwrap();

        assert_eq!(view.basis, EffectiveContextBasis::Preview);
        assert!(text.contains("[possible secret hidden]"));
        assert!(text.contains("Run tests."));
        assert!(!text.contains("sk-abcdef"));
    }

    #[test]
    fn a_task_reports_what_changed_since_it_started() {
        let repo = monorepo();
        let app = repo.path().join("packages/app");
        let at_start = resolve(&app, Some(repo.path()), "").unwrap();
        let recorded = at_start.sources.clone();

        write(repo.path(), "AGENTS.md", b"repository, revised");
        fs::remove_file(app.join("CLAUDE.md")).unwrap();
        write(repo.path(), "packages/other/AGENTS.md", b"new sibling");
        write(repo.path(), "packages/CLAUDE.md", b"new ancestor");
        let now = resolve(&app, Some(repo.path()), "").unwrap();

        let view = against_task(&recorded, &now);
        let change = |path: &str| {
            view.sources
                .iter()
                .find(|item| item.source.path == path)
                .and_then(|item| item.change)
        };

        assert_eq!(change("AGENTS.md"), Some(SourceChange::Changed));
        assert_eq!(
            change("packages/app/CLAUDE.md"),
            Some(SourceChange::Removed)
        );
        assert_eq!(
            change("packages/app/AGENTS.md"),
            Some(SourceChange::Unchanged)
        );
        assert_eq!(change("packages/CLAUDE.md"), Some(SourceChange::Added));
        assert!(change("packages/other/AGENTS.md").is_none());
        assert_eq!(view.basis, EffectiveContextBasis::Task);
    }

    #[test]
    fn an_unchanged_task_reports_no_changes() {
        let repo = monorepo();
        let app = repo.path().join("packages/app");
        let at_start = resolve(&app, Some(repo.path()), "").unwrap();
        let now = resolve(&app, Some(repo.path()), "").unwrap();

        let view = against_task(&at_start.sources, &now);
        assert!(view
            .sources
            .iter()
            .filter(|item| item.source.status == TaskContextStatus::Included)
            .all(|item| item.change == Some(SourceChange::Unchanged)));
    }
}
