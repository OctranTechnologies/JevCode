use crate::{
    commands::{self, CreateSession},
    domain::{
        now, ScheduledTask, ScheduledTaskRun, ScheduledTaskRunStatus, SessionStatus, TaskMode,
    },
    error::AppError,
    state::SharedState,
};
use std::time::Duration;
use tauri::{AppHandle, Manager};

const SCHEDULER_INTERVAL: Duration = Duration::from_secs(15);

pub async fn run(state: SharedState, app: AppHandle) {
    let mut interval = tokio::time::interval(SCHEDULER_INTERVAL);
    loop {
        interval.tick().await;
        refresh_run_statuses(&state);
        let due = match state.database.claim_due_scheduled_tasks(chrono::Utc::now()) {
            Ok(due) => due,
            Err(error) => {
                tracing::error!(%error, "Could not claim scheduled task runs");
                continue;
            }
        };
        for (task, run) in due {
            launch(state.clone(), app.clone(), task, run).await;
        }
    }
}

async fn launch(
    state: SharedState,
    app: AppHandle,
    task: ScheduledTask,
    mut run: ScheduledTaskRun,
) {
    let input = CreateSession {
        task_mode: TaskMode::Agent,
        project_id: task.project_id.clone(),
        provider_id: task.provider_id.clone(),
        model_id: task.model_id.clone(),
        permission_policy: task.permission_policy.clone(),
        workspace_mode: task.workspace_mode,
        base_branch: task.base_branch.clone(),
        task_request: Some(task.prompt.clone()),
        parent_session_id: None,
    };
    let created = commands::create_session(input, app.clone(), app.state::<SharedState>()).await;
    let session = match created {
        Ok(session) => session,
        Err(error) => {
            fail_run(&state, &mut run, &error);
            return;
        }
    };

    run.session_id = Some(session.id.clone());
    if let Err(error) = state.database.update_scheduled_task_run(&run) {
        tracing::error!(%error, scheduled_task_id = %task.id, "Could not link scheduled task run to its session");
        return;
    }

    match commands::send_message(
        session.id,
        task.prompt,
        None,
        app.clone(),
        app.state::<SharedState>(),
    ) {
        Ok(session) => {
            run.status = run_status(&session.status);
            if is_terminal(&run.status) {
                run.completed_at = Some(session.updated_at);
            }
            if let Err(error) = state.database.update_scheduled_task_run(&run) {
                tracing::error!(%error, scheduled_task_id = %task.id, "Could not save scheduled task run status");
            }
        }
        Err(error) => fail_run(&state, &mut run, &error),
    }
}

fn fail_run(state: &SharedState, run: &mut ScheduledTaskRun, error: &AppError) {
    tracing::warn!(code = %error.code, "Scheduled task could not start");
    run.status = ScheduledTaskRunStatus::Failed;
    run.error = Some(format!("Could not start this task ({})", error.code));
    run.completed_at = Some(now());
    if let Err(save_error) = state.database.update_scheduled_task_run(run) {
        tracing::error!(%save_error, "Could not save scheduled task startup failure");
    }
}

fn refresh_run_statuses(state: &SharedState) {
    let runs = match state.database.scheduled_task_runs(100) {
        Ok(runs) => runs,
        Err(error) => {
            tracing::error!(%error, "Could not read scheduled task history");
            return;
        }
    };
    for mut run in runs {
        let Some(session_id) = run.session_id.as_deref() else {
            continue;
        };
        if is_terminal(&run.status) {
            continue;
        }
        let session = match state.database.session(session_id) {
            Ok(session) => session,
            Err(_) => {
                run.status = ScheduledTaskRunStatus::Failed;
                run.error = Some("The task session is no longer available.".into());
                run.completed_at = Some(now());
                let _ = state.database.update_scheduled_task_run(&run);
                continue;
            }
        };
        let status = run_status(&session.status);
        if status == run.status {
            continue;
        }
        run.status = status;
        if is_terminal(&run.status) {
            run.completed_at = Some(session.updated_at);
        }
        if let Err(error) = state.database.update_scheduled_task_run(&run) {
            tracing::error!(%error, scheduled_task_id = %run.scheduled_task_id, "Could not refresh scheduled task status");
        }
    }
}

fn run_status(status: &SessionStatus) -> ScheduledTaskRunStatus {
    match status {
        SessionStatus::Queued | SessionStatus::Planning | SessionStatus::Working => {
            ScheduledTaskRunStatus::Running
        }
        SessionStatus::WaitingForPermission => ScheduledTaskRunStatus::WaitingForPermission,
        SessionStatus::WaitingForUser => ScheduledTaskRunStatus::WaitingForUser,
        SessionStatus::Completed => ScheduledTaskRunStatus::Completed,
        SessionStatus::Failed => ScheduledTaskRunStatus::Failed,
        SessionStatus::Cancelled => ScheduledTaskRunStatus::Cancelled,
    }
}

fn is_terminal(status: &ScheduledTaskRunStatus) -> bool {
    matches!(
        status,
        ScheduledTaskRunStatus::Completed
            | ScheduledTaskRunStatus::Failed
            | ScheduledTaskRunStatus::Cancelled
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirrors_every_task_status_without_hiding_permission_waits() {
        for (task, expected) in [
            (SessionStatus::Queued, ScheduledTaskRunStatus::Running),
            (SessionStatus::Planning, ScheduledTaskRunStatus::Running),
            (SessionStatus::Working, ScheduledTaskRunStatus::Running),
            (
                SessionStatus::WaitingForPermission,
                ScheduledTaskRunStatus::WaitingForPermission,
            ),
            (
                SessionStatus::WaitingForUser,
                ScheduledTaskRunStatus::WaitingForUser,
            ),
            (SessionStatus::Completed, ScheduledTaskRunStatus::Completed),
            (SessionStatus::Failed, ScheduledTaskRunStatus::Failed),
            (SessionStatus::Cancelled, ScheduledTaskRunStatus::Cancelled),
        ] {
            assert_eq!(run_status(&task), expected);
        }
    }
}
