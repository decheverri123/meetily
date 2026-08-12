use chrono::{DateTime, Utc};
use tauri::{AppHandle, Runtime};

use crate::database::models::{
    ModelAggregate, TimeBucket, TimeBucketAggregate, TokenUsage, UsageQueryOpts,
};
use crate::database::repositories::token_usage::TokenUsageRepository;
use crate::state::AppState;

fn parse_rfc3339(s: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| format!("Invalid timestamp (expected RFC 3339): {}", e))
}

#[tauri::command]
pub async fn api_record_token_usage<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    usage: TokenUsage,
) -> Result<i64, String> {
    let pool = state.db_manager.pool();
    TokenUsageRepository::record_usage(pool, &usage)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn api_list_token_usage<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    opts: UsageQueryOpts,
) -> Result<Vec<TokenUsage>, String> {
    let pool = state.db_manager.pool();
    TokenUsageRepository::list_usage(pool, opts)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn api_aggregate_token_usage_by_model<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    since: Option<String>,
) -> Result<Vec<ModelAggregate>, String> {
    let since = match since {
        Some(s) => Some(parse_rfc3339(&s)?),
        None => None,
    };
    let pool = state.db_manager.pool();
    TokenUsageRepository::aggregate_by_model(pool, since)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn api_aggregate_token_usage_over_time<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    bucket: String,
    since: String,
) -> Result<Vec<TimeBucketAggregate>, String> {
    let bucket = match bucket.as_str() {
        "hour" => TimeBucket::Hour,
        "day" => TimeBucket::Day,
        "month" => TimeBucket::Month,
        other => return Err(format!("Invalid bucket: {} (expected hour|day|month)", other)),
    };
    let since = parse_rfc3339(&since)?;

    let pool = state.db_manager.pool();
    TokenUsageRepository::aggregate_over_time(pool, bucket, since)
        .await
        .map_err(|e| e.to_string())
}
