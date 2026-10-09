use crate::{
    domain::*,
    error::{AppError, AppResult},
    providers::{self, LlmProvider, ProviderRequest, ProviderResponse},
    state::SharedState,
    tools, usage,
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tokio::{sync::watch, task::JoinSet};

pub trait EventSink: Send + Sync {
    fn session_updated(&self, session: &AgentSession);
    fn usage_updated(&self, record: &UsageRecord);
    fn stream_chunk(&self, _session_id: &str, _delta: &str, _reset: bool) {}
}

#[derive(Debug, Clone)]
pub struct AgentRuntimeConfig {
    pub max_iterations: u32,
    pub max_tool_calls: u32,
    pub max_retries: u32,
    pub request_timeout: Duration,
    pub tool_timeout: Duration,
}

impl Default for AgentRuntimeConfig {
    fn default() -> Self {
        Self {
            max_iterations: 32,
            max_tool_calls: 64,
            max_retries: 2,
            request_timeout: Duration::from_secs(120),
            tool_timeout: Duration::from_secs(30),
        }
    }
}

pub struct AgentRuntime {
    config: AgentRuntimeConfig,
}

struct RuntimeServices<'a> {
    state: &'a SharedState,
    sink: &'a dyn EventSink,
    project: &'a Project,
    model: &'a Model,
    protocol: &'a ProviderProtocol,
    provider: &'a dyn LlmProvider,
}

struct ModelTurn<'a> {
    model: &'a Model,
    protocol: &'a ProviderProtocol,
    provider: &'a dyn LlmProvider,
    messages: &'a [AgentMessage],
    tools: &'a [Tool],
}

impl Default for AgentRuntime {
    fn default() -> Self {
        Self::new(AgentRuntimeConfig::default())
    }
}

impl AgentRuntime {
    pub fn new(config: AgentRuntimeConfig) -> Self {
        Self { config }
    }

    async fn execute(
        &self,
        session: &mut AgentSession,
        services: RuntimeServices<'_>,
    ) -> AppResult<()> {
        let RuntimeServices {
            state,
            sink,
            project,
            model,
            protocol,
            provider,
        } = services;
        session.status = SessionStatus::Planning;
        record_activity(
            session,
            AgentActivityKind::Plan,
            "Inspect project context and use verified actions to complete the request.",
            None,
        );
        checkpoint(state, sink, session)?;

        let definitions = if model.supports_tools {
            tools::definitions()
                .into_iter()
                .filter(|tool| {
                    tool.category == ToolCategory::UserInteraction
                        || session.permission_policy.decision(&tool.category)
                            != PermissionDecision::Deny
                })
                .collect::<Vec<_>>()
        } else {
            vec![]
        };

        loop {
            while !session.queued_tool_calls.is_empty() {
                if self
                    .process_next_tools(state, sink, session, project)
                    .await?
                {
                    return Ok(());
                }
            }

            if session.iterations >= self.config.max_iterations {
                return Err(AppError::new(
                    "iteration_limit",
                    "The task reached its model-iteration limit. Send a follow-up to continue.",
                ));
            }
            if session.tool_rounds >= session.permission_policy.max_tool_rounds {
                return Err(AppError::new(
                    "round_limit",
                    "The tool-round limit was reached. Send a follow-up message to continue.",
                ));
            }

            let context = bounded_context(&session.messages, model.context_window)?;
            session.iterations += 1;
            session.status = SessionStatus::Working;
            record_activity(
                session,
                AgentActivityKind::Progress,
                format!("Requesting {}", model.display_name),
                None,
            );
            checkpoint(state, sink, session)?;

            let start = Instant::now();
            let response = self
                .request_with_retries(
                    session,
                    ModelTurn {
                        model,
                        protocol,
                        provider,
                        messages: &context,
                        tools: &definitions,
                    },
                    sink,
                )
                .await?;
            let record = usage::record(
                &state.database,
                session,
                response.input_tokens,
                response.output_tokens,
                start.elapsed().as_millis() as u64,
            )?;
            sink.usage_updated(&record);

            let has_calls = !response.tool_calls.is_empty();
            let mut message = AgentMessage::text(MessageRole::Assistant, response.content);
            message.tool_calls = response.tool_calls.clone();
            message.provider_data = response.provider_data;
            session.messages.push(message);
            session.queued_tool_calls = response.tool_calls;
            if has_calls {
                session.tool_rounds += 1;
                checkpoint(state, sink, session)?;
                continue;
            }

            session.status = SessionStatus::Completed;
            record_activity(
                session,
                AgentActivityKind::Progress,
                "Task completed.",
                None,
            );
            checkpoint(state, sink, session)?;
            return Ok(());
        }
    }

