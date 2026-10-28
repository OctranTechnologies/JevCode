use crate::{
    domain::*,
    error::{AppError, AppResult},
    permissions,
    providers::{self, LlmProvider, ProviderRequest, ProviderResponse},
    review,
    state::SharedState,
    tools, usage,
};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tokio::{sync::watch, task::JoinSet};

pub trait EventSink: Send + Sync {
    fn session_updated(&self, session: &AgentSession);
    fn usage_updated(&self, record: &UsageRecord);
    fn provider_account_updated(&self, _account: &crate::domain::ProviderAccount) {}
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
    decision_engine: Option<std::sync::Arc<dyn crate::decisions::DecisionEngine>>,
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

#[derive(Debug, Clone)]
struct DecisionGuidance {
    category: String,
    workflow: String,
    context_focus: String,
    tool_focus: String,
    scope_score: u8,
    clarification_probability: f64,
}

struct ShadowDecisionOutcome {
    response: Option<crate::decisions::DecisionResponse>,
    failure_code: Option<String>,
    duration_ms: u64,
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
        Self {
            config,
            decision_engine: None,
        }
    }

    /// Inject a deterministic decision engine for runtime tests or host-provided
    /// implementations. Production defaults to the native Jev adapter.
    #[cfg(test)]
    pub fn with_decision_engine(
        mut self,
        engine: std::sync::Arc<dyn crate::decisions::DecisionEngine>,
    ) -> Self {
        self.decision_engine = Some(engine);
        self
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

        let guidance = if session.iterations == 0 {
            Self::run_jev_advisory(state, sink, session, cancel, self.decision_engine.as_ref())
                .await?
        } else {
            None
        };

        let definitions = if model.supports_tools {
            let mut available = tools::definitions();
            available.extend(state.extensions.definitions(&project.id).await?);
            let mut filtered = available
                .into_iter()
                .filter(|tool| {
                    tool.permission == ToolCategory::UserInteraction
                        || session.permission_policy.decision(&tool.permission)
                            != PermissionDecision::Deny
                })
                .collect::<Vec<_>>();
            if let Some(guidance) = guidance.as_ref() {
                filtered
                    .sort_by_key(|tool| tool_relevance_priority(&tool.name, &guidance.tool_focus));
            }
            filtered
        } else {
            vec![]
        };

        loop {
            while !session.queued_tool_calls.is_empty() {
                if self
                    .process_next_tools(state, sink, session, project, &definitions, cancel)
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

            let (mut prepared_messages, compacted) = prepare_context(session);
            if let Some(guidance) = guidance.as_ref() {
                add_decision_guidance(&mut prepared_messages, guidance);
            }
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

            session.working_context.completion_review =
                Some(self.build_completion_review(state, session, cancel).await?);
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

    /// Send only the current user objective and context-availability metadata to
    /// Jev. Advisory mode records a category suggestion; Assisted mode may rank
    /// bounded workflow/context/tool hints. Neither mode can execute tools or
    /// grant permissions.
    async fn run_jev_advisory(
        state: &SharedState,
        sink: &dyn EventSink,
        session: &mut AgentSession,
        cancel: &mut watch::Receiver<bool>,
        decision_engine: Option<&std::sync::Arc<dyn crate::decisions::DecisionEngine>>,
    ) -> AppResult<Option<DecisionGuidance>> {
        let preferences = state.database.decision_preferences()?;
        if preferences.mode == DecisionMode::Off {
            return Ok(None);
        }

        let Some(objective) = session
            .messages
            .iter()
            .rev()
            .find(|message| message.role == MessageRole::User)
            .map(|message| message.content.chars().take(3_000).collect::<String>())
            .filter(|objective| !objective.trim().is_empty())
        else {
            return Ok(None);
        };

        let mut available_context = serde_json::json!({
            "projectInstructionCount": session.project_instruction_files.len(),
            "repositoryFactCount": session.working_context.repository_facts.len(),
            "recentActivityKinds": session.activity_events.iter().rev().take(8).map(|event| &event.kind).collect::<Vec<_>>(),
        });
        let include_guidance = matches!(
            preferences.mode,
            DecisionMode::Assisted | DecisionMode::Shadow
        );
        if include_guidance {
            available_context["hasPriorTaskDecisions"] =
                serde_json::json!(!session.working_context.decisions.is_empty());
        }
        let request = crate::decisions::DecisionRequest {
            model_id: preferences.model_id.clone(),
            state: serde_json::json!({ "objective": objective, "availableContext": available_context }),
            questions: crate::decisions::runtime_questions(include_guidance),
        };

        let started = Instant::now();
        let result = if *cancel.borrow() {
            Err(AppError::new("cancelled", "Run stopped by you."))
        } else {
            let operation = async {
                let engine: std::sync::Arc<dyn crate::decisions::DecisionEngine> =
                    if let Some(engine) = decision_engine {
                        std::sync::Arc::clone(engine)
                    } else {
                        std::sync::Arc::new(crate::decisions::JevClient::new(&state.credentials)?)
                    };
                tokio::time::timeout(Duration::from_secs(10), async {
                tokio::select! {
                    biased;
                    _ = cancel.changed() => Err(AppError::new("cancelled", "Run stopped by you.")),
                    result = engine.evaluate(request) => result,
                }
            })
            .await
            .unwrap_or_else(|_| {
                Err(AppError::new(
                    "decision_timeout",
                    "Jev did not respond in time.",
                ))
            })
            };
            operation.await
        };

        let shadow_mode = preferences.mode == DecisionMode::Shadow;
        let (
            success,
            selected_option,
            selected_probability,
            resolved_model_id,
            input_tokens,
            output_tokens,
            failure_code,
            summary,
            guidance,
        ) = match result {
            Ok(response) => {
                if *cancel.borrow() {
                    return Err(AppError::new("cancelled", "Run stopped by you."));
                }
                let resolved_model_id =
                    (!response.model_id.trim().is_empty()).then(|| response.model_id.clone());
                let parse_mode = if shadow_mode {
                    DecisionMode::Assisted
                } else {
                    preferences.mode
                };
                match Self::parse_decision_guidance(&response, parse_mode) {
                    Ok((guidance, selected_option, probability, summary)) => {
                        let (summary, guidance) = if shadow_mode {
                            (
                                format!("Jev shadow suggestion recorded; it did not steer this task. {selected_option}"),
                                None,
                            )
                        } else {
                            (summary, guidance)
                        };
                        (
                            true,
                            Some(selected_option),
                            probability,
                            resolved_model_id,
                            response.input_tokens,
                            response.output_tokens,
                            None,
                            summary,
                            guidance,
                        )
                    }
                    Err(error) => (
                        false,
                        None,
                        None,
                        resolved_model_id,
                        response.input_tokens,
                        response.output_tokens,
                        Some(error.code),
                        "Jev returned an incomplete decision; continuing under normal agent controls.".to_owned(),
                        None,
                    ),
                }
            }
            Err(error) if error.code == "cancelled" => return Err(error),
            Err(error) => {
                tracing::info!(session_id = %session.id, code = %error.code, "Jev advisory unavailable; continuing agent task");
                (
                    false,
                    None,
                    None,
                    None,
                    0,
                    0,
                    Some(error.code),
                    "Jev decision service unavailable; continuing under normal agent controls."
                        .to_owned(),
                    None,
                )
            }
        };

        let record = DecisionUsageRecord {
            id: id(),
            session_id: session.id.clone(),
            session_title: session.title.clone(),
            project_id: session.project_id.clone(),
            model_id: preferences.model_id,
            resolved_model_id,
            shadow_mode,
            input_tokens,
            output_tokens,
            duration_ms: started.elapsed().as_millis() as u64,
            success,
            failure_code,
            selected_option,
            selected_probability,
            created_at: now(),
        };
        if let Err(error) = state.database.save_decision_usage(&record) {
            tracing::warn!(session_id = %session.id, code = %error.code, "Could not save Jev decision usage");
        }
        record_activity(session, AgentActivityKind::DecisionAdvisory, summary, None);
        checkpoint(state, sink, session)?;
        Ok(guidance)
    }

    fn parse_decision_guidance(
        response: &crate::decisions::DecisionResponse,
        mode: DecisionMode,
    ) -> AppResult<(Option<DecisionGuidance>, String, Option<f64>, String)> {
        let (category, category_probability) = decision_choice(response, "task_category")?;
        let category_label = match category.as_str() {
            "coding" => "implementation",
            "debugging" => "debugging",
            "review" => "review",
            "explanation" => "explanation",
            _ => {
                return Err(AppError::new(
                    "provider_error",
                    "Jev returned an unknown category.",
                ))
            }
        };
        if mode == DecisionMode::Advisory {
            return Ok((
                None,
                category,
                category_probability,
                format!("Jev advisory: request category is {category_label}. The coding agent remains in control."),
            ));
        }

        let (workflow, _) = decision_choice(response, "workflow")?;
        let (context_focus, _) = decision_choice(response, "context_focus")?;
        let (tool_focus, _) = decision_choice(response, "tool_focus")?;
        let scope_score = decision_score(response, "scope")?;
        let clarification_probability = decision_probability(response, "needs_clarification")?;
        if !["explore", "implement", "verify", "explain"].contains(&workflow.as_str())
            || ![
                "repository_facts",
                "recent_activity",
                "earlier_task_decisions",
                "current_request",
            ]
            .contains(&context_focus.as_str())
            || !["inspect", "search", "edit", "verify", "respond"].contains(&tool_focus.as_str())
        {
            return Err(AppError::new(
                "provider_error",
                "Jev returned an unsupported assistance choice.",
            ));
        }
        let guidance = DecisionGuidance {
            category,
            workflow: workflow.clone(),
            context_focus: context_focus.clone(),
            tool_focus: tool_focus.clone(),
            scope_score,
            clarification_probability,
        };
        let scope_label = match scope_score {
            0 => "small",
            1 => "multi-file",
            _ => "cross-cutting",
        };
        let clarification_note = if clarification_probability >= 0.7 {
            " Consider whether a clarification is needed before an irreversible step."
        } else {
            ""
        };
        let selected = format!(
            "category={};workflow={workflow};context={context_focus};tools={tool_focus};scope={scope_score};clarification={clarification_probability:.2}",
            guidance.category
        );
        let summary = format!(
            "Jev assisted suggestion: {workflow}; focus {context_focus}; consider {tool_focus} tools first; {scope_label} scope.{clarification_note} Permissions remain unchanged."
        );
        Ok((Some(guidance), selected, category_probability, summary))
    }

    async fn process_next_tools(
        &self,
        state: &SharedState,
        sink: &dyn EventSink,
        session: &mut AgentSession,
        project: &Project,
        available_tools: &[Tool],
        cancel: &mut watch::Receiver<bool>,
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

        self.maybe_shadow_scope_review(state, sink, session, &call, cancel)
            .await?;

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
        self.maybe_record_local_focus_baseline(state, sink, session, available_tools)?;
        self.maybe_shadow_recovery_hint(state, sink, session, cancel)
            .await?;
        Ok(false)
    }

    fn maybe_record_local_focus_baseline(
        &self,
        state: &SharedState,
        sink: &dyn EventSink,
        session: &mut AgentSession,
        available_tools: &[Tool],
    ) -> AppResult<()> {
        if session.working_context.stage2_focus_baseline_emitted
            || !session.messages.iter().any(|message| {
                message.tool_result.as_ref().is_some_and(|result| {
                    !result.is_error
                        && matches!(
                            result.name.as_str(),
                            "search_files" | "search_text" | "find_symbol" | "find_references"
                        )
                })
            })
        {
            return Ok(());
        }
        let files = focus_file_candidates(session);
        if files.len() < 2 {
            return Ok(());
        }
        let objective = focus_objective(session);
        let successful_tool_ids = session
            .messages
            .iter()
            .filter_map(|message| message.tool_result.as_ref())
            .filter(|result| !result.is_error)
            .map(|result| result.tool_call_id.as_str())
            .collect::<HashSet<_>>();
        let changed_paths = session
            .messages
            .iter()
            .flat_map(|message| message.tool_calls.iter())
            .filter(|call| successful_tool_ids.contains(call.id.as_str()))
            .filter_map(|call| match call.name.as_str() {
                "apply_patch" | "create_file" | "delete_file" => call.arguments["path"].as_str(),
                "move_file" => call.arguments["destination"].as_str(),
                _ => None,
            })
            .filter_map(normalize_workspace_relative_path)
            .collect::<Vec<_>>();
        let preferred_file = rank_focus_path(&files, &objective, &changed_paths);
        let tests = files
            .iter()
            .filter(|path| is_test_candidate_path(path))
            .cloned()
            .collect::<Vec<_>>();
        let preferred_test = rank_focus_path(&tests, &objective, &changed_paths);
        let preferred_tool = rank_focus_tool(available_tools, session, &objective);

        session.working_context.stage2_focus_baseline_emitted = true;
        let mut recommendations = vec![format!(
            "file {}",
            preferred_file.as_deref().unwrap_or("none")
        )];
        if !tests.is_empty() {
            recommendations.push(format!(
                "test {}",
                preferred_test.as_deref().unwrap_or("none")
            ));
        }
        if let Some(tool) = preferred_tool {
            recommendations.push(format!("tool {tool}"));
        }
        let summary = format!(
            "Local focus baseline (not applied; no project metadata sent to Jev): {}; candidates {} files, {} test files, {} available tools.",
            recommendations.join("; "),
            files.len(),
            tests.len(),
            available_tools.len(),
        );
        record_activity(session, AgentActivityKind::FocusBaseline, summary, None);
        checkpoint(state, sink, session)
    }

    async fn evaluate_shadow_signal(
        &self,
        state: &SharedState,
        cancel: &mut watch::Receiver<bool>,
        model_id: String,
        signal_state: serde_json::Value,
        questions: BTreeMap<String, crate::decisions::DecisionQuestion>,
    ) -> AppResult<ShadowDecisionOutcome> {
        let started = Instant::now();
        if *cancel.borrow() {
            return Err(AppError::new("cancelled", "Run stopped by you."));
        }
        let engine: AppResult<std::sync::Arc<dyn crate::decisions::DecisionEngine>> =
            if let Some(engine) = self.decision_engine.as_ref() {
                Ok(std::sync::Arc::clone(engine))
            } else {
                crate::decisions::JevClient::new(&state.credentials)
                    .map(|client| std::sync::Arc::new(client) as _)
            };
        let result = match engine {
            Ok(engine) => {
                let request = crate::decisions::DecisionRequest {
                    model_id,
                    state: signal_state,
                    questions,
                };
                let evaluation = engine.evaluate(request);
                tokio::pin!(evaluation);
                tokio::time::timeout(Duration::from_secs(10), async {
                    tokio::select! {
                        biased;
                        _ = cancel.changed() => Err(AppError::new("cancelled", "Run stopped by you.")),
                        response = &mut evaluation => response,
                    }
                })
                .await
                .unwrap_or_else(|_| {
                    Err(AppError::new(
                        "decision_timeout",
                        "Jev did not respond in time.",
                    ))
                })
            }
            Err(error) => Err(error),
        };
        if *cancel.borrow() {
            return Err(AppError::new("cancelled", "Run stopped by you."));
        }
        let (response, failure_code) = match result {
            Ok(response) => (Some(response), None),
            Err(error) if error.code == "cancelled" => return Err(error),
            Err(error) => {
                tracing::info!(code = %error.code, "Jev Stage 1 shadow signal unavailable");
                (None, Some(error.code))
            }
        };
        Ok(ShadowDecisionOutcome {
            response,
            failure_code,
            duration_ms: started.elapsed().as_millis() as u64,
        })
    }

    async fn build_completion_review(
        &self,
        state: &SharedState,
        session: &AgentSession,
        cancel: &mut watch::Receiver<bool>,
    ) -> AppResult<CompletionReview> {
        let (requirements, omitted_requirement_count) = completion_requirements(session);
        let changes = match review::list_changes(&state.database, session).await {
            Ok(changes) => changes,
            Err(error) => {
                tracing::warn!(session_id = %session.id, code = %error.code, "Could not collect completion review changes");
                SessionChanges {
                    session_id: session.id.clone(),
                    files: Vec::new(),
                    additions: 0,
                    deletions: 0,
                    tests_summary: None,
                    working_tree: "unknown".into(),
                    baseline_head: None,
                    baseline_at: session.created_at.clone(),
                }
            }
        };
        let changed_files = changes
            .files
            .iter()
            .filter_map(|file| normalize_workspace_relative_path(&file.path))
            .take(24)
            .collect::<Vec<_>>();
        let test_runs = completion_test_runs(session);
        let mut report = CompletionReview {
            requirements,
            changed_files,
            test_runs,
            omitted_requirement_count,
            jev_shadow_assessed: false,
        };
        let preferences = match state.database.decision_preferences() {
            Ok(preferences) => preferences,
            Err(error) => {
                tracing::warn!(session_id = %session.id, code = %error.code, "Could not load Jev preferences for the completion receipt");
                return Ok(report);
            }
        };
        if preferences.mode != DecisionMode::Shadow || report.requirements.is_empty() {
            return Ok(report);
        }
        if *cancel.borrow() {
            return Ok(report);
        }

        let objective = session
            .working_context
            .objective
            .chars()
            .take(3_000)
            .collect::<String>();
        let requirements_state = report
            .requirements
            .iter()
            .enumerate()
            .map(|(index, item)| {
                serde_json::json!({
                    "id": format!("requirement_{index}"),
                    "text": item.text.chars().take(700).collect::<String>(),
                })
            })
            .collect::<Vec<_>>();
        let test_state = if report.test_runs.is_empty() {
            "not_run"
        } else if report
            .test_runs
            .iter()
            .any(|run| run.status == CompletionTestStatus::Failed)
        {
            "failed"
        } else if report
            .test_runs
            .iter()
            .all(|run| run.status == CompletionTestStatus::Passed)
        {
            "passed"
        } else {
            "unknown"
        };
        let request_state = serde_json::json!({
            "objective": objective,
            "requirements": requirements_state,
            "evidence": {
                "changedWorkspaceRelativeFiles": report.changed_files.clone(),
                "testOutcome": test_state,
                "testRunCount": report.test_runs.len(),
            }
        });
        let questions = completion_questions(report.requirements.len());
        let outcome = self
            .evaluate_shadow_signal(
                state,
                cancel,
                preferences.model_id.clone(),
                request_state,
                questions,
            )
            .await;
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) if error.code == "cancelled" => return Ok(report),
            Err(error) => return Err(error),
        };
        let mut selected = BTreeMap::new();
        let mut valid_count = 0usize;
        if let Some(response) = outcome.response.as_ref() {
            for (index, requirement) in report.requirements.iter_mut().enumerate() {
                if let Some((assessment, probability)) =
                    completion_assessment(response, &format!("requirement_{index}"))
                {
                    requirement.assessment = Some(assessment);
                    let key = match assessment {
                        CompletionAssessment::Supported => "supported",
                        CompletionAssessment::Partial => "partial",
                        CompletionAssessment::NoEvidence => "not_observed",
                        CompletionAssessment::Unclear => "unclear",
                    };
                    selected.insert(key, selected.get(key).copied().unwrap_or(0usize) + 1);
                    let _ = probability;
                    valid_count += 1;
                }
            }
            report.jev_shadow_assessed = valid_count == report.requirements.len();
            if !report.jev_shadow_assessed {
                for item in &mut report.requirements {
                    item.assessment = None;
                }
            }
        }
        let selected_option = (!selected.is_empty()).then(|| {
            selected
                .iter()
                .map(|(key, count)| format!("{key}={count}"))
                .collect::<Vec<_>>()
                .join(",")
        });
        record_shadow_usage(
            state,
            session,
            &preferences.model_id,
            &outcome,
            report.jev_shadow_assessed,
            selected_option,
            None,
        );
        Ok(report)
    }

    async fn maybe_shadow_scope_review(
        &self,
        state: &SharedState,
        sink: &dyn EventSink,
        session: &mut AgentSession,
        call: &ToolCall,
        cancel: &mut watch::Receiver<bool>,
    ) -> AppResult<()> {
        if !matches!(
            call.name.as_str(),
            "apply_patch" | "create_file" | "delete_file" | "move_file"
        ) || session.working_context.stage1_scope_checked_paths.len() >= 3
        {
            return Ok(());
        }
        let Some(path) = scope_candidate_path(call) else {
            return Ok(());
        };
        if session
            .working_context
            .stage1_scope_checked_paths
            .iter()
            .any(|checked| checked == &path)
        {
            return Ok(());
        }
        let preferences = match state.database.decision_preferences() {
            Ok(preferences) => preferences,
            Err(error) => {
                tracing::warn!(session_id = %session.id, code = %error.code, "Could not load Jev preferences for the scope signal");
                return Ok(());
            }
        };
        if preferences.mode != DecisionMode::Shadow {
            return Ok(());
        }
        let objective = session
            .working_context
            .objective
            .chars()
            .take(3_000)
            .collect::<String>();
        let question = crate::decisions::DecisionQuestion::Choice {
            instructions: "Classify whether this workspace-relative candidate path appears directly requested, plausibly supporting, or likely unrelated to the objective. This is only a shadow warning; do not direct the application to block or undo work.".into(),
            criteria: decision_criteria(&[
                ("direct", "The objective explicitly calls for this area."),
                ("supporting", "The path plausibly supports the requested outcome."),
                ("unrelated", "The path appears unrelated to the requested outcome."),
                ("unclear", "There is not enough information to tell."),
            ]),
        };
        let outcome = self
            .evaluate_shadow_signal(
                state,
                cancel,
                preferences.model_id.clone(),
                serde_json::json!({ "objective": objective, "candidateWorkspaceRelativePath": path.clone() }),
                BTreeMap::from([("scope_fit".into(), question)]),
            )
            .await?;
        session
            .working_context
            .stage1_scope_checked_paths
            .push(path.clone());
        let answer = outcome.response.as_ref().and_then(|response| {
            decision_choice(response, "scope_fit")
                .ok()
                .filter(|(choice, _)| {
                    ["direct", "supporting", "unrelated", "unclear"].contains(&choice.as_str())
                })
        });
        let valid = answer.is_some();
        let selected = answer.as_ref().map(|(choice, _)| choice.clone());
        record_shadow_usage(
            state,
            session,
            &preferences.model_id,
            &outcome,
            valid,
            selected.clone(),
            answer.and_then(|(_, probability)| probability),
        );
        if selected.as_deref() == Some("unrelated") {
            record_activity(
                session,
                AgentActivityKind::JevScopeWarning,
                format!("Jev shadow warning: {path} may be outside this task's scope. No action was blocked or reverted."),
                Some(call.id.clone()),
            );
            checkpoint(state, sink, session)?;
        }
        Ok(())
    }

    async fn maybe_shadow_recovery_hint(
        &self,
        state: &SharedState,
        sink: &dyn EventSink,
        session: &mut AgentSession,
        cancel: &mut watch::Receiver<bool>,
    ) -> AppResult<()> {
        if session.working_context.stage1_stall_signal_emitted {
            return Ok(());
        }
        let Some(signal) = repeated_failure_signal(session) else {
            return Ok(());
        };
        let preferences = match state.database.decision_preferences() {
            Ok(preferences) => preferences,
            Err(error) => {
                tracing::warn!(session_id = %session.id, code = %error.code, "Could not load Jev preferences for the recovery signal");
                return Ok(());
            }
        };
        if preferences.mode != DecisionMode::Shadow {
            return Ok(());
        }
        // Persist the one-shot bound before making the optional network request.
        session.working_context.stage1_stall_signal_emitted = true;
        checkpoint(state, sink, session)?;
        let question = crate::decisions::DecisionQuestion::Choice {
            instructions: "Choose one bounded next-step suggestion after repeated identical failed tool actions. Do not claim to have performed the action, and do not change permissions or retry automatically.".into(),
            criteria: decision_criteria(&[
                ("inspect_environment", "Inspect the environment or project setup."),
                ("search_differently", "Search for different evidence."),
                ("different_approach", "Try a different approach through existing tools."),
                ("ask_user", "Ask the user a specific question."),
                ("stop_report", "Stop and report the blocker."),
            ]),
        };
        let outcome = self
            .evaluate_shadow_signal(
                state,
                cancel,
                preferences.model_id.clone(),
                serde_json::json!({
                    "objective": session.working_context.objective.chars().take(3_000).collect::<String>(),
                    "repeatedActionKind": signal.kind,
                    "repeatedFailedActionCount": signal.repetitions,
                }),
                BTreeMap::from([("recovery".into(), question)]),
            )
            .await?;
        let answer = outcome.response.as_ref().and_then(|response| {
            decision_choice(response, "recovery")
                .ok()
                .filter(|(choice, _)| {
                    [
                        "inspect_environment",
                        "search_differently",
                        "different_approach",
                        "ask_user",
                        "stop_report",
                    ]
                    .contains(&choice.as_str())
                })
        });
        let valid = answer.is_some();
        let selected = answer.as_ref().map(|(choice, _)| choice.clone());
        record_shadow_usage(
            state,
            session,
            &preferences.model_id,
            &outcome,
            valid,
            selected.clone(),
            answer.and_then(|(_, probability)| probability),
        );
        if let Some(choice) = selected {
            let suggestion = match choice.as_str() {
                "inspect_environment" => "inspect the environment or project setup",
                "search_differently" => "search for different evidence",
                "different_approach" => "try a different approach",
                "ask_user" => "ask the user a specific question",
                _ => "stop and report the blocker",
            };
            record_activity(
                session,
                AgentActivityKind::JevRecoveryHint,
                format!("Jev shadow recovery hint: {suggestion} after {} repeated failed {} actions. No action was taken automatically.", signal.repetitions, signal.kind),
                None,
            );
            checkpoint(state, sink, session)?;
        }
        Ok(())
    }
}

fn decision_criteria(options: &[(&str, &str)]) -> BTreeMap<String, String> {
    options
        .iter()
        .map(|(key, label)| ((*key).to_owned(), (*label).to_owned()))
        .collect()
}

fn completion_requirements(session: &AgentSession) -> (Vec<CompletionRequirement>, u32) {
    let source = if !session.working_context.objective.trim().is_empty() {
        session.working_context.objective.as_str()
    } else {
        session
            .messages
            .iter()
            .find(|message| message.role == MessageRole::User)
            .map(|message| message.content.as_str())
            .unwrap_or_default()
    };
    let list_items = source
        .lines()
        .filter_map(|line| strip_requirement_marker(line.trim()))
        .map(normalize_requirement)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    let candidates = if list_items.len() >= 2 {
        list_items
    } else {
        let normalized = normalize_requirement(source);
        if normalized.is_empty() {
            Vec::new()
        } else {
            vec![normalized]
        }
    };
    let omitted = candidates.len().saturating_sub(8) as u32;
    let requirements = candidates
        .into_iter()
        .take(8)
        .map(|text| CompletionRequirement {
            text: text.chars().take(700).collect(),
            assessment: None,
        })
        .collect();
    (requirements, omitted)
}

fn strip_requirement_marker(line: &str) -> Option<&str> {
    for marker in ["- ", "* ", "+ ", "• "] {
        if let Some(rest) = line.strip_prefix(marker) {
            return Some(rest.trim());
        }
    }
    let digits = line
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .count();
    if digits > 0 {
        let remainder = &line[digits..];
        if let Some(rest) = remainder
            .strip_prefix(". ")
            .or_else(|| remainder.strip_prefix(") "))
        {
            return Some(rest.trim());
        }
    }
    None
}

fn normalize_requirement(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn completion_questions(count: usize) -> BTreeMap<String, crate::decisions::DecisionQuestion> {
    (0..count)
        .map(|index| {
            (
                format!("requirement_{index}"),
                crate::decisions::DecisionQuestion::Choice {
                    instructions: "Classify the request item against only the supplied changed workspace-relative paths and aggregate test outcome. This is a tentative evidence match, not a code-correctness claim or test result. Choose supported, partial, not_observed, or unclear.".into(),
                    criteria: decision_criteria(&[
                        ("supported", "The listed evidence appears to support the requirement."),
                        ("partial", "The evidence appears to cover only part of the requirement."),
                        ("not_observed", "No matching evidence is apparent in the provided summary."),
                        ("unclear", "The evidence is insufficient to classify the requirement."),
                    ]),
                },
            )
        })
        .collect()
}

fn completion_assessment(
    response: &crate::decisions::DecisionResponse,
    name: &str,
) -> Option<(CompletionAssessment, Option<f64>)> {
    let (choice, probability) = decision_choice(response, name).ok()?;
    let assessment = match choice.as_str() {
        "supported" => CompletionAssessment::Supported,
        "partial" => CompletionAssessment::Partial,
        "not_observed" => CompletionAssessment::NoEvidence,
        "unclear" => CompletionAssessment::Unclear,
        _ => return None,
    };
    Some((assessment, probability))
}

fn completion_test_runs(session: &AgentSession) -> Vec<CompletionTestRun> {
    let calls = session
        .messages
        .iter()
        .flat_map(|message| message.tool_calls.iter())
        .map(|call| (call.id.as_str(), call))
        .collect::<HashMap<_, _>>();
    let mut runs = session
        .messages
        .iter()
        .filter_map(|message| message.tool_result.as_ref())
        .filter_map(|result| {
            let call = calls.get(result.tool_call_id.as_str()).copied()?;
            is_test_invocation(call).then_some(result)
        })
        .map(|result| {
            let exit_code = result
                .structured_content
                .as_ref()
                .and_then(|value| value.get("exitCode"))
                .and_then(serde_json::Value::as_i64)
                .and_then(|value| i32::try_from(value).ok());
            let timed_out = result
                .structured_content
                .as_ref()
                .and_then(|value| value.get("timedOut"))
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let status = if timed_out || result.is_error || exit_code.is_some_and(|code| code != 0)
            {
                CompletionTestStatus::Failed
            } else if exit_code == Some(0) {
                CompletionTestStatus::Passed
            } else {
                CompletionTestStatus::Unknown
            };
            CompletionTestRun {
                label: String::new(),
                status,
                exit_code,
                tool_call_id: result.tool_call_id.clone(),
            }
        })
        .collect::<Vec<_>>();
    if runs.len() > 8 {
        runs.drain(..runs.len() - 8);
    }
    for (index, run) in runs.iter_mut().enumerate() {
        run.label = format!("Test run {}", index + 1);
    }
    runs
}

fn scope_candidate_path(call: &ToolCall) -> Option<String> {
    let key = if call.name == "move_file" {
        "destination"
    } else {
        "path"
    };
    normalize_workspace_relative_path(call.arguments.get(key)?.as_str()?)
}

fn normalize_workspace_relative_path(raw: &str) -> Option<String> {
    let path = Path::new(raw);
    if raw.trim().is_empty() || raw.len() > 1_024 || path.is_absolute() {
        return None;
    }
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(value) => {
                let value = value.to_string_lossy();
                if value.chars().any(char::is_control) {
                    return None;
                }
                components.push(value.into_owned());
            }
            std::path::Component::CurDir => {}
            _ => return None,
        }
    }
    let normalized = components.join("/");
    (!normalized.is_empty() && normalized.len() <= 300).then_some(normalized)
}

