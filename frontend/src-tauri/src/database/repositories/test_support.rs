//! Shared setup helpers for repository unit tests. Extracted because
//! `meeting.rs` and `summary.rs` each need an in-memory, migrated pool plus a
//! seed `meetings` row (for the `meeting_id` foreign key their own test
//! tables reference) - kept in one place so the two test modules can't drift.
#![cfg(test)]

use chrono::Utc;
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::SqlitePool;

pub async fn setup_pool() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("failed to open in-memory sqlite db");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("failed to run migrations");
    pool
}

pub async fn insert_meeting(pool: &SqlitePool, id: &str) {
    sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at) VALUES (?, ?, ?, ?)")
        .bind(id)
        .bind(format!("Meeting {}", id))
        .bind(Utc::now())
        .bind(Utc::now())
        .execute(pool)
        .await
        .expect("failed to insert meeting");
}
