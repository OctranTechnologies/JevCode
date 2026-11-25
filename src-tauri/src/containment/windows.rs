// Windows reduced-safety containment: one named Job Object per spawn plus the
// launcher that joins it before starting the target.
//
// The job carries `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`,
// `JOB_OBJECT_LIMIT_ACTIVE_PROCESS` and `JOB_OBJECT_LIMIT_JOB_MEMORY`. It does
// not restrict files, network or handle access; callers must resolve the
// profile through `resolve_spawn` and report `ReducedSafety` rather than
// claiming the task is contained.

use std::{
    ffi::{OsStr, OsString},
    io,
    mem::size_of,
    os::windows::ffi::OsStrExt,
    path::PathBuf,
    process::{Output, Stdio},
    ptr,
};

use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE},
    System::{
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            OpenJobObjectW, SetInformationJobObject, TerminateJobObject,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
            JOB_OBJECT_LIMIT_JOB_MEMORY, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        },
        SystemServices::{JOB_OBJECT_ASSIGN_PROCESS, JOB_OBJECT_QUERY},
        Threading::GetCurrentProcess,
    },
};

use super::{ProcessLimits, LAUNCHER_ARGUMENT};

/// A command for the reduced-safety backend. Arguments, working directory and
/// environment are carried explicitly so the launcher can reproduce the
/// caller's intent exactly.
#[derive(Debug)]
pub struct ContainedCommand {
    program: OsString,
    args: Vec<OsString>,
    current_dir: Option<PathBuf>,
    env_clear: bool,
    envs: Vec<(OsString, OsString)>,
    stdin: Stdio,
    stdout: Stdio,
    stderr: Stdio,
}

impl ContainedCommand {
    pub fn new(program: impl Into<OsString>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            current_dir: None,
            env_clear: false,
            envs: Vec::new(),
            stdin: Stdio::inherit(),
            stdout: Stdio::inherit(),
            stderr: Stdio::inherit(),
        }
    }

    pub fn arg(mut self, argument: impl Into<OsString>) -> Self {
        self.args.push(argument.into());
        self
    }

    pub fn args<I, S>(mut self, arguments: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(arguments.into_iter().map(Into::into));
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
}

/// A Job Object handle. Closing it is the containment teardown: with
/// `KILL_ON_JOB_CLOSE`, the kernel terminates every process still in the job.
struct JobHandle(HANDLE);

impl Drop for JobHandle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

/// The named job that owns one spawn's process tree.
///
/// Production creates a fresh job per spawn and keeps the handle until the
/// wrapper is dropped. The launcher opens the same job by name before it
/// starts the target.
pub struct ContainedJob {
    handle: Option<JobHandle>,
}

impl ContainedJob {
    /// Create the default job: 64 processes and 2 GiB of job memory (D6).
    pub fn create(name: &str) -> io::Result<Self> {
        let limits = ProcessLimits::default();
        Self::create_with_limits(name, limits.max_processes, limits.memory_bytes)
    }

