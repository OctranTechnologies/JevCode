// Phase 5 slice 2 routes production spawns through `command_for` and
// `std_command_for`. The default backend passes the command through unchanged;
// the types, inventory and guard remain the contract for later slices.
#![allow(dead_code)]

use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    time::Duration,
};

#[cfg(windows)]
mod windows;

/// Reserved first argument that turns the app binary into the containment
/// launcher instead of starting the desktop UI.
pub const LAUNCHER_ARGUMENT: &str = "--jevcode-containment-launch";

/// The capabilities a task can hold under a containment profile.
///
/// One variant per row of the Phase 5 capability table. Parameterised rows
/// (`env.inherit(keys)`, `external.read(path)`, `external.write(path)` and
/// `mcp.invoke(server, tool)`) keep their parameters in a grant's `target`
/// rather than in the capability identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Capability {
    WorkspaceRead,
    WorkspaceWrite,
    TempWrite,
    GitMetadata,
    ProcessSpawn,
    ProcessLimit,
    Network,
    EnvInherit,
    ExternalRead,
    ExternalWrite,
    McpInvoke,
}

impl Capability {
    /// Dotted name used in diagnostics, grants and the capability panel.
    pub fn name(self) -> &'static str {
        match self {
            Self::WorkspaceRead => "workspace.read",
            Self::WorkspaceWrite => "workspace.write",
            Self::TempWrite => "temp.write",
            Self::GitMetadata => "git.metadata",
            Self::ProcessSpawn => "process.spawn",
            Self::ProcessLimit => "process.limit",
            Self::Network => "network",
            Self::EnvInherit => "env.inherit",
            Self::ExternalRead => "external.read",
            Self::ExternalWrite => "external.write",
            Self::McpInvoke => "mcp.invoke",
        }
    }
}

/// Per-capability outcome after the host probe resolves a profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityState {
    Enforced,
    Reduced,
    Blocked,
}

/// Why a task or spawn is blocked instead of running contained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockReason {
    ContainmentUnavailable,
    ContainmentProfileUnsupported,
    ContainmentProbeFailed,
    CapabilityDenied,
}

impl BlockReason {
    /// Stable snake-case code used by the UI, activity events and diagnostics.
    pub fn code(self) -> &'static str {
        match self {
            Self::ContainmentUnavailable => "containment_unavailable",
            Self::ContainmentProfileUnsupported => "containment_profile_unsupported",
            Self::ContainmentProbeFailed => "containment_probe_failed",
            Self::CapabilityDenied => "capability_denied",
        }
    }
}

/// Overall enforcement result for a task or a spawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnforcementState {
    Enforced,
    ReducedSafety { unsupported: Vec<Capability> },
    Blocked { reason: BlockReason },
}

/// Host platform after the containment probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Windows,
    Linux,
    Other,
}

/// The four immutable task profiles. Reduced safety is opt-in and is never
/// called contained: it carries only `process.limit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileKind {
    Inspect,
    Edit,
    EditWithNetwork,
    ReducedSafety,
}

impl ProfileKind {
    /// The canonical capability set for this kind, in Phase 5 table order.
    ///
    /// This is the widest set the kind can hold. Profile resolution intersects
    /// a request with this set, so a request can narrow but never widen it.
    pub fn capabilities(self) -> Vec<Capability> {
        match self {
            Self::Inspect => vec![
                Capability::WorkspaceRead,
                Capability::GitMetadata,
                Capability::ProcessSpawn,
                Capability::ProcessLimit,
            ],
            Self::Edit => vec![
                Capability::WorkspaceRead,
                Capability::WorkspaceWrite,
                Capability::TempWrite,
                Capability::GitMetadata,
                Capability::ProcessSpawn,
                Capability::ProcessLimit,
            ],
            Self::EditWithNetwork => vec![
                Capability::WorkspaceRead,
                Capability::WorkspaceWrite,
                Capability::TempWrite,
                Capability::GitMetadata,
                Capability::ProcessSpawn,
                Capability::ProcessLimit,
                Capability::Network,
            ],
            Self::ReducedSafety => vec![Capability::ProcessLimit],
        }
    }
}

