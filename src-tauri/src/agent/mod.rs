use crate::{
    domain::*,
    error::{AppError, AppResult},
    providers::{self, ProviderRequest},
    state::SharedState,
    tools, usage,
};
use std::{path::Path, time::Instant};
use tokio::sync::watch;

pub trait EventSink: Send + Sync {
    fn session_updated(&self, session: &AgentSession);
    fn usage_updated(&self, record: &UsageRecord);
}

pub fn checkpoint(
    state: &SharedState,
    sink: &dyn EventSink,
    session: &mut AgentSession,
) -> AppResult<()> {
    session.updated_at = now();
    state.database.save_session(session)?;
    sink.session_updated(session);
    Ok(())
}

pub fn append_result(session: &mut AgentSession, result: ToolResult) {
    let mut message = AgentMessage::text(MessageRole::Tool, result.content.clone());
    message.tool_result = Some(result);
    session.messages.push(message);
}

/// Close outstanding tool calls before a retry; providers require one result per call.
pub fn settle_pending(session: &mut AgentSession, reason: &str) {
    session.close_pending_tools(reason);
}

pub async fn run(
    state: SharedState,
    sink: Box<dyn EventSink>,
    mut session: AgentSession,
    mut cancel: watch::Receiver<bool>,
) {
    let session_id = session.id.clone();
    let result = tokio::select! {
        biased;
        _ = cancel.changed() => Err(AppError::new("cancelled", "Run stopped by you.")),
        result = drive(&state, sink.as_ref(), &mut session) => result,
    };
    if let Err(error) = result {
        tracing::warn!(session_id = %session.id, code = %error.code, "Agent run ended");
        session.status = if error.code == "cancelled" {
            SessionStatus::Cancelled
        } else {
            SessionStatus::Failed
        };
        session.error = Some(error.message.clone());
        settle_pending(&mut session, &error.message);
        if let Err(error) = checkpoint(&state, sink.as_ref(), &mut session) {
            tracing::error!(code = %error.code, "Could not persist terminal session state");
        }
    }
    state.release_run(&session_id);
}

async fn drive(
    state: &SharedState,
    sink: &dyn EventSink,
    session: &mut AgentSession,
) -> AppResult<()> {
    let project = state.database.project(&session.project_id)?;
    let provider = state.config.provider(&session.provider_id)?;
    let secret = if provider.protocol == ProviderProtocol::Preview {
        None
    } else {
        state.credentials.get(&provider.id)?
    };
    let adapter = providers::adapter(provider, secret)?;
    let model = provider
        .models
        .iter()
        .find(|model| model.id == session.model_id)
        .ok_or_else(|| AppError::new("unknown_model", "This model is no longer configured."))?;
    let available: Vec<_> = if model.supports_tools {
        tools::definitions()
            .into_iter()
            .filter(|tool| {
                session.permission_policy.decision(&tool.category) != PermissionDecision::Deny
            })
            .collect()
    } else {
        vec![]
    };
    loop {
        while !session.queued_tool_calls.is_empty() {
            let call = session.queued_tool_calls.remove(0);
            if let Ok(tool) = tools::definition(&call.name) {
                if session.permission_policy.decision(&tool.category) == PermissionDecision::Ask {
                    session.pending_tool_call = Some(call);
                    session.status = SessionStatus::AwaitingPermission;
                    checkpoint(state, sink, session)?;
                    return Ok(());
                }
            }
            let result = tools::execute(
                Path::new(&project.path),
                &call,
                &session.permission_policy,
                false,
            )
            .await;
            append_result(session, result);
            checkpoint(state, sink, session)?;
        }
        if session.tool_rounds >= session.permission_policy.max_tool_rounds {
            return Err(AppError::new(
                "round_limit",
                "The tool-round limit was reached. Send a follow-up message to continue.",
            ));
        }
        let start = Instant::now();
        let response = adapter
            .complete(ProviderRequest {
                model_id: &session.model_id,
                messages: &session.messages,
                tools: &available,
            })
            .await?;
        session.tool_rounds += 1;
        let record = usage::record(
            &state.database,
            session,
            response.input_tokens,
            response.output_tokens,
            start.elapsed().as_millis() as u64,
        )?;
        sink.usage_updated(&record);
        let mut message = AgentMessage::text(MessageRole::Assistant, response.content);
        message.tool_calls = response.tool_calls.clone();
        message.provider_data = response.provider_data;
        session.messages.push(message);
        session.queued_tool_calls = response.tool_calls;
        if session.queued_tool_calls.is_empty() {
            session.status = SessionStatus::Completed;
            checkpoint(state, sink, session)?;
            return Ok(());
        }
        checkpoint(state, sink, session)?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::AppConfig, credentials::CredentialStore, persistence::Database, state::AppState,
        workspaces,
    };
    use std::sync::{Arc, Mutex};
    #[derive(Clone)]
    struct Sink(Arc<Mutex<Vec<AgentSession>>>);
    impl EventSink for Sink {
        fn session_updated(&self, session: &AgentSession) {
            self.0.lock().unwrap().push(session.clone());
        }
        fn usage_updated(&self, _: &UsageRecord) {}
    }
    #[tokio::test]
    async fn preview_runs_real_tools_and_persists_usage() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("README.md"), "A real project").unwrap();
        let state = Arc::new(AppState {
            database: Database::open(&root.path().join("app.sqlite")).unwrap(),
            config: AppConfig::load(root.path()).unwrap(),
            credentials: CredentialStore,
            runs: Mutex::new(Default::default()),
        });
        let project =
            workspaces::open_project(&state.database, root.path().to_str().unwrap()).unwrap();
        let session = AgentSession {
            id: id(),
            project_id: project.id,
            provider_id: "preview".into(),
            model_id: "workspace-explorer".into(),
            title: "Explore".into(),
            status: SessionStatus::Running,
            messages: vec![AgentMessage::text(
                MessageRole::User,
                "Explain the architecture",
            )],
            permission_policy: Default::default(),
            pending_tool_call: None,
            queued_tool_calls: vec![],
            created_at: now(),
            updated_at: now(),
            error: None,
            tool_rounds: 0,
        };
        state.database.save_session(&session).unwrap();
        let receiver = state.reserve_run(&session.id).unwrap();
        assert!(state.reserve_run(&session.id).is_err());
        let sink = Sink(Arc::new(Mutex::new(vec![])));
        run(
            state.clone(),
            Box::new(sink.clone()),
            session.clone(),
            receiver,
        )
        .await;
        let saved = state.database.session(&session.id).unwrap();
        assert_eq!(saved.status, SessionStatus::Completed);
        assert!(saved
            .messages
            .last()
            .unwrap()
            .content
            .contains("A real project"));
        assert_eq!(state.database.usage().unwrap().len(), 2);
        // A Git request pauses rather than silently executing an ask-policy tool.
        let mut git_session = session;
        git_session.id = id();
        git_session.messages = vec![AgentMessage::text(MessageRole::User, "Review Git status")];
        state.database.save_session(&git_session).unwrap();
        let receiver = state.reserve_run(&git_session.id).unwrap();
        run(state.clone(), Box::new(sink), git_session.clone(), receiver).await;
        assert_eq!(
            state.database.session(&git_session.id).unwrap().status,
            SessionStatus::AwaitingPermission
        );
        assert!(state
            .database
            .session(&git_session.id)
            .unwrap()
            .pending_tool_call
            .is_some());
    }
}
