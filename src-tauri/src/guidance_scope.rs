//! Guidance that applies only to some paths: folder guidance in subfolders and
//! file-scoped rules. Both are found when a task starts and recorded as pending,
//! with their text left out. A tool call that touches a matching path adds them
//! to the conversation. The file is read again at that point and refused if it
//! changed since the task started, so a running task never silently uses newer
//! guidance than the one it recorded.
use crate::{
    context_migration::{frontmatter_value, secret_line_numbers, split_frontmatter},
    domain::{
        AgentMessage, AgentSession, GuidancePrecedence, MessageRole, TaskContextScope,
        TaskContextSource, TaskContextStatus,
    },
    guidance::{
        display_path, fingerprint, included_bytes, included_summary, read_guidance, root_choices,
        RootChoice, MAX_TASK_GUIDANCE_BYTES,
    },
};
use ignore::WalkBuilder;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

const SKIPPED_DIRECTORIES: &[&str] = &[".git", "node_modules", "target"];
const MAX_SCANNED_ENTRIES: usize = 5_000;
const MAX_SCAN_DEPTH: usize = 8;

#[derive(Clone, Copy)]
enum Kind {
    Nested,
    ClaudeRule,
    CursorRule,
    CopilotRule,
}

/// Finds pending guidance under the opened folder. The root files are not
/// included here; `guidance::resolve` loads them when the task starts.
pub fn discover(
    project: &Path,
    repository: &Path,
    precedence: GuidancePrecedence,
) -> Vec<TaskContextSource> {
    let walker = WalkBuilder::new(project)
        .hidden(false)
        .max_depth(Some(MAX_SCAN_DEPTH))
        .filter_entry(|entry| {
            entry.depth() == 0
                || !SKIPPED_DIRECTORIES.contains(&entry.file_name().to_string_lossy().as_ref())
        })
        .build();
    let mut candidates = Vec::new();
    for entry in walker.flatten().take(MAX_SCANNED_ENTRIES) {
        let relative = display_path(project, entry.path());
        let Some(kind) = classify(&relative) else {
            continue;
        };
        let Ok(metadata) = std::fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if !metadata.is_dir() {
            candidates.push((relative, kind, entry.path().to_path_buf(), metadata));
        }
    }
    // Each folder reads one root file, chosen by the precedence, and reports the others.
    let present: HashSet<&str> = candidates
        .iter()
        .map(|(relative, ..)| relative.as_str())
        .collect();
    let folders: HashSet<&str> = candidates
        .iter()
        .map(|(relative, ..)| folder_of(relative))
        .collect();
    let mut choices: HashMap<String, RootChoice> = HashMap::new();
    for folder in folders {
        let exists = |name: &str| present.contains(join_relative(folder, name).as_str());
        for (name, choice) in root_choices(precedence, false, exists) {
            choices.insert(join_relative(folder, name), choice);
        }
    }
    let mut found = Vec::new();
    for (relative, kind, path, metadata) in candidates {
        match choices
            .get(&relative)
            .and_then(|choice| choice.skip_reason())
        {
            Some(reason) => found.push(skipped(&path, repository, &metadata, reason)),
            None => {
                if let Some(source) = record(&path, &relative, kind, repository, &metadata) {
                    found.push(source);
                }
            }
        }
    }
    found.sort_by(|left, right| left.path.cmp(&right.path));
    found
}

fn classify(relative: &str) -> Option<Kind> {
    let name = relative.rsplit('/').next().unwrap_or(relative);
    if relative.starts_with(".claude/rules/") && name.ends_with(".md") {
        return Some(Kind::ClaudeRule);
    }
    if relative.starts_with(".cursor/rules/") && name.ends_with(".mdc") {
        return Some(Kind::CursorRule);
    }
    if relative.starts_with(".github/instructions/") && name.ends_with(".instructions.md") {
        return Some(Kind::CopilotRule);
    }
    // Guidance folders that are hidden (such as `.github/`) are not treated as nested scopes.
    let folder = relative.rsplit_once('/').map(|(folder, _)| folder)?;
    let hidden_folder = folder.split('/').any(|part| part.starts_with('.'));
    let guidance_name = matches!(
        name,
        "AGENTS.md" | "AGENTS.override.md" | "GEMINI.md" | "CLAUDE.md"
    );
    if guidance_name && !hidden_folder {
        return Some(Kind::Nested);
    }
    None
}

