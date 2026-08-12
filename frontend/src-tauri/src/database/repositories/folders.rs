use crate::database::models::FolderModel;
use chrono::Utc;
use sqlx::{Error as SqlxError, SqlitePool};
use uuid::Uuid;

pub struct FoldersRepository;

/// True for Unicode code points that are visually invisible / have zero
/// display width and contribute no printable glyph. A folder name composed
/// entirely of these is invisible in the sidebar.
fn is_zero_width(c: char) -> bool {
    matches!(
        c,
        '\u{200B}' | // zero-width space
        '\u{200C}' | // zero-width non-joiner
        '\u{200D}' | // zero-width joiner
        '\u{2060}' | // word joiner
        '\u{FEFF}'   // byte order mark / zero-width no-break space
    )
}

/// True if `s` is empty after stripping whitespace and Unicode zero-width
/// characters. `str::trim()` only handles ASCII + a small set of ASCII-ish
/// whitespace, so a name made of e.g. `\u{200B}\u{200C}\u{FEFF}` passes
/// `trim().is_empty() == false` and would otherwise be stored.
fn is_blank_name(s: &str) -> bool {
    !s.chars().any(|c| !c.is_whitespace() && !is_zero_width(c))
}

impl FoldersRepository {
    pub async fn list_folders(pool: &SqlitePool) -> Result<Vec<FolderModel>, SqlxError> {
        let folders =
            sqlx::query_as::<_, FolderModel>("SELECT * FROM folders ORDER BY name COLLATE NOCASE ASC")
                .fetch_all(pool)
                .await?;
        Ok(folders)
    }

    pub async fn get_folder(
        pool: &SqlitePool,
        folder_id: &str,
    ) -> Result<Option<FolderModel>, SqlxError> {
        if folder_id.trim().is_empty() {
            return Err(SqlxError::Protocol("folder_id cannot be empty".to_string()));
        }
        let folder: Option<FolderModel> =
            sqlx::query_as("SELECT * FROM folders WHERE id = ?")
                .bind(folder_id)
                .fetch_optional(pool)
                .await?;
        Ok(folder)
    }

    pub async fn create_folder(pool: &SqlitePool, name: &str) -> Result<FolderModel, SqlxError> {
        let trimmed = name.trim();
        if is_blank_name(trimmed) {
            return Err(SqlxError::Protocol("folder name cannot be empty".to_string()));
        }

        let id = Uuid::new_v4().to_string();
        let now = Utc::now().naive_utc();

        sqlx::query("INSERT INTO folders (id, name, created_at, updated_at) VALUES (?, ?, ?, ?)")
            .bind(&id)
            .bind(trimmed)
            .bind(now)
            .bind(now)
            .execute(pool)
            .await?;

        Ok(FolderModel {
            id,
            name: trimmed.to_string(),
            created_at: crate::database::models::DateTimeUtc::from(now),
            updated_at: crate::database::models::DateTimeUtc::from(now),
        })
    }