    async fn request_with_retries(
        &self,
        session: &AgentSession,
        turn: ModelTurn<'_>,
        sink: &dyn EventSink,
    ) -> AppResult<ProviderResponse> {
        let ModelTurn {
            model,
            protocol,
            provider,
            messages,
            tools,
        } = turn;
        let mut attempt = 0;
        loop {
            sink.stream_chunk(&session.id, "", true);
            let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
            let request = ProviderRequest {
                model_id: &model.id,
                protocol,
                messages,
                tools,
            };
            let call = provider.complete_stream(request, sender);
            tokio::pin!(call);
            let mut stream_open = true;
            let response = tokio::time::timeout(self.config.request_timeout, async {
                loop {
                    tokio::select! {
                        result = &mut call => break result,
                        delta = receiver.recv(), if stream_open => match delta {
                            Some(delta) => sink.stream_chunk(&session.id, &delta, false),
                            None => stream_open = false,
                        }
                    }
                }
            })
            .await
            .unwrap_or_else(|_| {
                Err(AppError::new(
                    "request_timeout",
                    "The model request timed out. Retry the task or choose another model.",
                ))
            });
            while let Ok(delta) = receiver.try_recv() {
                sink.stream_chunk(&session.id, &delta, false);
            }

            match response {
                Ok(response) => return Ok(response),
                Err(error)
                    if attempt < self.config.max_retries
                        && matches!(
                            error.code.as_str(),
                            "network_error" | "provider_outage" | "request_timeout"
                        ) =>
                {
                    attempt += 1;
                    tracing::warn!(session_id = %session.id, attempt, code = %error.code, "Retrying provider request");
                    tokio::time::sleep(Duration::from_millis(200 * (1 << (attempt - 1)))).await;
                }
                Err(error) => return Err(error),
            }
        }
    }

