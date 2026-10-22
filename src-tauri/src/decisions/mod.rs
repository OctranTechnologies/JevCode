//! Provider-neutral decision interface for small, typed workflow judgments.
//! Decision results never grant tool permissions by themselves.

mod jev;

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
