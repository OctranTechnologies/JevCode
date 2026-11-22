//! Resolves the guidance a new task starts with. The order is stable: project
//! settings first, then `AGENTS.md` and `CLAUDE.md` in each folder between the
//! repository root and the opened folder (outermost first), then the same
//! files in the opened folder itself, so the most specific guidance comes last.
//! Folders outside the repository are never read, links are reported but never
//! followed, and unreadable or oversized files stay visible as skipped. The
//! result is captured with the task, so later edits affect new tasks only.
use crate::{
    containment::SpawnPolicy,
    context_migration::{preview, secret_line_numbers},
    domain::{
        AgentSession, EffectiveContext, EffectiveContextBasis, EffectiveContextSource,
        GuidancePrecedence, GuidanceRoot, SourceChange, TaskContextScope, TaskContextSource,
        TaskContextStatus, WorkspaceMode,
    },
    error::{AppError, AppResult},
    workspaces::MAX_INSTRUCTION_BYTES,
};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Guidance text one task may carry, counting settings, files, and rules added
/// later. Small-context models must still fit system instructions, so this stays
/// well under their window.
pub const MAX_TASK_GUIDANCE_BYTES: u64 = 80 * 1024;

/// Version of the rules that choose guidance files and label their sources. A
/// task records the version it was resolved with, so its snapshot can still be
/// read after these rules change. Tasks recorded before versions existed read as zero.
pub const RESOLVER_VERSION: u32 = 1;

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
    precedence: GuidancePrecedence,
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
        let is_repository_root = folder == repository;
        let mut files = root_choices(precedence, is_repository_root, |name| {
            std::fs::symlink_metadata(folder.join(name)).is_ok()
        });
        files.push(("CLAUDE.md", RootChoice::Read));
        for (name, choice) in files {
            let path = folder.join(name);
            let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            let label = display_path(&repository, &path);
            let text = match choice.skip_reason() {
                Some(reason) => Err(reason),
                None => read_guidance(&path, &metadata),
            };
            match text {
                Ok(text) => {
                    let bytes = text.len() as u64;
                    if budget + bytes > MAX_TASK_GUIDANCE_BYTES {
                        resolved.sources.push(source(
                            label,
                            scope,
                            TaskContextStatus::Skipped,
                            "Over the 80 KiB guidance budget for one task.".to_owned(),
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
    resolved.sources.extend(crate::guidance_scope::discover(
        &project,
        &repository,
        precedence,
    ));
    Ok(resolved)
}

/// The Copilot instructions live only at the repository root, so other folders
/// never read them.
const COPILOT_INSTRUCTIONS: &str = ".github/copilot-instructions.md";

/// How the current precedence treats one root guidance file a folder has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RootChoice {
    Read,
    /// Another file in the same folder takes precedence.
    Superseded(&'static str),
    /// The precedence does not read this kind of file.
    NotSelected,
}

impl RootChoice {
    /// Why the file is not read, or `None` when it is read.
    pub(crate) fn skip_reason(self) -> Option<String> {
        match self {
            Self::Read => None,
            Self::Superseded(winner) => Some(format!(
                "Not used: {winner} takes precedence in this folder."
            )),
            Self::NotSelected => Some("Not read under the current root guidance setting.".into()),
        }
    }
}

/// The files a precedence may read as a folder's root guidance, most preferred first.
fn preferred_files(
    precedence: GuidancePrecedence,
    repository_root: bool,
) -> &'static [&'static str] {
    match (precedence, repository_root) {
        (GuidancePrecedence::Default, _) => &["AGENTS.md"],
        (GuidancePrecedence::AgentsOverride, _) => &["AGENTS.override.md", "AGENTS.md"],
        (GuidancePrecedence::GeminiFirst, _) => &["GEMINI.md", "AGENTS.md"],
        (GuidancePrecedence::CopilotFirst, true) => &[COPILOT_INSTRUCTIONS, "AGENTS.md"],
        (GuidancePrecedence::CopilotFirst, false) => &["AGENTS.md"],
    }
}

/// Every root guidance file a folder can have, so each one that is present is reported.
fn known_files(repository_root: bool) -> &'static [&'static str] {
    if repository_root {
        &[
            "AGENTS.override.md",
            "AGENTS.md",
            "GEMINI.md",
            COPILOT_INSTRUCTIONS,
        ]
    } else {
        &["AGENTS.override.md", "AGENTS.md", "GEMINI.md"]
    }
}

/// How each root guidance file a folder has is treated. The first preferred file
/// that exists is read, and every other file the folder has is reported with its
/// reason. Nothing is merged silently. CLAUDE.md is read separately, as always.
pub(crate) fn root_choices(
    precedence: GuidancePrecedence,
    repository_root: bool,
    exists: impl Fn(&str) -> bool,
) -> Vec<(&'static str, RootChoice)> {
    let preferred = preferred_files(precedence, repository_root);
    let winner = preferred.iter().copied().find(|name| exists(name));
    known_files(repository_root)
        .iter()
        .copied()
        .filter(|name| exists(name))
        .map(|name| {
            let choice = match winner {
                Some(winner) if winner == name => RootChoice::Read,
                Some(winner) if preferred.contains(&name) => RootChoice::Superseded(winner),
                _ => RootChoice::NotSelected,
            };
            (name, choice)
        })
        .collect()
}

/// The folder resolved paths are shown relative to: the repository root when it
/// contains the opened folder, otherwise the opened folder itself.
pub(crate) fn display_base(project_root: &Path, repository_root: Option<&Path>) -> PathBuf {
    let project =
        std::fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
    repository_root
        .and_then(|root| std::fs::canonicalize(root).ok())
        .filter(|root| project.starts_with(root))
        .unwrap_or(project)
}

/// Where a task reads its guidance: the folder its tools work in, the directory
/// that bounds the lookup and labels every path, and which kind of directory that is.
pub struct GuidanceLocation {
    pub folder: PathBuf,
    pub repository: PathBuf,
    pub root: GuidanceRoot,
}

/// An isolated task reads the opened folder inside its own workspace, so its
/// guidance matches the files it edits. Other tasks read the opened folder.
pub async fn location(
    policy: SpawnPolicy,
    project_path: &Path,
    repository_root: Option<&Path>,
    workspace: Option<&Path>,
) -> AppResult<GuidanceLocation> {
    let Some(workspace) = workspace else {
        return Ok(GuidanceLocation {
            folder: project_path.to_path_buf(),
            repository: display_base(project_path, repository_root),
            root: GuidanceRoot::Project,
        });
    };
    let repository_root = repository_root.ok_or_else(|| {
        AppError::new(
            "not_git_repository",
            "Isolated workspaces need a Git repository.",
        )
    })?;
    let folder =
        crate::git::worktree::project_path(policy, repository_root, project_path, workspace)
            .await?;
    Ok(GuidanceLocation {
        folder,
        repository: workspace_base(workspace),
        root: GuidanceRoot::TaskWorkspace,
    })
}

/// The directory a task's guidance paths are relative to when scoped guidance is
/// activated. Isolated tasks use their workspace root, the same base their
/// guidance was labelled from when they started.
pub(crate) fn repository_base(
    session: &AgentSession,
    project_path: &Path,
    repository_root: Option<&Path>,
) -> PathBuf {
    match session.worktree_path.as_deref() {
        Some(workspace) if session.workspace_mode == WorkspaceMode::Isolated => {
            workspace_base(Path::new(workspace))
        }
        _ => display_base(project_path, repository_root),
    }
}

fn workspace_base(workspace: &Path) -> PathBuf {
    std::fs::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf())
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
        resolver_version: RESOLVER_VERSION,
        root: GuidanceRoot::Project,
        sources,
    }
}

/// Compares what a task recorded when it started with the files as they are now.
/// Sources that changed, disappeared, or stopped being readable are reported,
/// and guidance that appeared since the task started is listed as added.
/// Pending and activated sources are compared the same way. Without `current`,
/// the task's workspace no longer exists, so its sources are listed unchanged.
pub fn against_task(
    session: &AgentSession,
    current: Option<&ResolvedGuidance>,
) -> EffectiveContext {
    let recorded = &session.context_sources;
    let mut sources: Vec<EffectiveContextSource> = recorded
        .iter()
        .map(|source| {
            // An invoked skill or an imported transcript is pinned to what was added, so the folder does not change it.
            let pinned = matches!(
                source.scope,
                TaskContextScope::Skill | TaskContextScope::Transcript
            );
            let change = match current {
                Some(current) if source.status != TaskContextStatus::Skipped && !pinned => Some(
                    match current.sources.iter().find(|now| {
                        now.path == source.path && now.status != TaskContextStatus::Skipped
                    }) {
                        Some(now) if now.fingerprint == source.fingerprint => {
                            SourceChange::Unchanged
                        }
                        Some(_) => SourceChange::Changed,
                        None => SourceChange::Removed,
                    },
                ),
                _ => None,
            };
            EffectiveContextSource {
                source: source.clone(),
                change,
                preview: None,
            }
        })
        .collect();
    if let Some(current) = current {
        for now in current
            .sources
            .iter()
            .filter(|now| now.status != TaskContextStatus::Skipped)
        {
            let recorded_active = recorded.iter().any(|source| {
                source.path == now.path && source.status != TaskContextStatus::Skipped
            });
            if !recorded_active {
                sources.push(EffectiveContextSource {
                    source: now.clone(),
                    change: Some(SourceChange::Added),
                    preview: None,
                });
            }
        }
    }
    EffectiveContext {
        basis: EffectiveContextBasis::Task,
        budget_bytes: MAX_TASK_GUIDANCE_BYTES,
        used_bytes: included_bytes(recorded),
        resolver_version: session.context_resolver_version,
        root: session.context_root,
        sources,
    }
}

pub(crate) fn included_bytes(sources: &[TaskContextSource]) -> u64 {
    sources
        .iter()
        .filter(|source| source.status == TaskContextStatus::Included)
        .map(|source| source.bytes)
        .sum()
}

pub(crate) fn read_guidance(path: &Path, metadata: &std::fs::Metadata) -> Result<String, String> {
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

pub(crate) fn included_summary(secrets: &[u32], base: &str) -> String {
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
        globs: Vec::new(),
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
pub(crate) fn display_path(base: &Path, path: &Path) -> String {
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

    fn test_policy() -> SpawnPolicy {
        SpawnPolicy::uncontained_for_tests(crate::containment::ProfileKind::Inspect)
    }

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

    /// A task whose recorded guidance is `sources`, built from the shared wire fixture.
    fn recorded_session(sources: Vec<TaskContextSource>) -> AgentSession {
        let mut session: AgentSession =
            serde_json::from_str(include_str!("../../tests/fixtures/session.json")).unwrap();
        session.context_sources = sources;
        session
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
        let resolved = resolve(&app, Some(repo.path()), "", GuidancePrecedence::Default).unwrap();

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
        let resolved = resolve(&app, Some(repo.path()), "", GuidancePrecedence::Default).unwrap();
        let joined = texts(&resolved).join("\n");

        assert!(!joined.contains("sibling"));
        assert!(!joined.contains("nested"));
    }

    #[test]
    fn without_a_repository_only_the_opened_folder_is_read() {
        let repo = monorepo();
        let app = repo.path().join("packages/app");
        let resolved = resolve(&app, None, "", GuidancePrecedence::Default).unwrap();

        assert_eq!(included_paths(&resolved), ["AGENTS.md", "CLAUDE.md"]);
        assert_eq!(texts(&resolved), ["app", "app claude"]);
    }

    #[test]
    fn settings_come_first_and_are_recorded_as_a_source() {
        let repo = monorepo();
        let app = repo.path().join("packages/app");
        let resolved = resolve(
            &app,
            Some(repo.path()),
            "Prefer small diffs.",
            GuidancePrecedence::Default,
        )
        .unwrap();

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
        let resolved = resolve(&app, Some(repo.path()), "", GuidancePrecedence::Default).unwrap();
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

        let resolved = resolve(&app, Some(repo.path()), "", GuidancePrecedence::Default).unwrap();
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

        let resolved = resolve(&app, Some(repo.path()), "", GuidancePrecedence::Default).unwrap();
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

        let resolved = resolve(
            &app,
            Some(repo.path()),
            &settings,
            GuidancePrecedence::Default,
        )
        .unwrap();
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

        let resolved = resolve(&app, Some(repo.path()), "", GuidancePrecedence::Default).unwrap();
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
        let resolved = resolve(&app, Some(repo.path()), "", GuidancePrecedence::Default).unwrap();

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
        let at_start = resolve(&app, Some(repo.path()), "", GuidancePrecedence::Default).unwrap();
        let recorded = recorded_session(at_start.sources.clone());

        write(repo.path(), "AGENTS.md", b"repository, revised");
        fs::remove_file(app.join("CLAUDE.md")).unwrap();
        write(repo.path(), "packages/other/AGENTS.md", b"new sibling");
        write(repo.path(), "packages/CLAUDE.md", b"new ancestor");
        let now = resolve(&app, Some(repo.path()), "", GuidancePrecedence::Default).unwrap();

        let view = against_task(&recorded, Some(&now));
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
    fn tasks_report_the_resolver_version_they_were_recorded_with() {
        let repo = monorepo();
        let app = repo.path().join("packages/app");
        let now = resolve(&app, Some(repo.path()), "", GuidancePrecedence::Default).unwrap();
        assert_eq!(
            preview_for_new_task(&now).resolver_version,
            RESOLVER_VERSION
        );

        // The shared fixture records version zero, as tasks from before versions did.
        let mut session = recorded_session(now.sources.clone());
        assert_eq!(against_task(&session, Some(&now)).resolver_version, 0);

        session.context_resolver_version = RESOLVER_VERSION;
        assert_eq!(
            against_task(&session, Some(&now)).resolver_version,
            RESOLVER_VERSION
        );
    }

    #[test]
    fn a_task_whose_workspace_is_gone_lists_its_sources_without_changes() {
        let repo = monorepo();
        let app = repo.path().join("packages/app");
        let at_start = resolve(&app, Some(repo.path()), "", GuidancePrecedence::Default).unwrap();

        let view = against_task(&recorded_session(at_start.sources.clone()), None);

        assert_eq!(view.sources.len(), at_start.sources.len());
        assert!(view.sources.iter().all(|item| item.change.is_none()));
    }

    #[test]
    fn isolated_tasks_label_guidance_from_their_workspace_root() {
        let repo = monorepo();
        let workspace = tempfile::tempdir().unwrap();
        let project = repo.path().join("packages/app");
        let mut session = recorded_session(vec![]);

        session.workspace_mode = WorkspaceMode::Isolated;
        session.worktree_path = Some(workspace.path().to_string_lossy().into_owned());
        assert_eq!(
            repository_base(&session, &project, Some(repo.path())),
            std::fs::canonicalize(workspace.path()).unwrap()
        );

        session.workspace_mode = WorkspaceMode::Direct;
        session.worktree_path = None;
        assert_eq!(
            repository_base(&session, &project, Some(repo.path())),
            std::fs::canonicalize(repo.path()).unwrap()
        );
    }

    #[tokio::test]
    async fn an_isolated_task_reads_guidance_from_its_own_workspace() {
        let temp = tempfile::tempdir_in(std::env::current_dir().unwrap().join("target")).unwrap();
        let repo = temp.path().join("repository");
        write(&repo, "packages/app/AGENTS.md", b"committed app guidance");
        git(&repo, &["init", "--quiet"]);
        git(
            &repo,
            &["config", "user.email", "jevcode-test@example.invalid"],
        );
        git(&repo, &["config", "user.name", "JevCode Test"]);
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "--quiet", "-m", "guidance"]);
        let base = git(&repo, &["branch", "--show-current"]);
        // An uncommitted edit in the opened checkout must not reach the isolated task.
        write(&repo, "packages/app/AGENTS.md", b"uncommitted app guidance");
        let workspace = temp.path().join("task-workspace");
        crate::git::worktree::create(
            test_policy(),
            &repo,
            &workspace,
            &base,
            "jevcode/guidance-test",
        )
        .await
        .unwrap();

        let located = location(
            test_policy(),
            &repo.join("packages/app"),
            Some(&repo),
            Some(&workspace),
        )
        .await
        .unwrap();
        let resolved = resolve(
            &located.folder,
            Some(&located.repository),
            "",
            GuidancePrecedence::Default,
        )
        .unwrap();

        assert_eq!(located.root, GuidanceRoot::TaskWorkspace);
        assert_eq!(included_paths(&resolved), ["packages/app/AGENTS.md"]);
        assert_eq!(texts(&resolved), ["committed app guidance"]);
    }

    fn git(root: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .expect("Git is required for the isolated guidance test");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    #[test]
    fn an_unchanged_task_reports_no_changes() {
        let repo = monorepo();
        let app = repo.path().join("packages/app");
        let at_start = resolve(&app, Some(repo.path()), "", GuidancePrecedence::Default).unwrap();
        let now = resolve(&app, Some(repo.path()), "", GuidancePrecedence::Default).unwrap();

        let view = against_task(&recorded_session(at_start.sources.clone()), Some(&now));
        assert!(view
            .sources
            .iter()
            .filter(|item| item.source.status == TaskContextStatus::Included)
            .all(|item| item.change == Some(SourceChange::Unchanged)));
    }

    #[test]
    fn agents_override_replaces_agents_md_only_under_that_precedence() {
        let repo = monorepo();
        write(
            repo.path(),
            "packages/app/AGENTS.override.md",
            b"override app",
        );
        let app = repo.path().join("packages/app");

        let default = resolve(&app, Some(repo.path()), "", GuidancePrecedence::Default).unwrap();
        assert!(included_paths(&default).contains(&"packages/app/AGENTS.md"));
        assert!(!included_paths(&default).contains(&"packages/app/AGENTS.override.md"));

        let overridden = resolve(
            &app,
            Some(repo.path()),
            "",
            GuidancePrecedence::AgentsOverride,
        )
        .unwrap();
        assert!(included_paths(&overridden).contains(&"packages/app/AGENTS.override.md"));
        assert!(!included_paths(&overridden).contains(&"packages/app/AGENTS.md"));
        // CLAUDE.md is read under either precedence.
        assert!(included_paths(&overridden).contains(&"packages/app/CLAUDE.md"));
        assert_eq!(
            texts(&overridden),
            ["repository", "packages", "override app", "app claude"]
        );
    }

    #[test]
    fn gemini_first_reads_gemini_md_and_lists_agents_md_as_not_used() {
        let repo = monorepo();
        write(repo.path(), "GEMINI.md", b"gemini repository");
        let app = repo.path().join("packages/app");

        let resolved =
            resolve(&app, Some(repo.path()), "", GuidancePrecedence::GeminiFirst).unwrap();

        assert!(included_paths(&resolved).contains(&"GEMINI.md"));
        assert!(!included_paths(&resolved).contains(&"AGENTS.md"));
        let superseded = reported(&resolved, "AGENTS.md");
        assert_eq!(superseded.status, TaskContextStatus::Skipped);
        assert_eq!(
            superseded.summary,
            "Not used: GEMINI.md takes precedence in this folder."
        );
        // A folder without GEMINI.md still falls back to its AGENTS.md.
        assert!(included_paths(&resolved).contains(&"packages/app/AGENTS.md"));
    }

    #[test]
    fn default_lists_gemini_md_as_not_read() {
        let repo = monorepo();
        write(repo.path(), "GEMINI.md", b"gemini repository");
        let app = repo.path().join("packages/app");

        let resolved = resolve(&app, Some(repo.path()), "", GuidancePrecedence::Default).unwrap();

        assert!(!included_paths(&resolved).contains(&"GEMINI.md"));
        let unread = reported(&resolved, "GEMINI.md");
        assert_eq!(unread.status, TaskContextStatus::Skipped);
        assert!(unread.summary.starts_with("Not read"));
    }

    #[test]
    fn copilot_first_reads_repository_instructions_at_the_root_only() {
        let repo = monorepo();
        write(
            repo.path(),
            ".github/copilot-instructions.md",
            b"copilot repository",
        );
        write(
            repo.path(),
            "packages/app/.github/copilot-instructions.md",
            b"not read below the repository root",
        );
        let app = repo.path().join("packages/app");

        let resolved = resolve(
            &app,
            Some(repo.path()),
            "",
            GuidancePrecedence::CopilotFirst,
        )
        .unwrap();

        assert!(included_paths(&resolved).contains(&".github/copilot-instructions.md"));
        assert_eq!(
            reported(&resolved, "AGENTS.md").status,
            TaskContextStatus::Skipped
        );
        assert!(included_paths(&resolved).contains(&"packages/app/AGENTS.md"));
        assert!(resolved
            .sources
            .iter()
            .all(|source| source.path != "packages/app/.github/copilot-instructions.md"));
    }

    #[test]
    fn an_invoked_skill_stays_pinned_to_what_was_invoked() {
        let repo = monorepo();
        let app = repo.path().join("packages/app");
        let now = resolve(&app, Some(repo.path()), "", GuidancePrecedence::Default).unwrap();
        let skill = TaskContextSource {
            path: ".agents/skills/review/SKILL.md".into(),
            scope: TaskContextScope::Skill,
            status: TaskContextStatus::Included,
            summary: "Invoked by the user. Version 1.".into(),
            bytes: 120,
            secret_lines: Vec::new(),
            fingerprint: Some("pinned".into()),
            globs: Vec::new(),
        };

        let view = against_task(&recorded_session(vec![skill]), Some(&now));
        let recorded = view
            .sources
            .iter()
            .find(|item| item.source.path == ".agents/skills/review/SKILL.md")
            .unwrap();

        assert_eq!(recorded.change, None);
    }
}
