use crate::{domain::*, error::AppResult, persistence::Database};

mod pricing;
pub use pricing::estimate_cost;

pub struct RequestMetrics {
    pub input_tokens: u64,
    pub usage_available: bool,
    pub cached_input_tokens: Option<u64>,
    pub output_tokens: u64,
    pub duration_ms: u64,
    pub failure_code: Option<String>,
}

pub fn record(
    database: &Database,
    session: &AgentSession,
    model: &Model,
    metrics: RequestMetrics,
) -> AppResult<UsageRecord> {
    let RequestMetrics {
        input_tokens,
        usage_available,
        cached_input_tokens,
        output_tokens,
        duration_ms,
        failure_code,
    } = metrics;
    let success = failure_code.is_none();
    let record = UsageRecord {
        id: id(),
        session_id: session.id.clone(),
        project_id: session.project_id.clone(),
        provider_id: session.provider_id.clone(),
        model_id: session.model_id.clone(),
        input_tokens,
        cached_input_tokens,
        output_tokens,
        cost_usd: (success && usage_available)
            .then(|| estimate_cost(model, input_tokens, cached_input_tokens, output_tokens))
            .flatten(),
        duration_ms,
        success,
        usage_available,
        failure_code,
        created_at: now(),
    };
    database.save_usage(&record)?;
    Ok(record)
}