/// A pending source for a file that scopes guidance to paths. Files that name no
/// paths are project-wide rules; they are not applied per path, so they are not
/// recorded here.
fn record(
    path: &Path,
    relative: &str,
    kind: Kind,
    repository: &Path,
    metadata: &std::fs::Metadata,
) -> Option<TaskContextSource> {
    let display = display_path(repository, path);
    let scope = match kind {
        Kind::Nested => TaskContextScope::Nested,
        _ => TaskContextScope::Scoped,
    };
    let text = match read_guidance(path, metadata) {
        Ok(text) => text,
        Err(reason) => {
            return Some(TaskContextSource {
                path: display,
                scope,
                status: TaskContextStatus::Skipped,
                summary: reason,
                bytes: if metadata.is_file() {
                    metadata.len()
                } else {
                    0
                },
                secret_lines: Vec::new(),
                fingerprint: None,
                globs: Vec::new(),
            })
        }
    };
    let (pairs, _) = split_frontmatter(&text);
    let globs = match kind {
        Kind::Nested => {
            let folder = relative
                .rsplit_once('/')
                .map(|(folder, _)| folder)
                .unwrap_or("");
            vec![format!("{folder}/**")]
        }
        Kind::ClaudeRule => list(frontmatter_value(&pairs, "paths")),
        Kind::CursorRule => {
            if frontmatter_value(&pairs, "alwaysApply").as_deref() == Some("true") {
                return None;
            }
            list(frontmatter_value(&pairs, "globs"))
        }
        Kind::CopilotRule => list(frontmatter_value(&pairs, "applyTo")),
    };
    if globs.is_empty() {
        return None;
    }
    if globs.iter().any(|glob| matcher(glob).is_none()) {
        return Some(TaskContextSource {
            path: display,
            scope,
            status: TaskContextStatus::Skipped,
            summary: "Invalid path pattern in its frontmatter.".into(),
            bytes: text.len() as u64,
            secret_lines: Vec::new(),
            fingerprint: None,
            globs: Vec::new(),
        });
    }
    let secrets = secret_line_numbers(&text);
    Some(TaskContextSource {
        path: display,
        scope,
        status: TaskContextStatus::Pending,
        summary: included_summary(&secrets, "Applies when a matching file is touched."),
        bytes: text.len() as u64,
        secret_lines: secrets,
        fingerprint: Some(fingerprint(text.as_bytes())),
        globs,
    })
}

fn folder_of(relative: &str) -> &str {
    relative.rsplit_once('/').map_or("", |(folder, _)| folder)
}

fn join_relative(folder: &str, name: &str) -> String {
    if folder.is_empty() {
        name.to_owned()
    } else {
        format!("{folder}/{name}")
    }
}

/// Nested guidance the task will not read, with the reason it is listed.
fn skipped(
    path: &Path,
    repository: &Path,
    metadata: &std::fs::Metadata,
    reason: String,
) -> TaskContextSource {
    TaskContextSource {
        path: display_path(repository, path),
        scope: TaskContextScope::Nested,
        status: TaskContextStatus::Skipped,
        summary: reason,
        bytes: if metadata.is_file() {
            metadata.len()
        } else {
            0
        },
        secret_lines: Vec::new(),
        fingerprint: None,
        globs: Vec::new(),
    }
}

fn list(value: Option<String>) -> Vec<String> {
    value
        .unwrap_or_default()
        .split(',')
        .map(|item| {
            item.trim()
                .trim_matches(|character| character == '"' || character == '\'')
        })
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect()
}

/// A pattern without a slash matches at any depth, as in ignore files.
fn matcher(glob: &str) -> Option<globset::GlobMatcher> {
    let pattern = if glob.contains('/') {
        glob.to_owned()
    } else {
        format!("**/{glob}")
    };
    globset::GlobBuilder::new(&pattern)
        .literal_separator(true)
        .build()
        .ok()
        .map(|built| built.compile_matcher())
}

/// True when any of the patterns matches the project-relative path.
pub fn applies_to(globs: &[String], path: &str) -> bool {
    globs
        .iter()
        .any(|glob| matcher(glob).is_some_and(|matcher| matcher.is_match(path)))
}

