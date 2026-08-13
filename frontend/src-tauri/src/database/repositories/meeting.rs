use crate::api::{MeetingDetails, MeetingTranscript};
use crate::database::models::{MeetingModel, Transcript};
use chrono::Utc;
use sqlx::{Connection, Error as SqlxError, SqliteConnection, SqlitePool};
use tracing::{error, info};

pub struct MeetingsRepository;

impl MeetingsRepository {
    pub async fn get_meetings(pool: &SqlitePool) -> Result<Vec<MeetingModel>, sqlx::Error> {
        let meetings =
            sqlx::query_as::<_, MeetingModel>("SELECT * FROM meetings ORDER BY created_at DESC")
                .fetch_all(pool)
                .await?;
        Ok(meetings)
    }

    pub async fn get_meetings_by_ids(
        pool: &SqlitePool,
        ids: &[String],
    ) -> Result<Vec<MeetingModel>, sqlx::Error> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders: Vec<&str> = ids.iter().map(|_| "?").collect();
        let query = format!(
            "SELECT * FROM meetings WHERE id IN ({}) ORDER BY created_at DESC",
            placeholders.join(",")
        );
        let mut q = sqlx::query_as::<_, MeetingModel>(&query);
        for id in ids {
            q = q.bind(id);
        }
        q.fetch_all(pool).await
    }

    pub async fn delete_meeting(pool: &SqlitePool, meeting_id: &str) -> Result<bool, SqlxError> {
        if meeting_id.trim().is_empty() {
            return Err(SqlxError::Protocol(
                "meeting_id cannot be empty".to_string(),
            ));
        }

        let mut conn = pool.acquire().await?;
        let mut transaction = conn.begin().await?;

        match delete_meeting_with_transaction(&mut transaction, meeting_id).await {
            Ok(success) => {
                if success {
                    transaction.commit().await?;
                    info!(
                        "Successfully deleted meeting {} and all associated data",
                        meeting_id
                    );
                    Ok(true)
                } else {
                    transaction.rollback().await?;
                    Ok(false)
                }
            }
            Err(e) => {
                let _ = transaction.rollback().await;
                error!("Failed to delete meeting {}: {}", meeting_id, e);
                Err(e)
            }
        }
    }

    pub async fn get_meeting(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<Option<MeetingDetails>, SqlxError> {
        if meeting_id.trim().is_empty() {
            return Err(SqlxError::Protocol(
                "meeting_id cannot be empty".to_string(),
            ));
        }

        let mut conn = pool.acquire().await?;
        let mut transaction = conn.begin().await?;

        // Get meeting details
        let meeting: Option<MeetingModel> =
            sqlx::query_as("SELECT id, title, created_at, updated_at, folder_path, meeting_folder_id, icon FROM meetings WHERE id = ?")
                .bind(meeting_id)
                .fetch_optional(&mut *transaction)
                .await?;

        if meeting.is_none() {
            transaction.rollback().await?;
            return Err(SqlxError::RowNotFound);
        }

        if let Some(meeting) = meeting {
            // Get all transcripts for this meeting
            let transcripts =
                sqlx::query_as::<_, Transcript>("SELECT * FROM transcripts WHERE meeting_id = ?")
                    .bind(meeting_id)
                    .fetch_all(&mut *transaction)
                    .await?;

            transaction.commit().await?;

            // Convert Transcript to MeetingTranscript
            let meeting_transcripts = transcripts
                .into_iter()
                .map(|t| MeetingTranscript {
                    id: t.id,
                    text: t.transcript,
                    timestamp: t.timestamp,
                    audio_start_time: t.audio_start_time,
                    audio_end_time: t.audio_end_time,
                    duration: t.duration,
                })
                .collect::<Vec<_>>();

            Ok(Some(MeetingDetails {
                id: meeting.id,
                title: meeting.title,
                created_at: meeting.created_at.0.to_rfc3339(),
                updated_at: meeting.updated_at.0.to_rfc3339(),
                transcripts: meeting_transcripts,
            }))
        } else {
            transaction.rollback().await?;
            Ok(None)
        }
    }

    /// Get meeting metadata without transcripts (for pagination)
    pub async fn get_meeting_metadata(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Result<Option<MeetingModel>, SqlxError> {
        if meeting_id.trim().is_empty() {
            return Err(SqlxError::Protocol(
                "meeting_id cannot be empty".to_string(),
            ));
        }

        let meeting: Option<MeetingModel> =
            sqlx::query_as("SELECT id, title, created_at, updated_at, folder_path, meeting_folder_id, icon FROM meetings WHERE id = ?")
                .bind(meeting_id)
                .fetch_optional(pool)
                .await?;

        Ok(meeting)
    }

    /// Fetches only enough of a meeting's most recent transcript rows (by
    /// `audio_start_time`, same ordering convention as
    /// `get_meeting_transcripts_paginated`) to cover `max_chars` Unicode
    /// characters, joined in chronological order with "\n" - instead of
    /// `get_meeting`'s full-transcript fetch, which loads every row
    /// regardless of how much of it a caller will actually use. Used by
    /// `ask_about_meeting`, which only ever keeps the tail of the transcript
    /// after truncating to a char budget anyway.
    ///
    /// Each line is prefixed with its `[MM:SS]` recording-relative stamp so
    /// the LLM can cite the lines it used and the UI can resolve those
    /// citations back to transcript segments. The prefixes are not counted
    /// against `max_chars` (see the may-return-more note below).
    ///
    /// Relies on SQLite's `LENGTH()` counting Unicode characters (not bytes)
    /// for well-formed UTF-8 TEXT, matching the char-based truncation the
    /// caller applies on top. May return slightly more than `max_chars`
    /// characters (by up to one row's length), since the row that crosses
    /// the budget is still included whole; callers needing an exact bound
    /// should still apply their own final truncation.
    pub async fn get_recent_transcript_text(
        pool: &SqlitePool,
        meeting_id: &str,
        max_chars: i64,
    ) -> Result<String, SqlxError> {
        if meeting_id.trim().is_empty() {
            return Err(SqlxError::Protocol(
                "meeting_id cannot be empty".to_string(),
            ));
        }
        if max_chars <= 0 {
            return Ok(String::new());
        }

        let rows: Vec<(String, Option<f64>)> = sqlx::query_as(
            r#"
            SELECT transcript, audio_start_time FROM (
                SELECT transcript, audio_start_time, id,
                       SUM(LENGTH(transcript)) OVER (
                           ORDER BY audio_start_time DESC, id DESC
                       ) AS running_chars
                FROM transcripts
                WHERE meeting_id = ?
            ) AS recent
            WHERE running_chars - LENGTH(transcript) < ?
            ORDER BY audio_start_time ASC, id ASC
            "#,
        )
        .bind(meeting_id)
        .bind(max_chars)
        .fetch_all(pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|(text, audio_start_time)| {
                format!(
                    "{} {}",
                    crate::utils::format_recording_time(audio_start_time.unwrap_or(0.0)),
                    text
                )
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }

    /// Get meeting transcripts with pagination support
    pub async fn get_meeting_transcripts_paginated(
        pool: &SqlitePool,
        meeting_id: &str,
        limit: i64,
        offset: i64,
    ) -> Result<(Vec<Transcript>, i64), SqlxError> {
        if meeting_id.trim().is_empty() {
            return Err(SqlxError::Protocol(
                "meeting_id cannot be empty".to_string(),
            ));
        }

        // Get total count of transcripts for this meeting
        let total: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM transcripts WHERE meeting_id = ?"
        )
        .bind(meeting_id)
        .fetch_one(pool)
        .await?;

        // Get paginated transcripts ordered by audio_start_time
        let transcripts = sqlx::query_as::<_, Transcript>(
            "SELECT * FROM transcripts
             WHERE meeting_id = ?
             ORDER BY audio_start_time ASC
             LIMIT ? OFFSET ?"
        )
        .bind(meeting_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?;

        Ok((transcripts, total.0))
    }

    pub async fn update_meeting_title(
        pool: &SqlitePool,
        meeting_id: &str,
        new_title: &str,
    ) -> Result<bool, SqlxError> {
        if meeting_id.trim().is_empty() {
            return Err(SqlxError::Protocol(
                "meeting_id cannot be empty".to_string(),
            ));
        }

        let mut conn = pool.acquire().await?;
        let mut transaction = conn.begin().await?;

        let now = Utc::now().naive_utc();

        let rows_affected =
            sqlx::query("UPDATE meetings SET title = ?, updated_at = ? WHERE id = ?")
                .bind(new_title)
                .bind(now)
                .bind(meeting_id)
                .execute(&mut *transaction)
                .await?
                .rows_affected();
        if rows_affected == 0 {
            transaction.rollback().await?;
            return Ok(false);
        }
        transaction.commit().await?;
        Ok(true)
    }

    pub async fn update_meeting_icon(
        pool: &SqlitePool,
        meeting_id: &str,
        icon: &str,
    ) -> Result<bool, SqlxError> {
        if meeting_id.trim().is_empty() {
            return Err(SqlxError::Protocol(
                "meeting_id cannot be empty".to_string(),
            ));
        }

        let mut conn = pool.acquire().await?;
        let mut transaction = conn.begin().await?;

        let now = Utc::now().naive_utc();

        let rows_affected =
            sqlx::query("UPDATE meetings SET icon = ?, updated_at = ? WHERE id = ?")
                .bind(icon.trim())
                .bind(now)
                .bind(meeting_id)
                .execute(&mut *transaction)
                .await?
                .rows_affected();

        transaction.commit().await?;

        Ok(rows_affected > 0)
    }

    pub async fn update_meeting_name(
        pool: &SqlitePool,
        meeting_id: &str,
        new_title: &str,
    ) -> Result<bool, SqlxError> {
        let mut transaction = pool.begin().await?;
        let now = Utc::now();

        // Update meetings table
        let meeting_update =
            sqlx::query("UPDATE meetings SET title = ?, updated_at = ? WHERE id = ?")
                .bind(new_title)
                .bind(now)
                .bind(meeting_id)
                .execute(&mut *transaction)
                .await?;

        if meeting_update.rows_affected() == 0 {
            transaction.rollback().await?;
            return Ok(false); // Meeting not found
        }

        // Update transcript_chunks table
        sqlx::query("UPDATE transcript_chunks SET meeting_name = ? WHERE meeting_id = ?")
            .bind(new_title)
            .bind(meeting_id)
            .execute(&mut *transaction)
            .await?;

        transaction.commit().await?;
        Ok(true)
    }
}

async fn delete_meeting_with_transaction(
    transaction: &mut SqliteConnection,
    meeting_id: &str,
) -> Result<bool, SqlxError> {
    // Check if meeting exists
    let meeting_exists: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM meetings WHERE id = ?")
        .bind(meeting_id)
        .fetch_optional(&mut *transaction)
        .await?;

    if meeting_exists.is_none() {
        error!("Meeting {} not found for deletion", meeting_id);
        return Ok(false);
    }

    // Delete from related tables in proper order
    // 1. Delete from transcript_chunks
    sqlx::query("DELETE FROM transcript_chunks WHERE meeting_id = ?")
        .bind(meeting_id)
        .execute(&mut *transaction)
        .await?;

    // 2. Delete from summary_processes
    sqlx::query("DELETE FROM summary_processes WHERE meeting_id = ?")
        .bind(meeting_id)
        .execute(&mut *transaction)
        .await?;

    // 3. Delete from transcripts
    sqlx::query("DELETE FROM transcripts WHERE meeting_id = ?")
        .bind(meeting_id)
        .execute(&mut *transaction)
        .await?;

    // 4. Finally, delete the meeting
    let result = sqlx::query("DELETE FROM meetings WHERE id = ?")
        .bind(meeting_id)
        .execute(&mut *transaction)
        .await?;

    Ok(result.rows_affected() > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::repositories::test_support::{insert_meeting, setup_pool};

    async fn insert_transcript(
        pool: &SqlitePool,
        id: &str,
        meeting_id: &str,
        text: &str,
        audio_start_time: f64,
    ) {
        sqlx::query(
            "INSERT INTO transcripts (id, meeting_id, transcript, timestamp, audio_start_time) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(meeting_id)
        .bind(text)
        .bind("00:00:00")
        .bind(audio_start_time)
        .execute(pool)
        .await
        .expect("failed to insert transcript");
    }

    /// Five 10-char rows, oldest ("A") to newest ("E") by `audio_start_time`.
    /// A `max_chars` budget of 25 covers "E" + "D" + "C" (30 chars, since the
    /// row that crosses the budget is kept whole) but not "B" - verifying
    /// both that only the most recent rows are fetched and that they come
    /// back in chronological (oldest-of-the-kept-first) order.
    #[tokio::test]
    async fn get_recent_transcript_text_returns_only_most_recent_rows_within_budget() {
        let pool = setup_pool().await;
        insert_meeting(&pool, "m1").await;
        for (i, label) in ["A", "B", "C", "D", "E"].iter().enumerate() {
            insert_transcript(&pool, &format!("t{}", i), "m1", &label.repeat(10), i as f64).await;
        }

        let result = MeetingsRepository::get_recent_transcript_text(&pool, "m1", 25)
            .await
            .expect("query failed");

        assert_eq!(
            result,
            format!(
                "[00:02] {}\n[00:03] {}\n[00:04] {}",
                "C".repeat(10),
                "D".repeat(10),
                "E".repeat(10)
            )
        );
        assert!(!result.contains('B'));
        assert!(!result.contains('A'));
    }

    #[tokio::test]
    async fn get_recent_transcript_text_returns_everything_when_within_budget() {
        let pool = setup_pool().await;
        insert_meeting(&pool, "m1").await;
        for (i, label) in ["A", "B", "C"].iter().enumerate() {
            insert_transcript(&pool, &format!("t{}", i), "m1", &label.repeat(5), i as f64).await;
        }

        let result = MeetingsRepository::get_recent_transcript_text(&pool, "m1", 10_000)
            .await
            .expect("query failed");

        assert_eq!(
            result,
            format!(
                "[00:00] {}\n[00:01] {}\n[00:02] {}",
                "A".repeat(5),
                "B".repeat(5),
                "C".repeat(5)
            )
        );
    }

    #[tokio::test]
    async fn get_recent_transcript_text_zero_budget_returns_empty_string() {
        let pool = setup_pool().await;
        insert_meeting(&pool, "m1").await;
        insert_transcript(&pool, "t0", "m1", "some text", 0.0).await;

        let result = MeetingsRepository::get_recent_transcript_text(&pool, "m1", 0)
            .await
            .expect("query failed");

        assert_eq!(result, "");
    }

    #[tokio::test]
    async fn get_recent_transcript_text_no_transcripts_returns_empty_string() {
        let pool = setup_pool().await;
        insert_meeting(&pool, "m1").await;

        let result = MeetingsRepository::get_recent_transcript_text(&pool, "m1", 1000)
            .await
            .expect("query failed");

        assert_eq!(result, "");
    }
}
