// Phase 5 slice 3b-1 resolves every production spawn through
// [`prepare_command`]. A blocked policy never yields a command; on Windows an
// opted-in project receives the reduced-safety job launcher.
#![allow(dead_code)]

use crate::error::{AppError, AppResult};
use serde::Serialize;
use std::{
    ffi::{OsStr, OsString},
    io,
    path::{Path, PathBuf},
    process::{ExitStatus, Output, Stdio},
    time::Duration,
};
use tokio::io::AsyncReadExt;

#[cfg(windows)]
mod windows;

/// Reserved first argument that turns the app binary into the containment
/// launcher instead of starting the desktop UI.
pub const LAUNCHER_ARGUMENT: &str = "--jevcode-containment-launch";

/// Exact typed confirmation required to turn reduced safety on for a project.
pub const REDUCED_SAFETY_CONFIRMATION: &str = "reduced safety";

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

/// Wire form of one profile's enforcement state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ContainmentEnforcement {
    Enforced,
    ReducedSafety { unsupported: Vec<String> },
    Blocked { reason: String },
}

impl From<&EnforcementState> for ContainmentEnforcement {
    fn from(state: &EnforcementState) -> Self {
        match state {
            EnforcementState::Enforced => Self::Enforced,
            EnforcementState::ReducedSafety { unsupported } => Self::ReducedSafety {
                unsupported: unsupported
                    .iter()
                    .map(|capability| capability.name().into())
                    .collect(),
            },
            EnforcementState::Blocked { reason } => Self::Blocked {
                reason: reason.code().into(),
            },
        }
    }
}

/// One profile kind's resolved state for a project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainmentProfileStatus {
    pub kind: ProfileKind,
    pub enforcement: ContainmentEnforcement,
}

/// The containment status the UI reads for one project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainmentStatus {
    pub project_id: String,
    pub platform: String,
    pub project_opted_in: bool,
    pub machine_allowed: bool,
    pub profiles: Vec<ContainmentProfileStatus>,
}

/// The machine-level reduced-safety switch shared by every project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainmentMachineStatus {
    pub machine_allowed: bool,
}

/// Resolve every profile kind for one project.
pub fn status_for_project(
    project_id: &str,
    project_opted_in: bool,
    machine_allowed: bool,
) -> ContainmentStatus {
    let platform = Platform::current();
    let profiles = ProfileKind::ALL
        .into_iter()
        .map(|kind| {
            let policy = SpawnPolicy::new(kind, project_opted_in, machine_allowed);
            let enforcement = resolve_spawn(platform, policy);
            ContainmentProfileStatus {
                kind,
                enforcement: ContainmentEnforcement::from(&enforcement),
            }
        })
        .collect();
    ContainmentStatus {
        project_id: project_id.into(),
        platform: platform.name().into(),
        project_opted_in,
        machine_allowed,
        profiles,
    }
}

/// Host platform after the containment probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Windows,
    Linux,
    Other,
}

impl Platform {
    /// The platform this process is running on.
    pub fn current() -> Self {
        #[cfg(windows)]
        {
            Self::Windows
        }
        #[cfg(target_os = "linux")]
        {
            Self::Linux
        }
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            Self::Other
        }
    }

    /// Stable name used by the containment status command.
    pub fn name(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::Linux => "linux",
            Self::Other => "other",
        }
    }
}

/// The immutable task profiles. Reduced safety is opt-in and is never
/// called contained: it carries only `process.limit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileKind {
    Inspect,
    InspectWithNetwork,
    Edit,
    EditWithNetwork,
    ReducedSafety,
}

impl ProfileKind {
    /// Every profile kind in table order, for status and diagnostics.
    pub const ALL: [Self; 5] = [
        Self::Inspect,
        Self::InspectWithNetwork,
        Self::Edit,
        Self::EditWithNetwork,
        Self::ReducedSafety,
    ];

