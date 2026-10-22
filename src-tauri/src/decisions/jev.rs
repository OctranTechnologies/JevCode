use super::{DecisionAnswer, DecisionEngine, DecisionQuestion, DecisionRequest, DecisionResponse};
use crate::{
    credentials::CredentialStore,
    error::{AppError, AppResult},
};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::{collections::BTreeMap, time::Duration};
use zeroize::Zeroizing;

const API_BASE: &str = "https://api.typesafe.ai/v1";
const API_KEY_ID: &str = "typesafe-jev";
const DEFAULT_MODEL: &str = "jev-latest";

pub struct JevClient {
    http: reqwest::Client,
    api_key: Zeroizing<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JevModel {
    pub name: String,
    pub description: String,
    pub release_date: String,
}

#[derive(Debug, Deserialize)]
struct JevModelList {
    models: Vec<JevModel>,
}

#[derive(Debug, Deserialize)]
struct JevEnvelope {
    model: String,
    answers: BTreeMap<String, JevRawAnswer>,
    usage: JevUsage,
    #[serde(default)]
    id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct JevUsage {
    input_tokens: u64,
    output_tokens: u64,
}

#[derive(Debug, Deserialize)]
struct JevRawAnswer {
    #[serde(rename = "type")]
    kind: String,
    noul: Option<f64>,
    choice: Option<String>,
    score: Option<f64>,
    probabilities: Option<BTreeMap<String, f64>>,
    confidence: Option<f64>,
}

impl JevClient {
    pub fn new(credentials: &CredentialStore) -> AppResult<Self> {
        let api_key = credentials.get(API_KEY_ID)?.ok_or_else(|| {
            AppError::new(
                "credentials_required",
                "Connect a TypeSafe API key to use Jev.",
            )
        })?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(8))
            .build()
            .map_err(|_| AppError::new("network_error", "Could not prepare the Jev connection."))?;
        Ok(Self {
            http,
            api_key: Zeroizing::new(api_key),
        })
    }

    pub async fn validate_key(credentials: &CredentialStore, secret: &str) -> AppResult<()> {
        if secret.trim().len() < 12 || secret.len() > 16_384 {
            return Err(AppError::new(
                "invalid_credential",
                "Enter a valid TypeSafe API key.",
            ));
        }
        let client = Self::with_key(secret.trim())?;
        let models = client.available_models().await?;
        if models.is_empty() {
            return Err(AppError::new(
                "provider_unavailable",
                "TypeSafe did not return any available Jev models.",
            ));
        }
        credentials.save(API_KEY_ID, secret.trim())
    }

    fn with_key(secret: &str) -> AppResult<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(8))
            .build()
            .map_err(|_| AppError::new("network_error", "Could not prepare the Jev connection."))?;
        Ok(Self {
            http,
            api_key: Zeroizing::new(secret.to_owned()),
        })
    }

    pub async fn available_models(&self) -> AppResult<Vec<JevModel>> {
        let response = self
            .http
            .get(format!("{API_BASE}/models"))
            .bearer_auth(self.api_key.as_str())
            .send()
            .await
            .map_err(|_| AppError::new("network_error", "Could not connect to TypeSafe."))?;
        if !response.status().is_success() {
            return Err(map_http_error(response.status()));
        }
        response
            .json::<JevModelList>()
            .await
            .map(|catalog| catalog.models)
            .map_err(|_| {
                AppError::new(
                    "provider_error",
                    "TypeSafe returned an invalid Jev model catalog.",
                )
            })
    }
}

