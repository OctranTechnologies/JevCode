//! Commands and workflows that other agents keep in the project, converted into
//! Octran Code skills. Each one is listed with what a skill cannot keep, previewed as
//! the SKILL.md it would become, and written only after confirmation. The command
//! text is never run, and conversion never overwrites an existing skill or writes
//! through a link.
use crate::{
    context_migration::{frontmatter_value, secret_line_numbers, split_frontmatter},
    error::{AppError, AppResult},
    guidance::{fingerprint, read_guidance},
};
use serde::Serialize;
use std::{
    fs::OpenOptions,
    io::{ErrorKind, Write},
    path::Path,
};

/// Where other agents keep commands, and the format each folder uses.
const SOURCES: [(&str, CommandFormat); 3] = [
    (".claude/commands", CommandFormat::ClaudeCommand),
    (".agents/workflows", CommandFormat::AntigravityWorkflow),
    (".gemini/commands", CommandFormat::GeminiCommand),
];
const SKILLS_ROOT: &str = ".agents/skills";
const MAX_COMMANDS: usize = 200;
const MAX_NAME_CHARS: usize = 64;
const MAX_DESCRIPTION_CHARS: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandFormat {
    /// Markdown in `.claude/commands`, with optional frontmatter.
    ClaudeCommand,
    /// Markdown in `.agents/workflows`.
    AntigravityWorkflow,
    /// TOML in `.gemini/commands`. Listed so the gap is visible, not converted.
    GeminiCommand,
}

/// One command another agent keeps in this project.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomCommand {
    /// Project-relative path of the command. It names the command when it is previewed or converted.
    pub id: String,
    pub format: CommandFormat,
    /// The skill name conversion would use, when the file name allows one.
    pub name: Option<String>,
    pub description: Option<String>,
    /// Why the command cannot be converted. Absent when it can.
    pub blocked: Option<String>,
    /// Behavior a skill does not keep. Conversion needs confirmation when any are listed.
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomCommandPreview {
    pub command: CustomCommand,
    /// Project-relative path the skill would be written to, when it can be converted.
    pub target: Option<String>,
    /// The SKILL.md text conversion would write, when it can be converted.
    pub skill_text: Option<String>,
}

/// A command that can be converted, with everything conversion writes.
struct Converted {
    name: String,
    description: String,
    warnings: Vec<String>,
    skill_text: String,
    target: String,
}

pub fn list(project_root: &Path) -> Vec<CustomCommand> {
    let mut commands = Vec::new();
    for (folder, format) in SOURCES {
        let Ok(entries) = std::fs::read_dir(project_root.join(folder)) else {
            continue;
        };
        let mut files: Vec<String> = entries
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        files.sort();
        for file in files.into_iter().take(MAX_COMMANDS) {
            let id = format!("{folder}/{file}");
            let is_folder = std::fs::symlink_metadata(project_root.join(&id))
                .is_ok_and(|metadata| metadata.is_dir());
            let listed = match format {
                CommandFormat::GeminiCommand => file.ends_with(".toml"),
                _ => file.ends_with(".md") || is_folder,
            };
            if listed {
                commands.push(inspect(project_root, &id, format).0);
            }
        }
    }
    commands
}

pub fn preview(project_root: &Path, id: &str) -> AppResult<CustomCommandPreview> {
    let format = source_of(id)?;
    let (command, plan) = inspect(project_root, id, format);
    Ok(CustomCommandPreview {
        command,
        target: plan.as_ref().map(|plan| plan.target.clone()),
        skill_text: plan.map(|plan| plan.skill_text),
    })
}

/// Creates the skill for a command and returns its project-relative path. A
/// command with behavior changes is converted only when `confirmed` is set.
pub fn convert(project_root: &Path, id: &str, confirmed: bool) -> AppResult<String> {
    let format = source_of(id)?;
    let plan = convert_plan(project_root, id, format)
        .map_err(|reason| AppError::new("command_unavailable", reason))?;
    if !plan.warnings.is_empty() && !confirmed {
        return Err(AppError::new(
            "confirmation_required",
            "Confirm the behavior changes before creating this skill.",
        ));
    }
    write_skill(project_root, &plan)?;
    Ok(plan.target)
}

/// The command as it is listed, and the conversion plan when it can be converted.
fn inspect(
    project_root: &Path,
    id: &str,
    format: CommandFormat,
) -> (CustomCommand, Option<Converted>) {
    match convert_plan(project_root, id, format) {
        Ok(plan) => (
            CustomCommand {
                id: id.to_owned(),
                format,
                name: Some(plan.name.clone()),
                description: Some(plan.description.clone()),
                blocked: None,
                warnings: plan.warnings.clone(),
            },
            Some(plan),
        ),
        Err(reason) => (
            CustomCommand {
                id: id.to_owned(),
                format,
                name: name_hint(id),
                description: None,
                blocked: Some(reason),
                warnings: Vec::new(),
            },
            None,
        ),
    }
}