/// Project-relative paths a tool call names. Anything that could leave the
/// project (absolute paths, drive letters, `..`) is dropped rather than matched.
pub fn touched_paths(arguments: &Value) -> Vec<String> {
    let mut raw: Vec<&str> = ["path", "source", "destination"]
        .iter()
        .filter_map(|key| arguments.get(*key)?.as_str())
        .collect();
    if let Some(items) = arguments.get("paths").and_then(Value::as_array) {
        raw.extend(items.iter().filter_map(Value::as_str));
    }
    raw.into_iter().filter_map(normalize_relative).collect()
}

fn normalize_relative(path: &str) -> Option<String> {
    let slashed = path.replace('\\', "/");
    let trimmed = slashed.trim_start_matches("./").trim_end_matches('/');
    if trimmed.is_empty() || trimmed.starts_with('/') || trimmed.contains(':') {
        return None;
    }
    if trimmed
        .split('/')
        .any(|part| part.is_empty() || part == "..")
    {
        return None;
    }
    Some(trimmed.to_owned())
}

/// Adds pending guidance whose patterns match a touched path. Each addition is
/// re-read first and refused if it changed since the task started. Returns one
/// activity note per decision so the activity log records which rule applied.
pub fn activate(session: &mut AgentSession, repository: &Path, touched: &[String]) -> Vec<String> {
    let mut notes = Vec::new();
    if touched.is_empty() {
        return notes;
    }
    let mut used = included_bytes(&session.context_sources);
    for index in 0..session.context_sources.len() {
        let (path, globs, scope, expected, status) = {
            let source = &session.context_sources[index];
            (
                source.path.clone(),
                source.globs.clone(),
                source.scope,
                source.fingerprint.clone(),
                source.status,
            )
        };
        if status != TaskContextStatus::Pending {
            continue;
        }
        let Some(trigger) = touched
            .iter()
            .find(|touch| applies_to(&globs, touch.as_str()))
            .cloned()
        else {
            continue;
        };
        match load_for_activation(&join(repository, &path), expected.as_deref(), scope) {
            Ok(body) => {
                let bytes = body.len() as u64;
                if used + bytes > MAX_TASK_GUIDANCE_BYTES {
                    session.context_sources[index].status = TaskContextStatus::Skipped;
                    session.context_sources[index].summary =
                        "Over the 80 KiB guidance budget for one task.".into();
                    notes.push(format!(
                        "Did not apply {path} for {trigger}: over the guidance budget."
                    ));
                    continue;
                }
                used += bytes;
                let source = &mut session.context_sources[index];
                source.status = TaskContextStatus::Included;
                source.summary = format!("Applied when {trigger} was touched.");
                source.bytes = bytes;
                session.messages.push(AgentMessage::text(
                    MessageRole::System,
                    format!("Guidance from {path}, which applies to {trigger}:\n\n{body}"),
                ));
                notes.push(format!(
                    "Applied {path} because {trigger} matches its scope."
                ));
            }
            Err(reason) => {
                let source = &mut session.context_sources[index];
                source.status = TaskContextStatus::Skipped;
                source.summary = reason.clone();
                notes.push(format!("Did not apply {path}: {reason}"));
            }
        }
    }
    notes
}

fn load_for_activation(
    absolute: &Path,
    expected: Option<&str>,
    scope: TaskContextScope,
) -> Result<String, String> {
    let metadata =
        std::fs::symlink_metadata(absolute).map_err(|_| "No longer exists.".to_owned())?;
    let text = read_guidance(absolute, &metadata)?;
    if expected != Some(fingerprint(text.as_bytes()).as_str()) {
        return Err("Changed since the task started, so it was not applied.".to_owned());
    }
    let body = if scope == TaskContextScope::Scoped {
        split_frontmatter(&text).1
    } else {
        text.as_str()
    }
    .trim()
    .to_owned();
    if body.is_empty() {
        return Err("Empty after removing metadata.".to_owned());
    }
    Ok(body)
}