struct RepeatedFailureSignal {
    kind: &'static str,
    repetitions: usize,
}

fn repeated_failure_signal(session: &AgentSession) -> Option<RepeatedFailureSignal> {
    let calls = session
        .messages
        .iter()
        .flat_map(|message| message.tool_calls.iter())
        .map(|call| (call.id.as_str(), call))
        .collect::<HashMap<_, _>>();
    let mut signatures: HashMap<String, (&'static str, usize)> = HashMap::new();
    for message in session
        .messages
        .iter()
        .rev()
        .filter(|message| message.role == MessageRole::Tool)
        .take(12)
    {
        let Some(result) = message.tool_result.as_ref() else {
            continue;
        };
        let Some(call) = calls.get(result.tool_call_id.as_str()).copied() else {
            continue;
        };
        let kind = if call.name == "run_command" {
            let exit_code = result
                .structured_content
                .as_ref()
                .and_then(|value| value.get("exitCode"))
                .and_then(serde_json::Value::as_i64);
            let timed_out = result
                .structured_content
                .as_ref()
                .and_then(|value| value.get("timedOut"))
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            if !result.is_error && !timed_out && exit_code.is_none_or(|code| code == 0) {
                continue;
            }
            "command"
        } else if matches!(
            call.name.as_str(),
            "search_files" | "search_text" | "find_symbol" | "find_references"
        ) {
            let no_matches = result
                .structured_content
                .as_ref()
                .and_then(|value| value.get("total").or_else(|| value.get("totalAtMost")))
                .and_then(serde_json::Value::as_u64)
                == Some(0);
            if !result.is_error && !no_matches {
                continue;
            }
            "search"
        } else {
            continue;
        };
        let normalized_arguments = call
            .arguments
            .to_string()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let mut hash = Sha256::new();
        hash.update(call.name.as_bytes());
        hash.update([0]);
        hash.update(normalized_arguments.as_bytes());
        let fingerprint = format!("{:x}", hash.finalize());
        let entry = signatures.entry(fingerprint).or_insert((kind, 0));
        entry.1 += 1;
    }
    signatures
        .values()
        .filter(|(_, count)| *count >= 2)
        .max_by_key(|(_, count)| *count)
        .map(|(kind, repetitions)| RepeatedFailureSignal {
            kind,
            repetitions: *repetitions,
        })
}

fn focus_objective(session: &AgentSession) -> String {
    if !session.working_context.objective.trim().is_empty() {
        return session.working_context.objective.clone();
    }
    session
        .messages
        .iter()
        .rev()
        .find(|message| message.role == MessageRole::User)
        .map(|message| message.content.clone())
        .unwrap_or_default()
}

/// Extract only paths returned by the existing workspace-scoped file tools.
/// This local shortlist never leaves the machine in the Stage 2 baseline.
fn focus_file_candidates(session: &AgentSession) -> Vec<String> {
    let calls = session
        .messages
        .iter()
        .flat_map(|message| message.tool_calls.iter())
        .map(|call| (call.id.as_str(), call))
        .collect::<HashMap<_, _>>();
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    for message in &session.messages {
        let Some(result) = message
            .tool_result
            .as_ref()
            .filter(|result| !result.is_error)
        else {
            continue;
        };
        let Some(call) = calls.get(result.tool_call_id.as_str()).copied() else {
            continue;
        };
        let structured = result.structured_content.as_ref();
        let mut paths = Vec::new();
        match call.name.as_str() {
            "read_file" => {
                if let Some(path) = call.arguments["path"].as_str() {
                    paths.push(path.to_owned());
                }
            }
            "read_files" => {
                paths.extend(
                    call.arguments["paths"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(serde_json::Value::as_str)
                        .map(str::to_owned),
                );
            }
            "search_files" => {
                paths.extend(
                    structured
                        .and_then(|value| value.get("paths"))
                        .and_then(serde_json::Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(serde_json::Value::as_str)
                        .map(str::to_owned),
                );
            }
            "search_text" | "find_symbol" | "find_references" => {
                paths.extend(
                    structured
                        .and_then(|value| value.get("matches"))
                        .and_then(serde_json::Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(|entry| entry.get("path"))
                        .filter_map(serde_json::Value::as_str)
                        .map(str::to_owned),
                );
            }
            _ => {}
        }
        for raw in paths {
            if let Some(path) = normalize_workspace_relative_path(&raw) {
                if seen.insert(path.clone()) {
                    candidates.push(path);
                    if candidates.len() == 32 {
                        return candidates;
                    }
                }
            }
        }
    }
    candidates
}

fn is_test_candidate_path(path: &str) -> bool {
    let normalized = path.to_ascii_lowercase();
    let file_name = normalized.rsplit('/').next().unwrap_or(&normalized);
    normalized.split('/').any(|part| part == "tests")
        || file_name.starts_with("test_")
        || file_name.starts_with("test-")
        || file_name.contains(".test.")
        || file_name.contains(".spec.")
        || file_name.ends_with("_test.rs")
        || file_name.ends_with("_test.py")
}

fn focus_tokens(value: &str) -> HashSet<String> {
    const STOP_WORDS: &[&str] = &[
        "about", "after", "also", "and", "are", "for", "from", "into", "make", "need", "that",
        "the", "this", "with", "you", "your",
    ];
    value
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|token| token.len() > 2 && !STOP_WORDS.contains(token))
        .map(str::to_ascii_lowercase)
        .collect()
}

fn rank_focus_path(
    candidates: &[String],
    objective: &str,
    changed_paths: &[String],
) -> Option<String> {
    let objective_tokens = focus_tokens(objective);
    let changed_tokens = changed_paths
        .iter()
        .flat_map(|path| focus_tokens(path))
        .collect::<HashSet<_>>();
    let mut best: Option<(usize, String)> = None;
    for candidate in candidates {
        let path_tokens = focus_tokens(candidate);
        let objective_match = path_tokens.intersection(&objective_tokens).count();
        let change_match = path_tokens.intersection(&changed_tokens).count();
        let score = objective_match * 2 + change_match;
        if best
            .as_ref()
            .is_none_or(|(best_score, _)| score > *best_score)
        {
            best = Some((score, candidate.clone()));
        }
    }
    best.map(|(_, path)| path)
}

fn rank_focus_tool(
    available_tools: &[Tool],
    session: &AgentSession,
    objective: &str,
) -> Option<String> {
    let objective_tokens = focus_tokens(objective);
    let call_counts = session
        .messages
        .iter()
        .flat_map(|message| message.tool_calls.iter())
        .fold(HashMap::<&str, usize>::new(), |mut counts, call| {
            *counts.entry(&call.name).or_default() += 1;
            counts
        });
    let mut best: Option<(usize, usize, String)> = None;
    for tool in available_tools {
        let aliases = match tool.name.as_str() {
            "read_file" | "read_files" | "list_directory" | "file_metadata" => {
                "inspect read explore understand"
            }
            "search_files" | "search_text" | "find_symbol" | "find_references" => {
                "search find locate trace"
            }
            "apply_patch" | "create_file" | "move_file" | "delete_file" => {
                "edit change implement fix add remove"
            }
            "run_command" => "test run verify build check",
            "git_status" | "git_diff" | "git_log" | "git_show" => {
                "git review changes commit branch"
            }
            "ask_user" => "ask question clarify",
            _ => tool.name.as_str(),
        };
        let relevance = focus_tokens(aliases)
            .intersection(&objective_tokens)
            .count();
        let usage = call_counts.get(tool.name.as_str()).copied().unwrap_or(0);
        if best.as_ref().is_none_or(|(best_relevance, best_usage, _)| {
            (relevance, usage) > (*best_relevance, *best_usage)
        }) {
            best = Some((relevance, usage, tool.name.clone()));
        }
    }
    best.map(|(_, _, name)| safe_tool_display_name(&name))
}

fn safe_tool_display_name(name: &str) -> String {
    let display = name
        .chars()
        .filter(|character| !character.is_control())
        .take(96)
        .collect::<String>();
    if display.trim().is_empty() {
        "unnamed tool".into()
    } else {
        display
    }
}

fn record_shadow_usage(
    state: &SharedState,
    session: &AgentSession,
    model_id: &str,
    outcome: &ShadowDecisionOutcome,
    valid_decision: bool,
    selected_option: Option<String>,
    selected_probability: Option<f64>,
) {
    let response = outcome.response.as_ref();
    let success = valid_decision && response.is_some();
    let record = DecisionUsageRecord {
        id: id(),
        session_id: session.id.clone(),
        session_title: session.title.clone(),
        project_id: session.project_id.clone(),
        model_id: model_id.to_owned(),
        resolved_model_id: response
            .map(|response| response.model_id.clone())
            .filter(|model| !model.trim().is_empty()),
        shadow_mode: true,
        input_tokens: response.map_or(0, |response| response.input_tokens),
        output_tokens: response.map_or(0, |response| response.output_tokens),
        duration_ms: outcome.duration_ms,
        success,
        failure_code: outcome
            .failure_code
            .clone()
            .or_else(|| (!success).then(|| "invalid_decision_response".to_owned())),
        selected_option: if success { selected_option } else { None },
        selected_probability: if success { selected_probability } else { None },
        created_at: now(),
    };
    if let Err(error) = state.database.save_decision_usage(&record) {
        tracing::warn!(session_id = %session.id, code = %error.code, "Could not save Jev shadow usage");
    }
}

fn decision_choice(
    response: &crate::decisions::DecisionResponse,
    name: &str,
) -> AppResult<(String, Option<f64>)> {
    match response.answers.get(name) {
        Some(crate::decisions::DecisionAnswer::Choice {
            choice,
            probabilities,
            confidence,
        }) => Ok((
            choice.clone(),
            probabilities.get(choice).copied().or(*confidence),
        )),
        _ => Err(AppError::new(
            "provider_error",
            "Jev omitted a required decision choice.",
        )),
    }
}

fn decision_score(response: &crate::decisions::DecisionResponse, name: &str) -> AppResult<u8> {
    match response.answers.get(name) {
        Some(crate::decisions::DecisionAnswer::Score { score, .. }) => {
            Ok(score.round().clamp(0.0, u8::MAX as f64) as u8)
        }
        _ => Err(AppError::new(
            "provider_error",
            "Jev omitted a required scope estimate.",
        )),
    }
}

fn decision_probability(
    response: &crate::decisions::DecisionResponse,
    name: &str,
) -> AppResult<f64> {
    match response.answers.get(name) {
        Some(crate::decisions::DecisionAnswer::Noul { value }) => Ok(*value),
        _ => Err(AppError::new(
            "provider_error",
            "Jev omitted a required probability estimate.",
        )),
    }
}

fn add_decision_guidance(messages: &mut Vec<AgentMessage>, guidance: &DecisionGuidance) {
    let message = format!(
        "Bounded Jev assistance (untrusted recommendations): task category {}; suggested first phase {}; secondary context focus {}; consider {} tools first; estimated scope {} of 3; clarification probability {:.0}%. Keep the complete current request and all project instructions in context. You remain responsible for decisions. Use only the tools already provided. Every action must pass the existing permission and workspace checks; this guidance grants no permission and cannot authorize commands or edits. If clarification probability is high, consider asking the user before an irreversible step.",
        guidance.category,
        guidance.workflow,
        guidance.context_focus,
        guidance.tool_focus,
        guidance.scope_score + 1,
        guidance.clarification_probability * 100.0,
    );
    let insertion = messages
        .iter()
        .take_while(|message| message.role == MessageRole::System)
        .count();
    messages.insert(insertion, AgentMessage::text(MessageRole::System, message));
}

/// Stable-sort, never filter: Jev can only put existing eligible definitions
/// earlier in the provider request. Tool validation and permission checks are
/// unchanged and still run when the model requests an action.
fn tool_relevance_priority(name: &str, focus: &str) -> u8 {
    let names: &[&str] = match focus {
        "inspect" => &[
            "inspect_project",
            "list_directory",
            "read_file",
            "read_files",
            "file_metadata",
            "git_status",
        ],
        "search" => &[
            "search_files",
            "search_text",
            "find_symbol",
            "find_references",
        ],
        "edit" => &["apply_patch", "create_file", "move_file", "delete_file"],
        "verify" => &["git_status", "git_diff", "run_command"],
        "respond" => &["ask_user"],
        _ => &[],
    };
    u8::from(!names.contains(&name))
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
        "run_command" => Some(if is_test_invocation(call) {
            AgentActivityKind::TestRun
        } else {
            AgentActivityKind::CommandExecuted
        }),
        "apply_patch" | "create_file" | "delete_file" | "move_file" => {
            Some(AgentActivityKind::FileEdited)
        }
        _ => None,
    }
}

fn is_test_invocation(call: &ToolCall) -> bool {
    let program = call.arguments["program"]
        .as_str()
        .unwrap_or_default()
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let args = call.arguments["args"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .map(|arg| arg.to_ascii_lowercase())
        .collect::<Vec<_>>();
    let known_test_commands = [
        "pytest",
        "jest",
        "vitest",
        "mocha",
        "unittest",
        "tox",
        "nosetests",
        "nextest",
    ];
    known_test_commands.contains(&program.as_str())
        || args.iter().any(|arg| {
            matches!(
                arg.as_str(),
                "test"
                    | "tests"
                    | "pytest"
                    | "jest"
                    | "vitest"
                    | "mocha"
                    | "unittest"
                    | "tox"
                    | "check-tests"
            ) || arg.starts_with("test:")
                || arg.starts_with("test-")
        })
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
        if let Ok(provider) = state.config.provider(&session.provider_id) {
            let stored = state
                .database
                .provider_account(&session.provider_id)
                .ok()
                .flatten();
            if let Ok(Some(account)) =
                crate::auth::account_after_request_error(provider, stored, &error.code)
            {
                if state.database.save_provider_account(&account).is_ok() {
                    sink.provider_account_updated(&account);
                }
            }
        }
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
    let adapter = match provider_override {
        Some(provider) => provider,
        None => providers::adapter(provider_config, state.clone())?,
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
    use crate::decisions::DecisionResponse;
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
        captured_messages: Arc<Mutex<Vec<Vec<AgentMessage>>>>,
        captured_tools: Arc<Mutex<Vec<Vec<String>>>>,
    }

    #[derive(Clone)]
    struct MockExternalTool {
        calls: Arc<Mutex<u32>>,
    }

    struct BlockingDecisionEngine {
        started: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl crate::decisions::DecisionEngine for BlockingDecisionEngine {
        async fn evaluate(
            &self,
            _request: crate::decisions::DecisionRequest,
        ) -> AppResult<DecisionResponse> {
            self.started.notify_one();
            std::future::pending().await
        }
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
                captured_messages: Arc::new(Mutex::new(Vec::new())),
                captured_tools: Arc::new(Mutex::new(Vec::new())),
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
        async fn complete(&self, request: ProviderRequest<'_>) -> AppResult<ProviderResponse> {
            *self.calls.lock().unwrap() += 1;
            self.captured_messages
                .lock()
                .unwrap()
                .push(request.messages.to_vec());
            self.captured_tools
                .lock()
                .unwrap()
                .push(request.tools.iter().map(|tool| tool.name.clone()).collect());
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
    async fn advisory_jev_decision_is_recorded_separately_without_controlling_task() {
        let root = tempfile::tempdir().unwrap();
        let (state, _, session) = setup_session(root.path());
        state
            .database
            .save_decision_preferences(&DecisionPreferences {
                mode: DecisionMode::Advisory,
                model_id: "jev-test".into(),
            })
            .unwrap();
        let receiver = state.reserve_run(&session.id).unwrap();
        let sink = Sink::default();
        let provider = MockProvider::new(vec![response("Task completed.", vec![])]);
        let engine = crate::decisions::MockDecisionEngine::with_responses([DecisionResponse {
            model_id: "jev-test".into(),
            answers: BTreeMap::from([(
                "task_category".into(),
                crate::decisions::DecisionAnswer::Choice {
                    choice: "coding".into(),
                    probabilities: BTreeMap::from([
                        ("coding".into(), 0.9),
                        ("debugging".into(), 0.04),
                        ("review".into(), 0.03),
                        ("explanation".into(), 0.03),
                    ]),
                    confidence: Some(0.9),
                },
            )]),
            input_tokens: 12,
            output_tokens: 4,
            request_id: Some("mock-decision".into()),
        }]);

        run_inner(
            state.clone(),
            Box::new(sink),
            session.clone(),
            receiver,
            Some(Box::new(provider)),
            AgentRuntime::default().with_decision_engine(Arc::new(engine)),
        )
        .await;

        let saved = state.database.session(&session.id).unwrap();
        assert_eq!(saved.status, SessionStatus::Completed);
        assert!(saved.activity_events.iter().any(|event| {
            event.kind == AgentActivityKind::DecisionAdvisory
                && event.summary.contains("implementation")
        }));
        assert_eq!(state.database.usage().unwrap().len(), 1);
        let decisions = state.database.decision_usage().unwrap();
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].selected_option.as_deref(), Some("coding"));
        assert_eq!(decisions[0].input_tokens, 12);
        assert!(decisions[0].success);
    }

    #[tokio::test]
    async fn unavailable_jev_advisory_is_audited_but_does_not_block_task() {
        let root = tempfile::tempdir().unwrap();
        let (state, _, session) = setup_session(root.path());
        state
            .database
            .save_decision_preferences(&DecisionPreferences {
                mode: DecisionMode::Advisory,
                model_id: "jev-test".into(),
            })
            .unwrap();
        let receiver = state.reserve_run(&session.id).unwrap();
        let provider = MockProvider::new(vec![response("Task completed.", vec![])]);
        let runtime = AgentRuntime::default()
            .with_decision_engine(Arc::new(crate::decisions::MockDecisionEngine::default()));

        run_inner(
            state.clone(),
            Box::new(Sink::default()),
            session.clone(),
            receiver,
            Some(Box::new(provider)),
            runtime,
        )
        .await;

        assert_eq!(
            state.database.session(&session.id).unwrap().status,
            SessionStatus::Completed
        );
        let decisions = state.database.decision_usage().unwrap();
        assert_eq!(decisions.len(), 1);
        assert!(!decisions[0].success);
        assert_eq!(
            decisions[0].failure_code.as_deref(),
            Some("decision_unavailable")
        );
        assert_eq!(state.database.usage().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn assisted_jev_ranks_hints_without_removing_tools_or_bypassing_policy() {
        let root = tempfile::tempdir().unwrap();
        let (state, _, mut session) = setup_session(root.path());
        session.permission_policy.shell = PermissionDecision::Deny;
        session.messages.insert(
            0,
            AgentMessage::text(
                MessageRole::System,
                "Project instructions: preserve every user requirement.",
            ),
        );
        state.database.save_session(&session).unwrap();
        state
            .database
            .save_decision_preferences(&DecisionPreferences {
                mode: DecisionMode::Assisted,
                model_id: "jev-test".into(),
            })
            .unwrap();
        let receiver = state.reserve_run(&session.id).unwrap();
        let provider = MockProvider::new(vec![response("Task completed.", vec![])]);
        let captured_messages = provider.captured_messages.clone();
        let captured_tools = provider.captured_tools.clone();
        let engine = crate::decisions::MockDecisionEngine::with_responses([DecisionResponse {
            model_id: "jev-test".into(),
            answers: BTreeMap::from([
                (
                    "task_category".into(),
                    crate::decisions::DecisionAnswer::Choice {
                        choice: "review".into(),
                        probabilities: BTreeMap::from([
                            ("coding".into(), 0.05),
                            ("debugging".into(), 0.05),
                            ("review".into(), 0.85),
                            ("explanation".into(), 0.05),
                        ]),
                        confidence: Some(0.85),
                    },
                ),
                (
                    "workflow".into(),
                    crate::decisions::DecisionAnswer::Choice {
                        choice: "verify".into(),
                        probabilities: BTreeMap::from([
                            ("explore".into(), 0.05),
                            ("implement".into(), 0.05),
                            ("verify".into(), 0.85),
                            ("explain".into(), 0.05),
                        ]),
                        confidence: Some(0.85),
                    },
                ),
                (
                    "context_focus".into(),
                    crate::decisions::DecisionAnswer::Choice {
                        choice: "repository_facts".into(),
                        probabilities: BTreeMap::from([
                            ("repository_facts".into(), 0.7),
                            ("recent_activity".into(), 0.1),
                            ("earlier_task_decisions".into(), 0.1),
                            ("current_request".into(), 0.1),
                        ]),
                        confidence: Some(0.7),
                    },
                ),
                (
                    "tool_focus".into(),
                    crate::decisions::DecisionAnswer::Choice {
                        choice: "verify".into(),
                        probabilities: BTreeMap::from([
                            ("inspect".into(), 0.05),
                            ("search".into(), 0.05),
                            ("edit".into(), 0.05),
                            ("verify".into(), 0.8),
                            ("respond".into(), 0.05),
                        ]),
                        confidence: Some(0.8),
                    },
                ),
                (
                    "scope".into(),
                    crate::decisions::DecisionAnswer::Score {
                        score: 2.0,
                        probabilities: BTreeMap::from([
                            ("0".into(), 0.05),
                            ("1".into(), 0.1),
                            ("2".into(), 0.85),
                        ]),
                        confidence: Some(0.85),
                    },
                ),
                (
                    "needs_clarification".into(),
                    crate::decisions::DecisionAnswer::Noul { value: 0.8 },
                ),
            ]),
            input_tokens: 30,
            output_tokens: 12,
            request_id: None,
        }]);

        run_inner(
            state.clone(),
            Box::new(Sink::default()),
            session.clone(),
            receiver,
            Some(Box::new(provider)),
            AgentRuntime::default().with_decision_engine(Arc::new(engine)),
        )
        .await;

        let saved = state.database.session(&session.id).unwrap();
        assert_eq!(saved.status, SessionStatus::Completed);
        assert!(saved.activity_events.iter().any(|event| {
            event.kind == AgentActivityKind::DecisionAdvisory
                && event.summary.contains("focus repository_facts")
                && event.summary.contains("Permissions remain unchanged")
        }));
        let decision_usage = state.database.decision_usage().unwrap();
        assert_eq!(decision_usage.len(), 1);
        assert!(decision_usage[0]
            .selected_option
            .as_deref()
            .unwrap()
            .contains("tools=verify"));
        let provider_messages = captured_messages.lock().unwrap();
        assert!(provider_messages[0].iter().any(|message| {
            message.role == MessageRole::System
                && message.content.contains("Bounded Jev assistance")
        }));
        assert!(provider_messages[0].iter().any(
            |message| message.role == MessageRole::User && message.content == "Inspect README"
        ));
        assert!(provider_messages[0].iter().any(|message| {
            message.role == MessageRole::System
                && message.content == "Project instructions: preserve every user requirement."
        }));
        let tool_names = captured_tools.lock().unwrap();
        let tools = &tool_names[0];
        assert!(!tools.iter().any(|name| name == "run_command"));
        assert!(tools.iter().any(|name| name == "read_file"));
        assert!(tools.iter().any(|name| name == "apply_patch"));
        assert!(
            tools.iter().position(|name| name == "git_status")
                < tools.iter().position(|name| name == "read_file")
        );
    }

    #[tokio::test]
    async fn malformed_assisted_response_fails_closed_to_normal_agent_context() {
        let root = tempfile::tempdir().unwrap();
        let (state, _, mut session) = setup_session(root.path());
        session.permission_policy.shell = PermissionDecision::Deny;
        state.database.save_session(&session).unwrap();
        state
            .database
            .save_decision_preferences(&DecisionPreferences {
                mode: DecisionMode::Assisted,
                model_id: "jev-test".into(),
            })
            .unwrap();
        let receiver = state.reserve_run(&session.id).unwrap();
        let provider = MockProvider::new(vec![response("Normal path completed.", vec![])]);
        let captured_messages = provider.captured_messages.clone();
        let captured_tools = provider.captured_tools.clone();
        let engine = crate::decisions::MockDecisionEngine::with_responses([DecisionResponse {
            model_id: "jev-test".into(),
            answers: BTreeMap::from([(
                "task_category".into(),
                crate::decisions::DecisionAnswer::Choice {
                    choice: "coding".into(),
                    probabilities: BTreeMap::from([
                        ("coding".into(), 0.9),
                        ("debugging".into(), 0.03),
                        ("review".into(), 0.04),
                        ("explanation".into(), 0.03),
                    ]),
                    confidence: Some(0.9),
                },
            )]),
            input_tokens: 20,
            output_tokens: 2,
            request_id: None,
        }]);

        run_inner(
            state.clone(),
            Box::new(Sink::default()),
            session.clone(),
            receiver,
            Some(Box::new(provider)),
            AgentRuntime::default().with_decision_engine(Arc::new(engine)),
        )
        .await;

        assert_eq!(
            state.database.session(&session.id).unwrap().status,
            SessionStatus::Completed
        );
        let decisions = state.database.decision_usage().unwrap();
        assert_eq!(decisions.len(), 1);
        assert!(!decisions[0].success);
        assert_eq!(decisions[0].failure_code.as_deref(), Some("provider_error"));
        assert!(decisions[0].selected_option.is_none());
        assert!(!captured_messages.lock().unwrap()[0]
            .iter()
            .any(|message| message.content.contains("Bounded Jev assistance")));
        assert!(!captured_tools.lock().unwrap()[0]
            .iter()
            .any(|name| name == "run_command"));
    }

    #[tokio::test]
    async fn jev_is_not_called_when_decision_mode_is_off() {
        let root = tempfile::tempdir().unwrap();
        let (state, _, session) = setup_session(root.path());
        let receiver = state.reserve_run(&session.id).unwrap();
        let provider = MockProvider::new(vec![response("Task completed.", vec![])]);

        run_inner(
            state.clone(),
            Box::new(Sink::default()),
            session.clone(),
            receiver,
            Some(Box::new(provider)),
            AgentRuntime::default()
                .with_decision_engine(Arc::new(crate::decisions::MockDecisionEngine::default())),
        )
        .await;

        assert_eq!(
            state.database.session(&session.id).unwrap().status,
            SessionStatus::Completed
        );
        assert!(state.database.decision_usage().unwrap().is_empty());
    }

    #[tokio::test]
    async fn cancelling_during_a_jev_decision_stops_before_provider_request() {
        let root = tempfile::tempdir().unwrap();
        let (state, _, session) = setup_session(root.path());
        state
            .database
            .save_decision_preferences(&DecisionPreferences {
                mode: DecisionMode::Advisory,
                model_id: "jev-test".into(),
            })
            .unwrap();
        let receiver = state.reserve_run(&session.id).unwrap();
        let started = Arc::new(Notify::new());
        let started_signal = Arc::clone(&started);
        let run_state = Arc::clone(&state);
        let run_session = session.clone();
        let task = tokio::spawn(async move {
            run_inner(
                run_state,
                Box::new(Sink::default()),
                run_session,
                receiver,
                Some(Box::new(MockProvider::new(vec![response(
                    "This must not be requested.",
                    vec![],
                )]))),
                AgentRuntime::default().with_decision_engine(Arc::new(BlockingDecisionEngine {
                    started: started_signal,
                })),
            )
            .await;
        });
        started.notified().await;
        state
            .runs
            .lock()
            .unwrap()
            .get(&session.id)
            .unwrap()
            .send(true)
            .unwrap();
        task.await.unwrap();

        assert_eq!(
            state.database.session(&session.id).unwrap().status,
            SessionStatus::Cancelled
        );
        assert!(state.database.usage().unwrap().is_empty());
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