fn convert_plan(project_root: &Path, id: &str, format: CommandFormat) -> Result<Converted, String> {
    if format == CommandFormat::GeminiCommand {
        return Err(
            "Gemini CLI commands are TOML files. Octran Code converts Markdown commands only."
                .to_owned(),
        );
    }
    let path = project_root.join(id);
    let metadata = std::fs::symlink_metadata(&path)
        .map_err(|_| "This command could not be read.".to_owned())?;
    if metadata.file_type().is_symlink() {
        return Err("Symbolic link. Octran Code does not follow links for commands.".to_owned());
    }
    if metadata.is_dir() {
        return Err("Commands in subfolders are not converted yet.".to_owned());
    }
    let text = read_guidance(&path, &metadata)?;
    let name = name_hint(id).ok_or_else(|| {
        "The file name cannot become a skill name. Use letters, digits, and hyphens.".to_owned()
    })?;
    if let Some(line) = secret_line_numbers(&text).first() {
        return Err(format!(
            "Possible secret on line {line}. Remove it from the command before converting."
        ));
    }
    if std::fs::symlink_metadata(project_root.join(SKILLS_ROOT).join(&name)).is_ok() {
        return Err(format!(
            "A skill named {name} already exists in this project."
        ));
    }
    let (pairs, body) = split_frontmatter(&text);
    let description = description_of(&pairs, body, id);
    Ok(Converted {
        target: format!("{SKILLS_ROOT}/{name}/SKILL.md"),
        warnings: behavior_changes(&pairs, body),
        skill_text: skill_text(&name, &description, id, &fingerprint(text.as_bytes()), body),
        name,
        description,
    })
}

/// Commands keep their behavior only where a skill can express it. Everything
/// else is named here so the user can confirm it before the skill is written.
fn behavior_changes(pairs: &[(String, String)], body: &str) -> Vec<String> {
    let mut warnings = Vec::new();
    let placeholder =
        body.contains("$ARGUMENTS") || (1..=9).any(|n| body.contains(&format!("${n}")));
    if placeholder {
        warnings.push(
            "Arguments are not passed to skills. $ARGUMENTS and numbered placeholders stay in the text as written."
                .to_owned(),
        );
    }
    if body.contains("!`") {
        warnings.push(
            "Shell commands written as !`...` do not run from a skill. They stay in the text."
                .to_owned(),
        );
    }
    let file_reference = body.split_whitespace().any(|token| {
        token
            .strip_prefix('@')
            .is_some_and(|path| path.contains(['/', '.']))
    });
    if file_reference {
        warnings.push(
            "@file references are not expanded. The agent can read those files with its tools."
                .to_owned(),
        );
    }
    if frontmatter_value(pairs, "allowed-tools").is_some() {
        warnings.push(
            "allowed-tools is kept as information only. Skills never grant tools.".to_owned(),
        );
    }
    if frontmatter_value(pairs, "model").is_some() {
        warnings.push("The model setting is not kept. The task's model is used.".to_owned());
    }
    warnings
}

fn description_of(pairs: &[(String, String)], body: &str, id: &str) -> String {
    frontmatter_value(pairs, "description")
        .or_else(|| {
            let lines: Vec<&str> = body
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .collect();
            lines
                .iter()
                .find(|line| !line.starts_with('#'))
                .or(lines.first())
                .map(|line| line.trim_start_matches('#').trim().to_owned())
        })
        .map(|text| one_line(&text))
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| format!("Converted from {id}."))
}

/// Collapses a description to one quoted line that the skill parser reads back.
fn one_line(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|character| *character != '"' && *character != '\\')
        .take(MAX_DESCRIPTION_CHARS)
        .collect()
}

fn skill_text(
    name: &str,
    description: &str,
    source: &str,
    source_fingerprint: &str,
    body: &str,
) -> String {
    format!(
        "---\nname: {name}\ndescription: \"{description}\"\nmetadata:\n  converted-from: {source}\n  source-fingerprint: {source_fingerprint}\n---\n\n{}\n",
        body.trim()
    )
}

/// Skill names use lowercase letters, digits, and single hyphens, as skills do.
fn skill_name(stem: &str) -> Option<String> {
    let mut name = String::new();
    for character in stem.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            name.push(character);
        } else if !name.is_empty() && !name.ends_with('-') {
            name.push('-');
        }
    }
    name.truncate(MAX_NAME_CHARS);
    while name.ends_with('-') {
        name.pop();
    }
    (!name.is_empty()).then_some(name)
}

fn name_hint(id: &str) -> Option<String> {
    let stem = Path::new(id).file_stem()?.to_string_lossy().into_owned();
    skill_name(&stem)
}

