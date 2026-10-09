use crate::{
    domain::*,
    error::{AppError, AppResult},
    permissions,
    providers::{self, LlmProvider, ProviderRequest, ProviderResponse},
    review,
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
    fn tool_output(&self, _session_id: &str, _tool_call_id: &str, _stream: &str, _chunk: &str) {}
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
    cancel: &'a mut watch::Receiver<bool>,
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

#[derive(Clone)]
struct ToolExecutionContext {
    policy: PermissionPolicy,
    timeout: Duration,
    session_id: String,
    project_id: String,
    extensions: crate::extensions::ExtensionRegistry,
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
            cancel,
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
            let mut available = tools::definitions();
            available.extend(state.extensions.definitions(&project.id).await?);
            available
                .into_iter()
                .filter(|tool| {
                    tool.permission == ToolCategory::UserInteraction
                        || session.permission_policy.decision(&tool.permission)
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

            let (prepared_messages, compacted) = prepare_context(session);
            if compacted {
                record_activity(
                    session,
                    AgentActivityKind::ContextCompacted,
                    format!("Compacted {} earlier task turns; original requests remain available in task context.", session.working_context.compacted_turns),
                    None,
                );
                checkpoint(state, sink, session)?;
            }
            let context = bounded_context(&prepared_messages, model.context_window)?;
            session.iterations += 1;
            session.status = SessionStatus::Working;
            record_activity(
                session,
                AgentActivityKind::Progress,
                format!("Requesting {}", model.display_name),
                None,
            );
            checkpoint(state, sink, session)?;

            let response = self
                .request_with_retries(
                    &state.database,
                    cancel,
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
            session.working_context.outstanding_tasks.clear();
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
        database: &crate::persistence::Database,
        cancel: &mut watch::Receiver<bool>,
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
            if *cancel.borrow() {
                return Err(AppError::new("cancelled", "Run stopped by you."));
            }
            let attempt_started = Instant::now();
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
                        },
                        _ = cancel.changed() => break Err(AppError::new("cancelled", "Run stopped by you.")),
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
                Ok(response) => {
                    let record = usage::record(
                        database,
                        session,
                        model,
                        usage::RequestMetrics {
                            input_tokens: response.input_tokens,
                            usage_available: response.usage_available,
                            cached_input_tokens: response.cached_input_tokens,
                            output_tokens: response.output_tokens,
                            duration_ms: attempt_started.elapsed().as_millis() as u64,
                            failure_code: None,
                        },
                    )?;
                    sink.usage_updated(&record);
                    return Ok(response);
                }
                Err(error)
                    if attempt < self.config.max_retries
                        && matches!(
                            error.code.as_str(),
                            "network_error" | "provider_outage" | "request_timeout"
                        ) =>
                {
                    let record = usage::record(
                        database,
                        session,
                        model,
                        usage::RequestMetrics {
                            input_tokens: 0,
                            usage_available: false,
                            cached_input_tokens: None,
                            output_tokens: 0,
                            duration_ms: attempt_started.elapsed().as_millis() as u64,
                            failure_code: Some(error.code.clone()),
                        },
                    )?;
                    sink.usage_updated(&record);
                    attempt += 1;
                    tracing::warn!(session_id = %session.id, attempt, code = %error.code, "Retrying provider request");
                    tokio::time::sleep(Duration::from_millis(200 * (1 << (attempt - 1)))).await;
                }
                Err(error) => {
                    let record = usage::record(
                        database,
                        session,
                        model,
                        usage::RequestMetrics {
                            input_tokens: 0,
                            usage_available: false,
                            cached_input_tokens: None,
                            output_tokens: 0,
                            duration_ms: attempt_started.elapsed().as_millis() as u64,
                            failure_code: Some(error.code.clone()),
                        },
                    )?;
                    sink.usage_updated(&record);
                    return Err(error);
                }
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
        let tool = match runtime_tool(state, &project.id, &call).await {
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

        let assessment = permissions::assess(
            Path::new(&project.path),
            &call,
            &tool.permission,
            &session.permission_policy,
        )?;
        if let Some(reason) = assessment.denied.as_ref().cloned() {
            append_result(session, tool_error(&call, reason));
            record_activity(
                session,
                AgentActivityKind::ToolCompleted,
                format!("{} was denied by the permission policy.", call.name),
                Some(call.id),
            );
            checkpoint(state, sink, session)?;
            return Ok(false);
        }
        let is_dangerous = assessment
            .request
            .categories
            .contains(&PermissionCategory::Dangerous);
        let one_time_index = session
            .one_time_permission_grants
            .iter()
            .position(|grant| grant == &assessment.fingerprint);
        let has_session_grant = !is_dangerous
            && session
                .session_permission_grants
                .contains(&assessment.fingerprint);
        let has_project_grant = if !is_dangerous {
            state
                .database
                .has_permission_rule(&session.project_id, &assessment.fingerprint)?
        } else {
            false
        };
        if !assessment.automatic
            && one_time_index.is_none()
            && !has_session_grant
            && !has_project_grant
        {
            session.pending_tool_call = Some(call.clone());
            session.pending_permission = Some(assessment.request);
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
        if let Some(index) = one_time_index {
            session.one_time_permission_grants.remove(index);
        }

        let mut batch = vec![(call, tool)];
        if batch[0].1.parallel_safe
            && session.permission_policy.decision(&batch[0].1.permission)
                == PermissionDecision::Allow
        {
            while let Some(next) = session.queued_tool_calls.first() {
                let Ok(next_tool) = runtime_tool(state, &project.id, next).await else {
                    break;
                };
                let external =
                    tools::requires_external_access(Path::new(&project.path), next).unwrap_or(true);
                if !next_tool.parallel_safe
                    || next.name == "ask_user"
                    || session.permission_policy.decision(&next_tool.permission)
                        != PermissionDecision::Allow
                    || (external
                        && session.permission_policy.external_files != PermissionDecision::Allow)
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

        let mut command_snapshot = None;
        let mut preparation_error = None;
        if let Some((call, _)) = batch.first() {
            let result = match call.name.as_str() {
                "run_command" => review::command_snapshot_before(
                    &state.database,
                    &session.id,
                    Path::new(&project.path),
                )
                .await
                .map(|snapshot| command_snapshot = Some(snapshot)),
                "apply_patch" | "create_file" | "delete_file" | "move_file" => {
                    review::checkpoint_before(
                        &state.database,
                        &session.id,
                        Path::new(&project.path),
                        call,
                    )
                    .await
                }
                _ => Ok(()),
            };
            preparation_error = result.err();
        }

        for (call, tool) in &batch {
            record_activity(
                session,
                AgentActivityKind::ToolStarted,
                tool_summary(call),
                Some(call.id.clone()),
            );
            let _ = tool;
        }
        session.status = SessionStatus::Working;
        checkpoint(state, sink, session)?;

        let has_preparation_error = preparation_error.is_some();
        let mut results = if let Some(error) = preparation_error {
            vec![tool_error(&batch[0].0, error.message)]
        } else {
            execute_batch(
                PathBuf::from(&project.path),
                batch.clone(),
                ToolExecutionContext {
                    policy: session.permission_policy.clone(),
                    timeout: self.config.tool_timeout,
                    session_id: session.id.clone(),
                    project_id: project.id.clone(),
                    extensions: state.extensions.clone(),
                },
                sink,
            )
            .await
        };
        if let Some(snapshot) = command_snapshot {
            if let Err(error) =
                review::command_snapshot_after(&state.database, &session.id, snapshot)
            {
                if let Some(result) = results.first_mut() {
                    result.is_error = true;
                    result.content.push_str("\nThe command ran, but its file changes could not be checkpointed safely. Inspect the working tree before continuing.");
                    result.structured_content = Some(
                        serde_json::json!({"error":error.code,"message":error.message,"changesUntracked":true}),
                    );
                }
            }
        } else if !has_preparation_error {
            if let Some((call, _)) = batch.first() {
                if matches!(
                    call.name.as_str(),
                    "apply_patch" | "create_file" | "delete_file" | "move_file"
                ) && results.first().is_some_and(|result| !result.is_error)
                {
                    if let Err(error) = review::checkpoint_after(
                        &state.database,
                        &session.id,
                        Path::new(&project.path),
                        call,
                    ) {
                        if let Some(result) = results.first_mut() {
                            result.is_error = true;
                            result.content.push_str("\nThe file changed, but its review checkpoint could not be updated. Inspect it before continuing.");
                            result.structured_content = Some(
                                serde_json::json!({"error":error.code,"message":error.message,"changesUntracked":true}),
                            );
                        }
                    }
                }
            }
        }
        for ((call, _), result) in batch.into_iter().zip(results) {
            if !result.is_error {
                if let Some(kind) = completed_tool_activity(&call) {
                    record_activity(session, kind, tool_summary(&call), Some(call.id.clone()));
                }
            }
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
    context: ToolExecutionContext,
    sink: &dyn EventSink,
) -> Vec<ToolResult> {
    if batch.len() == 1 && !batch[0].1.parallel_safe {
        return vec![execute_one(&root, &batch[0].0, &context, sink).await];
    }
    let mut tasks = JoinSet::new();
    for (index, (call, _)) in batch.iter().enumerate() {
        let root = root.clone();
        let call = call.clone();
        let context = context.clone();
        tasks.spawn(async move {
            let result = execute(&root, &call, &context).await;
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

async fn execute(root: &Path, call: &ToolCall, context: &ToolExecutionContext) -> ToolResult {
    let operation = async {
        if let Some(result) = context.extensions.execute(&context.project_id, call).await {
            result
        } else {
            tools::execute(root, call, &context.policy, true).await
        }
    };
    match tokio::time::timeout(context.timeout, operation).await {
        Ok(result) => result,
        Err(_) => tool_error(call, "The tool execution timed out."),
    }
}

async fn execute_one(
    root: &Path,
    call: &ToolCall,
    context: &ToolExecutionContext,
    sink: &dyn EventSink,
) -> ToolResult {
    if context.extensions.owns(&call.name) {
        return match tokio::time::timeout(
            context.timeout,
            context.extensions.execute(&context.project_id, call),
        )
        .await
        {
            Ok(Some(result)) => result,
            Ok(None) => tool_error(call, "The MCP tool disconnected before execution."),
            Err(_) => tool_error(call, "The MCP tool execution timed out."),
        };
    }
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let execute = tools::execute_with_output(root, call, &context.policy, true, sender);
    tokio::pin!(execute);
    let mut stream_open = true;
    let run = async {
        loop {
            tokio::select! {
                result = &mut execute => break result,
                event = receiver.recv(), if stream_open => match event {
                    Some(event) => sink.tool_output(&context.session_id, &call.id, &event.stream, &event.chunk),
                    None => stream_open = false,
                }
            }
        }
    };
    match tokio::time::timeout(context.timeout, run).await {
        Ok(result) => result,
        Err(_) => tool_error(call, "The tool execution timed out."),
    }
}

async fn runtime_tool(state: &SharedState, project_id: &str, call: &ToolCall) -> AppResult<Tool> {
    if !state.extensions.owns(&call.name) {
        return tools::validate_call(call);
    }
    let tool = state
        .extensions
        .definitions(project_id)
        .await?
        .into_iter()
        .find(|tool| tool.name == call.name)
        .ok_or_else(|| {
            AppError::new(
                "unknown_tool",
                "The requested MCP tool is disconnected or unavailable.",
            )
        })?;
    tools::validate_external_schema(&call.arguments, &tool.input_schema)?;
    Ok(tool)
}

fn tool_error(call: &ToolCall, message: impl Into<String>) -> ToolResult {
    ToolResult {
        tool_call_id: call.id.clone(),
        name: call.name.clone(),
        content: message.into(),
        is_error: true,
        duration_ms: 0,
        structured_content: None,
    }
}

fn tool_summary(call: &ToolCall) -> String {
    let path = call.arguments["path"].as_str();
    match (call.name.as_str(), path) {
        ("read_file", Some(path)) => format!("Read {path}"),
        ("list_directory", Some(path)) => format!("Listed {path}"),
        ("git_status", _) => "Checked Git status".into(),
        (name, _) => format!("Ran {name}"),
    }
}

fn completed_tool_activity(call: &ToolCall) -> Option<AgentActivityKind> {
    if call.name.starts_with("mcp__") {
        return Some(AgentActivityKind::CommandExecuted);
    }
    match call.name.as_str() {
        "read_file" | "read_files" | "list_directory" | "file_metadata" => {
            Some(AgentActivityKind::FileInspected)
        }
        "search_files" | "search_text" | "find_symbol" | "find_references" => {
            Some(AgentActivityKind::SearchPerformed)
        }
        "run_command" => Some(
            if call.arguments["args"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|argument| argument.as_str() == Some("test"))
            {
                AgentActivityKind::TestRun
            } else {
                AgentActivityKind::CommandExecuted
            },
        ),
        "apply_patch" | "create_file" | "delete_file" | "move_file" => {
            Some(AgentActivityKind::FileEdited)
        }
        _ => None,
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

/// Build a compact provider view while keeping the complete local transcript.
/// User requests remain verbatim in the structured context; only older visible
/// assistant and tool output is reduced to concise progress and repository facts.
fn prepare_context(session: &mut AgentSession) -> (Vec<AgentMessage>, bool) {
    const RECENT_TURNS: usize = 8;
    let mut turns: Vec<Vec<AgentMessage>> = Vec::new();
    for message in session
        .messages
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
    let old_count = turns.len().saturating_sub(RECENT_TURNS);
    let mut changed = false;
    if old_count > session.working_context.compacted_turns as usize {
        let mut requests = session.working_context.protected_instructions.clone();
        for message in session
            .messages
            .iter()
            .filter(|message| message.role == MessageRole::User)
        {
            if !requests.contains(&message.content) {
                requests.push(message.content.clone());
            }
        }
        if session.working_context.objective.is_empty() {
            session.working_context.objective = requests.first().cloned().unwrap_or_default();
        }
        // The complete requests are retained exactly so explicit requirements
        // and corrections cannot be lost to summarization.
        session.working_context.protected_instructions = requests;
        session.working_context.decisions = session
            .activity_events
            .iter()
            .filter(|event| {
                matches!(
                    &event.kind,
                    AgentActivityKind::Plan | AgentActivityKind::Progress
                )
            })
            .map(|event| event.summary.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .take(16)
            .collect();
        session.working_context.repository_facts = session
            .messages
            .iter()
            .filter(|message| message.role == MessageRole::Tool)
            .map(|message| concise_context(&message.content, 520))
            .filter(|fact| !fact.is_empty())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .take(16)
            .collect();
        session.working_context.implementation_state = session
            .messages
            .iter()
            .rev()
            .find(|message| {
                message.role == MessageRole::Assistant && !message.content.trim().is_empty()
            })
            .map(|message| concise_context(&message.content, 1_000))
            .unwrap_or_default();
        session.working_context.outstanding_tasks = session
            .working_context
            .protected_instructions
            .last()
            .map(|request| vec![concise_context(request, 2_000)])
            .unwrap_or_default();
        session.working_context.compacted_through = session
            .messages
            .last()
            .map(|message| message.created_at.clone());
        session.working_context.compacted_turns = old_count as u32;
        changed = true;
    }

    let systems: Vec<_> = session
        .messages
        .iter()
        .filter(|message| message.role == MessageRole::System)
        .cloned()
        .collect();
    let mut prepared = systems;
    if session.working_context.compacted_turns > 0 {
        let context = &session.working_context;
        let mut summary = format!("Durable task context ({} earlier turns compacted). This is a summary of visible activity, not private reasoning.\n\nOBJECTIVE\n{}", context.compacted_turns, context.objective);
        if !context.protected_instructions.is_empty() {
            summary.push_str("\n\nORIGINAL USER REQUESTS (verbatim; preserve all constraints)\n");
            for (index, request) in context.protected_instructions.iter().enumerate() {
                summary.push_str(&format!("{}. {}\n", index + 1, request));
            }
        }
        append_context_section(&mut summary, "DECISIONS AND PROGRESS", &context.decisions);
        append_context_section(&mut summary, "REPOSITORY FACTS", &context.repository_facts);
        if !context.implementation_state.is_empty() {
            summary.push_str(&format!(
                "\nIMPLEMENTATION STATE\n{}",
                context.implementation_state
            ));
        }
        append_context_section(
            &mut summary,
            "OUTSTANDING TASKS",
            &context.outstanding_tasks,
        );
        let mut message = AgentMessage::text(MessageRole::System, summary);
        message.created_at = context.compacted_through.clone().unwrap_or_else(now);
        prepared.push(message);
    }
    if turns.len() > RECENT_TURNS {
        turns.drain(..turns.len() - RECENT_TURNS);
    }
    prepared.extend(turns.into_iter().flatten());
    (prepared, changed)
}

fn append_context_section(output: &mut String, heading: &str, entries: &[String]) {
    if entries.is_empty() {
        return;
    }
    output.push_str(&format!("\n\n{heading}\n"));
    for entry in entries {
        output.push_str("- ");
        output.push_str(entry);
        output.push('\n');
    }
}

fn concise_context(input: &str, limit: usize) -> String {
    let compacted = input.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut output: String = compacted.chars().take(limit).collect();
    if compacted.chars().count() > limit {
        output.push('…');
    }
    output
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
            structured_content: None,
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
        result = drive(&state, sink.as_ref(), &mut session, provider_override, &runtime, cancel.clone()) => result,
        _ = cancel.changed() => Err(AppError::new("cancelled", "Run stopped by you.")),
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
    mut cancel: watch::Receiver<bool>,
) -> AppResult<()> {
    let mut project = state.database.project(&session.project_id)?;
    if session.workspace_mode == WorkspaceMode::Isolated {
        let worktree = session.worktree_path.as_deref().ok_or_else(|| {
            AppError::new(
                "worktree_unavailable",
                "This isolated task's workspace was removed. Start a new task to continue.",
            )
        })?;
        let repository_root = project.repository_root.as_deref().ok_or_else(|| {
            AppError::new(
                "not_git_repository",
                "This task is missing its Git repository.",
            )
        })?;
        project.path = crate::git::worktree::project_path(
            Path::new(repository_root),
            Path::new(&project.path),
            Path::new(worktree),
        )
        .await?
        .to_string_lossy()
        .into_owned();
    }
    review::ensure_baseline(&state.database, session, Path::new(&project.path)).await?;
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
                cancel: &mut cancel,
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
    use crate::extensions::{ExtensionRegistry, ToolIntegration};
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

    #[derive(Clone)]
    struct MockExternalTool {
        calls: Arc<Mutex<u32>>,
    }

    #[async_trait::async_trait]
    impl ToolIntegration for MockExternalTool {
        fn owns(&self, name: &str) -> bool {
            name == "mcp__mock__search"
        }
        async fn definitions(&self, _: &str) -> AppResult<Vec<Tool>> {
            Ok(vec![Tool {
                name: "mcp__mock__search".into(),
                description: "Mock remote search".into(),
                permission: ToolCategory::Shell,
                risk_level: ToolRiskLevel::High,
                timeout_ms: 2_000,
                parallel_safe: false,
                input_schema: json!({"type":"object","properties":{"query":{"type":"string","minLength":1}},"required":["query"],"additionalProperties":false}),
            }])
        }
        async fn execute(&self, _: &str, call: &ToolCall) -> ToolResult {
            *self.calls.lock().unwrap() += 1;
            ToolResult {
                tool_call_id: call.id.clone(),
                name: call.name.clone(),
                content: "mock response".into(),
                is_error: false,
                duration_ms: 1,
                structured_content: None,
            }
        }
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
            usage_available: true,
            cached_input_tokens: None,
            output_tokens: 7,
        }
    }

    fn setup_session(root: &Path) -> (SharedState, Project, AgentSession) {
        let database = Database::open(&root.join("app.sqlite")).unwrap();
        let config = AppConfig::load(root).unwrap();
        let state = Arc::new(AppState::new(database, config, CredentialStore));
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
            pending_permission: None,
            session_permission_grants: vec![],
            one_time_permission_grants: vec![],
            pending_user_input: None,
            queued_tool_calls: vec![],
            iterations: 0,
            tool_calls: 0,
            activity_events: vec![],
            created_at: now(),
            updated_at: now(),
            error: None,
            tool_rounds: 0,
            archived_at: None,
            git_branch: None,
            workspace_mode: WorkspaceMode::Direct,
            base_branch: None,
            worktree_path: None,
            working_context: WorkingContext::default(),
            project_instruction_files: vec![],
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
                    tool_call("list", "list_directory", json!({"path":"."})),
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
    async fn external_tools_use_the_agent_permission_pause_and_audited_execution_path() {
        let root = tempfile::tempdir().unwrap();
        let (mut state, _, session) = setup_session(root.path());
        let calls = Arc::new(Mutex::new(0));
        Arc::get_mut(&mut state).unwrap().extensions =
            ExtensionRegistry::default().with(Arc::new(MockExternalTool {
                calls: calls.clone(),
            }));
        let receiver = state.reserve_run(&session.id).unwrap();
        run_inner(
            state.clone(),
            Box::new(Sink::default()),
            session.clone(),
            receiver,
            Some(Box::new(MockProvider::new(vec![response(
                "Searching the documentation.",
                vec![tool_call(
                    "remote-1",
                    "mcp__mock__search",
                    json!({"query":"permissions"}),
                )],
            )]))),
            AgentRuntime::default(),
        )
        .await;

        let mut waiting = state.database.session(&session.id).unwrap();
        assert_eq!(waiting.status, SessionStatus::WaitingForPermission);
        let request = waiting.pending_permission.as_ref().unwrap();
        assert!(request.categories.contains(&PermissionCategory::Command));
        assert!(request.categories.contains(&PermissionCategory::Network));
        assert!(request.categories.contains(&PermissionCategory::Dangerous));
        assert_eq!(*calls.lock().unwrap(), 0);

        let approved = waiting.pending_tool_call.take().unwrap();
        waiting.pending_permission = None;
        waiting
            .one_time_permission_grants
            .push(permissions::fingerprint(&approved).unwrap());
        waiting.queued_tool_calls.push(approved);
        waiting.status = SessionStatus::Queued;
        state.database.save_session(&waiting).unwrap();
        let receiver = state.reserve_run(&session.id).unwrap();
        run_inner(
            state.clone(),
            Box::new(Sink::default()),
            waiting,
            receiver,
            Some(Box::new(MockProvider::new(vec![response(
                "The documentation explains the permission flow.",
                vec![],
            )]))),
            AgentRuntime::default(),
        )
        .await;

        let finished = state.database.session(&session.id).unwrap();
        assert_eq!(finished.status, SessionStatus::Completed);
        assert_eq!(*calls.lock().unwrap(), 1);
        assert!(finished
            .messages
            .iter()
            .any(|message| message
                .tool_result
                .as_ref()
                .is_some_and(|result| result.name == "mcp__mock__search"
                    && result.content == "mock response")));
        assert!(finished
            .activity_events
            .iter()
            .any(|event| event.kind == AgentActivityKind::CommandExecuted
                && event.summary.contains("mcp__mock__search")));
    }

    #[tokio::test]
    async fn one_time_permission_resumes_only_the_approved_tool_call() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("README.md"), "Read under approval").unwrap();
        let (state, _, mut session) = setup_session(root.path());
        session.permission_policy.read_files = PermissionDecision::Ask;
        state.database.save_session(&session).unwrap();
        let receiver = state.reserve_run(&session.id).unwrap();
        run_inner(
            state.clone(),
            Box::new(Sink::default()),
            session.clone(),
            receiver,
            Some(Box::new(MockProvider::new(vec![response(
                "I will inspect the README.",
                vec![tool_call(
                    "read-once",
                    "read_file",
                    json!({"path":"README.md"}),
                )],
            )]))),
            AgentRuntime::default(),
        )
        .await;

        let mut waiting = state.database.session(&session.id).unwrap();
        assert_eq!(waiting.status, SessionStatus::WaitingForPermission);
        let approved = waiting.pending_tool_call.take().unwrap();
        waiting.pending_permission = None;
        waiting
            .one_time_permission_grants
            .push(permissions::fingerprint(&approved).unwrap());
        waiting.queued_tool_calls.push(approved);
        waiting.status = SessionStatus::Queued;
        state.database.save_session(&waiting).unwrap();

        let receiver = state.reserve_run(&session.id).unwrap();
        run_inner(
            state.clone(),
            Box::new(Sink::default()),
            waiting,
            receiver,
            Some(Box::new(MockProvider::new(vec![response(
                "The README says Read under approval.",
                vec![],
            )]))),
            AgentRuntime::default(),
        )
        .await;
        let completed = state.database.session(&session.id).unwrap();
        assert_eq!(completed.status, SessionStatus::Completed);
        assert!(completed.one_time_permission_grants.is_empty());
        assert!(completed.messages.iter().any(|message| message
            .tool_result
            .as_ref()
            .is_some_and(|result| result.tool_call_id == "read-once" && !result.is_error)));
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
                vec![tool_call("list", "list_directory", json!({"path":"."}))],
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
                vec![tool_call("list", "list_directory", json!({"path":"."}))],
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
        let usage = state.database.usage().unwrap();
        assert_eq!(usage.len(), 2);
        assert!(usage.iter().any(
            |record| !record.success && record.failure_code.as_deref() == Some("network_error")
        ));
        assert!(usage.iter().any(|record| record.success));
        assert!(usage
            .iter()
            .all(|record| record.project_id == session.project_id));
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
        let usage = state.database.usage().unwrap();
        assert_eq!(usage.len(), 1);
        assert!(!usage[0].success);
        assert_eq!(usage[0].failure_code.as_deref(), Some("cancelled"));
    }

    #[test]
    fn compaction_preserves_all_user_instructions_and_full_local_history() {
        let system = AgentMessage::text(MessageRole::System, "Project policy");
        let mut session = AgentSession {
            id: "compact-test".into(),
            project_id: "project".into(),
            provider_id: "preview".into(),
            model_id: "mock".into(),
            title: "context".into(),
            status: SessionStatus::Working,
            messages: vec![system],
            permission_policy: Default::default(),
            pending_tool_call: None,
            pending_permission: None,
            session_permission_grants: vec![],
            one_time_permission_grants: vec![],
            pending_user_input: None,
            queued_tool_calls: vec![],
            iterations: 0,
            tool_calls: 0,
            activity_events: vec![],
            created_at: now(),
            updated_at: now(),
            error: None,
            tool_rounds: 0,
            archived_at: None,
            git_branch: None,
            workspace_mode: WorkspaceMode::Direct,
            base_branch: None,
            worktree_path: None,
            working_context: Default::default(),
            project_instruction_files: vec![],
        };
        for index in 0..10 {
            session.messages.push(AgentMessage::text(
                MessageRole::User,
                format!("request {index}: preserve this constraint exactly"),
            ));
            session.messages.push(AgentMessage::text(
                MessageRole::Assistant,
                format!("Visible progress {index}"),
            ));
            session.messages.push(AgentMessage::text(
                MessageRole::Tool,
                format!("large repository output {index} {}", "detail ".repeat(200)),
            ));
        }
        let full_history_size = session.messages.len();
        let old_tool_output = session
            .messages
            .iter()
            .find(|message| message.role == MessageRole::Tool)
            .unwrap()
            .content
            .clone();
        let (context, compacted) = prepare_context(&mut session);
        assert!(compacted);
        assert_eq!(session.messages.len(), full_history_size);
        assert_eq!(session.working_context.protected_instructions.len(), 10);
        assert!(session.working_context.protected_instructions[0]
            .contains("preserve this constraint exactly"));
        assert!(context
            .iter()
            .any(|message| message.content.contains("ORIGINAL USER REQUESTS (verbatim")));
        assert!(context.iter().any(|message| message
            .content
            .contains("request 0: preserve this constraint exactly")));
        assert!(context
            .iter()
            .any(|message| message.content.contains("large repository output 0")));
        assert!(!context
            .iter()
            .any(|message| message.content == old_tool_output));
        assert!(context
            .iter()
            .any(|message| message.content.contains("large repository output 9")));
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
