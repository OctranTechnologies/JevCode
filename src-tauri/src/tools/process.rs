use crate::error::{AppError, AppResult};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{io::AsyncRead, process::Command};

#[derive(Debug)]
pub struct ProcessOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub truncated: bool,
    pub timed_out: bool,
}

#[derive(Debug, Clone)]
pub struct ProcessOutputChunk {
    pub stream: String,
    pub chunk: String,
}

async fn read_bounded<R: AsyncRead + Unpin>(
    mut stream: R,
    limit: usize,
    stream_name: &'static str,
    events: Option<tokio::sync::mpsc::UnboundedSender<ProcessOutputChunk>>,
) -> std::io::Result<(Vec<u8>, bool)> {
    use tokio::io::AsyncReadExt;
    let mut output = Vec::with_capacity(limit.min(8192));
    let mut buffer = [0u8; 8192];
    let mut truncated = false;
    let mut streamed = 0usize;
    let mut stream_truncation_sent = false;
    loop {
        let count = stream.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        if let Some(events) = &events {
            let remaining_stream = limit.saturating_sub(streamed);
            let visible = count.min(remaining_stream);
            if visible > 0 {
                let _ = events.send(ProcessOutputChunk {
                    stream: stream_name.into(),
                    chunk: String::from_utf8_lossy(&buffer[..visible]).into_owned(),
                });
                streamed += visible;
            }
            if visible < count && !stream_truncation_sent {
                let _ = events.send(ProcessOutputChunk {
                    stream: stream_name.into(),
                    chunk: "\n[live output truncated]".into(),
                });
                stream_truncation_sent = true;
            }
        }
        let remaining = limit.saturating_sub(output.len());
        output.extend_from_slice(&buffer[..count.min(remaining)]);
        truncated |= count > remaining;
    }
    Ok((output, truncated))
}

pub async fn run_program(
    program: &str,
    args: &[String],
    cwd: &Path,
    timeout: Duration,
    output_limit: usize,
) -> AppResult<ProcessOutput> {
    run_program_streaming(program, args, cwd, timeout, output_limit, None).await
}

pub async fn run_program_streaming(
    program: &str,
    args: &[String],
    cwd: &Path,
    timeout: Duration,
    output_limit: usize,
    events: Option<tokio::sync::mpsc::UnboundedSender<ProcessOutputChunk>>,
) -> AppResult<ProcessOutput> {
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .kill_on_drop(true)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in filtered_environment() {
        command.env(key, value);
    }
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let mut child = command.spawn().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            AppError::new(
                "program_unavailable",
                "The requested program is not installed or not available on PATH.",
            )
        } else {
            AppError::new(
                "process_start",
                "The requested program could not be started.",
            )
        }
    })?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AppError::new("process_output", "Could not capture program output."))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| AppError::new("process_output", "Could not capture program errors."))?;
    let stdout_task = tokio::spawn(read_bounded(stdout, output_limit, "stdout", events.clone()));
    let stderr_task = tokio::spawn(read_bounded(stderr, output_limit, "stderr", events));
    let status = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(result) => Some(result.map_err(|_| {
            AppError::new("process_wait", "Could not collect the program exit status.")
        })?),
        Err(_) => {
            let _ = child.kill().await;
            None
        }
    };
    let (stdout, stdout_truncated) = stdout_task
        .await
        .map_err(AppError::internal)?
        .map_err(AppError::internal)?;
    let (stderr, stderr_truncated) = stderr_task
        .await
        .map_err(AppError::internal)?
        .map_err(AppError::internal)?;
    let timed_out = status.is_none();
    Ok(ProcessOutput {
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
        exit_code: status.and_then(|status| status.code()),
        truncated: stdout_truncated || stderr_truncated,
        timed_out,
    })
}

/// Only standard execution variables are inherited by agent-launched programs.
/// Provider credentials and other application secrets are never copied into a
/// child process environment.
fn filtered_environment() -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    std::env::vars_os()
        .filter(|(key, _)| environment_key_is_allowed(key))
        .collect()
}

fn environment_key_is_allowed(key: &std::ffi::OsStr) -> bool {
    const ALLOWED: &[&str] = &[
        "PATH",
        "HOME",
        "USER",
        "LOGNAME",
        "USERPROFILE",
        "HOMEDRIVE",
        "HOMEPATH",
        "TEMP",
        "TMP",
        "SYSTEMROOT",
        "SYSTEMDRIVE",
        "WINDIR",
        "APPDATA",
        "LOCALAPPDATA",
        "PATHEXT",
        "COMSPEC",
        "LANG",
        "TERM",
        "SHELL",
        "CARGO_HOME",
        "RUSTUP_HOME",
        "NPM_CONFIG_CACHE",
    ];
    let key = key.to_string_lossy().to_ascii_uppercase();
    ALLOWED.contains(&key.as_str()) || key.starts_with("LC_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_environment_excludes_credentials_and_authentication_variables() {
        assert!(environment_key_is_allowed(std::ffi::OsStr::new("PATH")));
        for secret in [
            "OPENAI_API_KEY",
            "AWS_SECRET_ACCESS_KEY",
            "GH_TOKEN",
            "DATABASE_PASSWORD",
            "AUTHORIZATION",
        ] {
            assert!(
                !environment_key_is_allowed(std::ffi::OsStr::new(secret)),
                "{secret} must not reach child processes"
            );
        }
        let actual = filtered_environment();
        assert!(actual
            .iter()
            .all(|(key, _)| environment_key_is_allowed(key)));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn command_streams_both_channels_and_reports_exit_status() {
        let root = tempfile::tempdir().unwrap();
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let result = run_program_streaming(
            "sh",
            &["-c".into(), "printf out; printf err >&2; exit 7".into()],
            root.path(),
            Duration::from_secs(3),
            1024,
            Some(sender),
        )
        .await
        .unwrap();
        assert_eq!(result.exit_code, Some(7));
        assert!(!result.timed_out);
        let mut streams = Vec::new();
        while let Ok(chunk) = receiver.try_recv() {
            streams.push((chunk.stream, chunk.chunk));
        }
        assert!(streams
            .iter()
            .any(|(stream, text)| stream == "stdout" && text == "out"));
        assert!(streams
            .iter()
            .any(|(stream, text)| stream == "stderr" && text == "err"));
    }

    #[tokio::test]
    async fn command_timeout_kills_child_and_reports_timeout() {
        let root = tempfile::tempdir().unwrap();
        #[cfg(windows)]
        let (program, args) = (
            "powershell.exe",
            vec![
                "-NoLogo".into(),
                "-NoProfile".into(),
                "-Command".into(),
                "Start-Sleep -Seconds 2".into(),
            ],
        );
        #[cfg(unix)]
        let (program, args) = ("sh", vec!["-c".into(), "sleep 2".into()]);
        let result = run_program_streaming(
            program,
            &args,
            root.path(),
            Duration::from_millis(35),
            1024,
            None,
        )
        .await
        .unwrap();
        assert!(result.timed_out);
        assert_eq!(result.exit_code, None);
    }
}