    async fn process_next_tools(
        &self,
        state: &SharedState,
        sink: &dyn EventSink,
        session: &mut AgentSession,
        project: &Project,
    ) -> AppResult<bool> {
        let call = session.queued_tool_calls.remove(0);
        session.tool_calls += 1;
        if session.tool_calls > self.config.max_tool_calls {
            return Err(AppError::new(
                "tool_call_limit",
                "The task reached its tool-call limit.",
            ));
        }
        let tool = match tools::validate_call(&call) {
            Ok(tool) => tool,
            Err(error) => {
                append_result(session, tool_error(&call, error.message));
                record_activity(
                    session,
                    AgentActivityKind::ToolCompleted,
                    "Rejected an invalid tool request.",
                    Some(call.id),
                );
                checkpoint(state, sink, session)?;
                return Ok(false);
            }
        };

        if call.name == "ask_user" {
            let question = call.arguments["question"].as_str().ok_or_else(|| {
                AppError::new("invalid_tool_arguments", "The user question is invalid.")
            })?;
            session.pending_user_input = Some(call.clone());
            session.status = SessionStatus::WaitingForUser;
            record_activity(
                session,
                AgentActivityKind::Progress,
                format!("Waiting for your answer: {question}"),
                Some(call.id),
            );
            checkpoint(state, sink, session)?;
            return Ok(true);
        }

        if session.permission_policy.decision(&tool.category) == PermissionDecision::Ask {
            session.pending_tool_call = Some(call.clone());
            session.status = SessionStatus::WaitingForPermission;
            record_activity(
                session,
                AgentActivityKind::Progress,
                format!("Waiting for approval to use {}.", call.name),
                Some(call.id),
            );
            checkpoint(state, sink, session)?;
            return Ok(true);
        }

        let mut batch = vec![(call, tool)];
        if batch[0].1.parallel_safe
            && session.permission_policy.decision(&batch[0].1.category) == PermissionDecision::Allow
        {
            while let Some(next) = session.queued_tool_calls.first() {
                let Ok(next_tool) = tools::validate_call(next) else {
                    break;
                };
                if !next_tool.parallel_safe
                    || next.name == "ask_user"
                    || session.permission_policy.decision(&next_tool.category)
                        != PermissionDecision::Allow
                {
                    break;
                }
                let next = session.queued_tool_calls.remove(0);
                session.tool_calls += 1;
                if session.tool_calls > self.config.max_tool_calls {
                    session.queued_tool_calls.insert(0, next);
                    return Err(AppError::new(
                        "tool_call_limit",
                        "The task reached its tool-call limit.",
                    ));
                }
                batch.push((next, next_tool));
            }
        }

        for (call, tool) in &batch {
            record_activity(
                session,
                AgentActivityKind::ToolStarted,
                tool_summary(call),
                Some(call.id.clone()),
            );
            if call.name == "read_file" {
                record_activity(
                    session,
                    AgentActivityKind::FileInspected,
                    tool_summary(call),
                    Some(call.id.clone()),
                );
            }
            let _ = tool;
        }
        session.status = SessionStatus::Working;
        checkpoint(state, sink, session)?;

        let results = execute_batch(
            PathBuf::from(&project.path),
            batch.clone(),
            session.permission_policy.clone(),
            self.config.tool_timeout,
        )
        .await;
        for ((call, _), result) in batch.into_iter().zip(results) {
            record_activity(
                session,
                AgentActivityKind::ToolCompleted,
                if result.is_error {
                    format!("{} failed.", call.name)
                } else {
                    format!("{} completed.", call.name)
                },
                Some(call.id),
            );
            append_result(session, result);
            checkpoint(state, sink, session)?;
        }
        Ok(false)
    }
}

async fn execute_batch(
    root: PathBuf,
    batch: Vec<(ToolCall, Tool)>,
    policy: PermissionPolicy,
    timeout: Duration,
) -> Vec<ToolResult> {
    if batch.len() == 1 && !batch[0].1.parallel_safe {
        return vec![execute_one(&root, &batch[0].0, &policy, timeout).await];
    }
    let mut tasks = JoinSet::new();
    for (index, (call, _)) in batch.iter().enumerate() {
        let root = root.clone();
        let call = call.clone();
        let policy = policy.clone();
        tasks.spawn(async move {
            let result = execute_one(&root, &call, &policy, timeout).await;
            (index, result)
        });
    }
    let mut results = BTreeMap::new();
    while let Some(result) = tasks.join_next().await {
        if let Ok((index, result)) = result {
            results.insert(index, result);
        }
    }
    batch
        .iter()
        .enumerate()
        .map(|(index, (call, _))| {
            results
                .remove(&index)
                .unwrap_or_else(|| tool_error(call, "The tool task could not be joined."))
        })
        .collect()
}

async fn execute_one(
    root: &Path,
    call: &ToolCall,
    policy: &PermissionPolicy,
    timeout: Duration,
) -> ToolResult {
    match tokio::time::timeout(timeout, tools::execute(root, call, policy, false)).await {
        Ok(result) => result,
        Err(_) => tool_error(call, "The tool execution timed out."),
    }
}

fn tool_error(call: &ToolCall, message: impl Into<String>) -> ToolResult {
    ToolResult {
        tool_call_id: call.id.clone(),
        name: call.name.clone(),
        content: message.into(),
        is_error: true,
        duration_ms: 0,
    }
}

