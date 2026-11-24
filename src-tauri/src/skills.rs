//! Agent Skills: folders that hold a `SKILL.md`. Octran Code lists each skill's name
//! and description so people can find it, and reads a skill's body only when a
//! task invokes it. A skill can name the tools it expects, but listing or
//! reading one grants no tool access; anything a skill's instructions run still
//! goes through the ordinary approval path. Invalid skills stay listed with a
//! reason, so one bad folder never stops a task.
use crate::{
    context_migration::{frontmatter_value, secret_line_numbers, split_frontmatter},
    domain::{
        AgentActivityEvent, AgentActivityKind, AgentMessage, AgentSession, MessageRole,
        TaskContextScope, TaskContextSource, TaskContextStatus,
    },
    error::{AppError, AppResult},
    guidance::{fingerprint, included_bytes, MAX_TASK_GUIDANCE_BYTES},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

pub const MAX_SKILL_BYTES: u64 = 64 * 1024;
const MAX_SKILLS: usize = 200;
const MAX_DESCRIPTION_CHARS: usize = 1024;
const MAX_NAME_CHARS: usize = 64;
/// Folders searched inside each project, in order. Each holds one folder per skill.
const PROJECT_ROOTS: [&str; 3] = [".agents/skills", ".claude/skills", ".gemini/skills"];
/// Folders Antigravity also reads in each parent of the project, up to the
/// repository root. The project's own folder is searched with `PROJECT_ROOTS`.
const ANCESTOR_ROOTS: [&str; 1] = [".agents/skills"];
/// Folders searched in the user's home folder, after the project folders. Antigravity keeps
/// its global skills in `.gemini/antigravity/skills`.
const USER_ROOTS: [&str; 4] = [
    ".agents/skills",
    ".claude/skills",
    ".gemini/skills",
    ".gemini/antigravity/skills",
];

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SkillScope {
    Project,
    User,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillSummary {
    /// Stable identifier used to invoke the skill: its scope and display source.
    pub id: String,
    pub name: String,
    pub description: String,
    pub scope: SkillScope,
    /// Where the skill lives, shown as a project-relative or `~`-relative path.
    pub source: String,
    pub version: Option<String>,
    pub fingerprint: String,
    pub bytes: u64,
    /// Tools the skill names. Informational only; they are never granted.
    pub allowed_tools: Vec<String>,
    pub valid: bool,
    pub warnings: Vec<String>,
}

/// A skill whose body has been read for a task.
pub struct LoadedSkill {
    pub summary: SkillSummary,
    pub body: String,
}

struct Found {
    summary: SkillSummary,
    location: PathBuf,
}

pub fn discover(project_root: &Path, repository_root: Option<&Path>) -> Vec<SkillSummary> {
    find(project_root, repository_root)
        .into_iter()
        .map(|found| found.summary)
        .collect()
}

fn find(project_root: &Path, repository_root: Option<&Path>) -> Vec<Found> {
    let mut found = Vec::new();
    for root in PROJECT_ROOTS {
        collect(
            &mut found,
            &project_root.join(root),
            SkillScope::Project,
            root,
        );
    }
    // Nearer folders are searched first, so the nearest skill with a name wins.
    for (folder, prefix) in ancestor_folders(project_root, repository_root) {
        for root in ANCESTOR_ROOTS {
            collect(
                &mut found,
                &folder.join(root),
                SkillScope::Project,
                &format!("{prefix}{root}"),
            );
        }
    }
    if let Some(home) = home_dir() {
        for root in USER_ROOTS {
            collect(
                &mut found,
                &home.join(root),
                SkillScope::User,
                &format!("~/{root}"),
            );
        }
    }
    // Project skills come first, so a project skill replaces a user skill with the same name.
    let mut names = HashSet::new();
    for item in found.iter_mut() {
        if item.summary.valid && !names.insert(item.summary.name.clone()) {
            item.summary.valid = false;
            item.summary.warnings.push(format!(
                "Another skill named {} is used instead.",
                item.summary.name
            ));
        }
    }
    found.truncate(MAX_SKILLS);
    found
}

/// The parent folders of the project, nearest first, up to and including the
/// repository root. Each comes with the prefix that makes its skill paths
/// project-relative, such as `../` for the project's parent. Folders outside the
/// repository are never searched.
fn ancestor_folders(project_root: &Path, repository_root: Option<&Path>) -> Vec<(PathBuf, String)> {
    let Some(repository_root) = repository_root else {
        return Vec::new();
    };
    let (Ok(project), Ok(repository)) = (
        std::fs::canonicalize(project_root),
        std::fs::canonicalize(repository_root),
    ) else {
        return Vec::new();
    };
    let mut folders = Vec::new();
    let mut current = project.parent();
    while let Some(folder) = current.filter(|folder| folder.starts_with(&repository)) {
        let levels = project
            .strip_prefix(folder)
            .map_or(0, |relative| relative.components().count());
        folders.push((folder.to_path_buf(), "../".repeat(levels)));
        current = folder.parent();
    }
    folders
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

fn collect(found: &mut Vec<Found>, root: &Path, scope: SkillScope, display_root: &str) {
    let Ok(metadata) = std::fs::symlink_metadata(root) else {
        return;
    };
    if !metadata.is_dir() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let mut folders: Vec<(String, PathBuf)> = entries
        .flatten()
        .map(|entry| {
            (
                entry.file_name().to_string_lossy().into_owned(),
                entry.path(),
            )
        })
        .collect();
    folders.sort();
    for (folder, path) in folders.into_iter().take(MAX_SKILLS) {
        let Ok(link) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        let source = format!("{display_root}/{folder}/SKILL.md");
        if link.file_type().is_symlink() {
            found.push(invalid(
                scope,
                &folder,
                source,
                path,
                "Symbolic link. Octran Code does not follow links for skills.",
            ));
            continue;
        }
        if !link.is_dir() {
            continue;
        }
        let skill_file = path.join("SKILL.md");
        let Ok(file) = std::fs::symlink_metadata(&skill_file) else {
            // A folder without SKILL.md is not a skill.
            continue;
        };
        found.push(describe(scope, &folder, source, skill_file, &file));
    }
}

fn describe(
    scope: SkillScope,
    folder: &str,
    source: String,
    skill_file: PathBuf,
    file: &std::fs::Metadata,
) -> Found {
    let text = match read_skill_file(&skill_file, file) {
        Ok(text) => text,
        Err(reason) => return invalid(scope, folder, source, skill_file, &reason),
    };
    let (pairs, _) = split_frontmatter(&text);
    let mut warnings = Vec::new();
    let name = frontmatter_value(&pairs, "name")
        .unwrap_or_default()
        .trim()
        .to_owned();
    let description = frontmatter_value(&pairs, "description")
        .unwrap_or_default()
        .trim()
        .to_owned();
    if pairs.is_empty() || name.is_empty() || description.is_empty() {
        warnings.push("Frontmatter must give a name and a description.".to_owned());
    } else {
        if !valid_name(&name) {
            warnings.push(
                "Name must use lowercase letters, digits, and single hyphens, up to 64 characters."
                    .to_owned(),
            );
        }
        if name != folder {
            warnings.push("Name does not match its folder.".to_owned());
        }
        if description.chars().count() > MAX_DESCRIPTION_CHARS {
            warnings.push("Description is longer than 1024 characters.".to_owned());
        }
    }
    let secrets = secret_line_numbers(&text);
    if !secrets.is_empty() {
        warnings.push(format!(
            "Possible secret on line {}: remove it before invoking this skill.",
            secrets[0]
        ));
    }
    let valid = warnings.is_empty()
        || warnings
            .iter()
            .all(|warning| warning.starts_with("Possible secret"));
    let allowed_tools = list(frontmatter_value(&pairs, "allowed-tools"));
    let version = frontmatter_value(&pairs, "version").filter(|value| !value.trim().is_empty());
    let displayed_name = if name.is_empty() {
        folder.to_owned()
    } else {
        name
    };
    Found {
        summary: SkillSummary {
            id: id(scope, &source),
            name: displayed_name,
            description,
            scope,
            source,
            version,
            fingerprint: fingerprint(text.as_bytes()),
            bytes: text.len() as u64,
            allowed_tools,
            valid,
            warnings,
        },
        location: skill_file,
    }
}

fn invalid(
    scope: SkillScope,
    folder: &str,
    source: String,
    location: PathBuf,
    reason: &str,
) -> Found {
    Found {
        summary: SkillSummary {
            id: id(scope, &source),
            name: folder.to_owned(),
            description: String::new(),
            scope,
            source,
            version: None,
            fingerprint: String::new(),
            bytes: 0,
            allowed_tools: Vec::new(),
            valid: false,
            warnings: vec![reason.to_owned()],
        },
        location,
    }
}

fn read_skill_file(path: &Path, metadata: &std::fs::Metadata) -> Result<String, String> {
    if metadata.file_type().is_symlink() {
        return Err("Symbolic link. Octran Code does not follow links for skills.".into());
    }
    if !metadata.is_file() {
        return Err("Not a regular file.".into());
    }
    if metadata.len() > MAX_SKILL_BYTES {
        return Err("Larger than 64 KiB.".into());
    }
    let bytes = std::fs::read(path).map_err(|_| "Could not be read.".to_owned())?;
    if bytes.len() as u64 > MAX_SKILL_BYTES {
        return Err("Larger than 64 KiB.".into());
    }
    String::from_utf8(bytes).map_err(|_| "Not UTF-8 text.".to_owned())
}

fn id(scope: SkillScope, source: &str) -> String {
    let scope = match scope {
        SkillScope::Project => "project",
        SkillScope::User => "user",
    };
    format!("{scope}:{source}")
}

/// Lowercase letters, digits, and single hyphens between them.
fn valid_name(name: &str) -> bool {
    name.chars().count() <= MAX_NAME_CHARS
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--")
        && name.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
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

/// Reads a skill's body for one task. The id must name a skill that is listed
/// as valid now; the file is read again, so the recorded fingerprint is the one
/// the task actually received.
pub fn load(
    project_root: &Path,
    repository_root: Option<&Path>,
    id: &str,
) -> AppResult<LoadedSkill> {
    let found = find(project_root, repository_root)
        .into_iter()
        .find(|found| found.summary.id == id)
        .ok_or_else(|| {
            AppError::new(
                "skill_unavailable",
                "That skill is no longer available. Refresh the list and try again.",
            )
        })?;
    if !found.summary.valid {
        return Err(AppError::new(
            "skill_unavailable",
            format!(
                "{} cannot be used: {}",
                found.summary.name,
                found.summary.warnings.join(" ")
            ),
        ));
    }
    let metadata = std::fs::symlink_metadata(&found.location)
        .map_err(|_| AppError::new("skill_unavailable", "That skill is no longer available."))?;
    let text = read_skill_file(&found.location, &metadata)
        .map_err(|reason| AppError::new("skill_unavailable", reason))?;
    let body = split_frontmatter(&text).1.trim().to_owned();
    if body.is_empty() {
        return Err(AppError::new(
            "skill_unavailable",
            format!(
                "{} has no instructions after its frontmatter.",
                found.summary.name
            ),
        ));
    }
    let mut summary = found.summary;
    summary.fingerprint = fingerprint(text.as_bytes());
    summary.bytes = text.len() as u64;
    Ok(LoadedSkill { summary, body })
}

/// Adds an invoked skill to a task: its instructions become a system message,
/// and the task records the skill's source, version, and fingerprint.
pub fn attach(
    session: &mut AgentSession,
    project_root: &Path,
    repository_root: Option<&Path>,
    id: &str,
    created_at: &str,
) -> AppResult<()> {
    let loaded = load(project_root, repository_root, id)?;
    let skill = loaded.summary;
    if included_bytes(&session.context_sources) + loaded.body.len() as u64 > MAX_TASK_GUIDANCE_BYTES
    {
        return Err(AppError::new(
            "guidance_budget",
            format!("{} would exceed the guidance budget for this task. Remove a skill or start a new task.", skill.name),
        ));
    }
    let version = skill
        .version
        .clone()
        .unwrap_or_else(|| "unversioned".to_owned());
    let tools = if skill.allowed_tools.is_empty() {
        String::new()
    } else {
        format!(
            " It names these tools, which it cannot grant: {}.",
            skill.allowed_tools.join(", ")
        )
    };
    session.messages.push(AgentMessage::text(
        MessageRole::System,
        format!(
            "Skill {} (version {}, from {}). Follow these instructions for this task.{tools}\n\n{}",
            skill.name, version, skill.source, loaded.body
        ),
    ));
    let secrets = secret_line_numbers(&loaded.body);
    session.context_sources.push(TaskContextSource {
        path: skill.source.clone(),
        scope: TaskContextScope::Skill,
        status: TaskContextStatus::Included,
        summary: format!("Invoked by the user. Version {version}."),
        bytes: loaded.body.len() as u64,
        secret_lines: secrets,
        fingerprint: Some(skill.fingerprint.clone()),
        globs: Vec::new(),
    });
    session.activity_events.push(AgentActivityEvent {
        id: format!("skill-{}", skill.fingerprint.get(..12).unwrap_or("0")),
        session_id: session.id.clone(),
        kind: AgentActivityKind::ContextActivated,
        summary: format!("Skill {} was invoked from {}.", skill.name, skill.source),
        tool_call_id: None,
        created_at: created_at.to_owned(),
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(root: &Path, relative: &str, text: &str) {
        let path = relative
            .split('/')
            .fold(root.to_path_buf(), |path, part| path.join(part));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn skill(name: &str, body: &str) -> String {
        format!("---\nname: {name}\ndescription: Use when reviewing {name} changes.\n---\n{body}\n")
    }

    fn by_name<'a>(skills: &'a [SkillSummary], name: &str) -> &'a SkillSummary {
        skills.iter().find(|skill| skill.name == name).unwrap()
    }

    #[test]
    fn lists_valid_skills_by_name_and_description_without_their_bodies() {
        let project = tempfile::tempdir().unwrap();
        write(
            project.path(),
            ".agents/skills/review/SKILL.md",
            &skill("review", "Long body text."),
        );

        let skills = discover(project.path(), None);
        let review = by_name(&skills, "review");

        assert!(review.valid);
        assert_eq!(review.description, "Use when reviewing review changes.");
        assert_eq!(review.scope, SkillScope::Project);
        assert_eq!(review.source, ".agents/skills/review/SKILL.md");
        assert!(review.id.starts_with("project:"));
    }

    #[test]
    fn discovers_gemini_project_skills_beside_the_other_roots() {
        let project = tempfile::tempdir().unwrap();
        write(
            project.path(),
            ".gemini/skills/api-audit/SKILL.md",
            &skill("api-audit", "Audit API routes."),
        );

        let skills = discover(project.path(), None);
        let audit = by_name(&skills, "api-audit");

        assert!(audit.valid);
        assert_eq!(audit.scope, SkillScope::Project);
        assert_eq!(audit.source, ".gemini/skills/api-audit/SKILL.md");
    }

    #[test]
    fn invalid_skills_are_listed_with_clear_reasons() {
        let project = tempfile::tempdir().unwrap();
        write(
            project.path(),
            ".agents/skills/Bad_Name/SKILL.md",
            &skill("Bad_Name", "x"),
        );
        write(
            project.path(),
            ".agents/skills/no-meta/SKILL.md",
            "Just text, no frontmatter.",
        );
        write(
            project.path(),
            ".agents/skills/mismatch/SKILL.md",
            &skill("other", "x"),
        );

        let skills = discover(project.path(), None);

        assert!(!by_name(&skills, "Bad_Name").valid);
        assert!(by_name(&skills, "no-meta").warnings[0].contains("name and a description"));
        assert!(by_name(&skills, "other")
            .warnings
            .iter()
            .any(|warning| warning.contains("does not match its folder")));
    }

    #[test]
    fn duplicate_names_keep_the_first_and_warn_about_the_rest() {
        let project = tempfile::tempdir().unwrap();
        write(
            project.path(),
            ".agents/skills/review/SKILL.md",
            &skill("review", "one"),
        );
        write(
            project.path(),
            ".claude/skills/review/SKILL.md",
            &skill("review", "two"),
        );

        let skills = discover(project.path(), None);
        let reviews: Vec<&SkillSummary> = skills
            .iter()
            .filter(|skill| skill.name == "review")
            .collect();

        assert_eq!(reviews.iter().filter(|skill| skill.valid).count(), 1);
        assert!(reviews.iter().any(|skill| skill
            .warnings
            .iter()
            .any(|warning| warning.contains("used instead"))));
    }

    #[test]
    fn antigravity_reads_parent_skills_up_to_the_repository_root() {
        let outer = tempfile::tempdir().unwrap();
        let repo = outer.path().join("repository");
        let app = repo.join("packages/app");
        write(
            outer.path(),
            ".agents/skills/outside/SKILL.md",
            &skill("outside", "Above the repository."),
        );
        write(
            &repo,
            ".agents/skills/shared/SKILL.md",
            &skill("shared", "Repository copy."),
        );
        write(
            &repo,
            "packages/.agents/skills/team/SKILL.md",
            &skill("team", "Package copy."),
        );
        write(
            &app,
            ".agents/skills/shared/SKILL.md",
            &skill("shared", "Project copy."),
        );

        let skills = discover(&app, Some(&repo));

        assert!(skills.iter().all(|skill| skill.name != "outside"));
        let team = by_name(&skills, "team");
        assert!(team.valid);
        assert_eq!(team.source, "../.agents/skills/team/SKILL.md");
        // The project's own copy is nearer than the repository's, so it is the one used.
        let project_copy = skills
            .iter()
            .find(|skill| skill.source == ".agents/skills/shared/SKILL.md")
            .unwrap();
        assert!(project_copy.valid);
        let repository_copy = skills
            .iter()
            .find(|skill| skill.source == "../../.agents/skills/shared/SKILL.md")
            .unwrap();
        assert!(!repository_copy.valid);
        assert!(repository_copy
            .warnings
            .iter()
            .any(|warning| warning.contains("used instead")));
    }

    #[test]
    fn parent_skills_are_not_read_without_a_repository() {
        let repo = tempfile::tempdir().unwrap();
        let app = repo.path().join("packages/app");
        fs::create_dir_all(&app).unwrap();
        write(
            repo.path(),
            ".agents/skills/shared/SKILL.md",
            &skill("shared", "Repository copy."),
        );

        // The user's own skills are read too, so only the repository's copy is checked.
        assert!(discover(&app, None)
            .iter()
            .all(|skill| skill.name != "shared"));
        assert!(discover(&app, Some(repo.path()))
            .iter()
            .any(|skill| skill.name == "shared"));
    }

    #[test]
    fn oversized_and_non_utf8_skills_are_reported_not_read() {
        let project = tempfile::tempdir().unwrap();
        let big = format!("{}\n", skill("big", &"a".repeat(MAX_SKILL_BYTES as usize)));
        write(project.path(), ".agents/skills/big/SKILL.md", &big);
        let binary = project.path().join(".claude/skills/binary");
        fs::create_dir_all(&binary).unwrap();
        fs::write(binary.join("SKILL.md"), [0xff, 0xfe, 0x00]).unwrap();

        let skills = discover(project.path(), None);

        assert!(by_name(&skills, "big").warnings[0].contains("Larger than 64 KiB"));
        assert!(by_name(&skills, "binary").warnings[0].contains("Not UTF-8"));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_skill_folder_is_reported_and_never_read() {
        let project = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        write(
            outside.path(),
            "review/SKILL.md",
            &skill("review", "outside body"),
        );
        fs::create_dir_all(project.path().join(".agents/skills")).unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("review"),
            project.path().join(".agents/skills/review"),
        )
        .unwrap();

        let skills = discover(project.path(), None);
        let review = by_name(&skills, "review");

        assert!(!review.valid);
        assert!(review.warnings[0].contains("Symbolic link"));
        assert!(load(project.path(), None, &review.id).is_err());
    }

    #[test]
    fn invocation_records_the_body_its_source_and_its_fingerprint() {
        let project = tempfile::tempdir().unwrap();
        write(
            project.path(),
            ".agents/skills/review/SKILL.md",
            &skill("review", "Check the diff first."),
        );
        let id = by_name(&discover(project.path(), None), "review")
            .id
            .clone();

        let mut session: AgentSession =
            serde_json::from_str(include_str!("../../tests/fixtures/session.json")).unwrap();
        attach(
            &mut session,
            project.path(),
            None,
            &id,
            "2026-11-15T00:00:00Z",
        )
        .unwrap();

        let recorded = session.context_sources.last().unwrap();
        assert_eq!(recorded.scope, TaskContextScope::Skill);
        assert_eq!(recorded.path, ".agents/skills/review/SKILL.md");
        assert!(recorded.summary.contains("Invoked by the user"));
        assert!(session
            .messages
            .last()
            .unwrap()
            .content
            .contains("Check the diff first."));
        assert!(!session
            .messages
            .last()
            .unwrap()
            .content
            .contains("description:"));
        assert!(session
            .activity_events
            .last()
            .unwrap()
            .summary
            .contains("review was invoked"));
    }

    #[test]
    fn allowed_tools_are_informational_and_named_in_the_instructions() {
        let project = tempfile::tempdir().unwrap();
        write(
            project.path(),
            ".agents/skills/review/SKILL.md",
            "---\nname: review\ndescription: Review code.\nallowed-tools: git_diff, read_file\n---\nRead the diff.\n",
        );
        let skills = discover(project.path(), None);
        let review = by_name(&skills, "review");
        assert_eq!(review.allowed_tools, ["git_diff", "read_file"]);

        let mut session: AgentSession =
            serde_json::from_str(include_str!("../../tests/fixtures/session.json")).unwrap();
        attach(
            &mut session,
            project.path(),
            None,
            &review.id,
            "2026-11-15T00:00:00Z",
        )
        .unwrap();
        assert!(session
            .messages
            .last()
            .unwrap()
            .content
            .contains("cannot grant: git_diff, read_file"));
    }
}