/// Resolve the enforcement state for one spawn.
///
/// Slice 3a implements only the Windows process-limit backend (D1 and D6). An
/// opted-in Windows project gets `ReducedSafety`: `process.limit` is enforced,
/// and every other capability in the kind's set is reported as unsupported.
/// Without opt-in, and on Linux and other platforms where no backend exists in
/// this slice, command execution is blocked rather than silently downgraded.
///
/// D7 and D8 use the same resolver when the Git and stdio MCP spawns are
/// wired; no call site consumes the result in this slice.
pub fn resolve_spawn(
    platform: Platform,
    project_opted_in: bool,
    kind: ProfileKind,
) -> EnforcementState {
    match platform {
        Platform::Windows if project_opted_in => EnforcementState::ReducedSafety {
            unsupported: kind
                .capabilities()
                .into_iter()
                .filter(|capability| *capability != Capability::ProcessLimit)
                .collect(),
        },
        Platform::Windows | Platform::Linux | Platform::Other => EnforcementState::Blocked {
            reason: BlockReason::ContainmentUnavailable,
        },
    }
}

/// Run the containment launcher when this process was started with the
/// reserved launcher argument.
///
/// `main` calls this before Tauri starts. A normal launch returns `None`; a
/// launcher returns the exit code it must exit with and never opens a window.
pub fn run_launcher_if_requested() -> Option<i32> {
    let arguments = launcher_arguments(std::env::args_os())?;
    #[cfg(windows)]
    {
        Some(windows::run_launcher(arguments))
    }
    #[cfg(not(windows))]
    {
        let _ = arguments;
        eprintln!("jevcode containment launcher is not implemented on this platform");
        Some(1)
    }
}

/// Strip the executable name and the reserved marker, returning the launcher
/// payload when this process was started as a launcher.
fn launcher_arguments(arguments: impl IntoIterator<Item = OsString>) -> Option<Vec<OsString>> {
    let mut arguments = arguments.into_iter();
    arguments.next()?;
    if arguments
        .next()
        .is_none_or(|argument| argument != OsStr::new(LAUNCHER_ARGUMENT))
    {
        return None;
    }
    Some(arguments.collect())
}

/// Build the Tokio command for one production spawn.
///
/// Slice 2 ships a pass-through backend: the program is constructed exactly as
/// the direct constructor did, so arguments, environment, working directory and
/// stdio remain the caller's responsibility. Later slices replace the backend
/// for `kind` with enforced containment.
pub fn command_for(_kind: ProfileKind, program: &str) -> tokio::process::Command {
    tokio::process::Command::new(program)
}

/// Build the std command for a synchronous production spawn.
///
/// This is the matching std constructor for callers that cannot use the Tokio
/// command, such as the review diff helper. The pass-through contract is the
/// same as [`command_for`].
pub fn std_command_for(_kind: ProfileKind, program: &str) -> std::process::Command {
    std::process::Command::new(program)
}

/// Default resource envelope for a contained task. Process count includes the
/// root child. CPU and per-process handle caps are only enforced where the
/// platform supports them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessLimits {
    pub max_processes: u32,
    pub memory_bytes: u64,
    pub wall_clock: Duration,
    pub cpu_time: Duration,
    pub max_file_descriptors: u32,
}

impl Default for ProcessLimits {
    fn default() -> Self {
        Self {
            max_processes: 64,
            memory_bytes: 2 * 1024 * 1024 * 1024,
            wall_clock: Duration::from_secs(300),
            cpu_time: Duration::from_secs(120),
            max_file_descriptors: 1024,
        }
    }
}

/// A task's immutable containment assignment.
///
/// The roots and limits are fixed for the life of the task. Capabilities are
/// the intersection of what the caller requested and what the kind allows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainmentProfile {
    task_id: String,
    workspace_root: PathBuf,
    temp_root: PathBuf,
    kind: ProfileKind,
    capabilities: Vec<Capability>,
    limits: ProcessLimits,
    enforcement: EnforcementState,
}

impl ContainmentProfile {
    /// Resolve a profile without ever widening the capability set for `kind`.
    ///
    /// `requested` is intersected with [`ProfileKind::capabilities`]; unknown
    /// capabilities are dropped and the result keeps table order. An approval
    /// or grant therefore cannot add a capability the task kind does not
    /// already define.
    pub fn resolve(
        task_id: impl Into<String>,
        workspace_root: impl Into<PathBuf>,
        temp_root: impl Into<PathBuf>,
        kind: ProfileKind,
        requested: impl IntoIterator<Item = Capability>,
        limits: ProcessLimits,
        enforcement: EnforcementState,
    ) -> Self {
        let requested: Vec<Capability> = requested.into_iter().collect();
        let capabilities = kind
            .capabilities()
            .into_iter()
            .filter(|capability| requested.contains(capability))
            .collect();
        Self {
            task_id: task_id.into(),
            workspace_root: workspace_root.into(),
            temp_root: temp_root.into(),
            kind,
            capabilities,
            limits,
            enforcement,
        }
    }