/// The format of a command id, which must name one file directly inside a listed folder.
fn source_of(id: &str) -> AppResult<CommandFormat> {
    SOURCES
        .iter()
        .find_map(|(folder, format)| {
            let file = id.strip_prefix(folder)?.strip_prefix('/')?;
            let plain =
                !file.is_empty() && file != "." && file != ".." && !file.contains(['/', '\\']);
            plain.then_some(*format)
        })
        .ok_or_else(|| {
            AppError::new(
                "invalid_input",
                "That command is not in a folder Octran Code reads.",
            )
        })
}

fn write_skill(project_root: &Path, plan: &Converted) -> AppResult<()> {
    let folder = project_root.join(SKILLS_ROOT).join(&plan.name);
    ensure_plain_folders(project_root, &folder)?;
    std::fs::create_dir_all(&folder).map_err(|_| {
        AppError::new(
            "skill_unavailable",
            "The project's skill folder could not be created.",
        )
    })?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(folder.join("SKILL.md"))
        .map_err(|error| {
            if error.kind() == ErrorKind::AlreadyExists {
                AppError::new(
                    "skill_exists",
                    format!(
                        "A skill named {} already exists in this project.",
                        plan.name
                    ),
                )
            } else {
                AppError::new("skill_unavailable", "The skill file could not be created.")
            }
        })?;
    file.write_all(plan.skill_text.as_bytes())
        .map_err(|_| AppError::new("skill_unavailable", "The skill file could not be written."))
}