    pub async fn rename_folder(
        pool: &SqlitePool,
        folder_id: &str,
        new_name: &str,
    ) -> Result<bool, SqlxError> {
        if folder_id.trim().is_empty() {
            return Err(SqlxError::Protocol("folder_id cannot be empty".to_string()));
        }
        let trimmed = new_name.trim();
        if is_blank_name(trimmed) {
            return Err(SqlxError::Protocol("folder name cannot be empty".to_string()));
        }

        let now = Utc::now().naive_utc();
        let result = sqlx::query("UPDATE folders SET name = ?, updated_at = ? WHERE id = ?")
            .bind(trimmed)
            .bind(now)
            .bind(folder_id)
            .execute(pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Deletes a folder. The `meeting_folder_id` FK is `ON DELETE SET NULL`, so
    /// meetings that belonged to this folder become unfiled rather than being
    /// deleted.
    pub async fn delete_folder(pool: &SqlitePool, folder_id: &str) -> Result<bool, SqlxError> {
        if folder_id.trim().is_empty() {
            return Err(SqlxError::Protocol("folder_id cannot be empty".to_string()));
        }

        let result = sqlx::query("DELETE FROM folders WHERE id = ?")
            .bind(folder_id)
            .execute(pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn assign_meeting(
        pool: &SqlitePool,
        meeting_id: &str,
        folder_id: Option<&str>,
    ) -> Result<bool, SqlxError> {
        if meeting_id.trim().is_empty() {
            return Err(SqlxError::Protocol("meeting_id cannot be empty".to_string()));
        }

        let now = Utc::now().naive_utc();
        let result =
            sqlx::query("UPDATE meetings SET meeting_folder_id = ?, updated_at = ? WHERE id = ?")
                .bind(folder_id)
                .bind(now)
                .bind(meeting_id)
                .execute(pool)
                .await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn unfiled_meeting_ids(pool: &SqlitePool) -> Result<Vec<String>, SqlxError> {
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT id FROM meetings WHERE meeting_folder_id IS NULL")
                .fetch_all(pool)
                .await?;
        Ok(rows.into_iter().map(|(id,)| id).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::repositories::test_support::{insert_meeting, setup_pool};

    #[tokio::test]
    async fn create_folder_persists_row_and_returns_model() {
        let pool = setup_pool().await;
        let folder = FoldersRepository::create_folder(&pool, "  Q4 Planning  ")
            .await
            .expect("create failed");

        assert_eq!(folder.name, "Q4 Planning");
        assert!(!folder.id.is_empty());

        let stored = FoldersRepository::get_folder(&pool, &folder.id)
            .await
            .expect("get failed")
            .expect("folder missing");
        assert_eq!(stored.name, "Q4 Planning");
    }

    #[tokio::test]
    async fn create_folder_rejects_blank_name() {
        let pool = setup_pool().await;
        let err = FoldersRepository::create_folder(&pool, "   ")
            .await
            .expect_err("should reject blank");
        assert!(matches!(err, SqlxError::Protocol(_)), "got {:?}", err);
    }

    #[tokio::test]
    async fn list_folders_returns_alphabetical_order() {
        let pool = setup_pool().await;
        FoldersRepository::create_folder(&pool, "Bravo").await.unwrap();
        FoldersRepository::create_folder(&pool, "alpha").await.unwrap();
        FoldersRepository::create_folder(&pool, "Charlie").await.unwrap();

        let folders = FoldersRepository::list_folders(&pool).await.unwrap();
        let names: Vec<_> = folders.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "Bravo", "Charlie"]);
    }

    #[tokio::test]
    async fn rename_folder_updates_name() {
        let pool = setup_pool().await;
        let folder = FoldersRepository::create_folder(&pool, "Old")
            .await
            .unwrap();

        let ok = FoldersRepository::rename_folder(&pool, &folder.id, "  New  ")
            .await
            .unwrap();
        assert!(ok);

        let stored = FoldersRepository::get_folder(&pool, &folder.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.name, "New");
    }

    #[tokio::test]
    async fn rename_folder_returns_false_for_missing_id() {
        let pool = setup_pool().await;
        let ok = FoldersRepository::rename_folder(&pool, "nope", "Whatever")
            .await
            .unwrap();
        assert!(!ok);
    }

    #[tokio::test]
    async fn delete_folder_unfiles_associated_meetings_without_deleting_them() {
        let pool = setup_pool().await;
        let folder = FoldersRepository::create_folder(&pool, "Archive")
            .await
            .unwrap();
        insert_meeting(&pool, "m1").await;
        insert_meeting(&pool, "m2").await;
        FoldersRepository::assign_meeting(&pool, "m1", Some(&folder.id))
            .await
            .unwrap();
        FoldersRepository::assign_meeting(&pool, "m2", Some(&folder.id))
            .await
            .unwrap();

        let deleted = FoldersRepository::delete_folder(&pool, &folder.id)
            .await
            .unwrap();
        assert!(deleted);

        let still_there: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM meetings WHERE id IN ('m1','m2')")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(still_there.0, 2);

        let unfiled = FoldersRepository::unfiled_meeting_ids(&pool).await.unwrap();
        assert_eq!(unfiled.len(), 2);
    }

    #[tokio::test]
    async fn assign_meeting_to_folder_and_back_to_unfiled() {
        let pool = setup_pool().await;
        let folder = FoldersRepository::create_folder(&pool, "F").await.unwrap();
        insert_meeting(&pool, "m1").await;

        FoldersRepository::assign_meeting(&pool, "m1", Some(&folder.id))
            .await
            .unwrap();
        let row: (Option<String>,) =
            sqlx::query_as("SELECT meeting_folder_id FROM meetings WHERE id = 'm1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row.0.as_deref(), Some(folder.id.as_str()));

        FoldersRepository::assign_meeting(&pool, "m1", None)
            .await
            .unwrap();
        let row: (Option<String>,) =
            sqlx::query_as("SELECT meeting_folder_id FROM meetings WHERE id = 'm1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(row.0.is_none());
    }

    #[tokio::test]
    async fn assign_meeting_returns_false_for_missing_meeting() {
        let pool = setup_pool().await;
        let ok = FoldersRepository::assign_meeting(&pool, "missing", None)
            .await
            .unwrap();
        assert!(!ok);
    }

    #[tokio::test]
    async fn unfiled_meeting_ids_lists_only_unfiled() {
        let pool = setup_pool().await;
        let folder = FoldersRepository::create_folder(&pool, "F").await.unwrap();
        insert_meeting(&pool, "m1").await;
        insert_meeting(&pool, "m2").await;
        insert_meeting(&pool, "m3").await;
        FoldersRepository::assign_meeting(&pool, "m2", Some(&folder.id))
            .await
            .unwrap();

        let ids = FoldersRepository::unfiled_meeting_ids(&pool).await.unwrap();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(sorted, vec!["m1", "m3"]);
    }

    // =========================================================================
    // Adversarial tests (breaker agent pass 1)
    // =========================================================================
    // Focus: empty/whitespace/Unicode/long/null names, duplicate names, rename
    // collisions, delete semantics, concurrent races, dangling FKs.

    #[tokio::test]
    async fn create_folder_rejects_name_with_only_unicode_whitespace() {
        // U+00A0 (NBSP), U+2003 (em space), U+3000 (ideographic space)
        // str::trim() does NOT remove these. If the validator relies on
        // trim().is_empty(), a folder name made of these is accepted and stored,
        // but visually blank in the sidebar.
        let pool = setup_pool().await;
        let weird = "\u{00A0}\u{2003}\u{3000}";
        let result = FoldersRepository::create_folder(&pool, weird).await;
        assert!(
            result.is_err(),
            "create_folder accepted a name made entirely of non-trimmed Unicode \
             whitespace (len={} bytes). This produces a folder that is visually empty.",
            weird.len()
        );
    }

    #[tokio::test]
    async fn create_folder_rejects_zero_width_only_name() {
        // All five zero-width code points that pass `str::trim()`:
        //   U+200B (zero-width space)
        //   U+200C (zero-width non-joiner)
        //   U+200D (zero-width joiner)
        //   U+2060 (word joiner)
        //   U+FEFF (byte order mark / zero-width no-break space)
        // None of them are stripped by `str::trim`, so without an explicit
        // check a folder name composed of any combination of these is
        // accepted by the validator and stored in the DB — but visually
        // empty in the sidebar.
        let pool = setup_pool().await;
        for zw in [
            "\u{200B}",
            "\u{200C}",
            "\u{200D}",
            "\u{2060}",
            "\u{FEFF}",
            "\u{200B}\u{200C}",
            "\u{FEFF}\u{200D}\u{2060}",
        ] {
            let result = FoldersRepository::create_folder(&pool, zw).await;
            assert!(
                result.is_err(),
                "create_folder accepted a name composed only of zero-width characters \
                 (input: {:?}, len={}). Such a folder is invisible in the sidebar.",
                zw,
                zw.len()
            );
        }
    }

    #[tokio::test]
    async fn rename_folder_to_zero_width_name_rejected() {
        // Same zero-width rejection must apply on rename, otherwise a folder
        // could be made invisible by renaming it.
        let pool = setup_pool().await;
        let folder = FoldersRepository::create_folder(&pool, "Real").await.unwrap();
        for zw in [
            "\u{200B}",
            "\u{200C}",
            "\u{200D}",
            "\u{2060}",
            "\u{FEFF}",
            "\u{200B}\u{FEFF}\u{2060}",
        ] {
            let err = FoldersRepository::rename_folder(&pool, &folder.id, zw)
                .await
                .expect_err("zero-width rename should be rejected");
            assert!(
                matches!(err, SqlxError::Protocol(_)),
                "expected Protocol error for zero-width rename input {:?}, got {:?}",
                zw,
                err
            );
            // The original name must remain unchanged.
            let stored = FoldersRepository::get_folder(&pool, &folder.id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(stored.name, "Real", "zero-width rename leaked through for {:?}", zw);
        }
    }

    #[tokio::test]
    async fn create_folder_handles_extremely_long_name_without_panic() {
        let pool = setup_pool().await;
        let huge = "a".repeat(10_000);
        let result = FoldersRepository::create_folder(&pool, &huge).await;
        assert!(result.is_ok(), "10KB name should be accepted: {:?}", result.err());
        let stored = FoldersRepository::get_folder(&pool, &result.unwrap().id).await.unwrap().unwrap();
        assert_eq!(stored.name.len(), 10_000);
    }

    #[tokio::test]
    async fn create_folder_handles_unicode_and_rtl_names() {
        let pool = setup_pool().await;
        let cases = [
            "\u{0627}\u{0644}\u{0639}\u{0631}\u{0628}\u{064A}\u{0629}", // Arabic, RTL
            "\u{4E2D}\u{6587}\u{6587}\u{4EF6}\u{5939}",                  // Chinese
            "\u{1F4C1} Folder \u{1F680}",                                 // Emoji
            "\u{0645}\u{0634}\u{0627}\u{0631}\u{064A}\u{0639} 2024 / Team", // Mixed RTL+LTR
        ];
        for name in cases {
            let f = FoldersRepository::create_folder(&pool, name).await.unwrap();
            let stored = FoldersRepository::get_folder(&pool, &f.id).await.unwrap().unwrap();
            assert_eq!(stored.name, name, "round-trip failed for {:?}", name);
        }
    }

    #[tokio::test]
    async fn create_folder_treats_duplicate_names_as_distinct_rows() {
        // Schema has no UNIQUE constraint on name. Two folders with the same
        // name are stored. Confirm — sidebar/lookup must tolerate duplicates.
        let pool = setup_pool().await;
        let a = FoldersRepository::create_folder(&pool, "Same").await.unwrap();
        let b = FoldersRepository::create_folder(&pool, "Same").await.unwrap();
        assert_ne!(a.id, b.id, "expected distinct ids for duplicate names");
        let list = FoldersRepository::list_folders(&pool).await.unwrap();
        assert_eq!(list.iter().filter(|f| f.name == "Same").count(), 2);
    }

    #[tokio::test]
    async fn rename_to_existing_name_silently_succeeds() {
        // Renaming folder A to "B" when folder B already exists succeeds,
        // producing two folders with the same name. No unique constraint,
        // no conflict error.
        let pool = setup_pool().await;
        let a = FoldersRepository::create_folder(&pool, "A").await.unwrap();
        let _b = FoldersRepository::create_folder(&pool, "B").await.unwrap();
        let ok = FoldersRepository::rename_folder(&pool, &a.id, "B").await.unwrap();
        assert!(ok);
        let list = FoldersRepository::list_folders(&pool).await.unwrap();
        let b_count = list.iter().filter(|f| f.name == "B").count();
        assert_eq!(
            b_count, 2,
            "rename allowed collision; no unique constraint on folders.name"
        );
    }

    #[tokio::test]
    async fn delete_folder_is_idempotent_on_second_call() {
        let pool = setup_pool().await;
        let folder = FoldersRepository::create_folder(&pool, "F").await.unwrap();
        insert_meeting(&pool, "m1").await;
        FoldersRepository::assign_meeting(&pool, "m1", Some(&folder.id)).await.unwrap();

        assert!(FoldersRepository::delete_folder(&pool, &folder.id).await.unwrap());
        let second = FoldersRepository::delete_folder(&pool, &folder.id).await.unwrap();
        assert!(!second, "second delete should return false, not error");

        let still_there: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM meetings WHERE id = 'm1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(still_there.0, 1);
    }

    #[tokio::test]
    async fn delete_folder_with_whitespace_only_id_returns_error() {
        let pool = setup_pool().await;
        let err = FoldersRepository::delete_folder(&pool, "   ").await.unwrap_err();
        assert!(matches!(err, SqlxError::Protocol(_)), "got {:?}", err);
    }

    #[tokio::test]
    async fn assign_meeting_to_nonexistent_folder_is_rejected_by_fk() {
        // The meeting_folder_id column has a FK to folders(id). Assignment
        // to a missing folder id is rejected at the DB level (sqlx
        // returns Database error 787 = SQLITE_CONSTRAINT_FOREIGNKEY).
        // This is good: dangling folder_id cannot be created.
        let pool = setup_pool().await;
        insert_meeting(&pool, "m1").await;
        let result = FoldersRepository::assign_meeting(&pool, "m1", Some("ghost-folder-id")).await;
        assert!(result.is_err(), "expected FK violation, got {:?}", result);
    }

    #[tokio::test]
    async fn list_folders_unaffected_by_meeting_deletion() {
        // After a meeting is deleted while pointing at a folder, the folder
        // listing is unchanged (no cascade effects).
        let pool = setup_pool().await;
        let f = FoldersRepository::create_folder(&pool, "F").await.unwrap();
        insert_meeting(&pool, "m1").await;
        FoldersRepository::assign_meeting(&pool, "m1", Some(&f.id)).await.unwrap();
        sqlx::query("DELETE FROM meetings WHERE id = 'm1'")
            .execute(&pool)
            .await
            .unwrap();
        let list = FoldersRepository::list_folders(&pool).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, f.id);
    }

    #[tokio::test]
    async fn rename_folder_to_blank_inputs_returns_error() {
        let pool = setup_pool().await;
        let f = FoldersRepository::create_folder(&pool, "Real").await.unwrap();
        for bad in ["", "   ", "\t\n"] {
            let err = FoldersRepository::rename_folder(&pool, &f.id, bad).await.unwrap_err();
            assert!(matches!(err, SqlxError::Protocol(_)), "input {:?} should error", bad);
        }
    }

    #[tokio::test]
    async fn concurrent_creates_with_same_name_produce_two_rows() {
        // Race: two create_folder calls in parallel with the same name.
        // Without a UNIQUE constraint, both succeed.
        let pool = setup_pool().await;
        let pool_a = pool.clone();
        let pool_b = pool.clone();
        let ha = tokio::spawn(async move { FoldersRepository::create_folder(&pool_a, "Race").await });
        let hb = tokio::spawn(async move { FoldersRepository::create_folder(&pool_b, "Race").await });
        let a = ha.await.unwrap().unwrap();
        let b = hb.await.unwrap().unwrap();
        assert_ne!(a.id, b.id);
        let list = FoldersRepository::list_folders(&pool).await.unwrap();
        assert_eq!(list.iter().filter(|f| f.name == "Race").count(), 2);
    }
}