    pub fn task_id(&self) -> &str {
        &self.task_id
    }

    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub fn temp_root(&self) -> &Path {
        &self.temp_root
    }

    pub fn kind(&self) -> ProfileKind {
        self.kind
    }

    pub fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }

    pub fn limits(&self) -> &ProcessLimits {
        &self.limits
    }

    pub fn enforcement(&self) -> &EnforcementState {
        &self.enforcement
    }

    pub fn allows(&self, capability: Capability) -> bool {
        self.capabilities.contains(&capability)
    }
}

/// How long an approval-backed grant remains usable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantScope {
    Once,
    Task,
    Project,
}

/// One approval-backed capability grant. Data only; issuing, consuming and
/// expiring grants belongs to the grants slice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityGrant {
    pub id: String,
    pub task_id: String,
    pub capability: Capability,
    /// SHA-256 over the tool/argument fingerprint, normalized argv, cwd,
    /// executable, workspace-root digest and task id.
    pub operation_fingerprint: String,
    /// Canonical absolute path, environment key or MCP server/tool target.
    pub target: String,
    pub scope: GrantScope,
    /// Remaining uses; one for an allow-once grant.
    pub uses: u32,
    /// RFC 3339 expiry, or `None` when the grant ends with the task.
    pub expires_at: Option<String>,
    pub granted_by: String,
}

/// One production spawn site that must acquire a containment profile.
///
/// Paths are relative to `src-tauri/src`. `kind` is the minimum task profile
/// that reaches the site; task-scoped seams are noted in the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpawnSite {
    pub path: &'static str,
    pub function: &'static str,
    pub kind: ProfileKind,
}