    fn create_with_limits(name: &str, max_processes: u32, memory_bytes: u64) -> io::Result<Self> {
        if max_processes == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a job must allow at least one process",
            ));
        }
        let name = wide(name);
        let handle = unsafe { CreateJobObjectW(ptr::null(), name.as_ptr()) };
        if handle.is_null() {
            return Err(last_error("CreateJobObjectW"));
        }
        let job = Self {
            handle: Some(JobHandle(handle)),
        };
        job.apply_limits(max_processes, memory_bytes)?;
        Ok(job)
    }

    /// Open an existing named job. Used by the launcher, which must join the
    /// job the app created before it starts the target.
    pub fn open(name: &str) -> io::Result<Self> {
        let name = wide(name);
        let handle = unsafe {
            OpenJobObjectW(
                JOB_OBJECT_ASSIGN_PROCESS | JOB_OBJECT_QUERY,
                0,
                name.as_ptr(),
            )
        };
        if handle.is_null() {
            return Err(last_error("OpenJobObjectW"));
        }
        Ok(Self {
            handle: Some(JobHandle(handle)),
        })
    }

    fn apply_limits(&self, max_processes: u32, memory_bytes: u64) -> io::Result<()> {
        let mut information: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        information.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
            | JOB_OBJECT_LIMIT_JOB_MEMORY;
        information.BasicLimitInformation.ActiveProcessLimit = max_processes;
        information.JobMemoryLimit = memory_bytes as usize;

        let configured = unsafe {
            SetInformationJobObject(
                self.handle(),
                JobObjectExtendedLimitInformation,
                &information as *const _ as *const core::ffi::c_void,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if configured == 0 {
            return Err(last_error("SetInformationJobObject"));
        }
        Ok(())
    }

    /// Put the current process in the job.
    pub fn assign_current_process(&self) -> io::Result<()> {
        let assigned = unsafe { AssignProcessToJobObject(self.handle(), GetCurrentProcess()) };
        if assigned == 0 {
            return Err(last_error("AssignProcessToJobObject"));
        }
        Ok(())
    }

    /// Terminate every process in the job, including the launcher.
    pub fn terminate(&self) -> io::Result<()> {
        let terminated = unsafe { TerminateJobObject(self.handle(), 1) };
        if terminated == 0 {
            return Err(last_error("TerminateJobObject"));
        }
        Ok(())
    }

    fn handle(&self) -> HANDLE {
        self.handle
            .as_ref()
            .map(|handle| handle.0)
            .unwrap_or(ptr::null_mut())
    }

    fn close(&mut self) {
        self.handle.take();
    }
}

/// Build the launcher process for `command`.
///
/// Stdio, environment and working directory are inherited by the target from
/// the launcher, so the caller's pipes and the filtered environment reach the
/// target unchanged. The launcher adds no variables of its own.
pub fn launcher_command(
    name: &str,
    command: ContainedCommand,
) -> io::Result<std::process::Command> {
    let executable = std::env::current_exe()?;
    let mut launcher = std::process::Command::new(executable);
    launcher
        .arg(LAUNCHER_ARGUMENT)
        .arg(name)
        .arg(&command.program)
        .args(&command.args)
        .stdin(command.stdin)
        .stdout(command.stdout)
        .stderr(command.stderr);
    if let Some(directory) = &command.current_dir {
        launcher.current_dir(directory);
    }
    if command.env_clear {
        launcher.env_clear();
    }
    launcher.envs(command.envs.iter().map(|(key, value)| (key, value)));
    // The launcher must not create a console window of its own.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        launcher.creation_flags(0x08000000);
    }
    Ok(launcher)
}

/// A contained process tree: the job handle plus the launcher's child.
///
/// Dropping the wrapper closes the job handle first. Because the job was
/// created with `KILL_ON_JOB_CLOSE`, that terminates the launcher, the target
/// and every descendant, including processes that outlived their parent.
pub struct ContainedChild {
    job: ContainedJob,
    child: Option<std::process::Child>,
}

impl ContainedChild {
    /// Process id of the launcher. The launcher waits for the target and exits
    /// with its code, so this is not the target's id.
    pub fn id(&self) -> u32 {
        self.child().id()
    }

    fn child(&self) -> &std::process::Child {
        self.child
            .as_ref()
            .expect("a contained child is present until it is waited on")
    }

    fn child_mut(&mut self) -> &mut std::process::Child {
        self.child
            .as_mut()
            .expect("a contained child is present until it is waited on")
    }

    pub fn wait(&mut self) -> io::Result<std::process::ExitStatus> {
        self.child_mut().wait()
    }

    pub fn try_wait(&mut self) -> io::Result<Option<std::process::ExitStatus>> {
        self.child_mut().try_wait()
    }

    pub fn take_stdin(&mut self) -> Option<std::process::ChildStdin> {
        self.child_mut().stdin.take()
    }

    pub fn take_stdout(&mut self) -> Option<std::process::ChildStdout> {
        self.child_mut().stdout.take()
    }

    pub fn take_stderr(&mut self) -> Option<std::process::ChildStderr> {
        self.child_mut().stderr.take()
    }

    /// Wait for the launcher and collect the inherited pipes.
    ///
    /// The pipes are drained on separate threads so a descendant holding a
    /// pipe open cannot block the wait, and the job is closed as soon as the
    /// launcher exits, which terminates any such descendant before the readers
    /// are joined.
    pub fn wait_with_output(mut self) -> io::Result<Output> {
        use std::io::Read;

        let stdout = self.child_mut().stdout.take();
        let stderr = self.child_mut().stderr.take();
        let read = |mut pipe: std::process::ChildStdout| {
            std::thread::spawn(move || {
                let mut bytes = Vec::new();
                pipe.read_to_end(&mut bytes)?;
                Ok::<_, io::Error>(bytes)
            })
        };
        let read_err = |mut pipe: std::process::ChildStderr| {
            std::thread::spawn(move || {
                let mut bytes = Vec::new();
                pipe.read_to_end(&mut bytes)?;
                Ok::<_, io::Error>(bytes)
            })
        };
        let stdout_reader = stdout.map(read);
        let stderr_reader = stderr.map(read_err);
        let status = self.wait()?;
        self.job.close();
        let join = |reader: Option<std::thread::JoinHandle<io::Result<Vec<u8>>>>| match reader {
            Some(reader) => reader
                .join()
                .map_err(|_| io::Error::other("output reader panicked"))?,
            None => Ok(Vec::new()),
        };
        Ok(Output {
            status,
            stdout: join(stdout_reader)?,
            stderr: join(stderr_reader)?,
        })
    }

