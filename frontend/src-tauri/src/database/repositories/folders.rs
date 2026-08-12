use crate::database::models::FolderModel;
use chrono::Utc;
use sqlx::{Error as SqlxError, SqlitePool};
use uuid::Uuid;

pub struct FoldersRepository;

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
        if trimmed.is_empty() {
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
        if trimmed.is_empty() {
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
}
