//! Fire-and-forget helpers for recording per-call LLM token usage.
//!
//! Recording happens on a `tokio::spawn`ed task so the LLM call site doesn't
//! pay for an extra round-trip to the SQLite pool on the user-facing path.
//! Recording failures are logged at warn level only and never propagated -
//! the call site already has the generated text in hand and an instrumentation
//! failure must not break a summary, Q&A, or live insight.

use chrono::Utc;
use sqlx::SqlitePool;
use tracing::warn;

use crate::database::models::{TokenUsage, TokenUsagePurpose};
use crate::database::repositories::token_usage::TokenUsageRepository;
use crate::summary::llm_client::LLMUsage;

/// Schedule a `token_usage` row insert on the background runtime. Returns
/// immediately; failures inside the task are logged but never returned.
pub fn record_token_usage(
    pool: SqlitePool,
    meeting_id: Option<String>,
    usage: LLMUsage,
    purpose: TokenUsagePurpose,
) {
    tokio::spawn(async move {
        let record = TokenUsage {
            id: 0,
            meeting_id,
            provider: usage.provider.as_str().to_string(),
            model: usage.model,
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            total_tokens: usage.total_tokens,
            estimated_cost_usd: None,
            purpose: purpose.into(),
            created_at: Utc::now(),
            metadata: None,
        };
        if let Err(e) = TokenUsageRepository::record_usage(&pool, &record).await {
            warn!("failed to record token usage: {e}");
        }
    });
}
