# Action Item Extraction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** After a meeting summary is generated, extract discrete action items (task, optional owner, optional due date parsed from natural language) as structured rows in the local database and surface them as a checkable checklist in the meeting details UI.

**Architecture:** A new background task in `SummaryService::process_transcript_background` (`frontend/src-tauri/src/summary/service.rs`) fires a second, independent LLM call — reusing the same provider-agnostic `llm_client::generate_summary` primitive the summary pipeline and the "ask about this meeting" commands already call — that asks for a JSON array of action items instead of markdown. The raw due-date phrase the LLM returns (e.g. `"Friday"`) is resolved to a calendar date entirely offline via the `chrono-english` crate, anchored to the meeting's own timestamp rather than the system clock. Results are persisted as first-class rows in a new `action_items` SQLite table (via a new migration + repository), exposed through two new Tauri commands, and rendered as a checkbox list in `SummaryPanel.tsx` that round-trips completion state back through Rust on every toggle.

**Tech Stack:** Rust: `sqlx` (existing), `chrono` (existing), `chrono-english` (new, offline NL date parsing), `serde_json`, `tauri::async_runtime::spawn`. Frontend: React hook + `@tauri-apps/api/event` listener, shadcn `Checkbox` (new, via `pnpm dlx shadcn@latest add checkbox`).

## Global Constraints

- Extraction must reuse `llm_client::generate_summary` (`frontend/src-tauri/src/summary/llm_client.rs:115`) — the same provider abstraction (Ollama/BuiltInAI/OpenAI/Claude/Groq/OpenRouter/CustomOpenAI/LmStudio) every other LLM feature in this crate uses. No new HTTP client path.
- Due-date parsing from natural language must run entirely offline/locally (no network call) — this is a pure Rust function over the LLM's already-returned text, not a second LLM round-trip and not a hosted date-parsing API.
- Action items are a completely separate DB write path from `summary_processes.result`; they must never be encoded only inside the summary markdown blob. The existing `transcripts.action_items` TEXT column (from `frontend/src-tauri/migrations/20250916100000_initial_schema.sql:16`) is legacy/unused free text and is not reused or migrated — the new `action_items` table is intentionally separate and first-class.
- Extraction failures must never fail or roll back an already-completed summary. The checklist is best-effort: on failure it stays empty for that meeting until the next regeneration.
- All new Tauri commands follow the existing `#[tauri::command]` + `tauri::State<'_, AppState>` + `Result<T, String>` pattern (see `frontend/src-tauri/src/summary/commands.rs`), and are registered in `frontend/src-tauri/src/lib.rs`'s `generate_handler!` list.
- Do not add anything to `backend/app/main.py` (archived legacy FastAPI) — all new behavior lives in the Rust/Tauri core and the Next.js frontend per `CLAUDE.md`.
- Calendar/reminders OS sync is explicitly out of scope for this plan — see "Out of scope: v2" at the end.

---

### Task 1: `action_items` database schema and repository