/// Inventory of the production spawn sites named in the Phase 5 coverage
/// table. User-initiated terminals/openers, tests and the HTTP MCP transport
/// are intentionally absent.
pub const SPAWN_SITES: &[SpawnSite] = &[
    // Task-scoped generic seam: Git/rg run under Inspect, run_command under
    // Edit. The caller supplies the task's profile kind; Inspect is the floor.
    SpawnSite {
        path: "tools/process.rs",
        function: "run_program_streaming",
        kind: ProfileKind::Inspect,
    },
    // Arbitrary shell: the task runs under Edit; network is a separate grant,
    // never a widening of the Edit capability set.
    SpawnSite {
        path: "tools/shell.rs",
        function: "run_command",
        kind: ProfileKind::Edit,
    },
    SpawnSite {
        path: "tools/repository.rs",
        function: "git_command",
        kind: ProfileKind::Inspect,
    },
    SpawnSite {
        path: "tools/repository.rs",
        function: "git_status",
        kind: ProfileKind::Inspect,
    },
    SpawnSite {
        path: "tools/repository.rs",
        function: "git_branch",
        kind: ProfileKind::Inspect,
    },
    SpawnSite {
        path: "tools/filesystem.rs",
        function: "search_files",
        kind: ProfileKind::Inspect,
    },
    SpawnSite {
        path: "tools/filesystem.rs",
        function: "search_text",
        kind: ProfileKind::Inspect,
    },
    // Generic Git seam: read verbs use Inspect; switch_branch is the one Edit
    // caller.
    SpawnSite {
        path: "git/mod.rs",
        function: "output",
        kind: ProfileKind::Inspect,
    },
    SpawnSite {
        path: "git/mod.rs",
        function: "status",
        kind: ProfileKind::Inspect,
    },
    SpawnSite {
        path: "git/worktree.rs",
        function: "create",
        kind: ProfileKind::Edit,
    },
    SpawnSite {
        path: "git/worktree.rs",
        function: "inspect",
        kind: ProfileKind::Inspect,
    },
    SpawnSite {
        path: "git/worktree.rs",
        function: "diff",
        kind: ProfileKind::Inspect,
    },
    SpawnSite {
        path: "git/worktree.rs",
        function: "commit",
        kind: ProfileKind::Edit,
    },
    SpawnSite {
        path: "git/worktree.rs",
        function: "apply",
        kind: ProfileKind::Edit,
    },
    SpawnSite {
        path: "git/worktree.rs",
        function: "remove",
        kind: ProfileKind::Edit,
    },
    SpawnSite {
        path: "review.rs",
        function: "unstage_task_changes",
        kind: ProfileKind::Edit,
    },
    SpawnSite {
        path: "review.rs",
        function: "unified_diff",
        kind: ProfileKind::Inspect,
    },
    SpawnSite {
        path: "review.rs",
        function: "stage_checkpoint",
        kind: ProfileKind::Edit,
    },
    // Per-server profile: project scope starts read-only; network is added only
    // when the server declares it.
    SpawnSite {
        path: "mcp.rs",
        function: "connect_server",
        kind: ProfileKind::Inspect,
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::BTreeMap,
        fs,
        path::{Path, PathBuf},
    };

    /// Exact production-line counts for the two markers the guard scans.
    /// A line counts once when it contains `Command::new(` or
    /// `process::Command`. Keep this in sync with [`SPAWN_SITES`].
    const PRODUCTION_SPAWN_ALLOWLIST: &[(&str, usize)] = &[
        ("commands/mod.rs", 20),
        ("containment/mod.rs", 4),
        ("containment/windows.rs", 2),
        ("git/mod.rs", 1),
        ("review.rs", 8),
        ("tools/process.rs", 1),
    ];

    const ALL_CAPABILITIES: [Capability; 11] = [
        Capability::WorkspaceRead,
        Capability::WorkspaceWrite,
        Capability::TempWrite,
        Capability::GitMetadata,
        Capability::ProcessSpawn,
        Capability::ProcessLimit,
        Capability::Network,
        Capability::EnvInherit,
        Capability::ExternalRead,
        Capability::ExternalWrite,
        Capability::McpInvoke,
    ];

    fn source_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
    }

    fn rust_files(directory: &Path, files: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(directory).expect("read the Rust source directory") {
            let entry = entry.expect("read a Rust source entry");
            let path = entry.path();
            if path.is_dir() {
                rust_files(&path, files);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                files.push(path);
            }
        }
    }

    fn is_test_module_header(line: &str) -> bool {
        let trimmed = line.trim();
        let trimmed = trimmed
            .strip_prefix("pub(crate) ")
            .or_else(|| trimmed.strip_prefix("pub(super) "))
            .or_else(|| trimmed.strip_prefix("pub "))
            .unwrap_or(trimmed);
        trimmed.starts_with("mod ") && trimmed.ends_with('{')
    }

    /// Absolute line ranges of top-level `#[cfg(test)] mod ... { }` blocks.
    fn test_module_ranges(lines: &[&str]) -> Vec<(usize, usize)> {
        let mut ranges = Vec::new();
        let mut index = 0;
        while index < lines.len() {
            if lines[index].trim() != "#[cfg(test)]" {
                index += 1;
                continue;
            }
            let mut header = index + 1;
            while header < lines.len() {
                let trimmed = lines[header].trim();
                if trimmed.is_empty() || trimmed.starts_with("//") || trimmed.starts_with("#[") {
                    header += 1;
                    continue;
                }
                break;
            }
            if header >= lines.len() || !is_test_module_header(lines[header]) {
                index += 1;
                continue;
            }
            let indent = lines[header].len() - lines[header].trim_start().len();
            let mut end = header + 1;
            while end < lines.len() {
                let closing_indent = lines[end].len() - lines[end].trim_start().len();
                if lines[end].trim() == "}" && closing_indent == indent {
                    break;
                }
                end += 1;
            }
            ranges.push((header, end));
            index = end.saturating_add(1);
        }
        ranges
    }

    fn production_spawn_lines(source: &str) -> usize {
        let lines: Vec<&str> = source.lines().collect();
        let ranges = test_module_ranges(&lines);
        lines
            .iter()
            .enumerate()
            .filter(|(index, line)| {
                !ranges
                    .iter()
                    .any(|(start, end)| index >= start && index <= end)
                    && (line.contains("Command::new(") || line.contains("process::Command"))
            })
            .count()
    }

    #[test]
    fn spawn_constructors_pass_the_program_through_unchanged() {
        for kind in [
            ProfileKind::Inspect,
            ProfileKind::Edit,
            ProfileKind::EditWithNetwork,
            ProfileKind::ReducedSafety,
        ] {
            let routed = command_for(kind, "git");
            let routed = routed.as_std();
            assert_eq!(routed.get_program(), std::ffi::OsStr::new("git"));
            assert!(routed.get_args().next().is_none());
            assert!(routed.get_current_dir().is_none());
            assert!(routed.get_envs().next().is_none());

            let routed = std_command_for(kind, "rg");
            assert_eq!(routed.get_program(), std::ffi::OsStr::new("rg"));
            assert!(routed.get_args().next().is_none());
            assert!(routed.get_current_dir().is_none());
            assert!(routed.get_envs().next().is_none());
        }
    }

    #[test]
    fn guard_counts_production_lines_and_skips_test_modules() {
        let source = r#"
fn production_spawn() {
    Command::new("git");
    let _: process::Command;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spawn() {
        Command::new("git");
        let _: process::Command;
    }
}
"#;
        assert_eq!(production_spawn_lines(source), 2);
    }

    #[test]
    fn production_spawn_sites_match_the_allowlist() {
        let root = source_root();
        let mut files = Vec::new();
        rust_files(&root, &mut files);

        let mut observed = BTreeMap::new();
        for file in files {
            let source = fs::read_to_string(&file).expect("read a Rust source file");
            let count = production_spawn_lines(&source);
            if count == 0 {
                continue;
            }
            let relative = file
                .strip_prefix(&root)
                .expect("source file is under src")
                .to_string_lossy()
                .replace('\\', "/");
            observed.insert(relative, count);
        }

        let expected: BTreeMap<String, usize> = PRODUCTION_SPAWN_ALLOWLIST
            .iter()
            .map(|(path, count)| ((*path).to_owned(), *count))
            .collect();

        assert_eq!(
            observed, expected,
            "a production `Command::new` appeared, moved or was removed; route it through \
             containment and update SPAWN_SITES plus the allowlist deliberately"
        );
    }

    #[test]
    fn spawn_inventory_lists_real_production_functions() {
        assert!(!SPAWN_SITES.is_empty());
        let root = source_root();
        let mut seen = Vec::new();
        for site in SPAWN_SITES {
            assert!(
                root.join(site.path).is_file(),
                "spawn inventory names a missing file: {}",
                site.path
            );
            assert!(
                !site.function.is_empty(),
                "spawn inventory names an empty function for {}",
                site.path
            );
            assert!(
                !seen.contains(&(site.path, site.function)),
                "duplicate spawn inventory row: {} {}",
                site.path,
                site.function
            );
            seen.push((site.path, site.function));
        }
    }

    #[test]
    fn profile_kinds_define_the_documented_capability_sets() {
        assert_eq!(
            ProfileKind::Inspect.capabilities(),
            vec![
                Capability::WorkspaceRead,
                Capability::GitMetadata,
                Capability::ProcessSpawn,
                Capability::ProcessLimit,
            ]
        );
        assert_eq!(
            ProfileKind::Edit.capabilities(),
            vec![
                Capability::WorkspaceRead,
                Capability::WorkspaceWrite,
                Capability::TempWrite,
                Capability::GitMetadata,
                Capability::ProcessSpawn,
                Capability::ProcessLimit,
            ]
        );
        let mut edit_with_network = ProfileKind::Edit.capabilities();
        edit_with_network.push(Capability::Network);
        assert_eq!(
            ProfileKind::EditWithNetwork.capabilities(),
            edit_with_network
        );
        assert_eq!(
            ProfileKind::ReducedSafety.capabilities(),
            vec![Capability::ProcessLimit]
        );
    }

    #[test]
    fn launcher_argument_gate_only_accepts_the_reserved_first_argument() {
        let arguments = |values: &[&str]| {
            values
                .iter()
                .copied()
                .map(std::ffi::OsString::from)
                .collect::<Vec<_>>()
        };

        assert_eq!(
            launcher_arguments(arguments(&["jevcode.exe"])),
            None,
            "a normal launch has no second argument"
        );
        assert_eq!(
            launcher_arguments(arguments(&["jevcode.exe", "--not-the-launcher"])),
            None
        );
        assert_eq!(
            launcher_arguments(arguments(&["jevcode.exe", LAUNCHER_ARGUMENT])),
            Some(Vec::new())
        );
        assert_eq!(
            launcher_arguments(arguments(&[
                "jevcode.exe",
                LAUNCHER_ARGUMENT,
                "Local\\job",
                "git",
                "status",
            ])),
            Some(arguments(&["Local\\job", "git", "status"]))
        );
    }

    #[test]
    fn spawn_resolution_covers_every_platform_opt_in_and_kind() {
        let platforms = [Platform::Windows, Platform::Linux, Platform::Other];
        let kinds = [
            ProfileKind::Inspect,
            ProfileKind::Edit,
            ProfileKind::EditWithNetwork,
            ProfileKind::ReducedSafety,
        ];
        let mut covered = 0;

        for platform in platforms {
            for project_opted_in in [false, true] {
                for kind in kinds {
                    let resolved = resolve_spawn(platform, project_opted_in, kind);
                    let expected = if platform == Platform::Windows && project_opted_in {
                        EnforcementState::ReducedSafety {
                            unsupported: kind
                                .capabilities()
                                .into_iter()
                                .filter(|capability| *capability != Capability::ProcessLimit)
                                .collect(),
                        }
                    } else {
                        EnforcementState::Blocked {
                            reason: BlockReason::ContainmentUnavailable,
                        }
                    };
                    assert_eq!(
                        resolved, expected,
                        "{platform:?} opted_in={project_opted_in} kind={kind:?}"
                    );
                    covered += 1;
                }
            }
        }

        assert_eq!(covered, 3 * 2 * 4, "every combination must be covered");
    }

    #[test]
    fn windows_reduced_safety_lists_every_unenforced_capability() {
        assert_eq!(
            resolve_spawn(Platform::Windows, false, ProfileKind::Inspect),
            EnforcementState::Blocked {
                reason: BlockReason::ContainmentUnavailable,
            }
        );
        assert_eq!(
            resolve_spawn(Platform::Windows, true, ProfileKind::Inspect),
            EnforcementState::ReducedSafety {
                unsupported: vec![
                    Capability::WorkspaceRead,
                    Capability::GitMetadata,
                    Capability::ProcessSpawn,
                ],
            }
        );
        assert_eq!(
            resolve_spawn(Platform::Windows, true, ProfileKind::Edit),
            EnforcementState::ReducedSafety {
                unsupported: vec![
                    Capability::WorkspaceRead,
                    Capability::WorkspaceWrite,
                    Capability::TempWrite,
                    Capability::GitMetadata,
                    Capability::ProcessSpawn,
                ],
            }
        );
        assert_eq!(
            resolve_spawn(Platform::Windows, true, ProfileKind::EditWithNetwork),
            EnforcementState::ReducedSafety {
                unsupported: vec![
                    Capability::WorkspaceRead,
                    Capability::WorkspaceWrite,
                    Capability::TempWrite,
                    Capability::GitMetadata,
                    Capability::ProcessSpawn,
                    Capability::Network,
                ],
            }
        );
        assert_eq!(
            resolve_spawn(Platform::Windows, true, ProfileKind::ReducedSafety),
            EnforcementState::ReducedSafety {
                unsupported: Vec::new(),
            }
        );

        for kind in [
            ProfileKind::Inspect,
            ProfileKind::Edit,
            ProfileKind::EditWithNetwork,
            ProfileKind::ReducedSafety,
        ] {
            let EnforcementState::ReducedSafety { unsupported } =
                resolve_spawn(Platform::Windows, true, kind)
            else {
                panic!("an opted-in Windows project must resolve to reduced safety");
            };
            assert!(
                !unsupported.contains(&Capability::ProcessLimit),
                "{kind:?}: process.limit is enforced by the Windows job"
            );
        }
    }

    #[test]
    fn linux_and_other_stay_blocked_even_when_opted_in() {
        for platform in [Platform::Linux, Platform::Other] {
            for project_opted_in in [false, true] {
                for kind in [
                    ProfileKind::Inspect,
                    ProfileKind::Edit,
                    ProfileKind::EditWithNetwork,
                    ProfileKind::ReducedSafety,
                ] {
                    assert_eq!(
                        resolve_spawn(platform, project_opted_in, kind),
                        EnforcementState::Blocked {
                            reason: BlockReason::ContainmentUnavailable,
                        },
                        "{platform:?} has no backend in this slice"
                    );
                }
            }
        }
    }

    #[test]
    fn resolving_a_profile_never_widens_the_kind() {
        for kind in [
            ProfileKind::Inspect,
            ProfileKind::Edit,
            ProfileKind::EditWithNetwork,
            ProfileKind::ReducedSafety,
        ] {
            let profile = ContainmentProfile::resolve(
                "task-1",
                "workspace",
                "workspace/.tmp",
                kind,
                ALL_CAPABILITIES,
                ProcessLimits::default(),
                EnforcementState::Enforced,
            );
            assert_eq!(
                profile.capabilities(),
                kind.capabilities().as_slice(),
                "{kind:?} must not gain a capability from the request"
            );
            assert_eq!(profile.kind(), kind);
            assert_eq!(profile.task_id(), "task-1");
            assert_eq!(profile.workspace_root(), Path::new("workspace"));
            assert_eq!(profile.temp_root(), Path::new("workspace/.tmp"));
            assert_eq!(profile.limits(), &ProcessLimits::default());
            assert_eq!(profile.enforcement(), &EnforcementState::Enforced);
        }

        let narrowed = ContainmentProfile::resolve(
            "task-2",
            "workspace",
            "workspace/.tmp",
            ProfileKind::Edit,
            [Capability::WorkspaceRead, Capability::Network],
            ProcessLimits::default(),
            EnforcementState::Blocked {
                reason: BlockReason::CapabilityDenied,
            },
        );
        assert_eq!(
            narrowed.capabilities(),
            &[Capability::WorkspaceRead],
            "requests may narrow a profile but never add capabilities"
        );
        assert!(narrowed.allows(Capability::WorkspaceRead));
        assert!(!narrowed.allows(Capability::Network));
        assert!(!ProfileKind::Inspect
            .capabilities()
            .contains(&Capability::WorkspaceWrite));
        assert!(!ProfileKind::Edit
            .capabilities()
            .contains(&Capability::Network));
        assert_eq!(
            ProfileKind::ReducedSafety.capabilities(),
            vec![Capability::ProcessLimit]
        );
    }

    #[test]
    fn capability_names_are_stable_and_unique() {
        let names: Vec<&str> = ALL_CAPABILITIES.iter().map(|c| c.name()).collect();
        assert_eq!(
            names,
            vec![
                "workspace.read",
                "workspace.write",
                "temp.write",
                "git.metadata",
                "process.spawn",
                "process.limit",
                "network",
                "env.inherit",
                "external.read",
                "external.write",
                "mcp.invoke",
            ]
        );
        for (index, name) in names.iter().enumerate() {
            assert!(
                !names[..index].contains(name),
                "duplicate capability name: {name}"
            );
        }
    }

    #[test]
    fn block_reason_codes_are_stable() {
        assert_eq!(
            BlockReason::ContainmentUnavailable.code(),
            "containment_unavailable"
        );
        assert_eq!(
            BlockReason::ContainmentProfileUnsupported.code(),
            "containment_profile_unsupported"
        );
        assert_eq!(
            BlockReason::ContainmentProbeFailed.code(),
            "containment_probe_failed"
        );
        assert_eq!(BlockReason::CapabilityDenied.code(), "capability_denied");
    }

    #[test]
    fn default_process_limits_match_the_design() {
        let limits = ProcessLimits::default();
        assert_eq!(limits.max_processes, 64);
        assert_eq!(limits.memory_bytes, 2 * 1024 * 1024 * 1024);
        assert_eq!(limits.wall_clock, Duration::from_secs(300));
        assert_eq!(limits.cpu_time, Duration::from_secs(120));
        assert_eq!(limits.max_file_descriptors, 1024);
    }

    #[test]
    fn capability_states_cover_the_three_outcomes() {
        assert_eq!(CapabilityState::Enforced, CapabilityState::Enforced);
        assert_ne!(CapabilityState::Enforced, CapabilityState::Reduced);
        assert_ne!(CapabilityState::Reduced, CapabilityState::Blocked);

        let grant = CapabilityGrant {
            id: "grant-1".into(),
            task_id: "task-1".into(),
            capability: Capability::Network,
            operation_fingerprint: "fingerprint".into(),
            target: "https://example.test".into(),
            scope: GrantScope::Once,
            uses: 1,
            expires_at: None,
            granted_by: "user".into(),
        };
        assert_eq!(grant.scope, GrantScope::Once);
        assert_eq!(grant.capability.name(), "network");
        assert_eq!(
            EnforcementState::ReducedSafety {
                unsupported: vec![Capability::Network]
            },
            EnforcementState::ReducedSafety {
                unsupported: vec![Capability::Network]
            }
        );
    }
}