/// Refuses to write through a link: every folder on the way to the skill must be
/// a plain folder, or missing.
fn ensure_plain_folders(project_root: &Path, folder: &Path) -> AppResult<()> {
    let relative = folder.strip_prefix(project_root).unwrap_or(folder);
    let mut current = project_root.to_path_buf();
    for part in relative.components() {
        current.push(part);
        if std::fs::symlink_metadata(&current)
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            return Err(AppError::new(
                "skill_unavailable",
                "A link is in the way of the skill folder. Octran Code does not write through links.",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write(root: &Path, relative: &str, text: &str) {
        let path = relative
            .split('/')
            .fold(root.to_path_buf(), |path, part| path.join(part));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn project() -> TempDir {
        tempfile::tempdir().unwrap()
    }

    fn listed<'a>(commands: &'a [CustomCommand], id: &str) -> &'a CustomCommand {
        commands
            .iter()
            .find(|command| command.id == id)
            .unwrap_or_else(|| panic!("{id} was not listed"))
    }

    #[test]
    fn lists_claude_commands_and_antigravity_workflows_with_their_source() {
        let root = project();
        write(
            root.path(),
            ".claude/commands/review.md",
            "---\ndescription: Review the diff.\n---\nCheck the diff.\n",
        );
        write(
            root.path(),
            ".agents/workflows/ship.md",
            "# Ship\n\nRun the release checks.\n",
        );
        write(root.path(), ".claude/commands/notes.txt", "Not a command.");
        write(root.path(), ".claude/commands/frontend/test.md", "Nested.");

        let commands = list(root.path());

        assert_eq!(commands.len(), 3);
        let review = listed(&commands, ".claude/commands/review.md");
        assert_eq!(review.format, CommandFormat::ClaudeCommand);
        assert_eq!(review.name.as_deref(), Some("review"));
        assert_eq!(review.description.as_deref(), Some("Review the diff."));
        assert!(review.blocked.is_none());
        let ship = listed(&commands, ".agents/workflows/ship.md");
        assert_eq!(ship.description.as_deref(), Some("Run the release checks."));
        assert!(listed(&commands, ".claude/commands/frontend")
            .blocked
            .as_deref()
            .is_some_and(|reason| reason.contains("subfolders")));
    }

    #[test]
    fn warns_about_behavior_a_skill_cannot_keep() {
        let root = project();
        write(
            root.path(),
            ".claude/commands/fix.md",
            "---\ndescription: Fix it.\nallowed-tools: Bash\nmodel: haiku\n---\nFix $ARGUMENTS using !`git diff` and @src/main.rs\n",
        );

        let fix = list(root.path()).remove(0);

        assert!(fix.blocked.is_none());
        assert_eq!(fix.warnings.len(), 5);
        assert!(fix.warnings[0].contains("$ARGUMENTS"));
        assert!(fix.warnings[1].contains("!`"));
        assert!(fix.warnings[2].contains("@file"));
        assert!(fix.warnings[3].contains("allowed-tools"));
        assert!(fix.warnings[4].contains("model"));
    }

    #[test]
    fn converts_after_confirmation_and_the_skill_is_discovered() {
        let root = project();
        write(
            root.path(),
            ".claude/commands/review.md",
            "---\ndescription: Review the diff.\n---\nCheck the diff for risks.\n",
        );

        let target = convert(root.path(), ".claude/commands/review.md", false).unwrap();

        assert_eq!(target, ".agents/skills/review/SKILL.md");
        let text = fs::read_to_string(root.path().join(&target)).unwrap();
        assert!(text.contains("name: review"));
        assert!(text.contains("description: \"Review the diff.\""));
        assert!(text.contains("converted-from: .claude/commands/review.md"));
        assert!(text.contains("source-fingerprint: "));
        assert!(text.contains("Check the diff for risks."));
        let skills = crate::skills::discover(root.path(), None);
        let review = skills.iter().find(|skill| skill.name == "review").unwrap();
        assert!(review.valid);
        assert_eq!(review.source, ".agents/skills/review/SKILL.md");
    }

    #[test]
    fn behavior_changes_need_confirmation_before_the_skill_is_written() {
        let root = project();
        write(
            root.path(),
            ".claude/commands/fix.md",
            "Fix $ARGUMENTS now.\n",
        );

        let refused = convert(root.path(), ".claude/commands/fix.md", false).unwrap_err();

        assert_eq!(refused.code, "confirmation_required");
        assert!(!root.path().join(".agents/skills/fix/SKILL.md").exists());
        convert(root.path(), ".claude/commands/fix.md", true).unwrap();
        assert!(root.path().join(".agents/skills/fix/SKILL.md").exists());
    }

    #[test]
    fn an_existing_skill_is_never_overwritten() {
        let root = project();
        write(root.path(), ".agents/skills/review/SKILL.md", "Keep me.");
        write(
            root.path(),
            ".claude/commands/review.md",
            "Check the diff.\n",
        );

        let preview = preview(root.path(), ".claude/commands/review.md").unwrap();
        assert!(preview.skill_text.is_none());
        assert!(preview
            .command
            .blocked
            .as_deref()
            .is_some_and(|reason| reason.contains("already exists")));
        assert!(convert(root.path(), ".claude/commands/review.md", true).is_err());
        assert_eq!(
            fs::read_to_string(root.path().join(".agents/skills/review/SKILL.md")).unwrap(),
            "Keep me."
        );
    }

    #[test]
    fn likely_secrets_block_conversion_and_are_not_previewed() {
        let root = project();
        write(
            root.path(),
            ".claude/commands/deploy.md",
            "Deploy with OPENAI_API_KEY = \"sk-abcdefghijklmnopqrstuvwx\".\n",
        );

        let preview = preview(root.path(), ".claude/commands/deploy.md").unwrap();

        assert!(preview.skill_text.is_none());
        assert!(preview
            .command
            .blocked
            .as_deref()
            .is_some_and(|reason| reason.contains("Possible secret on line 1")));
    }

    #[test]
    fn ids_must_name_one_file_inside_a_listed_folder() {
        let root = project();
        write(root.path(), "secret.md", "Outside.");

        assert!(preview(root.path(), "../secret.md").is_err());
        assert!(preview(root.path(), ".claude/commands/../../secret.md").is_err());
        assert!(convert(root.path(), ".claude/commands/../../secret.md", true).is_err());
        assert!(preview(root.path(), ".claude/commands").is_err());
    }

    #[test]
    fn gemini_commands_are_listed_but_not_converted() {
        let root = project();
        write(
            root.path(),
            ".gemini/commands/deploy.toml",
            "description = \"Deploy\"\nprompt = \"Deploy {{args}}\"\n",
        );

        let deploy = list(root.path()).remove(0);

        assert_eq!(deploy.format, CommandFormat::GeminiCommand);
        assert!(deploy
            .blocked
            .as_deref()
            .is_some_and(|reason| reason.contains("TOML")));
    }

    #[test]
    fn file_names_become_lowercase_skill_names_or_are_refused() {
        assert_eq!(skill_name("Review Code!").as_deref(), Some("review-code"));
        assert_eq!(skill_name("a__b").as_deref(), Some("a-b"));
        assert_eq!(skill_name("!!!"), None);
        assert!(skill_name(&"x".repeat(100)).unwrap().len() <= MAX_NAME_CHARS);
    }

    #[cfg(unix)]
    #[test]
    fn linked_commands_are_reported_and_never_read() {
        let root = project();
        let outside = project();
        write(outside.path(), "notes.md", "Outside text.");
        fs::create_dir_all(root.path().join(".claude/commands")).unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("notes.md"),
            root.path().join(".claude/commands/linked.md"),
        )
        .unwrap();

        let linked = list(root.path()).remove(0);

        assert!(linked
            .blocked
            .as_deref()
            .is_some_and(|reason| reason.contains("Symbolic link")));
        assert!(convert(root.path(), ".claude/commands/linked.md", true).is_err());
    }
}