    /// Stable name used by the containment status command.
    pub fn name(self) -> &'static str {
        match self {
            Self::Inspect => "inspect",
            Self::InspectWithNetwork => "inspect_with_network",
            Self::Edit => "edit",
            Self::EditWithNetwork => "edit_with_network",
            Self::ReducedSafety => "reduced_safety",
        }
    }
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
            Self::InspectWithNetwork => vec![
                Capability::WorkspaceRead,
                Capability::GitMetadata,
                Capability::ProcessSpawn,
                Capability::ProcessLimit,
                Capability::Network,
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

/// The policy one spawn is resolved under.
///
/// `kind` is the profile the spawn site needs. `project_opted_in` is the
/// owning project's reduced-safety opt-in and `machine_allowed` is the
/// machine-level kill switch: when it is false no project can run reduced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpawnPolicy {
    pub kind: ProfileKind,
    pub project_opted_in: bool,
    pub machine_allowed: bool,
    #[cfg(test)]
    test_uncontained: bool,
}

impl SpawnPolicy {
    pub const fn new(kind: ProfileKind, project_opted_in: bool, machine_allowed: bool) -> Self {
        Self {
            kind,
            project_opted_in,
            machine_allowed,
            #[cfg(test)]
            test_uncontained: false,
        }
    }

    /// Resolve this policy on the host platform.
    pub fn enforcement(self) -> EnforcementState {
        resolve_spawn(Platform::current(), self)
    }

    /// Explicit pass-through for tests that exercise Git, review, MCP or shell
    /// mechanics without a real launcher process. Production builds cannot
    /// construct a policy that uses it.
    #[cfg(test)]
    pub const fn uncontained_for_tests(kind: ProfileKind) -> Self {
        Self {
            kind,
            project_opted_in: true,
            machine_allowed: true,
            test_uncontained: true,
        }
    }
}

/// Resolve the enforcement state for one spawn.
///
/// Slice 3b-1 implements only the Windows process-limit backend (D1 and D6). An
/// opted-in Windows project gets `ReducedSafety` when the machine switch allows
/// it: `process.limit` is enforced, and every other capability in the kind's
/// set is reported as unsupported. Without opt-in, when the machine switch is
/// off, and on Linux and every other platform, command execution is blocked
/// rather than silently downgraded.
///
/// D7 and D8 use the same resolver for Git, review and stdio MCP spawns.
pub fn resolve_spawn(platform: Platform, policy: SpawnPolicy) -> EnforcementState {
    match platform {
        Platform::Windows if policy.project_opted_in && policy.machine_allowed => {
            EnforcementState::ReducedSafety {
                unsupported: policy
                    .kind
                    .capabilities()
                    .into_iter()
                    .filter(|capability| *capability != Capability::ProcessLimit)
                    .collect(),
            }
        }
        Platform::Windows | Platform::Linux | Platform::Other => EnforcementState::Blocked {
            reason: BlockReason::ContainmentUnavailable,
        },
    }
}

/// Verify the typed confirmation that turns reduced safety on.
pub fn confirm_reduced_safety(confirmation: &str) -> AppResult<()> {
    if confirmation == REDUCED_SAFETY_CONFIRMATION {
        Ok(())
    } else {
        Err(AppError::new(
            "containment_confirmation",
            "Reduced safety can only be turned on by typing \"reduced safety\" exactly.",
        ))
    }
}