fn tool_summary(call: &ToolCall) -> String {
    let path = call.arguments["path"].as_str();
    match (call.name.as_str(), path) {
        ("read_file", Some(path)) => format!("Read {path}"),
        ("list_files", Some(path)) => format!("Listed {path}"),
        ("git_status", _) => "Checked Git status".into(),
        (name, _) => format!("Ran {name}"),
    }
}

fn record_activity(
    session: &mut AgentSession,
    kind: AgentActivityKind,
    summary: impl Into<String>,
    tool_call_id: Option<String>,
) {
    let summary: String = summary.into().chars().take(500).collect();
    session.activity_events.push(AgentActivityEvent {
        id: id(),
        session_id: session.id.clone(),
        kind,
        summary,
        tool_call_id,
        created_at: now(),
    });
    if session.activity_events.len() > 250 {
        let excess = session.activity_events.len() - 250;
        session.activity_events.drain(0..excess);
    }
}

/// Fit whole conversation turns to the model's input budget. System context and
/// the newest user turn are kept together; old turns are removed atomically.
pub fn bounded_context(
    messages: &[AgentMessage],
    context_window: Option<u64>,
) -> AppResult<Vec<AgentMessage>> {
    const DEFAULT_CONTEXT: u64 = 32_768;
    let context = context_window.unwrap_or(DEFAULT_CONTEXT).max(256);
    let output_reserve = (context / 8).max(512).min(context / 2);
    let budget = context.saturating_sub(output_reserve).max(128);
    let systems: Vec<_> = messages
        .iter()
        .filter(|message| message.role == MessageRole::System)
        .cloned()
        .collect();
    let system_cost: usize = systems.iter().map(estimate_message_tokens).sum();
    if system_cost as u64 > budget {
        return Err(AppError::new(
            "context_limit",
            "Project instructions exceed this model's context limit. Shorten the instructions or choose a larger-context model.",
        ));
    }

    let mut turns: Vec<Vec<AgentMessage>> = Vec::new();
    for message in messages
        .iter()
        .filter(|message| message.role != MessageRole::System)
    {
        if message.role == MessageRole::User || turns.is_empty() {
            turns.push(Vec::new());
        }
        if let Some(turn) = turns.last_mut() {
            turn.push(message.clone());
        }
    }
    let mut selected = Vec::new();
    let mut used = system_cost as u64;
    for turn in turns.into_iter().rev() {
        let cost: u64 = turn.iter().map(estimate_message_tokens).sum::<usize>() as u64;
        if used + cost > budget {
            if selected.is_empty() {
                return Err(AppError::new(
                    "context_limit",
                    "The current task is larger than this model's context limit. Start a new task or choose a larger-context model.",
                ));
            }
            break;
        }
        used += cost;
        selected.push(turn);
    }
    selected.reverse();
    let mut context = systems;
    context.extend(selected.into_iter().flatten());
    Ok(context)
}

