//! Provider-neutral decision interface for small, typed workflow judgments.
//! Decision results never grant tool permissions by themselves.

mod evaluation;
mod jev;

pub use evaluation::replay_evaluation_suite;
pub use jev::JevClient;

#[cfg(test)]
use crate::error::AppError;
use crate::error::AppResult;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
#[cfg(test)]
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DecisionQuestion {
    Noul {
        instructions: String,
    },
    Choice {
        instructions: String,
        criteria: BTreeMap<String, String>,
    },
    Score {
        instructions: String,
        criteria: Vec<String>,
    },
}

#[derive(Debug, Clone)]
pub struct DecisionRequest {
    pub model_id: String,
    pub state: Value,
    pub questions: BTreeMap<String, DecisionQuestion>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionAnswer {
    Noul {
        value: f64,
    },
    Choice {
        choice: String,
        probabilities: BTreeMap<String, f64>,
        confidence: Option<f64>,
    },
    Score {
        score: f64,
        probabilities: BTreeMap<String, f64>,
        confidence: Option<f64>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionResponse {
    pub model_id: String,
    pub answers: BTreeMap<String, DecisionAnswer>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub request_id: Option<String>,
}

#[async_trait]
pub trait DecisionEngine: Send + Sync {
    async fn evaluate(&self, request: DecisionRequest) -> AppResult<DecisionResponse>;
}

/// A deterministic test engine whose queued responses avoid network calls.
#[derive(Clone, Default)]
#[cfg(test)]
pub struct MockDecisionEngine {
    responses: Arc<Mutex<VecDeque<DecisionResponse>>>,
}

#[cfg(test)]
impl MockDecisionEngine {
    pub fn with_responses(responses: impl IntoIterator<Item = DecisionResponse>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses.into_iter().collect())),
        }
    }
}

#[cfg(test)]
#[async_trait]
impl DecisionEngine for MockDecisionEngine {
    async fn evaluate(&self, _request: DecisionRequest) -> AppResult<DecisionResponse> {
        self.responses
            .lock()
            .map_err(AppError::internal)?
            .pop_front()
            .ok_or_else(|| AppError::new("decision_unavailable", "No mock decision is queued."))
    }
}

/// Decision questions shared by live Assisted/Shadow requests and the synthetic
/// evaluation suite. The request objective remains supplied separately.
pub fn runtime_questions(include_guidance: bool) -> BTreeMap<String, DecisionQuestion> {
    let mut questions = BTreeMap::from([(
        "task_category".to_owned(),
        DecisionQuestion::Choice {
            instructions: "Classify the user's current request by its primary intent.".to_owned(),
            criteria: decision_criteria(&[
                ("coding", "Implement or modify project code."),
                ("debugging", "Investigate or fix existing behavior."),
                ("review", "Inspect, evaluate, or review existing work."),
                (
                    "explanation",
                    "Give information, a plan, or an explanation.",
                ),
            ]),
        },
    )]);
    if !include_guidance {
        return questions;
    }

    questions.insert(
        "workflow".to_owned(),
        DecisionQuestion::Choice {
            instructions: "Choose a useful first workflow phase; the coding model retains control of execution.".to_owned(),
            criteria: decision_criteria(&[
                ("explore", "Inspect relevant project context before deciding."),
                ("implement", "The request is specific enough to begin implementation."),
                ("verify", "Prioritize validation or tests for this request."),
                ("explain", "Answer or plan without changing files."),
            ]),
        },
    );
    questions.insert(
        "context_focus".to_owned(),
        DecisionQuestion::Choice {
            instructions: "Rank one secondary context source for attention. The current request and project instructions must always remain in context.".to_owned(),
            criteria: decision_criteria(&[
                ("repository_facts", "Previously discovered repository facts."),
                ("recent_activity", "Recent visible tool and task activity."),
                ("earlier_task_decisions", "Earlier user-approved decisions in this task."),
                ("current_request", "The latest user request, always retained regardless."),
            ]),
        },
    );
    questions.insert(
        "tool_focus".to_owned(),
        DecisionQuestion::Choice {
            instructions: "Choose which existing tool group the coding model should consider first. Do not remove or create tools.".to_owned(),
            criteria: decision_criteria(&[
                ("inspect", "Read-only file and project inspection."),
                ("search", "Repository and symbol search."),
                ("edit", "Workspace file edits."),
                ("verify", "Git inspection and test or build commands."),
                ("respond", "Answer or ask the user before acting."),
            ]),
        },
    );
    questions.insert(
        "scope".to_owned(),
        DecisionQuestion::Score {
            instructions: "Estimate the breadth of the requested change, not its importance."
                .to_owned(),
            criteria: vec![
                "One small file or example".to_owned(),
                "Several files or one module".to_owned(),
                "Cross-cutting project changes".to_owned(),
            ],
        },
    );
    questions.insert(
        "needs_clarification".to_owned(),
        DecisionQuestion::Noul {
            instructions: "Estimate the probability that a missing user decision blocks safe, correct progress. Do not flag a request when a reversible reasonable default is clear.".to_owned(),
        },
    );
    questions
}

fn decision_criteria(options: &[(&str, &str)]) -> BTreeMap<String, String> {
    options
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
}
