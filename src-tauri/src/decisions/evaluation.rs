use super::{runtime_questions, DecisionAnswer, DecisionEngine, DecisionRequest};
use crate::{
    domain::{now, DecisionEvaluationRecord},
    error::{AppError, AppResult},
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{collections::BTreeMap, time::Instant};

pub const EVALUATION_SUITE_VERSION: &str = "jevcode-stage0-v1";
const CASES: &str = include_str!("evaluation_cases.json");

#[derive(Debug, Deserialize)]
struct EvaluationSuite {
    version: String,
    cases: Vec<EvaluationCase>,
}

#[derive(Debug, Deserialize)]
struct EvaluationCase {
    id: String,
    title: String,
    objective: String,
    expected: BTreeMap<String, Vec<String>>,
}

/// Replay the versioned synthetic suite against a decision engine. The only
/// state sent to the provider is the fixture objective and zeroed availability
/// metadata; no project, session, user prompt, path, or repository text enters
/// this request path.
pub async fn replay_evaluation_suite(
    engine: &dyn DecisionEngine,
    model_id: &str,
) -> AppResult<Vec<DecisionEvaluationRecord>> {
    let suite: EvaluationSuite = serde_json::from_str(CASES).map_err(|_| {
        AppError::new(
            "decision_evaluation_invalid",
            "The local Jev evaluation set could not be loaded.",
        )
    })?;
    if suite.version != EVALUATION_SUITE_VERSION || suite.cases.is_empty() || suite.cases.len() > 16
    {
        return Err(AppError::new(
            "decision_evaluation_invalid",
            "The local Jev evaluation set has an unsupported version or size.",
        ));
    }

    let run_id = crate::domain::id();
    let mut records = Vec::with_capacity(suite.cases.len());
    for case in suite.cases {
        let request = DecisionRequest {
            model_id: model_id.to_owned(),
            state: fixture_state(&case.objective),
            questions: runtime_questions(true),
        };
        let started = Instant::now();
        let result = engine.evaluate(request).await;
        records.push(match result {
            Ok(response) => {
                let actual_answers = response
                    .answers
                    .iter()
                    .map(|(name, answer)| (name.clone(), answer_value(answer)))
                    .collect::<BTreeMap<_, _>>();
                let (matched_fields, mismatched_fields) =
                    compare_expected(&case.expected, &actual_answers);
                DecisionEvaluationRecord {
                    id: crate::domain::id(),
                    run_id: run_id.clone(),
                    suite_version: suite.version.clone(),
                    case_id: case.id,
                    case_title: case.title,
                    requested_model_id: model_id.to_owned(),
                    resolved_model_id: non_empty(response.model_id),
                    expected_answers: case.expected,
                    actual_answers,
                    matched_fields,
                    mismatched_fields,
                    input_tokens: response.input_tokens,
                    output_tokens: response.output_tokens,
                    duration_ms: started.elapsed().as_millis() as u64,
                    failure_code: None,
                    created_at: now(),
                }
            }
            Err(error) => {
                let mismatched_fields = case.expected.keys().cloned().collect();
                DecisionEvaluationRecord {
                    id: crate::domain::id(),
                    run_id: run_id.clone(),
                    suite_version: suite.version.clone(),
                    case_id: case.id,
                    case_title: case.title,
                    requested_model_id: model_id.to_owned(),
                    resolved_model_id: None,
                    expected_answers: case.expected,
                    actual_answers: BTreeMap::new(),
                    matched_fields: Vec::new(),
                    mismatched_fields,
                    input_tokens: 0,
                    output_tokens: 0,
                    duration_ms: started.elapsed().as_millis() as u64,
                    failure_code: Some(error.code),
                    created_at: now(),
                }
            }
        });
    }
    Ok(records)
}

fn fixture_state(objective: &str) -> Value {
    json!({
        "objective": objective,
        "availableContext": {
            "projectInstructionCount": 0,
            "repositoryFactCount": 0,
            "recentActivityKinds": [],
            "hasPriorTaskDecisions": false
        }
    })
}

fn answer_value(answer: &DecisionAnswer) -> String {
    match answer {
        DecisionAnswer::Noul { value } => format!("{value:.4}"),
        DecisionAnswer::Choice { choice, .. } => choice.clone(),
        DecisionAnswer::Score { score, .. } => format!("{score:.2}"),
    }
}

fn compare_expected(
    expected: &BTreeMap<String, Vec<String>>,
    actual: &BTreeMap<String, String>,
) -> (Vec<String>, Vec<String>) {
    let mut matched = Vec::new();
    let mut mismatched = Vec::new();
    for (field, acceptable) in expected {
        if actual
            .get(field)
            .is_some_and(|answer| acceptable.contains(answer))
        {
            matched.push(field.clone());
        } else {
            mismatched.push(field.clone());
        }
    }
    (matched, mismatched)
}

fn non_empty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stage0_replays_the_bounded_synthetic_suite_and_records_model_resolution() {
        let suite: EvaluationSuite = serde_json::from_str(CASES).unwrap();
        let responses = suite
            .cases
            .iter()
            .map(|case| crate::decisions::DecisionResponse {
                model_id: "jev-resolved-test-model".into(),
                answers: case
                    .expected
                    .iter()
                    .map(|(name, values)| {
                        (
                            name.clone(),
                            DecisionAnswer::Choice {
                                choice: values[0].clone(),
                                probabilities: BTreeMap::new(),
                                confidence: Some(1.0),
                            },
                        )
                    })
                    .collect(),
                input_tokens: 10,
                output_tokens: 3,
                request_id: None,
            })
            .collect::<Vec<_>>();
        let engine = crate::decisions::MockDecisionEngine::with_responses(responses);

        let records = replay_evaluation_suite(&engine, "jev-requested-test-model")
            .await
            .unwrap();

        assert_eq!(records.len(), 4);
        assert!(records.iter().all(|record| {
            record.suite_version == EVALUATION_SUITE_VERSION
                && record.requested_model_id == "jev-requested-test-model"
                && record.resolved_model_id.as_deref() == Some("jev-resolved-test-model")
                && record.mismatched_fields.is_empty()
                && record.matched_fields.len() == record.expected_answers.len()
        }));
    }
}
