use crate::error::{AppError, AppResult};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{io::AsyncRead, process::Command};

#[derive(Debug)]
pub struct ProcessOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub truncated: bool,
}

async fn read_bounded<R: AsyncRead + Unpin>(
    mut stream: R,
    limit: usize,
) -> std::io::Result<(Vec<u8>, bool)> {
    use tokio::io::AsyncReadExt;
    let mut output = Vec::with_capacity(limit.min(8192));
    let mut buffer = [0u8; 8192];
    let mut truncated = false;
    loop {
        let count = stream.read(&mut buffer).await?;
        if count == 0 {
            break;
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
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
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
    let stdout_task = tokio::spawn(read_bounded(stdout, output_limit));
    let stderr_task = tokio::spawn(read_bounded(stderr, output_limit));
    let status = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(result) => result.map_err(|_| {
            AppError::new("process_wait", "Could not collect the program exit status.")
        })?,
        Err(_) => {
            let _ = child.kill().await;
            return Err(AppError::new(
                "tool_timeout",
                "The program exceeded its execution timeout.",
            ));
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
    Ok(ProcessOutput {
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
        exit_code: status.code(),
        truncated: stdout_truncated || stderr_truncated,
    })
}