    /// Explicitly terminate the whole tree.
    pub fn kill(&mut self) -> io::Result<()> {
        if self.job.handle().is_null() {
            return Ok(());
        }
        if let Err(error) = self.job.terminate() {
            if let Some(child) = &mut self.child {
                let _ = child.kill();
                let _ = child.wait();
            }
            return Err(error);
        }
        self.child_mut().wait().map(|_| ())
    }
}

// Job and process handles are kernel objects with no thread affinity, and the
// wrapper is moved between the runtime's worker threads.
unsafe impl Send for ContainedChild {}

impl Drop for ContainedChild {
    fn drop(&mut self) {
        // Closing the job handle is the kill: `KILL_ON_JOB_CLOSE` reports the
        // job as terminated once its last handle closes. Do this before the
        // launcher's process handle is released.
        self.job.close();
    }
}

/// Create a job and start `command` in it through the launcher.
pub fn spawn(command: ContainedCommand) -> io::Result<ContainedChild> {
    let job_name = format!("Local\\jevcode-containment-{}", uuid::Uuid::new_v4());
    let job = ContainedJob::create(&job_name)?;
    let child = launcher_command(&job_name, command)?.spawn()?;
    Ok(ContainedChild {
        job,
        child: Some(child),
    })
}

/// Entry point of the launcher process. `arguments` are the arguments after
/// the reserved marker: the job name, the target program and its arguments.
pub fn run_launcher(arguments: Vec<OsString>) -> i32 {
    let mut arguments = arguments.into_iter();
    let (Some(job_name), Some(program)) = (arguments.next(), arguments.next()) else {
        eprintln!("jevcode containment launcher: missing job name or program");
        return 1;
    };
    let args: Vec<OsString> = arguments.collect();
    match launch(&job_name, &program, &args) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("jevcode containment launcher failed: {error}");
            1
        }
    }
}

fn launch(job_name: &OsStr, program: &OsStr, args: &[OsString]) -> io::Result<i32> {
    let job_name = job_name.to_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "the containment job name is not valid Unicode",
        )
    })?;
    let job = ContainedJob::open(job_name)?;

    // Join the job before creating the target. A process only enters a job at
    // creation or through an explicit assignment, so if the target were started
    // first it would run outside the job for a short window, and any child it
    // created during that window would never be captured. The launcher has
    // started nothing yet and is single-threaded here, so every descendant is
    // inside the job from its first instruction.
    job.assign_current_process()?;

    let mut target = std::process::Command::new(program);
    target.args(args);
    let status = target.spawn()?.wait()?;
    Ok(status.code().unwrap_or(1))
}