fn estimate_message_tokens(message: &AgentMessage) -> usize {
    let mut chars = message.content.chars().count();
    chars += message
        .provider_data
        .as_ref()
        .and_then(|value| serde_json::to_string(value).ok())
        .map_or(0, |value| value.len());
    chars += message
        .tool_calls
        .iter()
        .map(|call| call.name.len() + call.arguments.to_string().len())
        .sum::<usize>();
    chars / 4 + 8
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

/// Close outstanding tool calls before failure/cancellation; providers need one
/// result per call. Private chain-of-thought is never added to this history.
pub fn settle_pending(session: &mut AgentSession, reason: &str) {
    session.close_pending_tools(reason);
}

pub fn accept_user_input(session: &mut AgentSession, content: &str) -> AppResult<()> {
    if session.status != SessionStatus::WaitingForUser {
        return Err(AppError::new(
            "invalid_state",
            "This task is not waiting for user input.",
        ));
    }
    let call = session
        .pending_user_input
        .take()
        .ok_or_else(|| AppError::new("invalid_state", "This task has no pending user question."))?;
    append_result(
        session,
        ToolResult {
            tool_call_id: call.id,
            name: call.name,
            content: content.to_owned(),
            is_error: false,
            duration_ms: 0,
        },
    );
    session
        .messages
        .push(AgentMessage::text(MessageRole::User, content));
    session.status = SessionStatus::Queued;
    Ok(())
}

pub async fn run(
    state: SharedState,
    sink: Box<dyn EventSink>,
    session: AgentSession,
    cancel: watch::Receiver<bool>,
) {
    run_inner(state, sink, session, cancel, None, AgentRuntime::default()).await;
}

async fn run_inner(
    state: SharedState,
    sink: Box<dyn EventSink>,
    mut session: AgentSession,
    mut cancel: watch::Receiver<bool>,
    provider_override: Option<Box<dyn LlmProvider>>,
    runtime: AgentRuntime,
) {
    let session_id = session.id.clone();
    let result = tokio::select! {
        biased;
        _ = cancel.changed() => Err(AppError::new("cancelled", "Run stopped by you.")),
        result = drive(&state, sink.as_ref(), &mut session, provider_override, &runtime) => result,
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
        record_activity(
            &mut session,
            AgentActivityKind::Progress,
            if error.code == "cancelled" {
                "Task cancelled."
            } else {
                "Task failed."
            },
            None,
        );
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
    provider_override: Option<Box<dyn LlmProvider>>,
    runtime: &AgentRuntime,
) -> AppResult<()> {
    let project = state.database.project(&session.project_id)?;
    let provider_config = state.config.provider(&session.provider_id)?;
    let models = state
        .database
        .model_catalog(&provider_config.id)?
        .filter(|models| !models.is_empty())
        .unwrap_or_else(|| provider_config.models.clone());
    let model = models
        .into_iter()
        .find(|model| model.id == session.model_id && model.status != ModelStatus::Unavailable)
        .ok_or_else(|| {
            AppError::new(
                "unknown_model",
                "This model is no longer available. Choose another model from the picker.",
            )
        })?;
    let secret = if provider_config.protocol == ProviderProtocol::Preview {
        None
    } else {
        state.credentials.get(&provider_config.id)?
    };
    let adapter = match provider_override {
        Some(provider) => provider,
        None => providers::adapter(provider_config, secret)?,
    };
    runtime
        .execute(
            session,
            RuntimeServices {
                state,
                sink,
                project: &project,
                protocol: model
                    .api_protocol
                    .as_ref()
                    .unwrap_or(&provider_config.protocol),
                model: &model,
                provider: adapter.as_ref(),
            },
        )
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::AppConfig, credentials::CredentialStore, persistence::Database, state::AppState,
        workspaces,
    };
    use serde_json::json;
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
    };
    use tokio::sync::Notify;

    #[derive(Clone, Default)]
    struct Sink {
        sessions: Arc<Mutex<Vec<AgentSession>>>,
        deltas: Arc<Mutex<Vec<String>>>,
    }
    impl EventSink for Sink {
        fn session_updated(&self, session: &AgentSession) {
            self.sessions.lock().unwrap().push(session.clone());
        }
        fn usage_updated(&self, _: &UsageRecord) {}
        fn stream_chunk(&self, _: &str, delta: &str, reset: bool) {
            if reset {
                self.deltas.lock().unwrap().clear();
            } else {
                self.deltas.lock().unwrap().push(delta.into());
            }
        }
    }

    struct MockProvider {
        responses: Mutex<VecDeque<ProviderResponse>>,
        calls: Mutex<u32>,
        failures_left: Mutex<u32>,
        blocked: bool,
        started: Arc<Notify>,
    }
    impl MockProvider {
        fn new(responses: Vec<ProviderResponse>) -> Self {
            Self {
                responses: Mutex::new(responses.into()),
                calls: Mutex::new(0),
                failures_left: Mutex::new(0),
                blocked: false,
                started: Arc::new(Notify::new()),
            }
        }
        fn blocking() -> Self {
            Self {
                blocked: true,
                ..Self::new(vec![])
            }
        }
        fn failing_once(responses: Vec<ProviderResponse>) -> Self {
            let provider = Self::new(responses);
            *provider.failures_left.lock().unwrap() = 1;
            provider
        }
    }
    #[async_trait::async_trait]
    impl LlmProvider for MockProvider {
        async fn complete(&self, _: ProviderRequest<'_>) -> AppResult<ProviderResponse> {
            *self.calls.lock().unwrap() += 1;
            if self.blocked {
                self.started.notify_one();
                std::future::pending().await
            } else if *self.failures_left.lock().unwrap() > 0 {
                *self.failures_left.lock().unwrap() -= 1;
                Err(AppError::new("network_error", "Mock network interruption."))
            } else {
                self.responses.lock().unwrap().pop_front().ok_or_else(|| {
                    AppError::new("mock_exhausted", "The mock response queue is empty.")
                })
            }
        }

        async fn complete_stream(
            &self,
            request: ProviderRequest<'_>,
            deltas: tokio::sync::mpsc::UnboundedSender<String>,
        ) -> AppResult<ProviderResponse> {
            let response = self.complete(request).await?;
            for chunk in response.content.as_bytes().chunks(5) {
                if let Ok(chunk) = std::str::from_utf8(chunk) {
                    let _ = deltas.send(chunk.to_owned());
                }
            }
            Ok(response)
        }
    }

    fn response(content: &str, tool_calls: Vec<ToolCall>) -> ProviderResponse {
        ProviderResponse {
            content: content.into(),
            tool_calls,
            provider_data: None,
            input_tokens: 12,
            output_tokens: 7,
        }
    }

    fn setup_session(root: &Path) -> (SharedState, Project, AgentSession) {
        let database = Database::open(&root.join("app.sqlite")).unwrap();
        let config = AppConfig::load(root).unwrap();
        let state = Arc::new(AppState {
            database,
            config,
            credentials: CredentialStore,
            runs: std::sync::Mutex::new(Default::default()),
        });
        let project = workspaces::open_project(&state.database, root.to_str().unwrap()).unwrap();
        let session = AgentSession {
            id: id(),
            project_id: project.id.clone(),
            provider_id: "preview".into(),
            model_id: "workspace-explorer".into(),
            title: "Mock runtime test".into(),
            status: SessionStatus::Queued,
            messages: vec![AgentMessage::text(MessageRole::User, "Inspect README")],
            permission_policy: PermissionPolicy {
                git: PermissionDecision::Allow,
                ..Default::default()
            },
            pending_tool_call: None,
            pending_user_input: None,
            queued_tool_calls: vec![],
            iterations: 0,
            tool_calls: 0,
            activity_events: vec![],
            created_at: now(),
            updated_at: now(),
            error: None,
            tool_rounds: 0,
        };
        state.database.save_session(&session).unwrap();
        (state, project, session)
    }

    fn tool_call(id: &str, name: &str, arguments: serde_json::Value) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: name.into(),
            arguments,
        }
    }

    #[tokio::test]
    async fn mock_provider_streams_and_runs_parallel_tools_to_completion() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("README.md"), "A mock project").unwrap();
        let (state, _, session) = setup_session(root.path());
        let receiver = state.reserve_run(&session.id).unwrap();
        let sink = Sink::default();
        let mock = MockProvider::new(vec![
            response(
                "I will inspect the project.",
                vec![
                    tool_call("list", "list_files", json!({"path":"."})),
                    tool_call("read", "read_file", json!({"path":"README.md"})),
                ],
            ),
            response("The README says this is A mock project.", vec![]),
        ]);
        run_inner(
            state.clone(),
            Box::new(sink.clone()),
            session.clone(),
            receiver,
            Some(Box::new(mock)),
            AgentRuntime::default(),
        )
        .await;

        let saved = state.database.session(&session.id).unwrap();
        assert_eq!(saved.status, SessionStatus::Completed);
        assert_eq!(saved.iterations, 2);
        assert_eq!(saved.tool_calls, 2);
        assert!(saved.messages.iter().any(|message| {
            message
                .tool_result
                .as_ref()
                .is_some_and(|result| result.content == "A mock project")
        }));
        assert!(saved
            .activity_events
            .iter()
            .any(|event| event.kind == AgentActivityKind::FileInspected));
        assert_eq!(state.database.usage().unwrap().len(), 2);
        assert!(!sink.deltas.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn runtime_pauses_for_user_input_and_resumes_same_tool_call() {
        let root = tempfile::tempdir().unwrap();
        let (state, _, session) = setup_session(root.path());
        let receiver = state.reserve_run(&session.id).unwrap();
        let sink = Sink::default();
        let mock = MockProvider::new(vec![response(
            "I need one detail.",
            vec![tool_call(
                "question",
                "ask_user",
                json!({"question":"Which module?"}),
            )],
        )]);
        run_inner(
            state.clone(),
            Box::new(sink.clone()),
            session.clone(),
            receiver,
            Some(Box::new(mock)),
            AgentRuntime::default(),
        )
        .await;
        let mut paused = state.database.session(&session.id).unwrap();
        assert_eq!(paused.status, SessionStatus::WaitingForUser);
        assert_eq!(
            paused.pending_user_input.as_ref().unwrap().arguments["question"],
            "Which module?"
        );
        accept_user_input(&mut paused, "src/agent").unwrap();
        state.database.save_session(&paused).unwrap();

        let receiver = state.reserve_run(&session.id).unwrap();
        run_inner(
            state.clone(),
            Box::new(sink),
            paused.clone(),
            receiver,
            Some(Box::new(MockProvider::new(vec![response(
                "I will inspect src/agent.",
                vec![],
            )]))),
            AgentRuntime::default(),
        )
        .await;
        let resumed = state.database.session(&session.id).unwrap();
        assert_eq!(resumed.status, SessionStatus::Completed);
        let tool_result_index = resumed
            .messages
            .iter()
            .position(|message| {
                message.tool_result.as_ref().is_some_and(|result| {
                    result.name == "ask_user" && result.content == "src/agent"
                })
            })
            .unwrap();
        let answer_index = resumed
            .messages
            .iter()
            .position(|message| message.role == MessageRole::User && message.content == "src/agent")
            .unwrap();
        assert!(tool_result_index < answer_index);
    }

    #[tokio::test]
    async fn runtime_waits_for_permission_before_running_a_tool() {
        let root = tempfile::tempdir().unwrap();
        let (state, _, mut session) = setup_session(root.path());
        session.permission_policy.git = PermissionDecision::Ask;
        state.database.save_session(&session).unwrap();
        let receiver = state.reserve_run(&session.id).unwrap();
        run_inner(
            state.clone(),
            Box::new(Sink::default()),
            session.clone(),
            receiver,
            Some(Box::new(MockProvider::new(vec![response(
                "I will check the repository state.",
                vec![tool_call("status", "git_status", json!({}))],
            )]))),
            AgentRuntime::default(),
        )
        .await;

        let paused = state.database.session(&session.id).unwrap();
        assert_eq!(paused.status, SessionStatus::WaitingForPermission);
        assert_eq!(paused.pending_tool_call.unwrap().name, "git_status");
        assert!(!paused
            .messages
            .iter()
            .any(|message| message.tool_result.is_some()));
    }

    #[tokio::test]
    async fn runtime_enforces_tool_call_and_iteration_limits() {
        let root = tempfile::tempdir().unwrap();
        let (state, _, mut session) = setup_session(root.path());
        session.id = id();
        session.messages[0].content = "Use a tool".into();
        state.database.save_session(&session).unwrap();
        let receiver = state.reserve_run(&session.id).unwrap();
        run_inner(
            state.clone(),
            Box::new(Sink::default()),
            session.clone(),
            receiver,
            Some(Box::new(MockProvider::new(vec![response(
                "Inspecting.",
                vec![tool_call("list", "list_files", json!({"path":"."}))],
            )]))),
            AgentRuntime::new(AgentRuntimeConfig {
                max_tool_calls: 0,
                ..AgentRuntimeConfig::default()
            }),
        )
        .await;
        let stopped = state.database.session(&session.id).unwrap();
        assert_eq!(stopped.status, SessionStatus::Failed);
        assert!(stopped.error.unwrap().contains("tool-call limit"));

        session.id = id();
        session.messages[0].content = "Use one tool, then finish".into();
        state.database.save_session(&session).unwrap();
        let receiver = state.reserve_run(&session.id).unwrap();
        run_inner(
            state.clone(),
            Box::new(Sink::default()),
            session.clone(),
            receiver,
            Some(Box::new(MockProvider::new(vec![response(
                "Inspecting.",
                vec![tool_call("list", "list_files", json!({"path":"."}))],
            )]))),
            AgentRuntime::new(AgentRuntimeConfig {
                max_iterations: 1,
                ..AgentRuntimeConfig::default()
            }),
        )
        .await;
        let stopped = state.database.session(&session.id).unwrap();
        assert_eq!(stopped.status, SessionStatus::Failed);
        assert!(stopped.error.unwrap().contains("iteration limit"));
    }

    #[tokio::test]
    async fn runtime_retries_transient_provider_failures() {
        let root = tempfile::tempdir().unwrap();
        let (state, _, session) = setup_session(root.path());
        let receiver = state.reserve_run(&session.id).unwrap();
        let provider = MockProvider::failing_once(vec![response("Recovered after retry.", vec![])]);
        run_inner(
            state.clone(),
            Box::new(Sink::default()),
            session.clone(),
            receiver,
            Some(Box::new(provider)),
            AgentRuntime::new(AgentRuntimeConfig {
                max_retries: 1,
                ..AgentRuntimeConfig::default()
            }),
        )
        .await;
        let saved = state.database.session(&session.id).unwrap();
        assert_eq!(saved.status, SessionStatus::Completed);
        assert!(saved
            .messages
            .iter()
            .any(|message| message.content == "Recovered after retry."));
        assert_eq!(state.database.usage().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn cancellation_drops_an_inflight_provider_request() {
        let root = tempfile::tempdir().unwrap();
        let (state, _, session) = setup_session(root.path());
        let receiver = state.reserve_run(&session.id).unwrap();
        let sink = Sink::default();
        let mock = MockProvider::blocking();
        let started = mock.started.clone();
        let task = tokio::spawn(run_inner(
            state.clone(),
            Box::new(sink),
            session.clone(),
            receiver,
            Some(Box::new(mock)),
            AgentRuntime::default(),
        ));
        started.notified().await;
        {
            let runs = state.runs.lock().unwrap();
            runs.get(&session.id).unwrap().send(true).unwrap();
        }
        task.await.unwrap();
        assert_eq!(
            state.database.session(&session.id).unwrap().status,
            SessionStatus::Cancelled
        );
    }

    #[test]
    fn context_budget_keeps_system_and_latest_complete_turn() {
        let mut messages = vec![AgentMessage::text(MessageRole::System, "instructions")];
        messages.extend([
            AgentMessage::text(MessageRole::User, "old request ".repeat(400)),
            AgentMessage::text(MessageRole::Assistant, "old answer ".repeat(400)),
            AgentMessage::text(MessageRole::User, "current"),
        ]);
        let context = bounded_context(&messages, Some(256)).unwrap();
        assert_eq!(context[0].role, MessageRole::System);
        assert!(context.iter().any(|message| message.content == "current"));
        assert!(!context
            .iter()
            .any(|message| message.content.starts_with("old request")));
    }
}
