use crate::database::models::SummaryProcess;
use chrono::Utc;
use serde_json::Value;
use sqlx::SqlitePool;
use std::collections::HashMap;
use tracing::{error, info as log_info};

pub struct SummaryProcessesRepository;

impl SummaryProcessesRepository {
    /// Retrieves the current summary process state for a given meeting ID.
    pub async fn get_summary_data(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<Option<SummaryProcess>, sqlx::Error> {
        sqlx::query_as::<_, SummaryProcess>("SELECT * FROM summary_processes WHERE meeting_id = ?")
            .bind(meeting_id)
            .fetch_optional(pool)
            .await
    }

    pub async fn update_meeting_summary(
        pool: &SqlitePool,
        meeting_id: &str,
        summary: &Value,
    ) -> Result<bool, sqlx::Error> {
        let mut transaction = pool.begin().await?;

        let meeting_exists: bool = sqlx::query("SELECT 1 FROM meetings WHERE id = ?")
            .bind(meeting_id)
            .fetch_optional(&mut *transaction)
            .await?
            .is_some();

        if !meeting_exists {
            log_info!(
                "Attempted to save summary for a non-existent meeting_id: {}",
                meeting_id
            );
            transaction.rollback().await?;
            return Ok(false);
        }

        let result_json = serde_json::to_string(summary);
        if result_json.is_err() {
            error!("Can't convert the json to string for saving to Database");
            transaction.rollback().await?;
            return Ok(false);
        }
        let now = Utc::now();

        sqlx::query("UPDATE summary_processes SET result = ?, updated_at = ? WHERE meeting_id = ?")
            .bind(&result_json.unwrap())
            .bind(now)
            .bind(meeting_id)
            .execute(&mut *transaction)
            .await?;

        sqlx::query("UPDATE meetings SET updated_at = ? WHERE id = ?")
            .bind(now)
            .bind(meeting_id)
            .execute(&mut *transaction)
            .await?;

        transaction.commit().await?;

        log_info!(
            "Successfully updated summary and timestamp for meeting_id: {}",
            meeting_id
        );
        Ok(true)
    }

    pub async fn get_summary_data_for_meeting(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<Option<SummaryProcess>, sqlx::Error> {
        sqlx::query_as::<_, SummaryProcess>(
            "SELECT p.* FROM summary_processes p JOIN transcript_chunks t ON p.meeting_id = t.meeting_id WHERE p.meeting_id = ?",
        )
        .bind(meeting_id)
        .fetch_optional(pool)
        .await
    }

    /// Batched counterpart to `get_summary_data_for_meeting`'s `result` field:
    /// fetches the raw (still-JSON) `result` for every meeting_id in
    /// `meeting_ids` in a single `IN (...)` query instead of one query per
    /// meeting. Keeps the same `JOIN transcript_chunks` requirement
    /// `get_summary_data_for_meeting` uses, so a meeting is only included
    /// here if it also would have been by the single-meeting lookup.
    /// Meetings with no summary_processes row, no transcript_chunks row, or
    /// a NULL `result`, are simply absent from the returned map. Callers
    /// that need the parsed markdown (not the raw JSON) still do that
    /// extraction themselves, same as the single-meeting path.
    pub async fn get_summary_results_for_meetings(
        pool: &SqlitePool,
        meeting_ids: &[String],
    ) -> Result<HashMap<String, String>, sqlx::Error> {
        if meeting_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let placeholders = vec!["?"; meeting_ids.len()].join(",");
        let query = format!(
            "SELECT p.meeting_id, p.result FROM summary_processes p \
             JOIN transcript_chunks t ON p.meeting_id = t.meeting_id \
             WHERE p.meeting_id IN ({}) AND p.result IS NOT NULL",
            placeholders
        );

        let mut q = sqlx::query_as::<_, (String, String)>(&query);
        for id in meeting_ids {
            q = q.bind(id);
        }
        let rows = q.fetch_all(pool).await?;

        Ok(rows.into_iter().collect())
    }

    pub async fn create_or_reset_process(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<(), sqlx::Error> {
        log_info!(
            "Creating or resetting summary process for meeting_id: {}",
            meeting_id
        );
        let now = Utc::now();
        sqlx::query(
            r#"
            INSERT INTO summary_processes (meeting_id, status, created_at, updated_at, start_time, result, error)
            VALUES (?, 'PENDING', ?, ?, ?, NULL, NULL)
            ON CONFLICT(meeting_id) DO UPDATE SET
                status = 'PENDING',
                updated_at = excluded.updated_at,
                start_time = excluded.start_time,
                result_backup = result,
                result_backup_timestamp = excluded.updated_at,
                result = result,
                error = NULL
            "#
        )
        .bind(meeting_id)
        .bind(now)
        .bind(now)
        .bind(now)
        .execute(pool)
        .await?;
        log_info!(
            "Backed up existing summary before regeneration for meeting_id: {}",
            meeting_id
        );
        Ok(())
    }

    pub async fn update_process_completed(
        pool: &SqlitePool,
        meeting_id: &str,
        result: Value, // Keep this as Value to handle both old and new formats if needed
        chunk_count: i64,
        processing_time: f64,
    ) -> Result<(), sqlx::Error> {
        let now = Utc::now();
        let result_str = serde_json::to_string(&result)
            .map_err(|e| sqlx::Error::Protocol(format!("Failed to serialize result: {}", e)))?;

        sqlx::query(
            r#"
            UPDATE summary_processes
            SET status = 'completed', result = ?, updated_at = ?, end_time = ?, chunk_count = ?, processing_time = ?, error = NULL, result_backup = NULL, result_backup_timestamp = NULL
            WHERE meeting_id = ?
            "#
        )
        .bind(result_str)
        .bind(now)
        .bind(now)
        .bind(chunk_count)
        .bind(processing_time)
        .bind(meeting_id)
        .execute(pool)
        .await?;
        log_info!(
            "Summary completed and backup cleared for meeting_id: {}",
            meeting_id
        );
        Ok(())
    }

    pub async fn update_process_failed(
        pool: &SqlitePool,
        meeting_id: &str,
        error: &str,
    ) -> Result<(), sqlx::Error> {
        let now = Utc::now();

        // Restore from backup if it exists, otherwise keep current result
        sqlx::query(
            r#"
            UPDATE summary_processes
            SET
                status = 'failed',
                error = ?,
                updated_at = ?,
                end_time = ?,
                result = COALESCE(result_backup, result),
                result_backup = NULL,
                result_backup_timestamp = NULL
            WHERE meeting_id = ?
            "#,
        )
        .bind(error)
        .bind(now)
        .bind(now)
        .bind(meeting_id)
        .execute(pool)
        .await?;
        log_info!(
            "Summary generation failed and backup restored for meeting_id: {}",
            meeting_id
        );
        Ok(())
    }

    pub async fn update_process_cancelled(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<(), sqlx::Error> {
        let now = Utc::now();

        // Restore from backup if it exists, otherwise keep current result
        sqlx::query(
            r#"
            UPDATE summary_processes
            SET
                status = 'cancelled',
                updated_at = ?,
                end_time = ?,
                error = 'Generation was cancelled by user',
                result = COALESCE(result_backup, result),
                result_backup = NULL,
                result_backup_timestamp = NULL
            WHERE meeting_id = ?
            "#,
        )
        .bind(now)
        .bind(now)
        .bind(meeting_id)
        .execute(pool)
        .await?;
        log_info!(
            "Marked summary process as cancelled and restored backup for meeting_id: {}",
            meeting_id
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::repositories::test_support::{insert_meeting, setup_pool};

    async fn insert_summary_result(pool: &SqlitePool, meeting_id: &str, result: Option<&str>) {
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO summary_processes (meeting_id, status, created_at, updated_at, result) \
             VALUES (?, 'completed', ?, ?, ?)",
        )
        .bind(meeting_id)
        .bind(now)
        .bind(now)
        .bind(result)
        .execute(pool)
        .await
        .expect("failed to insert summary_processes row");
    }

    async fn insert_transcript_chunk(pool: &SqlitePool, meeting_id: &str) {
        sqlx::query(
            "INSERT INTO transcript_chunks (meeting_id, transcript_text, model, model_name, created_at) \
             VALUES (?, 'text', 'ollama', 'llama3', ?)",
        )
        .bind(meeting_id)
        .bind(Utc::now())
        .execute(pool)
        .await
        .expect("failed to insert transcript_chunks row");
    }

    /// Verifies the batched `get_summary_results_for_meetings` returns
    /// exactly what the old N+1 per-meeting loop (calling
    /// `get_summary_data_for_meeting` once per id and reading `.result`)
    /// would have produced, across the mix of cases `ask_across_meetings`
    /// actually hits: a meeting with a summary, one whose summary_processes
    /// row has a NULL result, one with no summary_processes row at all, and
    /// - matching `get_summary_data_for_meeting`'s existing
    /// `JOIN transcript_chunks` requirement - one with a non-NULL result but
    /// no transcript_chunks row.
    #[tokio::test]
    async fn get_summary_results_for_meetings_matches_per_meeting_loop() {
        let pool = setup_pool().await;

        for id in ["m1", "m2", "m3", "m4", "m5"] {
            insert_meeting(&pool, id).await;
        }

        insert_summary_result(&pool, "m1", Some(r#"{"markdown":"Summary one"}"#)).await;
        insert_summary_result(&pool, "m2", Some(r#"{"markdown":"Summary two"}"#)).await;
        insert_summary_result(&pool, "m3", None).await;
        // m4: no summary_processes row at all.
        insert_summary_result(&pool, "m5", Some(r#"{"markdown":"Summary five"}"#)).await;

        // Only meetings expected to survive the JOIN get a transcript_chunks row.
        insert_transcript_chunk(&pool, "m1").await;
        insert_transcript_chunk(&pool, "m2").await;
        insert_transcript_chunk(&pool, "m3").await;
        // m5 deliberately has no transcript_chunks row.

        let meeting_ids: Vec<String> = ["m1", "m2", "m3", "m4", "m5"]
            .iter()
            .map(|s| s.to_string())
            .collect();

        let mut expected: HashMap<String, String> = HashMap::new();
        for id in &meeting_ids {
            if let Some(process) =
                SummaryProcessesRepository::get_summary_data_for_meeting(&pool, id)
                    .await
                    .expect("get_summary_data_for_meeting failed")
            {
                if let Some(result) = process.result {
                    expected.insert(id.clone(), result);
                }
            }
        }

        let actual =
            SummaryProcessesRepository::get_summary_results_for_meetings(&pool, &meeting_ids)
                .await
                .expect("get_summary_results_for_meetings failed");

        assert_eq!(actual, expected);
        assert_eq!(actual.len(), 2);
        assert_eq!(actual.get("m1").unwrap(), r#"{"markdown":"Summary one"}"#);
        assert_eq!(actual.get("m2").unwrap(), r#"{"markdown":"Summary two"}"#);
        assert!(!actual.contains_key("m3"));
        assert!(!actual.contains_key("m4"));
        assert!(!actual.contains_key("m5"));
    }

    #[tokio::test]
    async fn get_summary_results_for_meetings_empty_input_returns_empty_map() {
        let pool = setup_pool().await;
        let actual = SummaryProcessesRepository::get_summary_results_for_meetings(&pool, &[])
            .await
            .expect("query failed");
        assert!(actual.is_empty());
    }
}