/// The single enforcement seam. A policy either produces a prepared command
/// that the platform backend can spawn, or an error. It never returns an
/// uncontained command for a blocked policy.
pub fn prepare_command(
    policy: SpawnPolicy,
    program: impl Into<OsString>,
) -> AppResult<PreparedCommand> {
    #[cfg(test)]
    if policy.test_uncontained {
        return Ok(PreparedCommand::uncontained(program.into()));
    }

    let enforcement = policy.enforcement();
    #[cfg(windows)]
    {
        match enforcement {
            EnforcementState::ReducedSafety { .. } => {
                Ok(PreparedCommand::contained(program.into()))
            }
            EnforcementState::Enforced | EnforcementState::Blocked { .. } => {
                Err(blocked_error(BlockReason::ContainmentUnavailable))
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (enforcement, program);
        Err(blocked_error(BlockReason::ContainmentUnavailable))
    }
}

fn blocked_error(reason: BlockReason) -> AppError {
    AppError::new(
        BlockReason::ContainmentUnavailable.code(),
        format!(
            "Command execution is unavailable: Octran Code cannot enforce this task's containment profile on this system ({}). File edits still work.",
            reason.code()
        ),
    )
}

enum PreparedCommandInner {
    #[cfg(windows)]
    Contained(windows::ContainedCommand),
    #[cfg(test)]
    Uncontained(test_support::UncontainedCommand),
    #[cfg(not(any(windows, test)))]
    Unavailable,
}

/// A command that has passed the containment seam.
///
/// Arguments, environment, working directory and stdio are carried until
/// [`PreparedCommand::spawn`]. A blocked policy never constructs one.
pub struct PreparedCommand {
    inner: PreparedCommandInner,
    stdin: Stdio,
    stdout: Stdio,
    stderr: Stdio,
}

impl PreparedCommand {
    #[cfg(windows)]
    fn contained(program: OsString) -> Self {
        Self {
            inner: PreparedCommandInner::Contained(windows::ContainedCommand::new(program)),
            stdin: Stdio::null(),
            stdout: Stdio::piped(),
            stderr: Stdio::piped(),
        }
    }

    #[cfg(test)]
    fn uncontained(program: OsString) -> Self {
        Self {
            inner: PreparedCommandInner::Uncontained(test_support::UncontainedCommand::new(
                program,
            )),
            stdin: Stdio::null(),
            stdout: Stdio::piped(),
            stderr: Stdio::piped(),
        }
    }

    pub fn arg(mut self, argument: impl Into<OsString>) -> Self {
        self.inner = match self.inner {
            #[cfg(windows)]
            PreparedCommandInner::Contained(command) => {
                PreparedCommandInner::Contained(command.arg(argument))
            }
            #[cfg(test)]
            PreparedCommandInner::Uncontained(command) => {
                PreparedCommandInner::Uncontained(command.arg(argument))
            }
            #[cfg(not(any(windows, test)))]
            PreparedCommandInner::Unavailable => PreparedCommandInner::Unavailable,
        };
        self
    }

    pub fn args<I, S>(mut self, arguments: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        for argument in arguments {
            self = self.arg(argument);
        }
        self
    }

    pub fn current_dir(mut self, directory: impl Into<PathBuf>) -> Self {
        self.inner = match self.inner {
            #[cfg(windows)]
            PreparedCommandInner::Contained(command) => {
                PreparedCommandInner::Contained(command.current_dir(directory))
            }
            #[cfg(test)]
            PreparedCommandInner::Uncontained(command) => {
                PreparedCommandInner::Uncontained(command.current_dir(directory))
            }
            #[cfg(not(any(windows, test)))]
            PreparedCommandInner::Unavailable => PreparedCommandInner::Unavailable,
        };
        self
    }

    pub fn env_clear(mut self) -> Self {
        self.inner = match self.inner {
            #[cfg(windows)]
            PreparedCommandInner::Contained(command) => {
                PreparedCommandInner::Contained(command.env_clear())
            }
            #[cfg(test)]
            PreparedCommandInner::Uncontained(command) => {
                PreparedCommandInner::Uncontained(command.env_clear())
            }
            #[cfg(not(any(windows, test)))]
            PreparedCommandInner::Unavailable => PreparedCommandInner::Unavailable,
        };
        self
    }

    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.inner = match self.inner {
            #[cfg(windows)]
            PreparedCommandInner::Contained(command) => {
                PreparedCommandInner::Contained(command.env(key, value))
            }
            #[cfg(test)]
            PreparedCommandInner::Uncontained(command) => {
                PreparedCommandInner::Uncontained(command.env(key, value))
            }
            #[cfg(not(any(windows, test)))]
            PreparedCommandInner::Unavailable => PreparedCommandInner::Unavailable,
        };
        self
    }

    pub fn stdin(mut self, stdio: Stdio) -> Self {
        self.stdin = stdio;
        self
    }

    pub fn stdout(mut self, stdio: Stdio) -> Self {
        self.stdout = stdio;
        self
    }

    pub fn stderr(mut self, stdio: Stdio) -> Self {
        self.stderr = stdio;
        self
    }

    /// Kept for call sites that hid the former direct child's console window.
    /// The launcher and its target run without a console on Windows.
    pub fn creation_flags(self, _flags: u32) -> Self {
        self
    }

    pub fn spawn(self) -> io::Result<ContainedProcess> {
        let PreparedCommand {
            inner,
            stdin,
            stdout,
            stderr,
        } = self;
        match inner {
            #[cfg(windows)]
            PreparedCommandInner::Contained(command) => {
                let command = command.stdin(stdin).stdout(stdout).stderr(stderr);
                Ok(ContainedProcess {
                    inner: ContainedProcessInner::Contained(windows::spawn(command)?),
                })
            }
            #[cfg(test)]
            PreparedCommandInner::Uncontained(command) => Ok(ContainedProcess {
                inner: ContainedProcessInner::Uncontained(command.spawn(stdin, stdout, stderr)?),
            }),
            #[cfg(not(any(windows, test)))]
            PreparedCommandInner::Unavailable => Err(io::Error::other(
                "containment is unavailable on this platform",
            )),
        }
    }
}

enum ContainedProcessInner {
    #[cfg(windows)]
    Contained(windows::ContainedChild),
    #[cfg(test)]
    Uncontained(std::process::Child),
    #[cfg(not(any(windows, test)))]
    Unavailable,
}

/// The process tree started through [`PreparedCommand`].
///
/// Dropping a Windows process closes its job handle, which terminates the
/// launcher, the target and every descendant.
pub struct ContainedProcess {
    inner: ContainedProcessInner,
}

impl ContainedProcess {
    pub fn take_stdin(&mut self) -> Option<std::process::ChildStdin> {
        match &mut self.inner {
            #[cfg(windows)]
            ContainedProcessInner::Contained(child) => child.take_stdin(),
            #[cfg(test)]
            ContainedProcessInner::Uncontained(child) => child.stdin.take(),
            #[cfg(not(any(windows, test)))]
            ContainedProcessInner::Unavailable => None,
        }
    }

    pub fn take_stdout(&mut self) -> Option<std::process::ChildStdout> {
        match &mut self.inner {
            #[cfg(windows)]
            ContainedProcessInner::Contained(child) => child.take_stdout(),
            #[cfg(test)]
            ContainedProcessInner::Uncontained(child) => child.stdout.take(),
            #[cfg(not(any(windows, test)))]
            ContainedProcessInner::Unavailable => None,
        }
    }

    pub fn take_stderr(&mut self) -> Option<std::process::ChildStderr> {
        match &mut self.inner {
            #[cfg(windows)]
            ContainedProcessInner::Contained(child) => child.take_stderr(),
            #[cfg(test)]
            ContainedProcessInner::Uncontained(child) => child.stderr.take(),
            #[cfg(not(any(windows, test)))]
            ContainedProcessInner::Unavailable => None,
        }
    }

    pub fn id(&self) -> u32 {
        match &self.inner {
            #[cfg(windows)]
            ContainedProcessInner::Contained(child) => child.id(),
            #[cfg(test)]
            ContainedProcessInner::Uncontained(child) => child.id(),
            #[cfg(not(any(windows, test)))]
            ContainedProcessInner::Unavailable => 0,
        }
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        match &mut self.inner {
            #[cfg(windows)]
            ContainedProcessInner::Contained(child) => child.try_wait(),
            #[cfg(test)]
            ContainedProcessInner::Uncontained(child) => child.try_wait(),
            #[cfg(not(any(windows, test)))]
            ContainedProcessInner::Unavailable => Ok(None),
        }
    }

    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        match &mut self.inner {
            #[cfg(windows)]
            ContainedProcessInner::Contained(child) => child.wait(),
            #[cfg(test)]
            ContainedProcessInner::Uncontained(child) => child.wait(),
            #[cfg(not(any(windows, test)))]
            ContainedProcessInner::Unavailable => Err(io::Error::other(
                "containment is unavailable on this platform",
            )),
        }
    }

    /// Poll the process without blocking the async runtime.
    pub async fn wait_async(&mut self) -> io::Result<ExitStatus> {
        loop {
            if let Some(status) = self.try_wait()? {
                return Ok(status);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Wait synchronously and collect both output streams.
    pub fn wait_with_output(self) -> io::Result<Output> {
        match self.inner {
            #[cfg(windows)]
            ContainedProcessInner::Contained(child) => child.wait_with_output(),
            #[cfg(test)]
            ContainedProcessInner::Uncontained(child) => child.wait_with_output(),
            #[cfg(not(any(windows, test)))]
            ContainedProcessInner::Unavailable => Err(io::Error::other(
                "containment is unavailable on this platform",
            )),
        }
    }

    /// Terminate the whole process tree.
    pub fn kill(&mut self) -> io::Result<()> {
        match &mut self.inner {
            #[cfg(windows)]
            ContainedProcessInner::Contained(child) => child.kill(),
            #[cfg(test)]
            ContainedProcessInner::Uncontained(child) => child.kill(),
            #[cfg(not(any(windows, test)))]
            ContainedProcessInner::Unavailable => Ok(()),
        }
    }
}

/// Collect a prepared process's output asynchronously.
///
/// Dropping the future (for example on timeout) drops the process wrapper,
/// which closes the Windows job and terminates the tree.
pub async fn collect_output(mut process: ContainedProcess) -> io::Result<Output> {
    let stdout = process
        .take_stdout()
        .map(tokio::process::ChildStdout::from_std)
        .transpose()?;
    let stderr = process
        .take_stderr()
        .map(tokio::process::ChildStderr::from_std)
        .transpose()?;
    let stdout_task = tokio::spawn(read_all(stdout));
    let stderr_task = tokio::spawn(read_all(stderr));
    let status = process.wait_async().await?;
    let stdout = stdout_task.await.map_err(io::Error::other)??;
    let stderr = stderr_task.await.map_err(io::Error::other)??;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

async fn read_all<R: tokio::io::AsyncRead + Unpin>(mut stream: Option<R>) -> io::Result<Vec<u8>> {
    let Some(stream) = stream.as_mut() else {
        return Ok(Vec::new());
    };
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).await?;
    Ok(bytes)
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
/// table. User-initiated terminals, their process cleanup, browser openers,
/// tests and the HTTP MCP transport are intentionally absent.
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
        path: "git/worktree.rs",
        function: "gh_output",
        kind: ProfileKind::EditWithNetwork,
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
mod test_support {
    use std::{
        ffi::OsString,
        path::PathBuf,
        process::{Child, Command, Stdio},
    };

    /// A direct std command used only by unit tests that exercise spawn
    /// plumbing. A production build has no variant of `PreparedCommand` that
    /// can reach this type.
    pub struct UncontainedCommand {
        program: OsString,
        args: Vec<OsString>,
        current_dir: Option<PathBuf>,
        env_clear: bool,
        envs: Vec<(OsString, OsString)>,
    }

    impl UncontainedCommand {
        pub fn new(program: OsString) -> Self {
            Self {
                program,
                args: Vec::new(),
                current_dir: None,
                env_clear: false,
                envs: Vec::new(),
            }
        }

        pub fn arg(mut self, argument: impl Into<OsString>) -> Self {
            self.args.push(argument.into());
            self
        }

        pub fn current_dir(mut self, directory: impl Into<PathBuf>) -> Self {
            self.current_dir = Some(directory.into());
            self
        }

        pub fn env_clear(mut self) -> Self {
            self.env_clear = true;
            self
        }

        pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
            self.envs.push((key.into(), value.into()));
            self
        }

        pub fn spawn(self, stdin: Stdio, stdout: Stdio, stderr: Stdio) -> std::io::Result<Child> {
            let mut command = Command::new(self.program);
            command
                .args(self.args)
                .stdin(stdin)
                .stdout(stdout)
                .stderr(stderr);
            if let Some(directory) = self.current_dir {
                command.current_dir(directory);
            }
            if self.env_clear {
                command.env_clear();
            }
            command.envs(self.envs);
            command.spawn()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::BTreeMap,
        fs,
        path::{Path, PathBuf},
    };

    /// Exact production-line counts for the two markers the guard scans.
    /// A line counts once when it constructs a `Command` (see
    /// [`line_constructs_a_command`]); identifier suffixes such as
    /// `ContainedCommand::new` do not count. Keep this in sync with
    /// [`SPAWN_SITES`].
    const PRODUCTION_SPAWN_ALLOWLIST: &[(&str, usize)] = &[
        // Includes the user-operated local preview launcher and its cleanup
        // and browser opener; these are intentionally not agent spawn sites.
        ("commands/mod.rs", 28),
        ("containment/windows.rs", 4),
        ("review.rs", 5),
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

    /// True when a source line constructs a process command. Identifiers that
    /// merely end in `Command` (such as `ContainedCommand::new`) are not spawn
    /// constructors.
    fn line_constructs_a_command(line: &str) -> bool {
        if line.contains("process::Command") {
            return true;
        }
        let mut rest = line;
        while let Some(index) = rest.find("Command::new(") {
            let preceded_by_identifier = rest[..index]
                .chars()
                .last()
                .is_some_and(|character| character.is_alphanumeric() || character == '_');
            if !preceded_by_identifier {
                return true;
            }
            rest = &rest[index + "Command".len()..];
        }
        false
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
                    && line_constructs_a_command(line)
            })
            .count()
    }

    #[test]
    fn blocked_policies_never_produce_a_command() {
        let blocked = [
            SpawnPolicy::new(ProfileKind::Inspect, false, true),
            SpawnPolicy::new(ProfileKind::Edit, true, false),
            SpawnPolicy::new(ProfileKind::ReducedSafety, false, false),
        ];
        for policy in blocked {
            let error = prepare_command(policy, "git")
                .err()
                .expect("a blocked policy must not produce a prepared command");
            assert_eq!(error.code, "containment_unavailable");
            assert!(error.message.contains("Command execution is unavailable"));
            assert!(error.message.contains("File edits still work"));
            assert_eq!(
                resolve_spawn(Platform::Windows, policy),
                EnforcementState::Blocked {
                    reason: BlockReason::ContainmentUnavailable,
                }
            );
        }
    }

    #[test]
    fn windows_reduced_safety_produces_a_contained_command() {
        let policy = SpawnPolicy::new(ProfileKind::Edit, true, true);
        let prepared = prepare_command(policy, "git");
        if Platform::current() == Platform::Windows {
            assert!(
                prepared.is_ok(),
                "an opted-in Windows project must prepare a command"
            );
        } else {
            assert_eq!(
                prepared.err().expect("non-Windows has no backend").code,
                "containment_unavailable"
            );
        }
    }

    #[test]
    fn confirmation_requires_the_exact_typed_phrase() {
        assert!(confirm_reduced_safety("reduced safety").is_ok());
        for invalid in [
            "",
            "reduced",
            "Reduced safety",
            "REDUCED SAFETY",
            " reduced safety",
            "reduced safety ",
            "reduced  safety",
        ] {
            assert_eq!(
                confirm_reduced_safety(invalid).unwrap_err().code,
                "containment_confirmation",
                "{invalid:?} must be rejected"
            );
        }
    }

    #[test]
    fn status_lists_every_profile_kind_with_resolved_enforcement() {
        let status = status_for_project("project-1", true, true);
        assert_eq!(status.project_id, "project-1");
        assert_eq!(status.platform, Platform::current().name());
        assert!(status.project_opted_in);
        assert!(status.machine_allowed);
        assert_eq!(status.profiles.len(), ProfileKind::ALL.len());
        assert_eq!(
            status
                .profiles
                .iter()
                .map(|profile| profile.kind)
                .collect::<Vec<_>>(),
            ProfileKind::ALL.to_vec()
        );
        for profile in &status.profiles {
            let expected = resolve_spawn(
                Platform::current(),
                SpawnPolicy::new(profile.kind, true, true),
            );
            assert_eq!(
                profile.enforcement,
                ContainmentEnforcement::from(&expected),
                "{:?}",
                profile.kind
            );
        }

        let wire = serde_json::to_value(&status).unwrap();
        assert_eq!(wire["projectId"], "project-1");
        assert_eq!(wire["projectOptedIn"], true);
        assert_eq!(wire["machineAllowed"], true);
        assert_eq!(wire["profiles"][0]["kind"], "inspect");
        assert_eq!(wire["profiles"][4]["kind"], "reduced_safety");
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
        let mut inspect_with_network = ProfileKind::Inspect.capabilities();
        inspect_with_network.push(Capability::Network);
        assert_eq!(
            ProfileKind::InspectWithNetwork.capabilities(),
            inspect_with_network
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
    fn spawn_resolution_covers_every_platform_opt_in_machine_switch_and_kind() {
        let platforms = [Platform::Windows, Platform::Linux, Platform::Other];
        let kinds = ProfileKind::ALL;
        let mut covered = 0;

        for platform in platforms {
            for project_opted_in in [false, true] {
                for machine_allowed in [false, true] {
                    for kind in kinds {
                        let policy = SpawnPolicy::new(kind, project_opted_in, machine_allowed);
                        let resolved = resolve_spawn(platform, policy);
                        let expected = if platform == Platform::Windows
                            && project_opted_in
                            && machine_allowed
                        {
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
                            "{platform:?} opted_in={project_opted_in} machine_allowed={machine_allowed} kind={kind:?}"
                        );
                        covered += 1;
                    }
                }
            }
        }

        assert_eq!(
            covered,
            platforms.len() * 2 * 2 * kinds.len(),
            "every combination must be covered"
        );
    }

    #[test]
    fn windows_reduced_safety_lists_every_unenforced_capability() {
        let policy = |kind| SpawnPolicy::new(kind, true, true);
        assert_eq!(
            resolve_spawn(
                Platform::Windows,
                SpawnPolicy::new(ProfileKind::Inspect, false, true)
            ),
            EnforcementState::Blocked {
                reason: BlockReason::ContainmentUnavailable,
            }
        );
        assert_eq!(
            resolve_spawn(Platform::Windows, policy(ProfileKind::Inspect)),
            EnforcementState::ReducedSafety {
                unsupported: vec![
                    Capability::WorkspaceRead,
                    Capability::GitMetadata,
                    Capability::ProcessSpawn,
                ],
            }
        );
        assert_eq!(
            resolve_spawn(Platform::Windows, policy(ProfileKind::Edit)),
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
            resolve_spawn(Platform::Windows, policy(ProfileKind::EditWithNetwork)),
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
            resolve_spawn(Platform::Windows, policy(ProfileKind::ReducedSafety)),
            EnforcementState::ReducedSafety {
                unsupported: Vec::new(),
            }
        );

        for kind in ProfileKind::ALL {
            let EnforcementState::ReducedSafety { unsupported } =
                resolve_spawn(Platform::Windows, policy(kind))
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
    fn the_machine_kill_switch_blocks_every_kind() {
        for kind in ProfileKind::ALL {
            assert_eq!(
                resolve_spawn(Platform::Windows, SpawnPolicy::new(kind, true, false)),
                EnforcementState::Blocked {
                    reason: BlockReason::ContainmentUnavailable,
                },
                "the machine switch must disable reduced safety for {kind:?}"
            );
        }
    }

    #[test]
    fn linux_and_other_stay_blocked_even_when_opted_in() {
        for platform in [Platform::Linux, Platform::Other] {
            for project_opted_in in [false, true] {
                for machine_allowed in [false, true] {
                    for kind in ProfileKind::ALL {
                        assert_eq!(
                            resolve_spawn(
                                platform,
                                SpawnPolicy::new(kind, project_opted_in, machine_allowed)
                            ),
                            EnforcementState::Blocked {
                                reason: BlockReason::ContainmentUnavailable,
                            },
                            "{platform:?} has no backend in this slice"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn resolving_a_profile_never_widens_the_kind() {
        for kind in [
            ProfileKind::Inspect,
            ProfileKind::InspectWithNetwork,
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
