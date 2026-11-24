//! Portable project profiles. A profile carries a project's instructions, its
//! preferred model, and its root guidance precedence, in a versioned file that
//! can move to another machine. It never carries credentials, permission grants,
//! trusted MCP decisions, or machine paths, and it does not import permission
//! policy at all, because those are decisions made on each machine. Importing
//! previews the change and writes nothing until it is applied.
use crate::{
    context_migration::secret_line_numbers,
    domain::{GuidancePrecedence, Project},
    error::{AppError, AppResult},
};
use serde::{
    de::{self, MapAccess, Visitor},
    Deserialize, Deserializer, Serialize,
};
use std::{collections::HashSet, fmt, path::Path};

pub const FORMAT: &str = "jevcode.project-profile";
pub const VERSION: u32 = 1;
pub const MAX_PROFILE_BYTES: u64 = 256 * 1024;
const MAX_INSTRUCTION_CHARS: usize = 50_000;
const MAX_MODEL_BYTES: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectProfile {
    pub format: String,
    pub version: u32,
    pub name: String,
    pub project_instructions: String,
    pub preferred_model: Option<String>,
    pub guidance_precedence: GuidancePrecedence,
}

/// What applying a profile would change, for the user to confirm first.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectProfilePreview {
    pub name: String,
    pub instruction_characters: usize,
    pub instructions_changed: bool,
    pub preferred_model: Option<String>,
    pub model_changed: bool,
    pub guidance_precedence: GuidancePrecedence,
    pub precedence_changed: bool,
}

/// Builds the profile for a project. Refuses when the instructions contain a
/// likely secret, so a profile never spreads a credential to another machine.
pub fn export(project: &Project) -> AppResult<ProjectProfile> {
    if !secret_line_numbers(&project.project_instructions).is_empty() {
        return Err(AppError::new(
            "profile_secret",
            "The project instructions contain a possible secret. Remove it before exporting a profile.",
        ));
    }
    Ok(ProjectProfile {
        format: FORMAT.into(),
        version: VERSION,
        name: project.name.clone(),
        project_instructions: project.project_instructions.clone(),
        preferred_model: project.preferred_model.clone(),
        guidance_precedence: project.guidance_precedence,
    })
}

/// Reads a profile file, refusing anything larger than the profile limit.
pub fn read_file(path: &Path) -> AppResult<Vec<u8>> {
    let metadata = std::fs::metadata(path)
        .map_err(|_| AppError::new("profile_unreadable", "The profile file could not be read."))?;
    if !metadata.is_file() || metadata.len() > MAX_PROFILE_BYTES {
        return Err(invalid(
            "This profile is larger than 256 KiB or is not a file.",
        ));
    }
    std::fs::read(path)
        .map_err(|_| AppError::new("profile_unreadable", "The profile file could not be read."))
}

/// Parses and validates a profile. A newer version is refused with an instruction
/// to update, and nothing from it is applied.
pub fn parse(bytes: &[u8]) -> AppResult<ProjectProfile> {
    if bytes.len() as u64 > MAX_PROFILE_BYTES {
        return Err(invalid("This profile is larger than 256 KiB."));
    }
    let text =
        std::str::from_utf8(bytes).map_err(|_| invalid("This profile is not UTF-8 text."))?;
    let profile: ProjectProfile = serde_json::from_str(text).map_err(|error| {
        invalid(&format!(
            "This is not a valid Octran Code profile: {error}."
        ))
    })?;
    if profile.format != FORMAT {
        return Err(invalid("This file is not an Octran Code project profile."));
    }
    if profile.version == 0 {
        return Err(invalid("The profile version must be 1 or later."));
    }
    if profile.version > VERSION {
        return Err(AppError::new(
            "profile_version",
            format!(
                "This profile uses version {}. Octran Code reads version {VERSION} and older; update Octran Code to import it.",
                profile.version
            ),
        ));
    }
    if profile.project_instructions.chars().count() > MAX_INSTRUCTION_CHARS {
        return Err(invalid(
            "The profile's project instructions exceed 50,000 characters.",
        ));
    }
    if profile
        .preferred_model
        .as_ref()
        .is_some_and(|model| model.len() > MAX_MODEL_BYTES)
    {
        return Err(invalid("The profile's preferred model name is too long."));
    }
    Ok(profile)
}

