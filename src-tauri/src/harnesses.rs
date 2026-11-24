//! Detects the external coding-agent tools installed on this computer. Detection
//! only looks for their commands on PATH. Nothing is started, and no login, token,
//! or settings file of those tools is read. An IDE or desktop app is not a command
//! on PATH, so it is not detected and not assumed to exist.
use serde::Serialize;
use std::{
    env,
    path::{Path, PathBuf},
};

/// Command-line tools Octran Code recognizes. Each keeps its own sign-in.
const HARNESSES: [(&str, &str, &str); 5] = [
    ("codex", "Codex CLI", "codex"),
    ("claude_code", "Claude Code", "claude"),
    ("gemini_cli", "Gemini CLI", "gemini"),
    ("opencode", "OpenCode", "opencode"),
    ("antigravity", "Antigravity CLI", "agy"),
];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessStatus {
    pub id: &'static str,
    pub name: &'static str,
    /// The command Octran Code looks for on PATH.
    pub executable: &'static str,
    pub installed: bool,
}

pub fn detect() -> Vec<HarnessStatus> {
    let directories: Vec<PathBuf> = env::var_os("PATH")
        .map(|value| env::split_paths(&value).collect())
        .unwrap_or_default();
    detect_in(&directories)
}

fn detect_in(directories: &[PathBuf]) -> Vec<HarnessStatus> {
    HARNESSES
        .iter()
        .map(|&(id, name, executable)| HarnessStatus {
            id,
            name,
            executable,
            installed: directories
                .iter()
                .any(|directory| on_path(directory, executable)),
        })
        .collect()
}

/// Windows runs a command through one of its extensions, so each is checked.
fn on_path(directory: &Path, executable: &str) -> bool {
    let extensions: &[&str] = if cfg!(windows) {
        &["", ".exe", ".cmd", ".bat", ".com"]
    } else {
        &[""]
    };
    extensions
        .iter()
        .any(|extension| directory.join(format!("{executable}{extension}")).is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn only_the_tool_whose_command_is_on_the_path_is_installed() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(directory.path().join("claude"), "").unwrap();

        let installed: Vec<&str> = detect_in(&[directory.path().to_path_buf()])
            .iter()
            .filter(|harness| harness.installed)
            .map(|harness| harness.id)
            .collect();

        assert_eq!(installed, ["claude_code"]);
    }

    #[test]
    fn an_empty_path_installs_nothing() {
        assert!(detect_in(&[]).iter().all(|harness| !harness.installed));
    }
}