#[async_trait]
impl DecisionEngine for JevClient {
    async fn evaluate(&self, request: DecisionRequest) -> AppResult<DecisionResponse> {
        validate_request(&request)?;
        let questions = request
            .questions
            .iter()
            .map(|(name, question)| (name.clone(), question_value(question)))
            .collect::<Map<_, _>>();
        let body = json!({
            "model": request.model_id,
            "state": request.state,
            "questions": questions,
        });
        let response = self
            .http
            .post(format!("{API_BASE}/systemone"))
            .bearer_auth(self.api_key.as_str())
            .json(&body)
            .send()
            .await
            .map_err(|_| AppError::new("network_error", "Could not connect to TypeSafe."))?;
        if !response.status().is_success() {
            return Err(map_http_error(response.status()));
        }
        let envelope = response.json::<JevEnvelope>().await.map_err(|_| {
            AppError::new(
                "provider_error",
                "TypeSafe returned an invalid decision response.",
            )
        })?;
        let answers = normalize_answers(&request.questions, envelope.answers)?;
        Ok(DecisionResponse {
            model_id: envelope.model,
            answers,
            input_tokens: envelope.usage.input_tokens,
            output_tokens: envelope.usage.output_tokens,
            request_id: envelope.id,
        })
    }
}

fn validate_request(request: &DecisionRequest) -> AppResult<()> {
    if request.model_id.trim().is_empty()
        || request.model_id.len() > 128
        || request.questions.is_empty()
        || request.questions.len() > 16
        || request.state.to_string().len() > 128 * 1024
    {
        return Err(AppError::new(
            "invalid_decision_request",
            "The Jev decision request is outside the supported size limits.",
        ));
    }
    for (name, question) in &request.questions {
        let instructions = match question {
            DecisionQuestion::Noul { instructions }
            | DecisionQuestion::Choice { instructions, .. }
            | DecisionQuestion::Score { instructions, .. } => instructions,
        };
        if name.is_empty()
            || name.len() > 80
            || instructions.trim().is_empty()
            || instructions.len() > 2_000
        {
            return Err(AppError::new(
                "invalid_decision_request",
                "A Jev question is invalid or too long.",
            ));
        }
        match question {
            DecisionQuestion::Choice { criteria, .. }
                if criteria.len() < 2
                    || criteria.len() > 32
                    || criteria.iter().any(|(key, label)| {
                        key.is_empty()
                            || key.len() > 80
                            || label.trim().is_empty()
                            || label.len() > 500
                    }) =>
            {
                return Err(AppError::new(
                    "invalid_decision_request",
                    "Jev choice criteria must define between 2 and 32 named options.",
                ))
            }
            DecisionQuestion::Score { criteria, .. }
                if criteria.len() < 2
                    || criteria.len() > 10
                    || criteria
                        .iter()
                        .any(|level| level.trim().is_empty() || level.len() > 500) =>
            {
                return Err(AppError::new(
                    "invalid_decision_request",
                    "Jev score criteria must define between 2 and 16 levels.",
                ))
            }
            _ => {}
        }
    }
    Ok(())
}

fn question_value(question: &DecisionQuestion) -> Value {
    match question {
        DecisionQuestion::Noul { instructions } => {
            json!({ "type": "noul", "instructions": instructions })
        }
        DecisionQuestion::Choice {
            instructions,
            criteria,
        } => json!({ "type": "choice", "instructions": instructions, "criteria": criteria }),
        DecisionQuestion::Score {
            instructions,
            criteria,
        } => json!({ "type": "score", "instructions": instructions, "criteria": criteria }),
    }
}