fn wide(value: &str) -> Vec<u16> {
    OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn last_error(operation: &str) -> io::Error {
    let error = io::Error::last_os_error();
    io::Error::new(error.kind(), format!("{operation} failed: {error}"))
}

#[cfg(test)]
#[cfg(windows)]
mod tests {
    use super::*;
    use std::{
        fs,
        path::Path,
        thread,
        time::{Duration, Instant},
    };
    use windows_sys::Win32::{
        Foundation::STILL_ACTIVE,
        System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA,
            PROCESS_TERMINATE,
        },
    };

    fn test_directory(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "jevcode-containment-{name}-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&directory).expect("create the containment test directory");
        directory
    }

    fn job_name() -> String {
        format!("Local\\jevcode-containment-test-{}", uuid::Uuid::new_v4())
    }

    fn write_script(path: &Path, template: &str, replacements: &[(&str, &str)]) {
        let mut body = template.to_owned();
        for (token, value) in replacements {
            body = body.replace(token, value);
        }
        fs::write(path, body).expect("write a Windows PowerShell test script");
    }

    fn script_path(path: &Path) -> String {
        path.to_string_lossy().replace('\'', "''")
    }

    fn spawn_powershell(script: &Path) -> std::process::Child {
        std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-WindowStyle",
                "Hidden",
                "-File",
            ])
            .arg(script)
            .spawn()
            .expect("start Windows PowerShell")
    }

    /// Move an already started helper into the job. The helper waits for a gate
    /// file before starting anything, so this reproduces the launcher's
    /// join-before-spawn ordering without racing the helper's first child.
    fn assign_to_job(job: &ContainedJob, process_id: u32) {
        let process = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, process_id) };
        assert!(
            !process.is_null(),
            "OpenProcess: {}",
            io::Error::last_os_error()
        );
        let assigned = unsafe { AssignProcessToJobObject(job.handle(), process) };
        let error = io::Error::last_os_error();
        unsafe {
            CloseHandle(process);
        }
        assert!(assigned != 0, "AssignProcessToJobObject: {error}");
    }

    fn wait_for_file(path: &Path, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if path.is_file() {
                return true;
            }
            thread::sleep(Duration::from_millis(25));
        }
        path.is_file()
    }

    /// Wait until a helper has finished writing. The helper publishes its
    /// result by renaming a temporary file, so the final path never exposes a
    /// partial line.
    fn wait_for_content(path: &Path, expected: &str, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if fs::read_to_string(path)
                .map(|content| content.trim() == expected)
                .unwrap_or(false)
            {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(25));
        }
    }

    fn read_trim(path: &Path) -> String {
        fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
            .trim()
            .to_owned()
    }

    fn process_is_alive(process_id: u32) -> bool {
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) };
        if process.is_null() {
            return false;
        }
        let mut exit_code = 0u32;
        let queried = unsafe { GetExitCodeProcess(process, &mut exit_code) };
        unsafe {
            CloseHandle(process);
        }
        queried != 0 && exit_code == STILL_ACTIVE as u32
    }

    fn wait_until_dead(process_id: u32, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if !process_is_alive(process_id) {
                return true;
            }
            thread::sleep(Duration::from_millis(50));
        }
        !process_is_alive(process_id)
    }

    #[test]
    fn active_process_limit_denies_the_extra_process() {
        let directory = test_directory("process-limit");
        let gate = directory.join("gate");
        let result = directory.join("result.txt");
        let script = directory.join("limit.ps1");
        write_script(
            &script,
            r#"$ErrorActionPreference = 'Stop'
while (-not (Test-Path -LiteralPath '@GATE@')) { Start-Sleep -Milliseconds 20 }
$blocker = Start-Process -FilePath 'ping.exe' -ArgumentList '-n 6 127.0.0.1' -PassThru -WindowStyle Hidden
Start-Sleep -Milliseconds 300
$spawned = $false
try {
    $extra = Start-Process -FilePath 'ping.exe' -ArgumentList '-n 2 127.0.0.1' -PassThru -WindowStyle Hidden
    $spawned = $true
} catch {
}
$temporary = '@RESULT@.tmp'
Set-Content -LiteralPath $temporary -Value ('blocker=' + ($null -ne $blocker) + ' spawned=' + $spawned)
Move-Item -LiteralPath $temporary -Destination '@RESULT@' -Force
"#,
            &[
                ("@GATE@", &script_path(&gate)),
                ("@RESULT@", &script_path(&result)),
            ],
        );

        // One slot for the helper, one for the blocker; the third create must
        // be denied by the kernel with ERROR_NOT_ENOUGH_QUOTA (1816), as in the
        // slice-0 spike.
        let job =
            ContainedJob::create_with_limits(&job_name(), 2, ProcessLimits::default().memory_bytes)
                .expect("create the process-limited job");
        let mut helper = spawn_powershell(&script);
        assign_to_job(&job, helper.id());
        fs::write(&gate, b"go").expect("open the helper gate");

        let expected = "blocker=True spawned=False";
        assert!(
            wait_for_content(&result, expected, Duration::from_secs(12)),
            "the process-limit helper did not report the expected result"
        );
        assert_eq!(
            read_trim(&result),
            expected,
            "the job's ActiveProcessLimit must deny a process past the limit"
        );
        helper.wait().expect("reap the process-limit helper");

        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn job_memory_limit_denies_an_oversized_allocation() {
        let directory = test_directory("job-memory");
        let gate = directory.join("gate");
        let result = directory.join("result.txt");
        let script = directory.join("memory.ps1");
        write_script(
            &script,
            r#"$ErrorActionPreference = 'Stop'
while (-not (Test-Path -LiteralPath '@GATE@')) { Start-Sleep -Milliseconds 20 }
$chunks = [System.Collections.Generic.List[byte[]]]::new()
$allocated = $false
try {
    for ($index = 0; $index -lt 64; $index++) {
        $chunks.Add([byte[]]::new(16 * 1024 * 1024))
    }
    $allocated = $true
} catch {
    $allocated = $false
}
$chunks = $null
[System.GC]::Collect()
Set-Content -LiteralPath '@RESULT@' -Value ('allocated=' + $allocated)
"#,
            &[
                ("@GATE@", &script_path(&gate)),
                ("@RESULT@", &script_path(&result)),
            ],
        );

        // 384 MiB leaves room for the helper but is far below the 1 GiB the
        // script tries to commit, so the allocation must fail inside the job.
        let job = ContainedJob::create_with_limits(&job_name(), 8, 384 * 1024 * 1024)
            .expect("create the memory-limited job");
        let mut helper = spawn_powershell(&script);
        assign_to_job(&job, helper.id());
        fs::write(&gate, b"go").expect("open the helper gate");

        assert!(
            wait_for_content(&result, "allocated=False", Duration::from_secs(12)),
            "the job-memory helper did not report a result"
        );
        assert_eq!(
            read_trim(&result),
            "allocated=False",
            "the job's JobMemoryLimit must deny an oversized allocation"
        );
        helper.wait().expect("reap the job-memory helper");

        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn dropping_the_wrapper_reaps_a_grandchild() {
        let directory = test_directory("kill-on-close");
        let gate = directory.join("gate");
        let heartbeat = directory.join("heartbeat.txt");
        let grandchild_pid_file = directory.join("grandchild.pid");
        let grandchild_script = directory.join("grandchild.ps1");
        let helper_script = directory.join("helper.ps1");

        write_script(
            &grandchild_script,
            r#"$heartbeat = '@HEARTBEAT@'
Set-Content -LiteralPath '@PID@' -Value $PID
while ($true) {
    Add-Content -LiteralPath $heartbeat -Value (Get-Date).Ticks
    Start-Sleep -Milliseconds 100
}
"#,
            &[
                ("@HEARTBEAT@", &script_path(&heartbeat)),
                ("@PID@", &script_path(&grandchild_pid_file)),
            ],
        );
        write_script(
            &helper_script,
            r#"$ErrorActionPreference = 'Stop'
while (-not (Test-Path -LiteralPath '@GATE@')) { Start-Sleep -Milliseconds 20 }
$arguments = '-NoProfile -NonInteractive -WindowStyle Hidden -File "' + '@GRANDCHILD@' + '"'
$grandchild = Start-Process -FilePath 'powershell.exe' -ArgumentList $arguments -PassThru -WindowStyle Hidden
Set-Content -LiteralPath '@PID@' -Value $grandchild.Id
Start-Sleep -Seconds 30
"#,
            &[
                ("@GATE@", &script_path(&gate)),
                ("@GRANDCHILD@", &script_path(&grandchild_script)),
                ("@PID@", &script_path(&grandchild_pid_file)),
            ],
        );

        let job = ContainedJob::create(&job_name()).expect("create the default job");
        let helper = spawn_powershell(&helper_script);
        let helper_pid = helper.id();
        assign_to_job(&job, helper_pid);
        fs::write(&gate, b"go").expect("open the helper gate");

        assert!(
            wait_for_file(&grandchild_pid_file, Duration::from_secs(12))
                && wait_for_file(&heartbeat, Duration::from_secs(12)),
            "the grandchild did not start inside the job"
        );
        let grandchild_pid: u32 = read_trim(&grandchild_pid_file)
            .parse()
            .expect("parse the grandchild process id");
        assert!(
            process_is_alive(grandchild_pid),
            "the grandchild must be running before the wrapper is dropped"
        );

        let wrapper = ContainedChild {
            job,
            child: Some(helper),
        };
        drop(wrapper);

        assert!(
            wait_until_dead(grandchild_pid, Duration::from_secs(5)),
            "closing the job handle must reap the grandchild"
        );
        assert!(
            wait_until_dead(helper_pid, Duration::from_secs(5)),
            "closing the job handle must reap the launcher-shaped helper"
        );

        let first_length = fs::metadata(&heartbeat).map(|meta| meta.len()).unwrap_or(0);
        thread::sleep(Duration::from_millis(300));
        let second_length = fs::metadata(&heartbeat).map(|meta| meta.len()).unwrap_or(0);
        assert_eq!(
            first_length, second_length,
            "the grandchild kept writing after the job handle closed"
        );

        let _ = fs::remove_dir_all(&directory);
    }
}
