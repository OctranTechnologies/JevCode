use crate::{
    domain::{
        PermissionCategory, PermissionDecision, PermissionMode, PermissionPolicy,
        PermissionRequest, ToolCall, ToolCategory,
    },
    error::{AppError, AppResult},
    tools,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::Path;

#[derive(Debug, Clone)]
pub struct Assessment {
    pub request: PermissionRequest,
    pub fingerprint: String,
    pub denied: Option<String>,
    pub automatic: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FingerprintInput<'a> {
    tool: &'a str,
    arguments: &'a serde_json::Value,
}

pub fn assess(
    root: &Path,
    call: &ToolCall,
    tool_category: &ToolCategory,
    policy: &PermissionPolicy,
) -> AppResult<Assessment> {
    let mut categories = vec![match tool_category {
        ToolCategory::ReadFiles | ToolCategory::Git => PermissionCategory::Read,
        ToolCategory::WriteFiles => PermissionCategory::Write,
        ToolCategory::Shell => PermissionCategory::Command,
        ToolCategory::UserInteraction => PermissionCategory::Read,
    }];
    let mut reasons = Vec::new();
    let mut hard_block = None;
    let external = tools::requires_external_access(root, call).unwrap_or(false)
        || command_has_external_path(root, call);
    if external {
        push_category(&mut categories, PermissionCategory::Dangerous);
        reasons.push("This operation reaches outside the active workspace.".to_owned());
    }

    if call.name.starts_with("mcp__") {
        push_category(&mut categories, PermissionCategory::Network);
        push_category(&mut categories, PermissionCategory::Dangerous);
        reasons.push("This call invokes an external MCP tool whose side effects cannot be verified locally; approve each call individually.".to_owned());
    }

    if call.name == "run_command" {
        let command = command_text(call);
        let lower = command.to_ascii_lowercase();
        if is_network_command(call, &lower) {
            push_category(&mut categories, PermissionCategory::Network);
            reasons.push("This command may access the network.".to_owned());
        }
        if let Some(reason) = dangerous_reason(call, &lower, external) {
            push_category(&mut categories, PermissionCategory::Dangerous);
            reasons.push(reason.clone());
            if is_privilege_escalation(&lower) {
                hard_block = Some(
                    "Agent commands cannot elevate privileges. Run this yourself in a terminal if it is required.".to_owned(),
                );
            }
        }
    } else {
        if call.name == "delete_file" {
            push_category(&mut categories, PermissionCategory::Dangerous);
            reasons.push("Deleting a file can permanently remove project data.".to_owned());
        }
        if accesses_credential_path(call) {
            push_category(&mut categories, PermissionCategory::Dangerous);
            reasons.push("This operation targets a path that may contain credentials.".to_owned());
        }
    }

    if external && policy.external_files == PermissionDecision::Deny {
        hard_block = Some("Outside-workspace access is denied by this project's policy.".into());
    }

    let primary = categories
        .iter()
        .find(|category| **category != PermissionCategory::Dangerous)
        .copied()
        .unwrap_or(PermissionCategory::Dangerous);
    let legacy_decision = policy.decision(&match primary {
        PermissionCategory::Read => {
            if matches!(tool_category, ToolCategory::Git) {
                ToolCategory::Git
            } else {
                ToolCategory::ReadFiles
            }
        }
        PermissionCategory::Write => ToolCategory::WriteFiles,
        PermissionCategory::Command
        | PermissionCategory::Network
        | PermissionCategory::Dangerous => ToolCategory::Shell,
    });
    if legacy_decision == PermissionDecision::Deny {
        hard_block.get_or_insert_with(|| {
            "This action is denied by the project permission policy.".into()
        });
    }

    let automatic = hard_block.is_none()
        && categories
            .iter()
            .all(|category| mode_allows(policy.mode, *category))
        && legacy_decision != PermissionDecision::Ask;
    let summary = if call.name == "run_command" {
        safe_command_summary(call)
    } else {
        call.name.clone()
    };
    let dangerous = categories.contains(&PermissionCategory::Dangerous);
    let request = PermissionRequest {
        categories,
        summary,
        reason: if reasons.is_empty() {
            format!(
                "{} requires approval in the current permission mode.",
                call.name
            )
        } else {
            reasons.join(" ")
        },
        can_always_allow: !dangerous && hard_block.is_none(),
    };
    Ok(Assessment {
        request,
        fingerprint: fingerprint(call)?,
        denied: hard_block,
        automatic,
    })
}

pub fn mode_allows(mode: PermissionMode, category: PermissionCategory) -> bool {
    match category {
        PermissionCategory::Read => true,
        PermissionCategory::Write => mode != PermissionMode::Ask,
        PermissionCategory::Command | PermissionCategory::Network => {
            mode == PermissionMode::FullAccess
        }
        PermissionCategory::Dangerous => false,
    }
}

pub fn fingerprint(call: &ToolCall) -> AppResult<String> {
    let input = serde_json::to_vec(&FingerprintInput {
        tool: &call.name,
        arguments: &call.arguments,
    })
    .map_err(AppError::internal)?;
    let digest = Sha256::digest(input);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn push_category(categories: &mut Vec<PermissionCategory>, category: PermissionCategory) {
    if !categories.contains(&category) {
        categories.push(category);
    }
}

fn command_text(call: &ToolCall) -> String {
    let mut parts = Vec::new();
    if let Some(program) = call.arguments["program"].as_str() {
        parts.push(program.to_owned());
    }
    if let Some(args) = call.arguments["args"].as_array() {
        parts.extend(
            args.iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_owned),
        );
    }
    parts.join(" ")
}

fn safe_command_summary(call: &ToolCall) -> String {
    let Some(program) = call.arguments["program"].as_str() else {
        return "Command execution".into();
    };
    let mut rendered = vec![program.to_owned()];
    if let Some(args) = call.arguments["args"].as_array() {
        let mut redact_next = false;
        for value in args.iter().filter_map(serde_json::Value::as_str) {
            if redact_next {
                rendered.push("[redacted]".into());
                redact_next = false;
            } else if is_secret_flag(value) && value.contains('=') {
                rendered.push(format!(
                    "{}=[redacted]",
                    value.split('=').next().unwrap_or("credential")
                ));
            } else if is_secret_flag(value) {
                rendered.push(value.to_owned());
                redact_next = true;
            } else if contains_secret_assignment(value) {
                rendered.push("[redacted credential argument]".into());
            } else {
                rendered.push(value.chars().take(96).collect());
            }
        }
    }
    rendered.join(" ").chars().take(420).collect()
}

fn is_secret_flag(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    let name = lower.split('=').next().unwrap_or(&lower);
    [
        "--token",
        "--auth-token",
        "--access-token",
        "--refresh-token",
        "--password",
        "--passphrase",
        "--secret",
        "--client-secret",
        "--api-key",
        "--apikey",
        "--key",
        "--credential",
        "--credentials",
        "--authorization",
    ]
    .contains(&name)
}

fn contains_secret_assignment(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    [
        "token=",
        "password=",
        "secret=",
        "api_key=",
        "api-key=",
        "access_key=",
        "apikey=",
        "authorization:",
        "bearer ",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn is_network_command(call: &ToolCall, lower: &str) -> bool {
    let program = call.arguments["program"].as_str().unwrap_or_default();
    let basename = Path::new(program)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(program)
        .trim_end_matches(".exe")
        .to_ascii_lowercase();
    matches!(
        basename.as_str(),
        "curl"
            | "wget"
            | "ssh"
            | "scp"
            | "sftp"
            | "ftp"
            | "nc"
            | "ncat"
            | "netcat"
            | "invoke-webrequest"
            | "invoke-restmethod"
            | "irm"
    ) || [
        "invoke-webrequest",
        "invoke-restmethod",
        "requests.get",
        "fetch(",
        "httpclient",
        "webclient",
        "curl ",
        "wget ",
        "ssh ",
        "scp ",
    ]
    .iter()
    .any(|term| lower.contains(term))
        || (basename == "git"
            && [" clone ", " fetch ", " pull ", " push ", " ls-remote "]
                .iter()
                .any(|verb| format!(" {lower} ").contains(verb)))
        || lower.contains("http://")
        || lower.contains("https://")
        || (matches!(basename.as_str(), "npm" | "pnpm" | "yarn" | "cargo")
            && [" install", " add ", " update", " upgrade", " publish"]
                .iter()
                .any(|verb| lower.contains(verb)))
}

fn dangerous_reason(call: &ToolCall, lower: &str, external: bool) -> Option<String> {
    if external {
        return Some("The command targets a path outside the active workspace.".into());
    }
    if is_privilege_escalation(lower) {
        return Some("The command attempts to run with elevated system privileges.".into());
    }
    if opaque_script_execution(call, lower) {
        return Some(
            "Inline shell or interpreter code can hide filesystem and system-level effects.".into(),
        );
    }
    let delete_flags = lower.split_whitespace().any(|flag| {
        flag == "--recursive"
            || flag == "--force"
            || (flag.len() > 1
                && !flag.starts_with("--")
                && flag.starts_with('-')
                && flag.contains('r')
                && flag.contains('f'))
    });
    let recursive_delete = (lower.starts_with("rm ") && delete_flags)
        || ["remove-item", " del ", " erase ", " rmdir ", " rd "]
            .iter()
            .any(|term| lower.contains(term))
            && (lower.contains("-recurse") || lower.contains("/s") || lower.contains("-r"));
    if recursive_delete {
        return Some("Recursive deletion can remove many files at once.".into());
    }
    if [
        "format ",
        "diskpart",
        "mkfs",
        "wipefs",
        "fdisk",
        "parted ",
        "dd if=",
        "shutdown",
        "reboot",
        "reg delete",
        "sc delete",
        "systemctl stop",
        "launchctl",
    ]
    .iter()
    .any(|term| lower.contains(term))
    {
        return Some("This command can format or overwrite a disk.".into());
    }
    if (lower.contains("git clean") && (lower.contains("-f") || lower.contains("--force")))
        || (lower.contains("git reset") && lower.contains("--hard"))
        || (lower.contains("git push") && (lower.contains("--force") || lower.contains(" -f")))
    {
        return Some("This Git operation can discard work or rewrite shared history.".into());
    }
    if [
        ".ssh",
        ".aws",
        ".azure",
        ".kube",
        ".config/gcloud",
        "credentials",
        "/etc/shadow",
        "keychain",
        ".env",
        "id_rsa",
        "id_ed25519",
        "os.environ",
        "process.env",
        "getenv(",
        "$env:",
    ]
    .iter()
    .any(|term| lower.contains(term))
        || [
            "printenv",
            " get-childitem env:",
            " env ",
            " export -p",
            " set ",
            "cmdkey",
            "vaultcmd",
            "find-generic-password",
            "secret-tool",
            "pass show",
            "credentialmanager",
            "get-secret",
        ]
        .iter()
        .any(|term| lower.contains(term))
        || (call.name == "run_command"
            && ["--token", "--password", "--secret", "--api-key", "--apikey"]
                .iter()
                .any(|flag| lower.contains(flag)))
    {
        return Some("The command refers to credential or authentication material.".into());
    }
    if has_parent_path_component(lower) {
        return Some(
            "The command includes a parent-directory path that may escape the workspace.".into(),
        );
    }
    None
}

fn accesses_credential_path(call: &ToolCall) -> bool {
    let paths: Vec<&str> = match call.name.as_str() {
        "read_file" | "file_metadata" | "list_directory" | "search_files" | "search_text"
        | "delete_file" => call.arguments["path"].as_str().into_iter().collect(),
        "read_files" => call.arguments["paths"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
            .collect(),
        "move_file" => [
            call.arguments["source"].as_str(),
            call.arguments["destination"].as_str(),
        ]
        .into_iter()
        .flatten()
        .collect(),
        _ => Vec::new(),
    };
    paths.into_iter().any(is_credential_path)
}

fn is_credential_path(path: &str) -> bool {
    let normalized = path.replace('\\', "/").to_ascii_lowercase();
    normalized.split('/').any(|component| {
        component == ".env"
            || component.starts_with(".env.")
            || matches!(
                component,
                ".aws"
                    | ".azure"
                    | ".ssh"
                    | ".kube"
                    | ".npmrc"
                    | ".pypirc"
                    | ".netrc"
                    | ".git-credentials"
                    | "credentials"
                    | "credentials.json"
                    | "secrets"
                    | "id_rsa"
                    | "id_ed25519"
                    | "config.json"
            )
            || component.ends_with(".pem")
            || component.ends_with(".p12")
            || component.ends_with(".pfx")
            || component.ends_with(".key")
    })
}

fn opaque_script_execution(call: &ToolCall, lower: &str) -> bool {
    let program = call.arguments["program"].as_str().unwrap_or_default();
    let basename = Path::new(program)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(program)
        .trim_end_matches(".exe")
        .to_ascii_lowercase();
    let shell =
        ["sh", "bash", "zsh", "fish", "cmd", "powershell", "pwsh"].contains(&basename.as_str());
    let inline_runtime = ["python", "python3", "node", "deno", "ruby", "perl"]
        .contains(&basename.as_str())
        && [
            " -c ",
            " -e ",
            " --eval ",
            " -command ",
            " -encodedcommand ",
        ]
        .iter()
        .any(|flag| format!(" {lower} ").contains(flag));
    shell || inline_runtime
}

fn is_privilege_escalation(lower: &str) -> bool {
    let program = lower.split_whitespace().next().unwrap_or_default();
    let basename = program
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(program)
        .trim_end_matches(".exe");
    ["sudo", "doas", "pkexec", "runas", "su"].contains(&basename)
        || lower.contains("start-process -verb runas")
}

fn has_parent_path_component(command: &str) -> bool {
    command
        .split(|character: char| character.is_whitespace() || character == '"' || character == '\'')
        .any(|part| {
            part.trim_matches(|character: char| ",;()[]{}".contains(character))
                .split(['/', '\\'])
                .any(|component| component == "..")
        })
}

fn command_has_external_path(root: &Path, call: &ToolCall) -> bool {
    let Some(args) = call.arguments["args"].as_array() else {
        return false;
    };
    let mut tokens = Vec::new();
    for argument in args.iter().filter_map(serde_json::Value::as_str) {
        tokens.extend(split_command_tokens(argument));
    }
    tokens.into_iter().any(|token| {
        let path_text = token
            .rsplit_once('=')
            .map_or(token.as_str(), |(_, value)| value);
        let normalized = path_text.trim_matches(|character: char| ",;()[]{}".contains(character));
        let path = Path::new(normalized);
        let windows_absolute = normalized.as_bytes().get(1) == Some(&b':')
            && normalized
                .as_bytes()
                .get(2)
                .is_some_and(|byte| *byte == b'\\' || *byte == b'/');
        if !path.is_absolute() && !windows_absolute && !has_parent_path_component(normalized) {
            return false;
        }
        let scoped = ToolCall {
            id: call.id.clone(),
            name: "run_command".into(),
            arguments: serde_json::json!({"path": normalized, "args": []}),
        };
        tools::requires_external_access(root, &scoped).unwrap_or(true)
    })
}

fn split_command_tokens(value: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    for character in value.chars() {
        match (quote, character) {
            (Some(open), next) if open == next => quote = None,
            (Some(_), next) => current.push(next),
            (None, '\'' | '"') => quote = Some(character),
            (None, next) if next.is_whitespace() => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            (None, next) => current.push(next),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn command(program: &str, args: &[&str]) -> ToolCall {
        ToolCall {
            id: "command-1".into(),
            name: "run_command".into(),
            arguments: json!({"program":program,"args":args}),
        }
    }

    #[test]
    fn ask_mode_allows_reads_but_prompts_for_command() {
        assert!(mode_allows(PermissionMode::Ask, PermissionCategory::Read));
        assert!(!mode_allows(
            PermissionMode::Ask,
            PermissionCategory::Command
        ));
        assert!(!mode_allows(
            PermissionMode::Ask,
            PermissionCategory::Dangerous
        ));
    }

    #[test]
    fn mcp_tools_always_request_external_command_and_network_permissions() {
        let root = tempfile::tempdir().unwrap();
        let call = ToolCall {
            id: "mcp-call".into(),
            name: "mcp__u__docs-abcd__1234567890".into(),
            arguments: json!({"query":"permissions"}),
        };
        let assessment = assess(
            root.path(),
            &call,
            &ToolCategory::Shell,
            &PermissionPolicy::default(),
        )
        .unwrap();
        assert!(assessment
            .request
            .categories
            .contains(&PermissionCategory::Command));
        assert!(assessment
            .request
            .categories
            .contains(&PermissionCategory::Network));
        assert!(assessment
            .request
            .categories
            .contains(&PermissionCategory::Dangerous));
        assert!(!assessment.automatic);
        assert!(!assessment.request.can_always_allow);
        assert!(assessment.request.reason.contains("external MCP tool"));

        let denied = assess(
            root.path(),
            &call,
            &ToolCategory::Shell,
            &PermissionPolicy {
                shell: PermissionDecision::Deny,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(denied.denied.is_some());
    }

    #[test]
    fn detects_network_and_recursive_delete_with_redacted_credential_display() {
        let root = tempfile::tempdir().unwrap();
        let policy = PermissionPolicy {
            shell: PermissionDecision::Allow,
            ..Default::default()
        };
        let network = assess(
            root.path(),
            &command("curl", &["https://example.test"]),
            &ToolCategory::Shell,
            &policy,
        )
        .unwrap();
        assert!(network
            .request
            .categories
            .contains(&PermissionCategory::Network));
        let delete = assess(
            root.path(),
            &command("rm", &["-rf", "./build"]),
            &ToolCategory::Shell,
            &policy,
        )
        .unwrap();
        assert!(delete
            .request
            .categories
            .contains(&PermissionCategory::Dangerous));
        assert!(!delete.request.can_always_allow);
        let secret = assess(
            root.path(),
            &command("tool", &["--token", "never-show-this"]),
            &ToolCategory::Shell,
            &policy,
        )
        .unwrap();
        assert!(!secret.request.summary.contains("never-show-this"));
    }

    #[test]
    fn privilege_escalation_is_blocked_instead_of_silently_launched() {
        let root = tempfile::tempdir().unwrap();
        let call = command("sudo", &["rm", "-rf", "./build"]);
        let assessment = assess(
            root.path(),
            &call,
            &ToolCategory::Shell,
            &PermissionPolicy::default(),
        )
        .unwrap();
        assert!(assessment.denied.is_some());
    }

    #[test]
    fn command_fingerprint_is_stable_and_does_not_include_command_text() {
        let call = command("npm", &["test"]);
        let key = fingerprint(&call).unwrap();
        assert_eq!(key, fingerprint(&call).unwrap());
        assert!(!key.contains("npm"));
    }

    #[test]
    fn command_assessment_keeps_workspace_roots_scoped() {
        let root = tempfile::tempdir().unwrap();
        let outside = root.path().parent().unwrap().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let call = ToolCall {
            id: "x".into(),
            name: "run_command".into(),
            arguments: json!({"program":"pwd","args":[],"path":outside.to_string_lossy()}),
        };
        let assessment = assess(
            root.path(),
            &call,
            &ToolCategory::Shell,
            &PermissionPolicy {
                external_files: PermissionDecision::Ask,
                shell: PermissionDecision::Allow,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(assessment
            .request
            .categories
            .contains(&PermissionCategory::Dangerous));
        let embedded = ToolCall {
            id: "y".into(),
            name: "run_command".into(),
            arguments: json!({"program":"powershell.exe","args":["-Command",format!("Get-Content '{}'", outside.display())]}),
        };
        let embedded_assessment = assess(
            root.path(),
            &embedded,
            &ToolCategory::Shell,
            &PermissionPolicy {
                external_files: PermissionDecision::Ask,
                shell: PermissionDecision::Allow,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(embedded_assessment
            .request
            .categories
            .contains(&PermissionCategory::Dangerous));
        std::fs::remove_dir_all(outside).unwrap();
    }

    #[test]
    fn deletes_and_credential_paths_always_get_fresh_scrutiny() {
        let root = tempfile::tempdir().unwrap();
        let policy = PermissionPolicy {
            mode: PermissionMode::WorkspaceWrite,
            ..Default::default()
        };
        let delete = ToolCall {
            id: "delete".into(),
            name: "delete_file".into(),
            arguments: serde_json::json!({"path":"src/old.rs"}),
        };
        let delete_assessment =
            assess(root.path(), &delete, &ToolCategory::WriteFiles, &policy).unwrap();
        assert!(delete_assessment
            .request
            .categories
            .contains(&PermissionCategory::Dangerous));
        assert!(!delete_assessment.automatic);
        assert!(!delete_assessment.request.can_always_allow);

        let credential_read = ToolCall {
            id: "credential".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({"path":"config/.npmrc"}),
        };
        let credential_assessment = assess(
            root.path(),
            &credential_read,
            &ToolCategory::ReadFiles,
            &policy,
        )
        .unwrap();
        assert!(credential_assessment
            .request
            .categories
            .contains(&PermissionCategory::Dangerous));
        assert!(!credential_assessment.automatic);
        assert!(credential_assessment
            .request
            .reason
            .contains("may contain credentials"));
    }
}