pub fn preview(project: &Project, profile: &ProjectProfile) -> ProjectProfilePreview {
    ProjectProfilePreview {
        name: profile.name.clone(),
        instruction_characters: profile.project_instructions.chars().count(),
        instructions_changed: profile.project_instructions != project.project_instructions,
        preferred_model: profile.preferred_model.clone(),
        model_changed: profile.preferred_model != project.preferred_model,
        guidance_precedence: profile.guidance_precedence,
        precedence_changed: profile.guidance_precedence != project.guidance_precedence,
    }
}

/// Applies the profile's own fields. Permissions and everything else on the
/// project stay exactly as they were.
pub fn apply(project: &mut Project, profile: &ProjectProfile) {
    project.project_instructions = profile.project_instructions.clone();
    project.preferred_model = profile.preferred_model.clone();
    project.guidance_precedence = profile.guidance_precedence;
}

fn invalid(message: &str) -> AppError {
    AppError::new("invalid_profile", message)
}

impl<'de> Deserialize<'de> for ProjectProfile {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(ProfileVisitor)
    }
}

/// Reads the profile field by field so that an unknown or repeated field is an
/// error. Derived deserialization would silently keep the last of two values.
struct ProfileVisitor;

impl<'de> Visitor<'de> for ProfileVisitor {
    type Value = ProjectProfile;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("an Octran Code project profile")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<ProjectProfile, A::Error> {
        let mut seen = HashSet::new();
        let mut format = None;
        let mut version = None;
        let mut name = None;
        let mut instructions = None;
        let mut model = None;
        let mut precedence = None;
        while let Some(key) = map.next_key::<String>()? {
            if !seen.insert(key.clone()) {
                return Err(de::Error::custom(format!(
                    "The field `{key}` appears more than once."
                )));
            }
            match key.as_str() {
                "format" => format = Some(map.next_value::<String>()?),
                "version" => version = Some(map.next_value::<u32>()?),
                "name" => name = Some(map.next_value::<String>()?),
                "projectInstructions" => instructions = Some(map.next_value::<String>()?),
                "preferredModel" => model = Some(map.next_value::<Option<String>>()?),
                "guidancePrecedence" => precedence = Some(map.next_value::<GuidancePrecedence>()?),
                other => {
                    return Err(de::Error::custom(format!(
                        "The field `{other}` is not part of an Octran Code profile."
                    )))
                }
            }
        }
        Ok(ProjectProfile {
            format: format.ok_or_else(|| de::Error::missing_field("format"))?,
            version: version.ok_or_else(|| de::Error::missing_field("version"))?,
            name: name.unwrap_or_default(),
            project_instructions: instructions.unwrap_or_default(),
            preferred_model: model.unwrap_or(None),
            guidance_precedence: precedence.unwrap_or_default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn project(instructions: &str) -> Project {
        serde_json::from_value(json!({
            "id": "project-1",
            "workspaceId": "workspace-1",
            "name": "Billing",
            "path": "C:/work/billing",
            "createdAt": "2026-11-15T00:00:00Z",
            "projectInstructions": instructions,
            "preferredModel": "gpt-5",
            "guidancePrecedence": "default",
        }))
        .unwrap()
    }

    fn profile_text(extra: &str) -> String {
        format!(
            r#"{{"format":"jevcode.project-profile","version":1,"name":"Billing","projectInstructions":"Keep edits small.","preferredModel":null,"guidancePrecedence":"agents_override"{extra}}}"#
        )
    }

    #[test]
    fn an_exported_profile_reads_back_unchanged() {
        let exported = export(&project("Keep edits small.")).unwrap();
        let text = serde_json::to_string(&exported).unwrap();
        let parsed = parse(text.as_bytes()).unwrap();

        assert_eq!(parsed, exported);
        assert_eq!(parsed.version, VERSION);
    }

    #[test]
    fn a_profile_carries_no_permissions_credentials_or_machine_paths() {
        let text = serde_json::to_string(&export(&project("Keep edits small.")).unwrap()).unwrap();

        assert!(!text.contains("permissions"));
        assert!(!text.contains("C:/work"));
        assert!(!text.contains("token"));
    }

    #[test]
    fn export_refuses_instructions_that_contain_a_secret() {
        let error =
            export(&project("OPENAI_API_KEY = \"sk-abcdefghijklmnopqrstuvwx\"")).unwrap_err();
        assert_eq!(error.code, "profile_secret");
    }

    #[test]
    fn unknown_and_repeated_fields_are_rejected_with_the_field_named() {
        let unknown = parse(profile_text(",\"permissions\":{}").as_bytes()).unwrap_err();
        assert!(unknown.message.contains("`permissions`"));

        let repeated = r#"{"format":"jevcode.project-profile","version":1,"projectInstructions":"a","projectInstructions":"b"}"#;
        let error = parse(repeated.as_bytes()).unwrap_err();
        assert!(error.message.contains("more than once"));
    }

    #[test]
    fn a_newer_version_is_refused_with_an_instruction_to_update() {
        let newer = profile_text("").replace("\"version\":1", "\"version\":2");
        let error = parse(newer.as_bytes()).unwrap_err();

        assert_eq!(error.code, "profile_version");
        assert!(error.message.contains("update Octran Code"));
    }

    #[test]
    fn foreign_and_oversized_files_are_refused() {
        assert!(parse(br#"{"format":"other","version":1}"#).is_err());
        let big = vec![b' '; MAX_PROFILE_BYTES as usize + 1];
        assert!(parse(&big).unwrap_err().message.contains("larger"));
        assert!(parse(&[0xff, 0xfe]).unwrap_err().message.contains("UTF-8"));
    }

    #[test]
    fn preview_describes_the_change_and_writes_nothing() {
        let current = project("Keep edits small.");
        let profile = parse(profile_text("").as_bytes()).unwrap();

        let view = preview(&current, &profile);

        assert!(!view.instructions_changed);
        // The profile names no model, so applying it would clear the project's.
        assert!(view.model_changed);
        assert!(view.precedence_changed);
        assert_eq!(current.guidance_precedence, GuidancePrecedence::Default);
    }

    #[test]
    fn apply_sets_only_the_profile_fields_and_keeps_permissions() {
        let mut target = project("Old notes.");
        let profile = parse(profile_text("").as_bytes()).unwrap();
        let before = serde_json::to_value(&target.permissions).unwrap();

        apply(&mut target, &profile);

        assert_eq!(target.project_instructions, "Keep edits small.");
        assert_eq!(
            target.guidance_precedence,
            GuidancePrecedence::AgentsOverride
        );
        assert_eq!(serde_json::to_value(&target.permissions).unwrap(), before);
    }

    #[test]
    fn profiles_carry_every_root_guidance_setting() {
        for (name, expected) in [
            ("default", GuidancePrecedence::Default),
            ("agents_override", GuidancePrecedence::AgentsOverride),
            ("gemini_first", GuidancePrecedence::GeminiFirst),
            ("copilot_first", GuidancePrecedence::CopilotFirst),
        ] {
            let text = profile_text("").replace(
                "\"guidancePrecedence\":\"agents_override\"",
                &format!("\"guidancePrecedence\":\"{name}\""),
            );
            let profile = parse(text.as_bytes()).unwrap();

            assert_eq!(profile.guidance_precedence, expected);
        }
    }
}