fn join(base: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(base.to_path_buf(), |path, part| path.join(part))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write(root: &Path, relative: &str, bytes: &[u8]) {
        let path = join(root, relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn fixture_session() -> AgentSession {
        serde_json::from_str(include_str!("../../tests/fixtures/session.json")).unwrap()
    }

    fn project_with_rules() -> TempDir {
        let repo = tempfile::tempdir().unwrap();
        write(repo.path(), "src/api/AGENTS.md", b"Return JSON errors.");
        write(
            repo.path(),
            ".claude/rules/typescript.md",
            b"---\npaths:\n  - \"src/**/*.ts\"\n---\nPrefer explicit return types.\n",
        );
        write(
            repo.path(),
            ".cursor/rules/rust.mdc",
            b"---\nglobs: \"**/*.rs\"\n---\nRun clippy before finishing.\n",
        );
        write(
            repo.path(),
            ".github/instructions/docs.instructions.md",
            b"---\napplyTo: \"docs/**\"\n---\nWrite short sentences.\n",
        );
        write(
            repo.path(),
            ".claude/rules/always.md",
            b"A rule with no paths applies project-wide and is not scoped here.\n",
        );
        repo
    }

    fn pending_session(repo: &TempDir) -> AgentSession {
        let mut session = fixture_session();
        session.context_sources = discover(repo.path(), repo.path(), GuidancePrecedence::Default);
        session
    }

    #[test]
    fn scoped_guidance_is_recorded_as_pending_without_its_text() {
        let repo = project_with_rules();
        let found = discover(repo.path(), repo.path(), GuidancePrecedence::Default);
        let paths: Vec<&str> = found.iter().map(|source| source.path.as_str()).collect();

        assert_eq!(
            paths,
            [
                ".claude/rules/typescript.md",
                ".cursor/rules/rust.mdc",
                ".github/instructions/docs.instructions.md",
                "src/api/AGENTS.md",
            ]
        );
        assert!(found
            .iter()
            .all(|source| source.status == TaskContextStatus::Pending
                && source.fingerprint.is_some()));
        let nested = found
            .iter()
            .find(|source| source.path == "src/api/AGENTS.md")
            .unwrap();
        assert_eq!(nested.scope, TaskContextScope::Nested);
        assert_eq!(nested.globs, ["src/api/**"]);
    }

    #[test]
    fn a_rule_is_added_only_when_a_matching_path_is_touched() {
        let repo = project_with_rules();
        let mut session = pending_session(&repo);

        // A docs-only change matches the Copilot rule for docs/, but not the TypeScript rule.
        let docs_only = activate(&mut session, repo.path(), &["docs/readme.md".into()]);
        assert!(docs_only.iter().all(|note| !note.contains("typescript")));
        let typescript = session
            .context_sources
            .iter()
            .find(|source| source.path == ".claude/rules/typescript.md")
            .unwrap();
        assert_eq!(typescript.status, TaskContextStatus::Pending);
        assert!(session
            .messages
            .iter()
            .all(|message| !message.content.contains("explicit return types")));

        let notes = activate(&mut session, repo.path(), &["src/app/main.ts".into()]);
        let rule = session
            .context_sources
            .iter()
            .find(|source| source.path == ".claude/rules/typescript.md")
            .unwrap();
        assert_eq!(rule.status, TaskContextStatus::Included);
        assert!(session
            .messages
            .iter()
            .any(|message| message.content.contains(
                "Guidance from .claude/rules/typescript.md, which applies to src/app/main.ts"
            ) && message.content.contains("Prefer explicit return types.")));
        assert_eq!(
            notes,
            ["Applied .claude/rules/typescript.md because src/app/main.ts matches its scope."]
        );
    }

    #[test]
    fn folder_guidance_applies_only_under_its_folder() {
        let repo = project_with_rules();
        let mut session = pending_session(&repo);

        activate(&mut session, repo.path(), &["src/api/routes.rs".into()]);
        let nested = session
            .context_sources
            .iter()
            .find(|source| source.path == "src/api/AGENTS.md")
            .unwrap();
        assert_eq!(nested.status, TaskContextStatus::Included);

        let mut other = pending_session(&repo);
        activate(&mut other, repo.path(), &["src/web/page.tsx".into()]);
        assert!(other
            .context_sources
            .iter()
            .find(|source| source.path == "src/api/AGENTS.md")
            .is_some_and(|source| source.status == TaskContextStatus::Pending));
    }

    #[test]
    fn a_changed_rule_is_refused_rather_than_silently_replaced() {
        let repo = project_with_rules();
        let mut session = pending_session(&repo);
        write(
            repo.path(),
            ".claude/rules/typescript.md",
            b"---\npaths:\n  - \"src/**/*.ts\"\n---\nUse something else.\n",
        );

        let notes = activate(&mut session, repo.path(), &["src/app/main.ts".into()]);
        let rule = session
            .context_sources
            .iter()
            .find(|source| source.path == ".claude/rules/typescript.md")
            .unwrap();

        assert_eq!(rule.status, TaskContextStatus::Skipped);
        assert!(rule.summary.contains("Changed since the task started"));
        assert!(session
            .messages
            .iter()
            .all(|message| !message.content.contains("Use something else")));
        assert_eq!(notes.len(), 1);
    }

    #[test]
    fn activation_stays_within_the_task_budget() {
        let repo = project_with_rules();
        let mut session = pending_session(&repo);
        session.context_sources.push(TaskContextSource {
            path: "settings".into(),
            scope: TaskContextScope::Settings,
            status: TaskContextStatus::Included,
            summary: String::new(),
            bytes: MAX_TASK_GUIDANCE_BYTES - 4,
            secret_lines: Vec::new(),
            fingerprint: None,
            globs: Vec::new(),
        });

        let notes = activate(&mut session, repo.path(), &["src/app/main.ts".into()]);
        let rule = session
            .context_sources
            .iter()
            .find(|source| source.path == ".claude/rules/typescript.md")
            .unwrap();

        assert_eq!(rule.status, TaskContextStatus::Skipped);
        assert!(rule.summary.contains("guidance budget"));
        assert!(notes[0].contains("over the guidance budget"));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_rule_is_reported_as_skipped() {
        let repo = project_with_rules();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("rule.md"), "Outside text.").unwrap();
        let link = repo.path().join("src/api/AGENTS.md");
        fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(outside.path().join("rule.md"), &link).unwrap();

        let found = discover(repo.path(), repo.path(), GuidancePrecedence::Default);
        let entry = found
            .iter()
            .find(|source| source.path == "src/api/AGENTS.md")
            .unwrap();

        assert_eq!(entry.status, TaskContextStatus::Skipped);
        assert!(entry.summary.contains("Symbolic link"));
        assert!(entry.globs.is_empty());
    }

    #[test]
    fn touched_paths_stay_inside_the_project() {
        let arguments = serde_json::json!({
            "path": "./src\\app/main.ts",
            "paths": ["docs/readme.md", "../secret.md", "/etc/passwd", "C:\\Windows\\win.ini", "src//x.ts"],
            "source": "old/name.rs",
        });

        assert_eq!(
            touched_paths(&arguments),
            ["src/app/main.ts", "old/name.rs", "docs/readme.md"]
        );
    }

    fn status_of(found: &[TaskContextSource], path: &str) -> Option<TaskContextStatus> {
        found
            .iter()
            .find(|source| source.path == path)
            .map(|source| source.status)
    }

    #[test]
    fn nested_override_replaces_nested_agents_md_under_its_precedence() {
        let repo = project_with_rules();
        write(
            repo.path(),
            "src/api/AGENTS.override.md",
            b"Override API rules.",
        );

        let default = discover(repo.path(), repo.path(), GuidancePrecedence::Default);
        assert_eq!(
            status_of(&default, "src/api/AGENTS.md"),
            Some(TaskContextStatus::Pending)
        );
        let unused = default
            .iter()
            .find(|source| source.path == "src/api/AGENTS.override.md")
            .unwrap();
        assert_eq!(unused.status, TaskContextStatus::Skipped);
        assert!(unused.summary.starts_with("Not read"));

        let overridden = discover(repo.path(), repo.path(), GuidancePrecedence::AgentsOverride);
        assert_eq!(
            status_of(&overridden, "src/api/AGENTS.override.md"),
            Some(TaskContextStatus::Pending)
        );
        let superseded = overridden
            .iter()
            .find(|source| source.path == "src/api/AGENTS.md")
            .unwrap();
        assert_eq!(superseded.status, TaskContextStatus::Skipped);
        assert_eq!(
            superseded.summary,
            "Not used: AGENTS.override.md takes precedence in this folder."
        );
    }

    #[test]
    fn nested_gemini_files_follow_the_root_guidance_setting() {
        let repo = project_with_rules();
        write(repo.path(), "src/api/GEMINI.md", b"Gemini API rules.");

        let default = discover(repo.path(), repo.path(), GuidancePrecedence::Default);
        assert_eq!(
            status_of(&default, "src/api/GEMINI.md"),
            Some(TaskContextStatus::Skipped)
        );

        let gemini = discover(repo.path(), repo.path(), GuidancePrecedence::GeminiFirst);
        assert_eq!(
            status_of(&gemini, "src/api/GEMINI.md"),
            Some(TaskContextStatus::Pending)
        );
        assert_eq!(
            status_of(&gemini, "src/api/AGENTS.md"),
            Some(TaskContextStatus::Skipped)
        );
    }
}
