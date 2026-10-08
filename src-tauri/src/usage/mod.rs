use crate::{domain::*, error::AppResult, persistence::Database};

pub fn record(
    database: &Database,
    session: &AgentSession,
    input_tokens: u64,
    output_tokens: u64,
    duration_ms: u64,
) -> AppResult<UsageRecord> {
    let record = UsageRecord {
        id: id(),
        session_id: session.id.clone(),
        provider_id: session.provider_id.clone(),
        model_id: session.model_id.clone(),
        input_tokens,
        output_tokens,
        cost_usd: None,
        duration_ms,
        created_at: now(),
    };
    database.save_usage(&record)?;
    Ok(record)
}