fn normalize_answers(
    questions: &BTreeMap<String, DecisionQuestion>,
    raw_answers: BTreeMap<String, JevRawAnswer>,
) -> AppResult<BTreeMap<String, DecisionAnswer>> {
    let mut answers = BTreeMap::new();
    for (name, question) in questions {
        let raw = raw_answers.get(name).ok_or_else(|| {
            AppError::new("provider_error", "TypeSafe omitted a requested decision.")
        })?;
        let answer = match (question, raw.kind.as_str()) {
            (DecisionQuestion::Noul { .. }, "noul") => {
                let value = raw
                    .noul
                    .filter(|value| value.is_finite() && (0.0..=1.0).contains(value))
                    .ok_or_else(|| {
                        AppError::new(
                            "provider_error",
                            "TypeSafe returned an invalid probability.",
                        )
                    })?;
                DecisionAnswer::Noul { value }
            }
            (DecisionQuestion::Choice { criteria, .. }, "choice") => {
                let choice = raw
                    .choice
                    .as_ref()
                    .filter(|choice| criteria.contains_key(*choice))
                    .cloned()
                    .ok_or_else(|| {
                        AppError::new(
                            "provider_error",
                            "TypeSafe selected an option that was not requested.",
                        )
                    })?;
                let probabilities =
                    validate_probabilities(raw.probabilities.as_ref(), criteria.keys())?;
                let confidence = raw
                    .confidence
                    .filter(|value| value.is_finite() && (0.0..=1.0).contains(value));
                DecisionAnswer::Choice {
                    choice,
                    probabilities,
                    confidence,
                }
            }
            (DecisionQuestion::Score { criteria, .. }, "score") => {
                let score = raw
                    .score
                    .filter(|value| {
                        value.is_finite() && *value >= 0.0 && *value <= (criteria.len() - 1) as f64
                    })
                    .ok_or_else(|| {
                        AppError::new("provider_error", "TypeSafe returned an invalid score.")
                    })?;
                let expected = (0..criteria.len())
                    .map(|index| index.to_string())
                    .collect::<Vec<_>>();
                let probabilities =
                    validate_probabilities(raw.probabilities.as_ref(), expected.iter())?;
                let confidence = validate_confidence(raw.confidence)?;
                DecisionAnswer::Score {
                    score,
                    probabilities,
                    confidence,
                }
            }
            _ => {
                return Err(AppError::new(
                    "provider_error",
                    "TypeSafe returned an answer with the wrong type.",
                ))
            }
        };
        answers.insert(name.clone(), answer);
    }
    Ok(answers)
}

fn validate_probabilities<'a>(
    probabilities: Option<&BTreeMap<String, f64>>,
    expected: impl Iterator<Item = &'a String>,
) -> AppResult<BTreeMap<String, f64>> {
    let probabilities = probabilities.ok_or_else(|| {
        AppError::new("provider_error", "TypeSafe omitted decision probabilities.")
    })?;
    let expected = expected.cloned().collect::<std::collections::BTreeSet<_>>();
    if probabilities.len() != expected.len()
        || probabilities.keys().any(|key| !expected.contains(key))
    {
        return Err(AppError::new(
            "provider_error",
            "TypeSafe returned probabilities for unexpected options.",
        ));
    }
    let mut total = 0.0;
    for key in &expected {
        let value = probabilities.get(key).ok_or_else(|| {
            AppError::new(
                "provider_error",
                "TypeSafe omitted a decision option probability.",
            )
        })?;
        if !value.is_finite() || !(0.0..=1.0).contains(value) {
            return Err(AppError::new(
                "provider_error",
                "TypeSafe returned an invalid decision probability.",
            ));
        }
        total += value;
    }
    if !(0.95..=1.05).contains(&total) {
        return Err(AppError::new(
            "provider_error",
            "TypeSafe returned an incomplete probability distribution.",
        ));
    }
    Ok(probabilities.clone())
}

fn validate_confidence(confidence: Option<f64>) -> AppResult<Option<f64>> {
    confidence
        .map(|value| {
            if value.is_finite() && (0.0..=1.0).contains(&value) {
                Ok(value)
            } else {
                Err(AppError::new(
                    "provider_error",
                    "TypeSafe returned invalid confidence metadata.",
                ))
            }
        })
        .transpose()
}

fn map_http_error(status: reqwest::StatusCode) -> AppError {
    match status.as_u16() {
        401 => AppError::new(
            "invalid_credential",
            "TypeSafe rejected the saved API key. Reconnect Jev in Settings.",
        ),
        429 => AppError::new(
            "rate_limited",
            "Jev reached the TypeSafe rate limit. Retry after a short wait.",
        ),
        529 => AppError::new(
            "provider_outage",
            "TypeSafe is temporarily overloaded. Try again shortly.",
        ),
        400 | 413 | 415 | 422 => AppError::new(
            "invalid_decision_request",
            "TypeSafe could not accept this decision request.",
        ),
        _ => AppError::new(
            "provider_error",
            "TypeSafe could not complete the decision request.",
        ),
    }
}