**Files:**
- Create: `frontend/src-tauri/migrations/20260811000000_add_action_items.sql`
- Modify: `frontend/src-tauri/src/database/models.rs:133` (append `ActionItem` struct after the file's last line)
- Modify: `frontend/src-tauri/src/database/repositories/mod.rs:1` (add module declaration)
- Create: `frontend/src-tauri/src/database/repositories/action_item.rs`

**Interfaces:**
- Produces: `database::models::ActionItem { id, meeting_id, task, owner, due_date_text, due_date, completed, sort_order, created_at, updated_at }`
- Produces: `database::repositories::action_item::NewActionItem { task, owner, due_date_text, due_date: Option<chrono::NaiveDate> }`
- Produces: `ActionItemsRepository::replace_for_meeting(pool, meeting_id, &[NewActionItem]) -> Result<Vec<ActionItem>, sqlx::Error>`
- Produces: `ActionItemsRepository::get_for_meeting(pool, meeting_id) -> Result<Vec<ActionItem>, sqlx::Error>`
- Produces: `ActionItemsRepository::set_completed(pool, meeting_id, item_id, completed: bool) -> Result<Option<ActionItem>, sqlx::Error>`
- Consumes: `database::repositories::test_support::{setup_pool, insert_meeting}` (existing, `frontend/src-tauri/src/database/repositories/test_support.rs`)

- [ ] **Step 1: Write the failing repository tests**

  Create `frontend/src-tauri/src/database/repositories/action_item.rs` with the struct/impl stubs below (bodies calling `unimplemented!()`) plus this test module:

  ```rust
  use crate::database::models::ActionItem;
  use chrono::{NaiveDate, Utc};
  use sqlx::{Connection, SqlitePool};
  use tracing::info;
  use uuid::Uuid;

  pub struct ActionItemsRepository;

  /// A freshly-extracted action item, not yet assigned an id or persisted.
  /// `due_date` is the already-locally-parsed calendar date (see
  /// `summary::action_items::due_date::parse_due_date`) - this repository
  /// never parses dates itself, only stores what it's given.
  #[derive(Debug, Clone)]
  pub struct NewActionItem {
      pub task: String,
      pub owner: Option<String>,
      pub due_date_text: Option<String>,
      pub due_date: Option<NaiveDate>,
  }

  impl ActionItemsRepository {
      pub async fn replace_for_meeting(
          _pool: &SqlitePool,
          _meeting_id: &str,
          _items: &[NewActionItem],
      ) -> Result<Vec<ActionItem>, sqlx::Error> {
          unimplemented!()
      }

      pub async fn get_for_meeting(
          _pool: &SqlitePool,
          _meeting_id: &str,
      ) -> Result<Vec<ActionItem>, sqlx::Error> {
          unimplemented!()
      }

      pub async fn set_completed(
          _pool: &SqlitePool,
          _meeting_id: &str,
          _item_id: &str,
          _completed: bool,
      ) -> Result<Option<ActionItem>, sqlx::Error> {
          unimplemented!()
      }
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      use crate::database::repositories::test_support::{insert_meeting, setup_pool};

      fn item(task: &str) -> NewActionItem {
          NewActionItem {
              task: task.to_string(),
              owner: Some("Alice".to_string()),
              due_date_text: Some("Friday".to_string()),
              due_date: NaiveDate::from_ymd_opt(2025, 1, 10),
          }
      }

      #[tokio::test]
      async fn replace_for_meeting_persists_items_in_order() {
          let pool = setup_pool().await;
          insert_meeting(&pool, "m1").await;

          let saved = ActionItemsRepository::replace_for_meeting(
              &pool,
              "m1",
              &[item("Send the deck"), item("Book the room")],
          )
          .await
          .expect("insert failed");

          assert_eq!(saved.len(), 2);
          assert_eq!(saved[0].task, "Send the deck");
          assert_eq!(saved[0].sort_order, 0);
          assert_eq!(saved[1].sort_order, 1);
          assert_eq!(saved[0].owner.as_deref(), Some("Alice"));
          assert_eq!(saved[0].due_date.as_deref(), Some("2025-01-10"));
          assert!(!saved[0].completed);
      }

      #[tokio::test]
      async fn replace_for_meeting_clears_previous_items() {
          let pool = setup_pool().await;
          insert_meeting(&pool, "m1").await;

          ActionItemsRepository::replace_for_meeting(&pool, "m1", &[item("Old task")])
              .await
              .expect("first insert failed");
          ActionItemsRepository::replace_for_meeting(&pool, "m1", &[item("New task")])
              .await
              .expect("second insert failed");

          let items = ActionItemsRepository::get_for_meeting(&pool, "m1")
              .await
              .expect("query failed");

          assert_eq!(items.len(), 1);
          assert_eq!(items[0].task, "New task");
      }

      #[tokio::test]
      async fn get_for_meeting_returns_empty_for_unknown_meeting() {
          let pool = setup_pool().await;
          let items = ActionItemsRepository::get_for_meeting(&pool, "does-not-exist")
              .await
              .expect("query failed");
          assert!(items.is_empty());
      }

      #[tokio::test]
      async fn set_completed_toggles_and_returns_updated_row() {
          let pool = setup_pool().await;
          insert_meeting(&pool, "m1").await;
          let saved = ActionItemsRepository::replace_for_meeting(&pool, "m1", &[item("Task")])
              .await
              .expect("insert failed");

          let updated = ActionItemsRepository::set_completed(&pool, "m1", &saved[0].id, true)
              .await
              .expect("update failed")
              .expect("row not found");

          assert!(updated.completed);
      }

      #[tokio::test]
      async fn set_completed_scoped_to_meeting_id_returns_none_for_wrong_meeting() {
          let pool = setup_pool().await;
          insert_meeting(&pool, "m1").await;
          insert_meeting(&pool, "m2").await;
          let saved = ActionItemsRepository::replace_for_meeting(&pool, "m1", &[item("Task")])
              .await
              .expect("insert failed");

          let result = ActionItemsRepository::set_completed(&pool, "m2", &saved[0].id, true)
              .await
              .expect("update query failed");

          assert!(result.is_none());
      }
  }
  ```

  Add the model to `frontend/src-tauri/src/database/models.rs` (append after line 133):

  ```rust
  #[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
  pub struct ActionItem {
      pub id: String,
      pub meeting_id: String,
      pub task: String,
      pub owner: Option<String>,
      pub due_date_text: Option<String>,
      /// ISO 8601 date (YYYY-MM-DD), or `None` if the extracted due-date
      /// phrase couldn't be resolved by
      /// `summary::action_items::due_date::parse_due_date`. Typed as `String`
      /// (not `chrono::NaiveDate`) so the Tauri command boundary serializes it
      /// as a plain ISO string for the frontend without pulling chrono's date
      /// JSON representation into that contract.
      pub due_date: Option<String>,
      pub completed: bool,
      pub sort_order: i64,
      pub created_at: chrono::DateTime<chrono::Utc>,
      pub updated_at: chrono::DateTime<chrono::Utc>,
  }
  ```

  Add the migration `frontend/src-tauri/migrations/20260811000000_add_action_items.sql`:

  ```sql
  -- Add action_items table for structural (non-prose) action item extraction
  CREATE TABLE IF NOT EXISTS action_items (
      id TEXT PRIMARY KEY NOT NULL,
      meeting_id TEXT NOT NULL,
      task TEXT NOT NULL,
      owner TEXT,
      due_date_text TEXT,
      due_date TEXT,
      completed INTEGER NOT NULL DEFAULT 0,
      sort_order INTEGER NOT NULL DEFAULT 0,
      created_at TEXT NOT NULL,
      updated_at TEXT NOT NULL,
      FOREIGN KEY (meeting_id) REFERENCES meetings(id) ON DELETE CASCADE
  );

  CREATE INDEX IF NOT EXISTS idx_action_items_meeting_id ON action_items(meeting_id);
  ```

  Register the module in `frontend/src-tauri/src/database/repositories/mod.rs` (insert as the new first line):

  ```rust
  pub mod action_item;
  pub mod meeting;
  pub mod setting;
  pub mod summary;
  #[cfg(test)]
  mod test_support;
  pub mod transcript;
  pub mod transcript_chunk;
  ```

- [ ] **Step 2: Run tests to verify they fail**

  ```bash
  cd frontend/src-tauri && cargo test --lib database::repositories::action_item
  ```

  Expected: compiles (stubs exist), then panics with `not implemented` from the `unimplemented!()` bodies.

- [ ] **Step 3: Write the real implementation**

  Replace the three `unimplemented!()` bodies in `frontend/src-tauri/src/database/repositories/action_item.rs`:

  ```rust
  impl ActionItemsRepository {
      /// Replaces every action item for `meeting_id` with `items`, inside a
      /// transaction - mirrors how `SummaryProcessesRepository::update_meeting_summary`
      /// overwrites the summary wholesale on regeneration rather than merging:
      /// a regenerated summary's action items supersede the previous
      /// extraction rather than being unioned with it, so stale items don't
      /// accumulate across repeated regenerations of the same meeting.
      pub async fn replace_for_meeting(
          pool: &SqlitePool,
          meeting_id: &str,
          items: &[NewActionItem],
      ) -> Result<Vec<ActionItem>, sqlx::Error> {
          let mut conn = pool.acquire().await?;
          let mut tx = conn.begin().await?;
          let now = Utc::now();

          sqlx::query("DELETE FROM action_items WHERE meeting_id = ?")
              .bind(meeting_id)
              .execute(&mut *tx)
              .await?;

          let mut saved = Vec::with_capacity(items.len());
          for (index, item) in items.iter().enumerate() {
              let id = Uuid::new_v4().to_string();
              sqlx::query(
                  r#"
                  INSERT INTO action_items
                      (id, meeting_id, task, owner, due_date_text, due_date, completed, sort_order, created_at, updated_at)
                  VALUES (?, ?, ?, ?, ?, ?, 0, ?, ?, ?)
                  "#,
              )
              .bind(&id)
              .bind(meeting_id)
              .bind(&item.task)
              .bind(&item.owner)
              .bind(&item.due_date_text)
              .bind(item.due_date)
              .bind(index as i64)
              .bind(now)
              .bind(now)
              .execute(&mut *tx)
              .await?;

              saved.push(ActionItem {
                  id,
                  meeting_id: meeting_id.to_string(),
                  task: item.task.clone(),
                  owner: item.owner.clone(),
                  due_date_text: item.due_date_text.clone(),
                  due_date: item.due_date.map(|d| d.to_string()),
                  completed: false,
                  sort_order: index as i64,
                  created_at: now,
                  updated_at: now,
              });
          }

          tx.commit().await?;
          info!(
              "Replaced action items for meeting_id {}: {} item(s)",
              meeting_id,
              saved.len()
          );
          Ok(saved)
      }

      /// All action items for a meeting, in extraction order.
      pub async fn get_for_meeting(
          pool: &SqlitePool,
          meeting_id: &str,
      ) -> Result<Vec<ActionItem>, sqlx::Error> {
          sqlx::query_as::<_, ActionItem>(
              "SELECT * FROM action_items WHERE meeting_id = ? ORDER BY sort_order ASC",
          )
          .bind(meeting_id)
          .fetch_all(pool)
          .await
      }

      /// Sets `completed` on a single item, scoped to `meeting_id` so an item
      /// id from a different meeting can never be toggled through this call.
      /// Returns the updated row, or `None` if no row matched both ids.
      pub async fn set_completed(
          pool: &SqlitePool,
          meeting_id: &str,
          item_id: &str,
          completed: bool,
      ) -> Result<Option<ActionItem>, sqlx::Error> {
          let now = Utc::now();
          let result = sqlx::query(
              "UPDATE action_items SET completed = ?, updated_at = ? WHERE id = ? AND meeting_id = ?",
          )
          .bind(completed)
          .bind(now)
          .bind(item_id)
          .bind(meeting_id)
          .execute(pool)
          .await?;

          if result.rows_affected() == 0 {
              return Ok(None);
          }

          sqlx::query_as::<_, ActionItem>("SELECT * FROM action_items WHERE id = ?")
              .bind(item_id)
              .fetch_optional(pool)
              .await
      }
  }
  ```

- [ ] **Step 4: Run tests to verify they pass**

  ```bash
  cd frontend/src-tauri && cargo test --lib database::repositories::action_item
  ```

  Expected: all 5 tests pass.

- [ ] **Step 5: Commit**

  ```bash
  git add frontend/src-tauri/migrations/20260811000000_add_action_items.sql \
          frontend/src-tauri/src/database/models.rs \
          frontend/src-tauri/src/database/repositories/mod.rs \
          frontend/src-tauri/src/database/repositories/action_item.rs
  git commit -m "Add action_items table, model, and repository"
  ```

---

### Task 2: Local natural-language due-date parsing

**Files:**
- Modify: `frontend/src-tauri/Cargo.toml:82` (add `chrono-english` dependency after the `chrono` line)
- Modify: `frontend/src-tauri/src/summary/mod.rs:33` (register the new `action_items` module so the crate's module tree can see it)
- Create: `frontend/src-tauri/src/summary/action_items/mod.rs`
- Create: `frontend/src-tauri/src/summary/action_items/due_date.rs`

**Interfaces:**
- Produces: `summary::action_items::due_date::parse_due_date(phrase: &str, reference: chrono::DateTime<chrono::Utc>) -> Option<chrono::NaiveDate>`

- [ ] **Step 1: Write the failing tests**

  Add the dependency in `frontend/src-tauri/Cargo.toml` right after line 82 (`chrono = { version = "0.4.31", features = ["serde"] }`):

  ```toml
  chrono = { version = "0.4.31", features = ["serde"] }
  chrono-english = "0.1.7"
  ```

  Create `frontend/src-tauri/src/summary/action_items/due_date.rs`:

  ```rust
  use chrono::{DateTime, NaiveDate, Utc};

  /// Placeholder - replaced in Step 3.
  pub fn parse_due_date(_phrase: &str, _reference: DateTime<Utc>) -> Option<NaiveDate> {
      unimplemented!()
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      use chrono::TimeZone;

      /// Wednesday, Jan 8 2025, 10:00 UTC - a fixed reference so date-math
      /// assertions below don't depend on when the test suite runs.
      fn reference() -> DateTime<Utc> {
          Utc.with_ymd_and_hms(2025, 1, 8, 10, 0, 0).unwrap()
      }

      #[test]
      fn parses_bare_weekday_to_the_upcoming_occurrence() {
          let date = parse_due_date("Friday", reference()).expect("should parse");
          assert_eq!(date, NaiveDate::from_ymd_opt(2025, 1, 10).unwrap());
      }

      #[test]
      fn parses_tomorrow() {
          let date = parse_due_date("tomorrow", reference()).expect("should parse");
          assert_eq!(date, NaiveDate::from_ymd_opt(2025, 1, 9).unwrap());
      }

      #[test]
      fn parses_next_monday() {
          let date = parse_due_date("next Monday", reference()).expect("should parse");
          assert_eq!(date, NaiveDate::from_ymd_opt(2025, 1, 13).unwrap());
      }

      #[test]
      fn parses_relative_day_count() {
          let date = parse_due_date("in 3 days", reference()).expect("should parse");
          assert_eq!(date, NaiveDate::from_ymd_opt(2025, 1, 11).unwrap());
      }

      #[test]
      fn empty_phrase_returns_none() {
          assert_eq!(parse_due_date("", reference()), None);
      }

      #[test]
      fn whitespace_only_phrase_returns_none() {
          assert_eq!(parse_due_date("   ", reference()), None);
      }

      #[test]
      fn unparseable_phrase_returns_none_instead_of_erroring() {
          assert_eq!(parse_due_date("engineering", reference()), None);
      }
  }
  ```

  Create `frontend/src-tauri/src/summary/action_items/mod.rs`:

  ```rust
  pub mod commands;
  pub mod due_date;
  pub mod extractor;

  pub use extractor::{extract_action_items, ExtractedActionItem};
  ```

  (`commands` and `extractor` are fleshed out in Tasks 3 and 5 below; create minimal stub files now so `mod.rs` compiles: `frontend/src-tauri/src/summary/action_items/extractor.rs` containing `pub struct ExtractedActionItem; pub async fn extract_action_items() { unimplemented!() }`, and `frontend/src-tauri/src/summary/action_items/commands.rs` empty. Both are fully overwritten in Tasks 3 and 5.)

  Register the module in `frontend/src-tauri/src/summary/mod.rs` — without this, `summary::action_items` isn't part of the crate's module tree yet and the `cargo test --lib summary::action_items::due_date` command in Step 2 below won't find it. Insert `pub mod action_items;` alphabetically, right before the existing line 33 `pub mod commands;`:

  ```rust
  pub mod action_items;
  pub mod commands;
  pub(crate) mod language_detection;
  ```

- [ ] **Step 2: Run tests to verify they fail**

  ```bash
  cd frontend/src-tauri && cargo test --lib summary::action_items::due_date
  ```

  Expected: panics with `not implemented`.

- [ ] **Step 3: Write the real implementation**

  ```rust
  use chrono::{DateTime, NaiveDate, Utc};
  use chrono_english::{parse_date_string, Dialect};

  /// Resolves a natural-language due-date phrase (e.g. "Friday", "next
  /// Monday", "in 3 days") extracted from the transcript by the LLM into a
  /// concrete calendar date, anchored to `reference` (the meeting's own
  /// timestamp) rather than the system clock - a meeting from three weeks
  /// ago that said "Friday" meant the Friday after *that* meeting, not the
  /// Friday after whenever the user happens to open the app.
  ///
  /// Runs entirely offline via `chrono-english`'s local grammar - no network
  /// call. `chrono-english` only supports bare expressions ("Friday", "next
  /// Friday", "in 3 days"), not phrases with leading filler words like "by
  /// Friday" or "due Friday" - the extraction prompt in
  /// `extractor::ACTION_ITEM_SYSTEM_PROMPT` is written to ask the LLM for
  /// exactly that bare form.
  ///
  /// Returns `None` for phrases the parser can't resolve (empty string, or
  /// something that isn't a date at all) rather than erroring - this is a
  /// best-effort structured field, not something extraction should fail
  /// over.
  pub fn parse_due_date(phrase: &str, reference: DateTime<Utc>) -> Option<NaiveDate> {
      let trimmed = phrase.trim();
      if trimmed.is_empty() {
          return None;
      }
      parse_date_string(trimmed, reference, Dialect::Us)
          .ok()
          .map(|dt| dt.date_naive())
  }
  ```

- [ ] **Step 4: Run tests to verify they pass**

  ```bash
  cd frontend/src-tauri && cargo test --lib summary::action_items::due_date
  ```

  Expected: all 7 tests pass.

- [ ] **Step 5: Commit**

  ```bash
  git add frontend/src-tauri/Cargo.toml frontend/src-tauri/Cargo.lock \
          frontend/src-tauri/src/summary/action_items/mod.rs \
          frontend/src-tauri/src/summary/action_items/due_date.rs \
          frontend/src-tauri/src/summary/action_items/extractor.rs \
          frontend/src-tauri/src/summary/action_items/commands.rs
  git commit -m "Add offline natural-language due-date parsing via chrono-english"
  ```

---

### Task 3: LLM extraction prompt and JSON parsing

**Files:**
- Modify: `frontend/src-tauri/src/summary/action_items/extractor.rs` (replace Task 2's placeholder)

**Interfaces:**
- Consumes: `summary::llm_client::{generate_summary, LLMProvider}` (`frontend/src-tauri/src/summary/llm_client.rs:115`)
- Consumes: `summary::action_items::due_date::parse_due_date` (Task 2)
- Produces: `summary::action_items::extractor::ExtractedActionItem { task, owner, due_date_text, due_date: Option<chrono::NaiveDate> }`
- Produces: `summary::action_items::extract_action_items(client, provider, model_name, api_key, transcript, reference_time, ollama_endpoint, custom_openai_endpoint, max_tokens, temperature, top_p, app_data_dir, cancellation_token) -> Result<Vec<ExtractedActionItem>, String>` — consumed by Task 4

- [ ] **Step 1: Write the failing tests**

  Replace `frontend/src-tauri/src/summary/action_items/extractor.rs`:

  ```rust
  use crate::summary::llm_client::{generate_summary, LLMProvider};
  use chrono::{DateTime, NaiveDate, Utc};
  use reqwest::Client;
  use serde::Deserialize;
  use std::path::PathBuf;
  use tokio_util::sync::CancellationToken;
  use tracing::{info, warn};

  use super::due_date::parse_due_date;

  /// Hard cap on how many action items a single extraction pass will keep,
  /// regardless of how many the LLM returns - protects the checklist UI (and
  /// the `action_items` table) from an unbounded or runaway response.
  const MAX_ACTION_ITEMS: usize = 50;

  #[derive(Debug, Clone, PartialEq)]
  pub struct ExtractedActionItem {
      pub task: String,
      pub owner: Option<String>,
      pub due_date_text: Option<String>,
      pub due_date: Option<NaiveDate>,
  }

  #[derive(Debug, Deserialize)]
  struct RawActionItem {
      task: String,
      #[serde(default)]
      owner: Option<String>,
      #[serde(default)]
      due_date_text: Option<String>,
  }

  const ACTION_ITEM_SYSTEM_PROMPT: &str = "placeholder"; // replaced in Step 3

  fn build_action_item_user_prompt(_transcript: &str) -> String {
      unimplemented!()
  }

  fn strip_json_fence(_raw: &str) -> &str {
      unimplemented!()
  }

  fn parse_action_items_json(_raw: &str) -> Result<Vec<RawActionItem>, String> {
      unimplemented!()
  }

  #[allow(clippy::too_many_arguments)]
  pub async fn extract_action_items(
      _client: &Client,
      _provider: &LLMProvider,
      _model_name: &str,
      _api_key: &str,
      _transcript: &str,
      _reference_time: DateTime<Utc>,
      _ollama_endpoint: Option<&str>,
      _custom_openai_endpoint: Option<&str>,
      _max_tokens: Option<u32>,
      _temperature: Option<f32>,
      _top_p: Option<f32>,
      _app_data_dir: Option<&PathBuf>,
      _cancellation_token: Option<&CancellationToken>,
  ) -> Result<Vec<ExtractedActionItem>, String> {
      unimplemented!()
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn strip_json_fence_removes_json_code_fence() {
          let raw = "```json\n[{\"task\": \"Do it\"}]\n```";
          assert_eq!(strip_json_fence(raw), "[{\"task\": \"Do it\"}]");
      }

      #[test]
      fn strip_json_fence_removes_bare_code_fence() {
          let raw = "```\n[]\n```";
          assert_eq!(strip_json_fence(raw), "[]");
      }

      #[test]
      fn strip_json_fence_passes_through_unfenced_input() {
          assert_eq!(strip_json_fence("[]"), "[]");
      }

      #[test]
      fn parse_action_items_json_parses_well_formed_array() {
          let raw = r#"[{"task": "Send the deck", "owner": "Alice", "due_date_text": "Friday"}]"#;
          let items = parse_action_items_json(raw).expect("should parse");
          assert_eq!(items.len(), 1);
          assert_eq!(items[0].task, "Send the deck");
          assert_eq!(items[0].owner.as_deref(), Some("Alice"));
      }

      #[test]
      fn parse_action_items_json_handles_code_fenced_reply() {
          let raw = "```json\n[{\"task\": \"Book the room\"}]\n```";
          let items = parse_action_items_json(raw).expect("should parse");
          assert_eq!(items.len(), 1);
          assert_eq!(items[0].task, "Book the room");
      }

      #[test]
      fn parse_action_items_json_drops_empty_task_entries() {
          let raw = r#"[{"task": ""}, {"task": "Real task"}]"#;
          let items = parse_action_items_json(raw).expect("should parse");
          assert_eq!(items.len(), 1);
          assert_eq!(items[0].task, "Real task");
      }

      #[test]
      fn parse_action_items_json_empty_array_returns_empty_vec() {
          let items = parse_action_items_json("[]").expect("should parse");
          assert!(items.is_empty());
      }

      #[test]
      fn parse_action_items_json_truncates_to_max_items() {
          let entries: Vec<String> = (0..(MAX_ACTION_ITEMS + 10))
              .map(|i| format!(r#"{{"task": "Task {}"}}"#, i))
              .collect();
          let raw = format!("[{}]", entries.join(","));
          let items = parse_action_items_json(&raw).expect("should parse");
          assert_eq!(items.len(), MAX_ACTION_ITEMS);
      }

      #[test]
      fn parse_action_items_json_rejects_malformed_json() {
          assert!(parse_action_items_json("not json").is_err());
      }

      #[test]
      fn build_action_item_user_prompt_wraps_transcript_in_tags() {
          let prompt = build_action_item_user_prompt("Alice: I'll send the deck by Friday.");
          assert!(prompt.contains("<transcript>"));
          assert!(prompt.contains("I'll send the deck by Friday."));
      }
  }
  ```

- [ ] **Step 2: Run tests to verify they fail**

  ```bash
  cd frontend/src-tauri && cargo test --lib summary::action_items::extractor
  ```

  Expected: panics with `not implemented` from the unimplemented helper bodies (the `extract_action_items` async fn itself isn't exercised by these pure tests).

- [ ] **Step 3: Write the real implementation**

  Replace the placeholder items in `frontend/src-tauri/src/summary/action_items/extractor.rs`:

  ```rust
  const ACTION_ITEM_SYSTEM_PROMPT: &str = r#"You extract action items from a meeting transcript. An action item is a concrete task someone agreed to do, not a general topic or observation. Return ONLY a JSON array, nothing else - no markdown code fences, no commentary. Each element must be an object with exactly these fields:
  - "task": a short imperative description of the task (required, non-empty)
  - "owner": the person's name if the transcript names who is responsible, else null
  - "due_date_text": ONLY the bare date expression as said or implied (e.g. "Friday", "next Monday", "in 3 days", "April 1"), with no leading words like "by", "due", "on", or "before" - or null if no date was mentioned

  If there are no action items, return an empty JSON array: []"#;

  fn build_action_item_user_prompt(transcript: &str) -> String {
      format!("<transcript>\n{transcript}\n</transcript>")
  }

  /// Strips a leading/trailing ```json or ``` code fence, mirroring
  /// `clean_llm_markdown_output`'s fence-stripping in `summary::processor`
  /// (`frontend/src-tauri/src/summary/processor.rs:261`) - duplicated in
  /// miniature here rather than reused because that helper also strips
  /// `<think>` tags and trims to a markdown document, neither of which
  /// applies to a JSON array reply.
  fn strip_json_fence(raw: &str) -> &str {
      let trimmed = raw.trim();
      for prefix in ["```json\n", "```json", "```\n", "```"] {
          if let Some(rest) = trimmed.strip_prefix(prefix) {
              if let Some(body) = rest.strip_suffix("```") {
                  return body.trim();
              }
          }
      }
      trimmed
  }

  /// Parses the LLM's JSON array reply into raw items, dropping entries with
  /// an empty/whitespace-only `task` (a model returning `{"task": ""}` for a
  /// non-item is a known failure mode, not a real action item) and
  /// truncating to `MAX_ACTION_ITEMS`. Pure/sync - unit-testable without a
  /// DB or network.
  fn parse_action_items_json(raw: &str) -> Result<Vec<RawActionItem>, String> {
      let body = strip_json_fence(raw);
      let items: Vec<RawActionItem> = serde_json::from_str(body)
          .map_err(|e| format!("Failed to parse action items JSON: {e}"))?;

      Ok(items
          .into_iter()
          .filter(|item| !item.task.trim().is_empty())
          .take(MAX_ACTION_ITEMS)
          .collect())
  }

  /// Extracts structured action items from a meeting transcript (or, for
  /// long transcripts, its already-generated summary - see Task 4's caller)
  /// using the app's already-resolved LLM provider/model for that meeting.
  ///
  /// Reuses `llm_client::generate_summary` (the same provider-agnostic
  /// "prompt in, text out" primitive `SummaryService`, `ask_configured_llm`,
  /// and the suggested-questions commands all call - see
  /// `frontend/src-tauri/src/summary/commands.rs:788`) rather than adding a
  /// second HTTP client path. This is a separate call from the main summary
  /// prompt, not a section grafted onto it: the summary prompt's contract is
  /// "output ONLY the completed Markdown report"
  /// (`build_final_report_system_prompt`,
  /// `frontend/src-tauri/src/summary/processor.rs:149`) and
  /// `clean_llm_markdown_output` is tuned to that; asking the same call to
  /// also emit a JSON block risks corrupting the markdown output
  /// `BlockNoteSummaryView` depends on, for the cost of one extra
  /// request.
  #[allow(clippy::too_many_arguments)]
  pub async fn extract_action_items(
      client: &Client,
      provider: &LLMProvider,
      model_name: &str,
      api_key: &str,
      transcript: &str,
      reference_time: DateTime<Utc>,
      ollama_endpoint: Option<&str>,
      custom_openai_endpoint: Option<&str>,
      max_tokens: Option<u32>,
      temperature: Option<f32>,
      top_p: Option<f32>,
      app_data_dir: Option<&PathBuf>,
      cancellation_token: Option<&CancellationToken>,
  ) -> Result<Vec<ExtractedActionItem>, String> {
      let user_prompt = build_action_item_user_prompt(transcript);

      let raw = generate_summary(
          client,
          provider,
          model_name,
          api_key,
          ACTION_ITEM_SYSTEM_PROMPT,
          &user_prompt,
          ollama_endpoint,
          custom_openai_endpoint,
          max_tokens,
          temperature,
          top_p,
          app_data_dir,
          cancellation_token,
      )
      .await?;

      let raw_items = parse_action_items_json(&raw)?;
      info!("Extracted {} raw action item(s) from transcript", raw_items.len());

      Ok(raw_items
          .into_iter()
          .map(|item| {
              let due_date = item
                  .due_date_text
                  .as_deref()
                  .and_then(|text| parse_due_date(text, reference_time));
              if item.due_date_text.is_some() && due_date.is_none() {
                  warn!(
                      "Could not resolve due date phrase {:?} to a calendar date",
                      item.due_date_text
                  );
              }
              ExtractedActionItem {
                  task: item.task.trim().to_string(),
                  owner: item
                      .owner
                      .map(|o| o.trim().to_string())
                      .filter(|o| !o.is_empty()),
                  due_date_text: item.due_date_text,
                  due_date,
              }
          })
          .collect())
  }
  ```

  (The `pub mod action_items;` registration in `frontend/src-tauri/src/summary/mod.rs` was already added in Task 2 — no further module-tree change needed here.)

- [ ] **Step 4: Run tests to verify they pass**

  ```bash
  cd frontend/src-tauri && cargo test --lib summary::action_items::extractor
  ```

  Expected: all 9 pure tests pass.

- [ ] **Step 5: Commit**

  ```bash
  git add frontend/src-tauri/src/summary/action_items/extractor.rs
  git commit -m "Add LLM-driven action item extraction with JSON parsing"
  ```

---

### Task 4: Wire extraction into the summary generation pipeline

**Files:**
- Modify: `frontend/src-tauri/src/summary/service.rs:1-21` (imports)
- Modify: `frontend/src-tauri/src/summary/service.rs:286,295,440` (rename unused `_app` param and its two references to `app`)
- Modify: `frontend/src-tauri/src/summary/service.rs:582-583` (spawn extraction after summary completes)
- Modify: `frontend/src-tauri/src/summary/service.rs:619` (new private method, inserted just before the `impl SummaryService` block's closing brace)

**Interfaces:**
- Consumes: `summary::action_items::extract_action_items` (Task 3), `database::repositories::action_item::{ActionItemsRepository, NewActionItem}` (Task 1)
- Produces: pure helper `select_action_item_source(text: &str, english_markdown: &str, total_tokens: usize, token_threshold: usize) -> String`, unit-tested below
- Produces: Tauri event `action-items-updated` with payload `{ meeting_id: String, count: usize }`, consumed by the frontend hook in Task 6

- [ ] **Step 1: Write the failing test**

  Add this pure helper (with a failing body) plus its test module near the top of `frontend/src-tauri/src/summary/service.rs`, just below the existing `template_cache_fingerprint` function (after line 131):

  ```rust
  /// Chooses what text action-item extraction runs against. Short/single-pass
  /// transcripts (the common case: cloud providers, or local providers under
  /// `token_threshold`) use the raw transcript directly. Transcripts long
  /// enough to have required `generate_meeting_summary`'s multi-level
  /// chunking use the already-condensed English summary instead of
  /// re-chunking a second time here - `build_chunk_summary_user_prompt`
  /// (`frontend/src-tauri/src/summary/processor.rs:137`) explicitly
  /// instructs each chunk-level summary to retain "action items", so they
  /// survive that condensation.
  fn select_action_item_source(text: &str, english_markdown: &str, total_tokens: usize, token_threshold: usize) -> String {
      unimplemented!()
  }

  #[cfg(test)]
  mod action_item_source_tests {
      use super::*;

      #[test]
      fn short_transcript_uses_raw_transcript() {
          let source = select_action_item_source("raw transcript text", "condensed summary", 100, 4000);
          assert_eq!(source, "raw transcript text");
      }

      #[test]
      fn long_transcript_uses_condensed_summary() {
          let source = select_action_item_source("raw transcript text", "condensed summary", 8000, 4000);
          assert_eq!(source, "condensed summary");
      }

      #[test]
      fn exactly_at_threshold_uses_raw_transcript() {
          // Mirrors generate_meeting_summary's own `total_tokens < token_threshold`
          // single-pass condition (frontend/src-tauri/src/summary/processor.rs:369).
          let source = select_action_item_source("raw transcript text", "condensed summary", 4000, 4000);
          assert_eq!(source, "condensed summary");
      }
  }
  ```

- [ ] **Step 2: Run test to verify it fails**

  ```bash
  cd frontend/src-tauri && cargo test --lib summary::service::action_item_source_tests
  ```

  Expected: panics with `not implemented`.

- [ ] **Step 3: Write the real implementation**

  Implement `select_action_item_source`:

  ```rust
  fn select_action_item_source(text: &str, english_markdown: &str, total_tokens: usize, token_threshold: usize) -> String {
      if total_tokens < token_threshold {
          text.to_string()
      } else {
          english_markdown.to_string()
      }
  }
  ```

  Update the imports at the top of `frontend/src-tauri/src/summary/service.rs` (lines 1-21):

  ```rust
  use crate::database::repositories::{
      action_item::{ActionItemsRepository, NewActionItem},
      meeting::MeetingsRepository, setting::SettingsRepository, summary::SummaryProcessesRepository,
  };
  use crate::summary::action_items::extract_action_items;
  use crate::summary::llm_client::LLMProvider;
  use crate::summary::language_detection::detect_summary_language;
  use crate::summary::metadata::read_detected_summary_language_from_metadata;
  use crate::summary::processor::{
      extract_meeting_name_from_markdown, generate_meeting_summary, language_name_from_code,
      rough_token_count,
  };
  use crate::summary::templates::{self, Template};
  use crate::ollama::metadata::ModelMetadataCache;
  use chrono::Utc;
  use serde::{Deserialize, Serialize};
  use sqlx::SqlitePool;
  use std::collections::HashMap;
  use std::path::{Path, PathBuf};
  use std::sync::{Arc, Mutex};
  use std::time::{Duration, Instant};
  use tauri::{AppHandle, Emitter, Manager};
  use tokio_util::sync::CancellationToken;
  use tracing::{error, info, warn};
  use once_cell::sync::Lazy;
  ```

  Rename the unused `_app` parameter (line 295) to `app` since it's now used to emit the completion event, and update its two other references in the same function: the doc comment at line 286 (`/// * \`_app\` - Tauri app handle (for future use)` → `/// * \`app\` - Tauri app handle, used to emit \`action-items-updated\` once background action-item extraction completes`) and the existing usage at line 440:

  ```rust
  pub async fn process_transcript_background<R: tauri::Runtime>(
      app: AppHandle<R>,
      pool: SqlitePool,
  ```

  ```rust
      let app_data_dir = app.path().app_data_dir().ok();
  ```

  Insert the extraction spawn between the existing lines 582 and 583 (right after the `info!("Summary saved successfully for meeting_id: {}", meeting_id);` `else` branch closes, still inside the `Ok((final_markdown, english_markdown, num_chunks)) =>` arm):

  ```rust
                  }

                  // Structural action-item extraction: its own background
                  // task so it never delays the "completed" status
                  // `api_get_summary` polls for above. Independent of the
                  // language/translation pipeline earlier in this function -
                  // extraction always runs against the English working text,
                  // matching `english_markdown`'s role as the canonical
                  // AI-generated English text throughout this module.
                  let total_tokens = rough_token_count(&text);
                  let action_item_source =
                      select_action_item_source(&text, &english_markdown, total_tokens, token_threshold);
                  let reference_time = Utc::now();
                  tauri::async_runtime::spawn(Self::extract_and_save_action_items(
                      app.clone(),
                      pool.clone(),
                      meeting_id.clone(),
                      action_item_source,
                      provider.clone(),
                      model_name.clone(),
                      final_api_key.clone(),
                      reference_time,
                      ollama_endpoint.clone(),
                      custom_openai_endpoint.clone(),
                      custom_openai_max_tokens,
                      custom_openai_temperature,
                      custom_openai_top_p,
                      app_data_dir.clone(),
                  ));
              }
  ```

  Add the new private method just before the closing brace of `impl SummaryService` (after the existing `update_process_failed` method, i.e. right before line 619's `}`):

  ```rust
      /// Runs action-item extraction and persists the result, as its own
      /// fire-and-forget task spawned from the successful branch of
      /// `process_transcript_background` above. Failures are logged and
      /// swallowed rather than propagated - a failed extraction should never
      /// retroactively mark an already-completed summary as failed, and the
      /// checklist UI simply shows nothing for this meeting until the next
      /// regeneration.
      #[allow(clippy::too_many_arguments)]
      async fn extract_and_save_action_items<R: tauri::Runtime>(
          app: AppHandle<R>,
          pool: SqlitePool,
          meeting_id: String,
          source_text: String,
          provider: LLMProvider,
          model_name: String,
          api_key: String,
          reference_time: chrono::DateTime<chrono::Utc>,
          ollama_endpoint: Option<String>,
          custom_openai_endpoint: Option<String>,
          max_tokens: Option<u32>,
          temperature: Option<f32>,
          top_p: Option<f32>,
          app_data_dir: Option<std::path::PathBuf>,
      ) {
          let client = reqwest::Client::new();
          let extracted = match extract_action_items(
              &client,
              &provider,
              &model_name,
              &api_key,
              &source_text,
              reference_time,
              ollama_endpoint.as_deref(),
              custom_openai_endpoint.as_deref(),
              max_tokens,
              temperature,
              top_p,
              app_data_dir.as_ref(),
              None,
          )
          .await
          {
              Ok(items) => items,
              Err(e) => {
                  warn!("Action item extraction failed for meeting_id {}: {}", meeting_id, e);
                  return;
              }
          };

          let new_items: Vec<NewActionItem> = extracted
              .into_iter()
              .map(|item| NewActionItem {
                  task: item.task,
                  owner: item.owner,
                  due_date_text: item.due_date_text,
                  due_date: item.due_date,
              })
              .collect();

          match ActionItemsRepository::replace_for_meeting(&pool, &meeting_id, &new_items).await {
              Ok(saved) => {
                  info!(
                      "Saved {} action item(s) for meeting_id: {}",
                      saved.len(),
                      meeting_id
                  );
                  if let Err(e) = app.emit(
                      "action-items-updated",
                      serde_json::json!({ "meeting_id": meeting_id, "count": saved.len() }),
                  ) {
                      warn!("Failed to emit action-items-updated for {}: {}", meeting_id, e);
                  }
              }
              Err(e) => error!("Failed to save action items for meeting_id {}: {}", meeting_id, e),
          }
      }
  ```

- [ ] **Step 4: Run tests to verify they pass**

  ```bash
  cd frontend/src-tauri && cargo test --lib summary::service
  cd frontend/src-tauri && cargo check
  ```

  Expected: the 3 new `action_item_source_tests` pass, all pre-existing `summary::service` tests still pass, and `cargo check` compiles cleanly (confirms the spawn/method wiring type-checks even though it isn't itself covered by an automated test - this repo's own convention, e.g. `process_transcript_background` itself, is to unit-test the pure helpers around a live-LLM-calling function rather than the network-calling function itself).

- [ ] **Step 5: Commit**

  ```bash
  git add frontend/src-tauri/src/summary/service.rs
  git commit -m "Wire action-item extraction into summary generation background task"
  ```

---

### Task 5: Tauri commands for fetching and toggling action items

**Files:**
- Modify: `frontend/src-tauri/src/summary/action_items/commands.rs` (replace Task 2's empty placeholder)
- Modify: `frontend/src-tauri/src/summary/mod.rs:44-69` (re-export the new commands)
- Modify: `frontend/src-tauri/src/lib.rs:687` (register in `generate_handler!`)

**Interfaces:**
- Consumes: `database::repositories::action_item::ActionItemsRepository` (Task 1), `state::AppState`
- Produces: Tauri commands `api_get_action_items(meeting_id: String) -> Result<Vec<ActionItem>, String>` and `api_toggle_action_item(meeting_id: String, item_id: String, completed: bool) -> Result<ActionItem, String>`, consumed by the frontend hook in Task 6

- [ ] **Step 1: Write the failing test**

  These commands are thin wrappers over the already-tested `ActionItemsRepository` (Task 1), so there's no new business logic to unit test here - the repository tests are the coverage, matching how `frontend/src-tauri/src/summary/commands.rs`'s other thin command wrappers (e.g. `api_cancel_summary`) have no dedicated unit tests of their own. Verification for this task is `cargo check` succeeding with the commands registered (Step 4).

- [ ] **Step 2: N/A**

  (No separate "run to verify it fails" step - see Step 1.)

- [ ] **Step 3: Write the implementation**

  Replace `frontend/src-tauri/src/summary/action_items/commands.rs`:

  ```rust
  use crate::database::models::ActionItem;
  use crate::database::repositories::action_item::ActionItemsRepository;
  use crate::state::AppState;
  use log::{error as log_error, info as log_info};
  use tauri::{AppHandle, Runtime};

  /// Returns the structurally-extracted action items for a meeting, ordered
  /// the same way they were extracted (`sort_order` ASC) - the checklist's
  /// display order.
  #[tauri::command]
  pub async fn api_get_action_items<R: Runtime>(
      _app: AppHandle<R>,
      state: tauri::State<'_, AppState>,
      meeting_id: String,
  ) -> Result<Vec<ActionItem>, String> {
      log_info!("api_get_action_items called for meeting_id: {}", meeting_id);
      ActionItemsRepository::get_for_meeting(state.db_manager.pool(), &meeting_id)
          .await
          .map_err(|e| {
              log_error!("Failed to load action items for meeting_id {}: {}", meeting_id, e);
              format!("Failed to load action items: {}", e)
          })
  }

  /// Toggles an action item's completed state, scoped to the given meeting
  /// id so an id from a different meeting can never be toggled through this
  /// command.
  #[tauri::command]
  pub async fn api_toggle_action_item<R: Runtime>(
      _app: AppHandle<R>,
      state: tauri::State<'_, AppState>,
      meeting_id: String,
      item_id: String,
      completed: bool,
  ) -> Result<ActionItem, String> {
      log_info!(
          "api_toggle_action_item called for meeting_id: {}, item_id: {}, completed: {}",
          meeting_id, item_id, completed
      );
      ActionItemsRepository::set_completed(state.db_manager.pool(), &meeting_id, &item_id, completed)
          .await
          .map_err(|e| {
              log_error!("Failed to toggle action item {}: {}", item_id, e);
              format!("Failed to update action item: {}", e)
          })?
          .ok_or_else(|| "Action item not found for this meeting.".to_string())
  }
  ```

  Re-export from `frontend/src-tauri/src/summary/mod.rs` — add after the existing template commands re-export block (after line 69, following the same `__cmd__`/`__tauri_command_name_` pattern Tauri's macro generates):

  ```rust
  // Re-export action item commands
  pub use action_items::commands::{
      __cmd__api_get_action_items, __cmd__api_toggle_action_item,
      __tauri_command_name_api_get_action_items, __tauri_command_name_api_toggle_action_item,
      api_get_action_items, api_toggle_action_item,
  };
  ```

  Register in `frontend/src-tauri/src/lib.rs`, inserting between line 687 (`summary::template_commands::api_validate_template,`) and line 688 (`// Built-in AI commands`):

  ```rust
              summary::template_commands::api_validate_template,
              // Action item commands
              summary::action_items::commands::api_get_action_items,
              summary::action_items::commands::api_toggle_action_item,
              // Built-in AI commands
  ```

- [ ] **Step 4: Verify it compiles and the repository-level behavior it wraps is covered**

  ```bash
  cd frontend/src-tauri && cargo check
  cd frontend/src-tauri && cargo test --lib database::repositories::action_item
  ```

  Expected: `cargo check` succeeds (confirms both commands are registered and type-check against `AppState`/`ActionItem`); the Task 1 repository tests still pass, exercising the exact `ActionItemsRepository` calls these commands make.

- [ ] **Step 5: Commit**

  ```bash
  git add frontend/src-tauri/src/summary/action_items/commands.rs \
          frontend/src-tauri/src/summary/mod.rs \
          frontend/src-tauri/src/lib.rs
  git commit -m "Add api_get_action_items and api_toggle_action_item Tauri commands"
  ```

---

### Task 6: Frontend types and `useActionItems` hook

**Files:**
- Modify: `frontend/src/types/index.ts:95` (insert `ActionItem` interface after `MeetingMetadata`)
- Create: `frontend/src/hooks/meeting-details/useActionItems.ts`

**Interfaces:**
- Consumes: Tauri commands `api_get_action_items`, `api_toggle_action_item` (Task 5); Tauri event `action-items-updated` (Task 4)
- Produces: `useActionItems(meetingId: string) -> { items: ActionItem[], isLoading: boolean, toggleItem: (itemId: string, completed: boolean) => Promise<void> }`, consumed by `SummaryPanel`/`page-content.tsx` in Task 7

- [ ] **Step 1: Add the type**

  Insert into `frontend/src/types/index.ts` after line 95 (the closing `}` of `MeetingMetadata`, before `export interface PaginatedTranscriptsResponse {`):

  ```typescript
  export interface ActionItem {
    id: string;
    meeting_id: string;
    task: string;
    owner: string | null;
    due_date_text: string | null;
    due_date: string | null; // ISO 8601 date (YYYY-MM-DD), if resolvable
    completed: boolean;
    sort_order: number;
    created_at: string;
    updated_at: string;
  }
  ```

  This repo has no existing unit-test coverage for its meeting-details hooks (`useMeetingData`, `useSummaryGeneration`, etc. have no colocated `.test.ts` files despite `bun test` being configured in `package.json`), so there's no established pattern to extend here with a TDD red/green cycle. Steps 2-5 below implement `useActionItems` directly and verify it by running the app, consistent with how its sibling hooks are verified in this codebase.

- [ ] **Step 2: Write `useActionItems`**

  Create `frontend/src/hooks/meeting-details/useActionItems.ts`:

  ```typescript
  'use client';

  import { useCallback, useEffect, useRef, useState } from 'react';
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { toast } from 'sonner';
  import { ActionItem } from '@/types';

  /**
   * Structural action items for a meeting, extracted by
   * `SummaryService::extract_and_save_action_items`
   * (frontend/src-tauri/src/summary/service.rs) after each summary
   * generation. Loads the current list on mount/meeting change, refetches
   * when Rust emits `action-items-updated` for this meeting (extraction runs
   * as its own background task, decoupled from summary completion), and
   * exposes an optimistic toggle that rolls back on failure.
   */
  export function useActionItems(meetingId: string) {
    const [items, setItems] = useState<ActionItem[]>([]);
    const [isLoading, setIsLoading] = useState(false);
    const loadVersionRef = useRef(0);

    const load = useCallback(async () => {
      const version = ++loadVersionRef.current;
      setIsLoading(true);
      try {
        const result = await invoke<ActionItem[]>('api_get_action_items', { meetingId });
        if (loadVersionRef.current === version) {
          setItems(result);
        }
      } catch (err) {
        console.error('Failed to load action items:', err);
        if (loadVersionRef.current === version) {
          setItems([]);
        }
      } finally {
        if (loadVersionRef.current === version) {
          setIsLoading(false);
        }
      }
    }, [meetingId]);

    useEffect(() => {
      void load();
    }, [load]);

    useEffect(() => {
      let unlisten: (() => void) | undefined;
      let cancelled = false;

      listen<{ meeting_id: string }>('action-items-updated', (event) => {
        if (event.payload.meeting_id === meetingId) {
          void load();
        }
      }).then((fn) => {
        if (cancelled) {
          fn();
        } else {
          unlisten = fn;
        }
      });

      return () => {
        cancelled = true;
        unlisten?.();
      };
    }, [meetingId, load]);

    const toggleItem = useCallback(
      async (itemId: string, completed: boolean) => {
        const previous = items;
        setItems((current) =>
          current.map((item) => (item.id === itemId ? { ...item, completed } : item))
        );

        try {
          await invoke<ActionItem>('api_toggle_action_item', { meetingId, itemId, completed });
        } catch (err) {
          console.error('Failed to toggle action item:', err);
          toast.error('Failed to update action item');
          setItems(previous);
        }
      },
      [items, meetingId]
    );

    return { items, isLoading, toggleItem };
  }
  ```

- [ ] **Step 3: Verify types compile**

  ```bash
  cd frontend && pnpm exec tsc --noEmit
  ```

  Expected: no new type errors from `types/index.ts` or `useActionItems.ts`.

- [ ] **Step 4: Manual smoke check (no automated test harness exists for this layer)**

  This hook is exercised end-to-end once wired into the UI in Task 7; there is no isolated way to invoke it without a mounted component and a live Tauri backend, matching this repo's existing hooks.

- [ ] **Step 5: Commit**

  ```bash
  git add frontend/src/types/index.ts frontend/src/hooks/meeting-details/useActionItems.ts
  git commit -m "Add ActionItem type and useActionItems hook"
  ```

---

### Task 7: Checklist UI in the meeting details summary panel

**Files:**
- Create: `frontend/src/components/MeetingDetails/ActionItemsChecklist.tsx`
- Modify: `frontend/src/components/MeetingDetails/SummaryPanel.tsx:1-64` (import + props)
- Modify: `frontend/src/components/MeetingDetails/SummaryPanel.tsx:409` (render checklist above `BlockNoteSummaryView`)
- Modify: `frontend/src/app/meeting-details/page-content.tsx:22` (import `useActionItems`)
- Modify: `frontend/src/app/meeting-details/page-content.tsx:83` (call the hook)
- Modify: `frontend/src/app/meeting-details/page-content.tsx:251` (pass props to `SummaryPanel`)

**Interfaces:**
- Consumes: `useActionItems` (Task 6), `ActionItem` type (Task 6)
- Produces: `<ActionItemsChecklist items={...} onToggle={...} />`

- [ ] **Step 1: Add the shadcn `Checkbox` primitive**

  This repo already uses shadcn/Radix for its UI primitives (`frontend/components.json`, `frontend/src/components/ui/switch.tsx`, `button.tsx`, etc.) but has no checkbox component yet. Add it the same way the others were generated:

  ```bash
  cd frontend && pnpm dlx shadcn@latest add checkbox
  ```

  Expected: creates `frontend/src/components/ui/checkbox.tsx` and adds `@radix-ui/react-checkbox` to `frontend/package.json`.

- [ ] **Step 2: Create `ActionItemsChecklist`**

  Create `frontend/src/components/MeetingDetails/ActionItemsChecklist.tsx`:

  ```tsx
  'use client';

  import { Checkbox } from '@/components/ui/checkbox';
  import { ActionItem } from '@/types';
  import { cn } from '@/lib/utils';

  interface ActionItemsChecklistProps {
    items: ActionItem[];
    onToggle: (itemId: string, completed: boolean) => void;
  }

  /**
   * Structural checklist rendered above the free-form BlockNote summary in
   * `SummaryPanel`. The summary's own "Action Items" section (from
   * `standard_meeting.json` et al.) stays as prose inside the editor; this
   * is the separately-extracted, checkable counterpart backed by the
   * `action_items` table (see
   * docs/superpowers/plans/2026-08-11-action-item-extraction.md).
   */
  export function ActionItemsChecklist({ items, onToggle }: ActionItemsChecklistProps) {
    if (items.length === 0) return null;

    return (
      <div className="glass-card p-4 mx-6 lg:mx-10 mt-4">
        <h4 className="font-medium mb-2 text-foreground">Action Items</h4>
        <ul className="space-y-2">
          {items.map((item) => (
            <li key={item.id} className="flex items-start gap-2">
              <Checkbox
                checked={item.completed}
                onCheckedChange={(checked) => onToggle(item.id, checked === true)}
                className="mt-0.5"
                aria-label={`Mark "${item.task}" as ${item.completed ? 'not done' : 'done'}`}
              />
              <div className="flex-1 min-w-0">
                <span
                  className={cn(
                    'text-sm text-foreground/90',
                    item.completed && 'line-through text-foreground/50'
                  )}
                >
                  {item.task}
                </span>
                {(item.owner || item.due_date_text) && (
                  <div className="text-xs text-foreground/50 mt-0.5">
                    {item.owner && <span>{item.owner}</span>}
                    {item.owner && item.due_date_text && <span> • </span>}
                    {item.due_date_text && <span>{item.due_date_text}</span>}
                  </div>
                )}
              </div>
            </li>
          ))}
        </ul>
      </div>
    );
  }
  ```

- [ ] **Step 3: Wire into `SummaryPanel`**

  In `frontend/src/components/MeetingDetails/SummaryPanel.tsx`, add the import after line 6 (`import { EmptyStateSummary } from '@/components/EmptyStateSummary';`):

  ```typescript
  import { ActionItemsChecklist } from '@/components/MeetingDetails/ActionItemsChecklist';
  ```

  Add two props to the `SummaryPanelProps` interface, right after `onOpenModelSettings?: (openFn: () => void) => void;` (line 63):

  ```typescript
    onOpenModelSettings?: (openFn: () => void) => void;
    actionItems: ActionItem[];
    onToggleActionItem: (itemId: string, completed: boolean) => void;
  ```

  Import `ActionItem` alongside the existing type import at line 3:

  ```typescript
  import { Summary, SummaryResponse, Transcript, ActionItem } from '@/types';
  ```

  Destructure the new props in the function signature, after `onOpenModelSettings` (line 99):

  ```typescript
    onOpenModelSettings,
    actionItems,
    onToggleActionItem
  }: SummaryPanelProps) {
  ```

  Render the checklist between line 409 (`)}` closing the `summaryResponse &&` block) and line 410 (`<div className="p-6 lg:px-10 w-full">`):

  ```tsx
            </div>
          )}
          <ActionItemsChecklist items={actionItems} onToggle={onToggleActionItem} />
          <div className="p-6 lg:px-10 w-full">
  ```

- [ ] **Step 4: Wire into `page-content.tsx`**

  Add the import after line 22 (`import { useConfig } from '@/contexts/ConfigContext';`):

  ```typescript
  import { useActionItems } from '@/hooks/meeting-details/useActionItems';
  ```

  Call the hook after line 83 (`const templates = useTemplates();`):

  ```typescript
    const actionItems = useActionItems(meeting.id);
  ```

  Pass the props to `<SummaryPanel .../>`, after `onOpenModelSettings={handleRegisterModalOpen}` (line 251):

  ```tsx
            onOpenModelSettings={handleRegisterModalOpen}
            actionItems={actionItems.items}
            onToggleActionItem={actionItems.toggleItem}
          />
  ```

- [ ] **Step 5: Manually verify and commit**

  ```bash
  cd frontend && pnpm exec tsc --noEmit
  cd frontend && ./clean_run.sh
  ```

  In the running app: open a meeting with a completed summary, confirm the Action Items checklist renders above the summary editor once extraction finishes (it may take a few seconds after the summary itself completes, since extraction is a separate background task per Task 4), check an item off, reload the meeting, and confirm the checked state persisted.

  ```bash
  git add frontend/src/components/MeetingDetails/ActionItemsChecklist.tsx \
          frontend/src/components/MeetingDetails/SummaryPanel.tsx \
          frontend/src/app/meeting-details/page-content.tsx \
          frontend/package.json frontend/pnpm-lock.yaml \
          frontend/src/components/ui/checkbox.tsx
  git commit -m "Render structural action items as a checkable list in the summary panel"
  ```

---

## Out of scope: v2 — calendar/reminders sync

Syncing extracted action items to the OS's native Calendar/Reminders app (macOS EventKit, Windows equivalents) is explicitly deferred to a v2 and is not part of this plan. Unlike everything above, which is pure Rust/SQLite/React work reusing infrastructure this crate already has (the LLM provider abstraction, the migration/repository pattern, Tauri commands, React hooks), OS calendar/reminders access has no existing foothold anywhere in this codebase: it requires a new Tauri plugin wrapping platform-specific, permissioned native APIs (EventKit on macOS, an equivalent on Windows, and likely nothing usable on Linux), with its own permission-prompt UX, its own per-platform build and signing implications, and its own failure modes (calendar not granted, no default calendar, duplicate-write handling on re-sync) that are unrelated to and substantially larger than the extraction-and-checklist work itself. Structural extraction plus the in-app checklist (this plan) is the right v1 boundary: it delivers the core value (action items become data instead of buried prose) without coupling that to a separate, platform-heavy integration effort that deserves its own scoped plan.
