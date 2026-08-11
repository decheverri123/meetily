# Cross-Meeting Semantic Search (RAG) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace `ask_across_meetings`'s "pack summaries until 100k chars, drop the rest" approach with real retrieval — hybrid keyword + local-vector search over chunked transcripts across every stored meeting, fused with RRF, so answers are pulled from the whole corpus (not just the most recent ~65 meetings) and cited back to the specific meeting and timestamp.

**Architecture:** A background indexer chunks each meeting's transcript on speaker-turn/segment boundaries after its summary completes, embeds each chunk locally with `fastembed-rs` (ONNX, no network), and stores chunk text + metadata in a new `rag_chunks` table with vectors in a `sqlite-vec` virtual table in the same SQLite file the app already uses. `ask_across_meetings` rewrites follow-up questions into standalone queries, retrieves the top chunks via FTS5 (keyword) + vector cosine similarity fused with Reciprocal Rank Fusion, and answers from those chunks instead of summaries — returning structured `(meeting_id, title, timestamp)` citations that `GlobalAskPanel` renders as clickable chips navigating to `/meeting-details?id=...&focusTime=...`.

**Tech Stack:** `sqlite-vec` (vector storage/KNN, statically linked via `SQLITE_CORE`), `fastembed-rs` (local ONNX embeddings + reranking), SQLite FTS5 (already built into SQLite, zero new dependency), existing `sqlx` 0.8 pool and migration system, existing `llm_client::generate_summary` / `ask_configured_llm` for the LLM calls.

## Global Constraints

- Fully local/offline: embedding, reranking, and vector search must never make a network call at query time. Cloud LLM providers are still used only for the final answer generation and query rewriting, exactly as `ask_configured_llm` already does today — no new cloud dependency is introduced.
- Reuse the existing `sqlx::SqlitePool` (`state.db_manager.pool()`) and the existing `frontend/src-tauri/migrations/` timestamped-migration system — do not stand up a second database or a sidecar vector-DB process.
- Reuse existing infra instead of duplicating it: `rough_token_count`/token math from [frontend/src-tauri/src/summary/processor.rs](../../../frontend/src-tauri/src/summary/processor.rs), the provider-agnostic `ask_configured_llm` helper in [frontend/src-tauri/src/summary/commands.rs](../../../frontend/src-tauri/src/summary/commands.rs), the `useAskAI` hook, and the `[MM:SS]` citation-chip machinery in `askCitations.ts`/`CitationChip.tsx` wherever its contract actually fits.
- Do not confuse the existing `transcript_chunks` table / `TranscriptChunksRepository` ([frontend/src-tauri/src/database/repositories/transcript_chunk.rs](../../../frontend/src-tauri/src/database/repositories/transcript_chunk.rs)) with RAG chunking — that table stores one row per meeting holding the *whole* transcript text plus Whisper processing parameters (`chunk_size`/`overlap` are LLM-summarization chunk params, not retrieval chunks). The new RAG chunk table must have a distinct name (`rag_chunks`) to avoid this collision.
- Version the vector schema and embedding model identifier from day one (`rag_chunks.embedding_model`, `rag_chunks.embedding_dims`, `rag_chunks.schema_version`) — per the research doc, `sqlite-vec` is pre-1.0 and you will change embedding models, and both are migrations, not overwrites.
- Follow the decisions already made in [docs/rag-token-research.md](../../rag-token-research.md): brute-force KNN (no ANN index — verified fast enough at this scale), 384–768-dim embedding model, speaker-turn/segment-based chunking (~200–400 tokens, one turn of overlap), hybrid FTS5+vector retrieval fused with RRF (not naive score blending), and contextual query rewriting for follow-up questions. Do not re-derive these; build on them.

## Non-Goals (explicitly out of scope for this plan)

- **Live in-progress meeting Q&A (`ask_about_live_transcript`) and single-meeting Q&A (`ask_about_meeting`)** are unaffected. The research doc's Path A/B fixes (prompt caching, rolling summaries) are a separate, unrelated plan — this plan is Path C only.
- **ANN indexing** (usearch/hnsw_rs/external engine) — the research doc's verified benchmark shows brute force stays under 75ms through 100k vectors at ≤1024 dims, and 200 meetings is ~20-40k chunks. Not needed now; the research doc's own upgrade ladder (metadata pre-filter → binary quantization → dimension truncation → ANN) is the trigger order if it ever is.
- **Metadata pre-filtering by date/meeting before KNN** — the research doc flags this as unmeasured/roadmap in `sqlite-vec`. Task 7 below filters by joining back to `rag_chunks` in SQL, not via an unverified `sqlite-vec` partition-key feature.
- **Hierarchical "search meeting summaries first, then drill into chunks"** — explicitly flagged in the research doc as "no verified evidence, design intuition, not a finding." Left for a future plan once the flat retrieval in Task 7 has been measured.
- **Query rewriting for `ask_about_meeting`/`ask_about_live_transcript`** — the research doc recommends contextual query rewriting for all three ask paths (section 5, Phase 0 item 3), but Task 8 below applies it only to `ask_across_meetings`, since retrieval (where a bad query hurts most) only exists on that path. Apply the same `rewrite_followup_query` pattern to the other two panels in the separate Path A/B plan.
- **A formal recall@k/MRR evaluation harness** — the research doc's section 6 measurement guidance ("hand-label ~50 questions... measure recall@k and MRR") is explicitly "standard practice, not a finding," with no verified methodology to build against. Measure ad hoc against Task 7's retrieval once it ships, per the doc's own "re-measure on your own data" framing, rather than building eval tooling speculatively here.

---

### Task 1: FTS5 keyword search over transcript segments

**Files:**
- Create: `frontend/src-tauri/migrations/20260811010000_add_transcripts_fts.sql`
- Modify: `frontend/src-tauri/src/database/repositories/transcript.rs` (add method after `search_transcripts`, currently ending around line 118)
- Test: `frontend/src-tauri/src/database/repositories/transcript.rs` (inline `#[cfg(test)]` module, using `test_support::setup_pool`/`insert_meeting` from [frontend/src-tauri/src/database/repositories/test_support.rs](../../../frontend/src-tauri/src/database/repositories/test_support.rs))

**Interfaces:**
- Consumes: `sqlx::SqlitePool`, existing `transcripts` table (`id`, `meeting_id`, `transcript`, `audio_start_time`).
- Produces: `pub struct FtsSearchHit { pub transcript_id: String, pub meeting_id: String, pub snippet: String }` and `pub async fn TranscriptsRepository::search_transcripts_fts(pool: &SqlitePool, query: &str, limit: i64) -> Result<Vec<FtsSearchHit>, SqlxError>`, plus `pub fn sanitize_fts_query(raw: &str) -> String` (pure, used by Task 7's hybrid retrieval too).

This is Phase 0 item 4 from the research doc: SQLite already ships FTS5, so this needs zero new dependencies and, standalone, already fixes "find the meeting where we mentioned $SPECIFIC_THING" — the case `build_cross_meeting_context`'s truncation currently breaks.

`transcripts.id` is a `TEXT` UUID-style key (`transcript-<uuid>`), not an `INTEGER` rowid, so FTS5's `content=`/`content_rowid=` external-content mode (which requires an integer rowid alias) doesn't apply here. Use a standalone FTS5 table kept in sync with triggers instead.

- [ ] **Step 1: Write the failing test**

  In `frontend/src-tauri/src/database/repositories/transcript.rs`, append:
  ```rust
  #[cfg(test)]
  mod fts_tests {
      use super::*;
      use crate::database::repositories::test_support::{insert_meeting, setup_pool};

      async fn insert_transcript(pool: &sqlx::SqlitePool, id: &str, meeting_id: &str, text: &str) {
          sqlx::query(
              "INSERT INTO transcripts (id, meeting_id, transcript, timestamp, audio_start_time) \
               VALUES (?, ?, ?, ?, ?)",
          )
          .bind(id)
          .bind(meeting_id)
          .bind(text)
          .bind("00:00:00")
          .bind(0.0)
          .execute(pool)
          .await
          .expect("failed to insert transcript");
      }

      #[tokio::test]
      async fn search_transcripts_fts_finds_inserted_rows() {
          let pool = setup_pool().await;
          insert_meeting(&pool, "meeting-1").await;
          insert_transcript(&pool, "t-1", "meeting-1", "we should ship the redesign on Friday").await;
          insert_transcript(&pool, "t-2", "meeting-1", "the weather has been nice lately").await;

          let hits = TranscriptsRepository::search_transcripts_fts(&pool, "redesign", 10)
              .await
              .expect("search failed");

          assert_eq!(hits.len(), 1);
          assert_eq!(hits[0].transcript_id, "t-1");
          assert_eq!(hits[0].meeting_id, "meeting-1");
      }

      #[tokio::test]
      async fn search_transcripts_fts_handles_multi_word_queries() {
          let pool = setup_pool().await;
          insert_meeting(&pool, "meeting-1").await;
          insert_transcript(&pool, "t-1", "meeting-1", "the launch date moved to next quarter").await;

          let hits = TranscriptsRepository::search_transcripts_fts(&pool, "launch date", 10)
              .await
              .expect("search failed");

          assert_eq!(hits.len(), 1);
      }

      #[test]
      fn sanitize_fts_query_quotes_each_term() {
          assert_eq!(sanitize_fts_query("launch date"), "\"launch\" OR \"date\"");
      }

      #[test]
      fn sanitize_fts_query_strips_embedded_quotes() {
          // A raw double quote is FTS5 phrase syntax; a naive pass-through
          // would let a question containing one open an unterminated phrase.
          assert_eq!(sanitize_fts_query(r#"what did "acme corp" decide"#), "\"what\" OR \"did\" OR \"acme\" OR \"corp\" OR \"decide\"");
      }
  }
  ```

- [ ] **Step 2: Run test to verify it fails**

  ```bash
  cd frontend/src-tauri && cargo test --lib fts_tests
  ```
  Expected: compile error (`search_transcripts_fts` and `sanitize_fts_query` don't exist yet) or, once stubbed to compile, a runtime failure because `transcripts_fts` doesn't exist in the migrated schema.

- [ ] **Step 3: Write the migration**

  `frontend/src-tauri/migrations/20260811010000_add_transcripts_fts.sql`:
  ```sql
  -- FTS5 keyword index over transcript segments, kept in sync with the
  -- `transcripts` table via triggers (not FTS5 external-content mode, since
  -- transcripts.id is a TEXT uuid rather than an INTEGER rowid).
  CREATE VIRTUAL TABLE IF NOT EXISTS transcripts_fts USING fts5(
      transcript,
      transcript_id UNINDEXED,
      meeting_id UNINDEXED,
      tokenize = 'porter unicode61'
  );

  INSERT INTO transcripts_fts(transcript_id, meeting_id, transcript)
  SELECT id, meeting_id, transcript FROM transcripts;

  CREATE TRIGGER IF NOT EXISTS transcripts_fts_ai AFTER INSERT ON transcripts BEGIN
      INSERT INTO transcripts_fts(transcript_id, meeting_id, transcript)
      VALUES (new.id, new.meeting_id, new.transcript);
  END;

  CREATE TRIGGER IF NOT EXISTS transcripts_fts_ad AFTER DELETE ON transcripts BEGIN
      DELETE FROM transcripts_fts WHERE transcript_id = old.id;
  END;

  CREATE TRIGGER IF NOT EXISTS transcripts_fts_au AFTER UPDATE ON transcripts BEGIN
      DELETE FROM transcripts_fts WHERE transcript_id = old.id;
      INSERT INTO transcripts_fts(transcript_id, meeting_id, transcript)
      VALUES (new.id, new.meeting_id, new.transcript);
  END;
  ```

- [ ] **Step 4: Implement `sanitize_fts_query` and `search_transcripts_fts`**

  In `frontend/src-tauri/src/database/repositories/transcript.rs`, after the existing `search_transcripts`/`get_match_context` methods:
  ```rust
  /// A hit from `search_transcripts_fts`: which segment/meeting matched and a
  /// highlighted snippet, ranked by FTS5's built-in bm25 rank.
  #[derive(Debug, Clone)]
  pub struct FtsSearchHit {
      pub transcript_id: String,
      pub meeting_id: String,
      pub snippet: String,
  }

  /// Turns a raw natural-language query into a safe FTS5 MATCH expression:
  /// each whitespace-separated term is double-quoted (so punctuation and FTS5
  /// operators in the user's own text can't be interpreted as query syntax,
  /// and an unbalanced `"` in the input can't open an unterminated phrase)
  /// and the terms are OR'd together, since a natural-language question isn't
  /// a phrase match.
  pub fn sanitize_fts_query(raw: &str) -> String {
      raw.split_whitespace()
          .map(|term| format!("\"{}\"", term.replace('"', "")))
          .filter(|t| t.len() > 2) // skip bare `""` from an all-quote term
          .collect::<Vec<_>>()
          .join(" OR ")
  }

  impl TranscriptsRepository {
      /// Keyword search across every meeting's transcript segments via the
      /// `transcripts_fts` FTS5 index, ranked by SQLite's built-in bm25().
      /// Distinct from `search_transcripts` above (a `LIKE '%query%'` scan,
      /// still used by the meeting-search box) - this is the keyword half of
      /// Task 7's hybrid cross-meeting retrieval.
      pub async fn search_transcripts_fts(
          pool: &SqlitePool,
          query: &str,
          limit: i64,
      ) -> Result<Vec<FtsSearchHit>, SqlxError> {
          let match_expr = sanitize_fts_query(query);
          if match_expr.is_empty() {
              return Ok(Vec::new());
          }

          let rows: Vec<(String, String, String)> = sqlx::query_as(
              "SELECT transcript_id, meeting_id, \
                      snippet(transcripts_fts, 0, '[', ']', '…', 12) \
               FROM transcripts_fts \
               WHERE transcripts_fts MATCH ? \
               ORDER BY rank \
               LIMIT ?",
          )
          .bind(&match_expr)
          .bind(limit)
          .fetch_all(pool)
          .await?;

          Ok(rows
              .into_iter()
              .map(|(transcript_id, meeting_id, snippet)| FtsSearchHit {
                  transcript_id,
                  meeting_id,
                  snippet,
              })
              .collect())
      }
  }
  ```

- [ ] **Step 5: Run test to verify it passes**

  ```bash
  cd frontend/src-tauri && cargo test --lib fts_tests
  ```
  Expected: `test result: ok. 4 passed`.

- [ ] **Step 6: Commit**

  ```bash
  git add frontend/src-tauri/migrations/20260811010000_add_transcripts_fts.sql frontend/src-tauri/src/database/repositories/transcript.rs
  git commit -m "feat(rag): add FTS5 keyword search over transcript segments"
  ```

---

### Task 2: Spike — decide whether `sqlite-vec` registers through the `sqlx` pool

**Files:**
- Modify: `frontend/src-tauri/Cargo.toml` (add dependencies after the `sqlx` line, currently line 147)
- Create: `frontend/src-tauri/src/rag/mod.rs`
- Create: `frontend/src-tauri/src/rag/vector_store.rs`
- Modify: `frontend/src-tauri/src/lib.rs` (register `mod rag;`)
- Test: `frontend/src-tauri/src/rag/vector_store.rs` (inline `#[cfg(test)]`)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: a proven (or disproven) `pub fn register_sqlite_vec_extension()` used by every later task that touches `rag_chunk_vectors`, and a written decision recorded in this file's Step 6.

This is the research doc's single highest-value, highest-risk spike (section 8, question 1): "Does the sqlx path work with sqlite-vec at all? ... Binary outcome, changes everything downstream." Do this before Task 3's schema commits to the approach.

- [ ] **Step 1: Add dependencies**

  In `frontend/src-tauri/Cargo.toml`, after the `sqlx = { version = "0.8", ... }` line (currently line 147):
  ```toml
  # Local vector storage/search for cross-meeting RAG (Task 2+). Pre-1.0 per
  # its own README ("expect breaking changes") - pin the exact patch version
  # here once verified against docs.rs, and bump deliberately.
  sqlite-vec = "0.1"
  # libsqlite3-sys must resolve to the SAME instance sqlx-sqlite links
  # against for sqlite3_auto_extension registration (below) to affect
  # connections sqlx opens. `cargo tree -p libsqlite3-sys` after adding this
  # must show exactly one resolved version.
  libsqlite3-sys = { version = "0.30", features = ["bundled"] }
  ```

- [ ] **Step 2: Write the failing test**

  `frontend/src-tauri/src/rag/vector_store.rs`:
  ```rust
  //! Local vector storage on top of `sqlite-vec`, statically linked into the
  //! app binary (no `.dylib`/`.dll` to bundle or notarize - see
  //! docs/rag-token-research.md section 4.1). `register_sqlite_vec_extension`
  //! must be called exactly once, before the first `sqlx::SqlitePool` is
  //! opened - `sqlite3_auto_extension` is a process-global registration that
  //! only affects connections opened after it runs.

  use std::sync::Once;

  static REGISTER_ONCE: Once = Once::new();

  /// Registers the `sqlite-vec` extension with SQLite's auto-extension
  /// mechanism, so every connection this process opens afterward - including
  /// ones `sqlx::SqlitePool` opens internally - has `vec0` virtual tables and
  /// the `vec_distance_cosine`/`vec_version` functions available. Idempotent.
  pub fn register_sqlite_vec_extension() {
      REGISTER_ONCE.call_once(|| {
          unsafe {
              let init_fn = std::mem::transmute::<
                  unsafe extern "C" fn(
                      *mut libsqlite3_sys::sqlite3,
                      *mut *mut std::os::raw::c_char,
                      *const libsqlite3_sys::sqlite3_api_routines,
                  ) -> std::os::raw::c_int,
                  Option<unsafe extern "C" fn() -> std::os::raw::c_int>,
              >(sqlite_vec::sqlite3_vec_init);
              libsqlite3_sys::sqlite3_auto_extension(init_fn);
          }
      });
  }

  #[cfg(test)]
  mod spike_tests {
      use super::*;
      use sqlx::sqlite::SqlitePoolOptions;

      /// The spike itself: if this passes, sqlite-vec registers through the
      /// sqlx pool and Task 3+ proceed as planned. If it fails to compile or
      /// fails at runtime (extension not found / vec0 unrecognized), stop -
      /// do not build Task 3 on top of this - and follow the Fallback note
      /// in Step 6 below instead.
      #[tokio::test]
      async fn sqlite_vec_registers_through_sqlx_pool() {
          register_sqlite_vec_extension();

          let pool = SqlitePoolOptions::new()
              .max_connections(1)
              .connect("sqlite::memory:")
              .await
              .expect("failed to open in-memory sqlite db");

          sqlx::query("CREATE VIRTUAL TABLE spike_vec USING vec0(embedding float[4])")
              .execute(&pool)
              .await
              .expect("vec0 virtual table creation failed - extension not registered");

          sqlx::query("INSERT INTO spike_vec(rowid, embedding) VALUES (1, ?)")
              .bind(vec![0.1f32, 0.2, 0.3, 0.4].iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>())
              .execute(&pool)
              .await
              .expect("vec0 insert failed");

          let (matched_rowid,): (i64,) = sqlx::query_as(
              "SELECT rowid FROM spike_vec WHERE embedding MATCH ? AND k = 1",
          )
          .bind(vec![0.1f32, 0.2, 0.3, 0.4].iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>())
          .fetch_one(&pool)
          .await
          .expect("vec0 KNN query failed");

          assert_eq!(matched_rowid, 1);
      }
  }
  ```

  `frontend/src-tauri/src/rag/mod.rs`:
  ```rust
  pub mod vector_store;
  ```

  In `frontend/src-tauri/src/lib.rs`, add `mod rag;` alongside the other top-level module declarations (next to the existing `mod summary;` / `mod database;` lines).

- [ ] **Step 2: Run test to verify it fails**

  ```bash
  cd frontend/src-tauri && cargo test --lib spike_tests -- --nocapture
  ```
  Expected: either a compile error (crate not yet resolvable / API mismatch — the exact `sqlite_vec` symbol name and `libsqlite3-sys` version must be checked against docs.rs at implementation time, per the research doc's explicit warning that this fact scored only 1–2/3 in verification) or a runtime panic on `CREATE VIRTUAL TABLE ... USING vec0`.

- [ ] **Step 3: Make it pass, or prove it can't**

  Iterate on `register_sqlite_vec_extension` and the `Cargo.toml` dependency versions until the test in Step 2 passes. Concretely check, in order:
  1. `cargo tree -p libsqlite3-sys` — must show one resolved version shared by `sqlx-sqlite` and the direct `libsqlite3-sys` dependency. Two versions means two separate statically-linked SQLite cores and the registration will silently affect neither of sqlx's connections.
  2. The exact export name `sqlite-vec`'s Rust crate provides for its C init function (`sqlite3_vec_init` is the C symbol per the research doc's build-script excerpt; confirm the Rust crate re-exports it under that name on docs.rs).
  3. Whether `sqlite3_auto_extension`'s signature in the resolved `libsqlite3-sys` version matches the transmute above exactly (SQLite's C signature for extension init functions can vary by binding-generation flags).

- [ ] **Step 4: Run test to verify it passes**

  ```bash
  cd frontend/src-tauri && cargo test --lib spike_tests
  ```
  Expected: `test result: ok. 1 passed`.

- [ ] **Step 5: Record the decision**

  Add a doc comment at the top of `vector_store.rs` stating the outcome and the exact `sqlite-vec`/`libsqlite3-sys` versions that were proven to work together, so Task 3 doesn't have to re-derive it.

- [ ] **Step 6: Commit**

  ```bash
  git add frontend/src-tauri/Cargo.toml frontend/src-tauri/Cargo.lock frontend/src-tauri/src/rag frontend/src-tauri/src/lib.rs
  git commit -m "spike(rag): prove sqlite-vec registers through the sqlx pool"
  ```

  **Fallback (only if Step 3 cannot be made to pass):** open one dedicated `rusqlite::Connection` (which has first-party, documented `sqlite3_auto_extension` support) pointed at the *same* SQLite file `DatabaseManager` uses, reserved exclusively for `rag_chunk_vectors` reads/writes, stored alongside `db_manager` in `AppState`. All other tables (including `rag_chunks` itself) stay on the `sqlx` pool as planned; only vector inserts/KNN queries in Task 6/7 go through the `rusqlite` connection. Re-scope Tasks 3, 6, and 7's function signatures to take `&rusqlite::Connection` for their vector-table calls instead of `&SqlitePool` before continuing — do not proceed with Task 3 until this substitution (or the primary path) is confirmed.

---

### Task 3: `rag_chunks` and `rag_chunk_vectors` schema

**Files:**
- Create: `frontend/src-tauri/migrations/20260811020000_add_rag_chunks.sql`
- Create: `frontend/src-tauri/src/database/repositories/rag_chunk.rs`
- Modify: `frontend/src-tauri/src/database/repositories/mod.rs` (export the new repository)
- Modify: `frontend/src-tauri/src/database/models.rs` (add `RagChunk` struct, after the existing `TranscriptChunk` struct at line 58)
- Test: `frontend/src-tauri/src/database/repositories/rag_chunk.rs` (inline `#[cfg(test)]`)

**Interfaces:**
- Consumes: `register_sqlite_vec_extension()` from Task 2 (must run once at app startup before `DatabaseManager` opens its pool — Step 5 below wires that).
- Produces: `pub struct RagChunk { pub id: i64, pub chunk_uuid: String, pub meeting_id: String, pub chunk_text: String, pub segment_ids: Vec<String>, pub start_time: Option<f64>, pub end_time: Option<f64>, pub token_count: i64, pub embedding_model: String, pub embedding_dims: i64, pub schema_version: i64, pub created_at: chrono::DateTime<chrono::Utc> }`, `pub struct RagChunksRepository`, `pub async fn RagChunksRepository::insert_chunk_with_vector(pool: &SqlitePool, meeting_id: &str, chunk_text: &str, segment_ids: &[String], start_time: Option<f64>, end_time: Option<f64>, token_count: i64, embedding_model: &str, embedding: &[f32]) -> Result<i64, SqlxError>`, `pub async fn RagChunksRepository::delete_chunks_for_meeting(pool: &SqlitePool, meeting_id: &str) -> Result<(), SqlxError>`, `pub async fn RagChunksRepository::count_chunks_for_model(pool: &SqlitePool, embedding_model: &str) -> Result<i64, SqlxError>`.

`rag_chunks.id` is an `INTEGER PRIMARY KEY` (a real SQLite rowid), which is what gets written into `rag_chunk_vectors`'s implicit `rowid` — this is the standard `sqlite-vec` "content table + rowid-joined vec0 table" pattern, and avoids relying on any unverified `sqlite-vec` metadata/partition-key feature.

- [ ] **Step 1: Write the failing test**

  `frontend/src-tauri/src/database/repositories/rag_chunk.rs`:
  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      use crate::database::repositories::test_support::{insert_meeting, setup_pool};
      use crate::rag::vector_store::register_sqlite_vec_extension;

      const DIMS: usize = 4;

      fn fake_embedding(seed: f32) -> Vec<f32> {
          vec![seed, seed + 0.1, seed + 0.2, seed + 0.3]
      }

      #[tokio::test]
      async fn insert_chunk_with_vector_round_trips() {
          register_sqlite_vec_extension();
          let pool = setup_pool().await;
          insert_meeting(&pool, "meeting-1").await;

          let chunk_id = RagChunksRepository::insert_chunk_with_vector(
              &pool,
              "meeting-1",
              "we decided to ship on Friday",
              &["t-1".to_string(), "t-2".to_string()],
              Some(12.5),
              Some(18.0),
              7,
              "bge-small-en-v1.5",
              &fake_embedding(0.1),
          )
          .await
          .expect("insert failed");

          assert!(chunk_id > 0);

          let count = RagChunksRepository::count_chunks_for_model(&pool, "bge-small-en-v1.5")
              .await
              .expect("count failed");
          assert_eq!(count, 1);
      }

      #[tokio::test]
      async fn delete_chunks_for_meeting_removes_both_tables() {
          register_sqlite_vec_extension();
          let pool = setup_pool().await;
          insert_meeting(&pool, "meeting-1").await;

          RagChunksRepository::insert_chunk_with_vector(
              &pool, "meeting-1", "text", &["t-1".to_string()], None, None, 2,
              "bge-small-en-v1.5", &fake_embedding(0.5),
          )
          .await
          .expect("insert failed");

          RagChunksRepository::delete_chunks_for_meeting(&pool, "meeting-1")
              .await
              .expect("delete failed");

          let count = RagChunksRepository::count_chunks_for_model(&pool, "bge-small-en-v1.5")
              .await
              .expect("count failed");
          assert_eq!(count, 0);

          // The vec0 side-table row must be gone too (via the AFTER DELETE
          // trigger), not just the rag_chunks row - a dangling vector would
          // be silently invisible to `count_chunks_for_model` but still
          // returned by Task 7's KNN queries.
          let (orphaned,): (i64,) =
              sqlx::query_as("SELECT COUNT(*) FROM rag_chunk_vectors")
                  .fetch_one(&pool)
                  .await
                  .expect("count query failed");
          assert_eq!(orphaned, 0);
      }
  }
  ```

- [ ] **Step 2: Run test to verify it fails**

  ```bash
  cd frontend/src-tauri && cargo test --lib rag_chunk::tests
  ```
  Expected: compile error — `rag_chunks`/`rag_chunk_vectors` tables and the repository don't exist yet.

- [ ] **Step 3: Write the migration**

  `frontend/src-tauri/migrations/20260811020000_add_rag_chunks.sql`:
  ```sql
  -- Cross-meeting RAG chunk store. `id` is a real SQLite rowid (INTEGER
  -- PRIMARY KEY) so it can be written as the matching rowid into the
  -- sqlite-vec `rag_chunk_vectors` virtual table below - the standard
  -- sqlite-vec "content table + rowid-joined vec0 table" pattern.
  CREATE TABLE IF NOT EXISTS rag_chunks (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      chunk_uuid TEXT NOT NULL UNIQUE,
      meeting_id TEXT NOT NULL,
      chunk_text TEXT NOT NULL,
      segment_ids TEXT NOT NULL,       -- JSON array of transcripts.id
      start_time REAL,
      end_time REAL,
      token_count INTEGER NOT NULL,
      embedding_model TEXT NOT NULL,
      embedding_dims INTEGER NOT NULL,
      schema_version INTEGER NOT NULL DEFAULT 1,
      created_at TEXT NOT NULL,
      FOREIGN KEY (meeting_id) REFERENCES meetings(id) ON DELETE CASCADE
  );

  CREATE INDEX IF NOT EXISTS idx_rag_chunks_meeting_id ON rag_chunks(meeting_id);
  CREATE INDEX IF NOT EXISTS idx_rag_chunks_embedding_model ON rag_chunks(embedding_model);

  -- 384 dims matches bge-small-en-v1.5 (Task 5). If the embedding model ever
  -- changes dimensionality, this table needs a new migration + full
  -- re-embed, not an ALTER - vec0 column width is fixed at creation.
  CREATE VIRTUAL TABLE IF NOT EXISTS rag_chunk_vectors USING vec0(
      embedding float[384]
  );

  -- `meetings(id) ON DELETE CASCADE` only cascades into real tables SQLite
  -- enforces FKs on; a vec0 virtual table isn't one, so deleting a meeting
  -- (or a chunk) needs an explicit trigger to keep rag_chunk_vectors from
  -- accumulating orphaned rows.
  CREATE TRIGGER IF NOT EXISTS rag_chunks_ad AFTER DELETE ON rag_chunks BEGIN
      DELETE FROM rag_chunk_vectors WHERE rowid = old.id;
  END;
  ```

- [ ] **Step 4: Implement the model and repository**

  In `frontend/src-tauri/src/database/models.rs`, after `TranscriptChunk` (line 58-67):
  ```rust
  #[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
  pub struct RagChunk {
      pub id: i64,
      pub chunk_uuid: String,
      pub meeting_id: String,
      pub chunk_text: String,
      /// Stored as a JSON array in SQLite; deserialized by callers via
      /// `serde_json::from_str` on the raw `segment_ids` TEXT column -
      /// `sqlx::FromRow` can't derive JSON columns automatically, so
      /// `RagChunksRepository` methods return this pre-parsed instead of
      /// deriving `FromRow` directly on this struct.
      #[sqlx(skip)]
      pub segment_ids: Vec<String>,
      pub start_time: Option<f64>,
      pub end_time: Option<f64>,
      pub token_count: i64,
      pub embedding_model: String,
      pub embedding_dims: i64,
      pub schema_version: i64,
      pub created_at: chrono::DateTime<chrono::Utc>,
  }
  ```

  `frontend/src-tauri/src/database/repositories/rag_chunk.rs`:
  ```rust
  use chrono::Utc;
  use sqlx::{Error as SqlxError, SqlitePool};
  use uuid::Uuid;

  pub struct RagChunksRepository;

  /// Packs an `f32` embedding into the little-endian byte blob `sqlite-vec`
  /// expects for a `float[N]` vec0 column.
  fn embedding_to_blob(embedding: &[f32]) -> Vec<u8> {
      embedding.iter().flat_map(|f| f.to_le_bytes()).collect()
  }

  impl RagChunksRepository {
      /// Inserts one chunk row plus its matching vector row, in a
      /// transaction so a chunk can never exist without a vector (or vice
      /// versa) - Task 7's retrieval joins the two tables by rowid and
      /// assumes that invariant. Returns the new `rag_chunks.id` (also the
      /// `rag_chunk_vectors` rowid).
      pub async fn insert_chunk_with_vector(
          pool: &SqlitePool,
          meeting_id: &str,
          chunk_text: &str,
          segment_ids: &[String],
          start_time: Option<f64>,
          end_time: Option<f64>,
          token_count: i64,
          embedding_model: &str,
          embedding: &[f32],
      ) -> Result<i64, SqlxError> {
          let mut tx = pool.begin().await?;
          let chunk_uuid = format!("rag-chunk-{}", Uuid::new_v4());
          let segment_ids_json = serde_json::to_string(segment_ids)
              .map_err(|e| SqlxError::Protocol(format!("segment_ids serialize failed: {}", e)))?;
          let now = Utc::now();

          let insert_result = sqlx::query(
              "INSERT INTO rag_chunks \
               (chunk_uuid, meeting_id, chunk_text, segment_ids, start_time, end_time, \
                token_count, embedding_model, embedding_dims, schema_version, created_at) \
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?)",
          )
          .bind(&chunk_uuid)
          .bind(meeting_id)
          .bind(chunk_text)
          .bind(&segment_ids_json)
          .bind(start_time)
          .bind(end_time)
          .bind(token_count)
          .bind(embedding_model)
          .bind(embedding.len() as i64)
          .bind(now)
          .execute(&mut *tx)
          .await?;

          let chunk_id = insert_result.last_insert_rowid();

          sqlx::query("INSERT INTO rag_chunk_vectors(rowid, embedding) VALUES (?, ?)")
              .bind(chunk_id)
              .bind(embedding_to_blob(embedding))
              .execute(&mut *tx)
              .await?;

          tx.commit().await?;
          Ok(chunk_id)
      }

      /// Deletes every chunk (and, via the `rag_chunks_ad` trigger, its
      /// vector) for a meeting - used before re-indexing after a transcript
      /// edit or an embedding-model change (Task 6).
      pub async fn delete_chunks_for_meeting(
          pool: &SqlitePool,
          meeting_id: &str,
      ) -> Result<(), SqlxError> {
          sqlx::query("DELETE FROM rag_chunks WHERE meeting_id = ?")
              .bind(meeting_id)
              .execute(pool)
              .await?;
          Ok(())
      }

      pub async fn count_chunks_for_model(
          pool: &SqlitePool,
          embedding_model: &str,
      ) -> Result<i64, SqlxError> {
          let (count,): (i64,) =
              sqlx::query_as("SELECT COUNT(*) FROM rag_chunks WHERE embedding_model = ?")
                  .bind(embedding_model)
                  .fetch_one(pool)
                  .await?;
          Ok(count)
      }
  }

  #[cfg(test)]
  mod tests {
      // ... (Step 1 content)
  }
  ```

  The `#[cfg(test)] mod tests { ... }` block at the bottom of this file is exactly the test module already written in Step 1 above — nothing further to add there.

  In `frontend/src-tauri/src/database/repositories/mod.rs`, add `pub mod rag_chunk;` next to the existing `pub mod transcript_chunk;` line.

- [ ] **Step 5: Wire extension registration into app startup**

  In `frontend/src-tauri/src/database/setup.rs`, at the top of `initialize_database_on_startup` (line 9, before the `is_first_launch` check), add:
  ```rust
  crate::rag::vector_store::register_sqlite_vec_extension();
  ```
  This must run before `DatabaseManager::new_from_app_handle`/`DatabaseManager::is_first_launch` open the pool, on both the first-launch and normal-launch branches.

- [ ] **Step 6: Run test to verify it passes**

  ```bash
  cd frontend/src-tauri && cargo test --lib rag_chunk::tests
  ```
  Expected: `test result: ok. 2 passed`.

- [ ] **Step 7: Commit**

  ```bash
  git add frontend/src-tauri/migrations/20260811020000_add_rag_chunks.sql frontend/src-tauri/src/database/repositories/rag_chunk.rs frontend/src-tauri/src/database/repositories/mod.rs frontend/src-tauri/src/database/models.rs frontend/src-tauri/src/database/setup.rs
  git commit -m "feat(rag): add rag_chunks table and sqlite-vec-backed vector storage"
  ```

---

### Task 4: Speaker-turn/segment chunker

**Files:**
- Create: `frontend/src-tauri/src/rag/chunking.rs`
- Modify: `frontend/src-tauri/src/rag/mod.rs` (add `pub mod chunking;`)
- Modify: `frontend/src-tauri/src/database/repositories/transcript.rs` (add `get_all_for_meeting_ordered`)
- Test: `frontend/src-tauri/src/rag/chunking.rs` (inline `#[cfg(test)]`, pure/no DB)

**Interfaces:**
- Consumes: `rough_token_count` from [frontend/src-tauri/src/summary/processor.rs:175](../../../frontend/src-tauri/src/summary/processor.rs) (reused, not reimplemented — this codebase's existing ~0.35 tokens/char estimate).
- Produces: `pub struct TranscriptSegmentInput { pub id: String, pub text: String, pub start_time: Option<f64>, pub end_time: Option<f64> }`, `pub struct PendingChunk { pub text: String, pub segment_ids: Vec<String>, pub start_time: Option<f64>, pub end_time: Option<f64>, pub token_count: usize }`, `pub fn chunk_transcript_segments(segments: &[TranscriptSegmentInput], target_tokens: usize, overlap_turns: usize) -> Vec<PendingChunk>`. Task 6 calls this with `target_tokens = 300, overlap_turns = 1` (midpoint of the research doc's 200–400 token guidance).

Per the research doc section 2.2/4.1: "you already have speaker-turn segments with timestamps... chunk on speaker-turn boundaries, grouped to roughly 200–400 tokens, with one turn of overlap, and carry `segment_id` + start/end timestamp as metadata on every chunk" — each row in the existing `transcripts` table already *is* one speaker-turn/VAD segment, so chunking is grouping consecutive rows up to the token budget, not re-deriving turn boundaries.

- [ ] **Step 1: Write the failing test**

  `frontend/src-tauri/src/rag/chunking.rs`:
  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;

      fn segment(id: &str, text: &str, start: f64, end: f64) -> TranscriptSegmentInput {
          TranscriptSegmentInput {
              id: id.to_string(),
              text: text.to_string(),
              start_time: Some(start),
              end_time: Some(end),
          }
      }

      #[test]
      fn groups_short_segments_into_one_chunk() {
          let segments = vec![
              segment("t-1", "let's start the meeting", 0.0, 3.0),
              segment("t-2", "sure, first item is the roadmap", 3.0, 6.0),
          ];
          let chunks = chunk_transcript_segments(&segments, 300, 1);
          assert_eq!(chunks.len(), 1);
          assert_eq!(chunks[0].segment_ids, vec!["t-1", "t-2"]);
          assert_eq!(chunks[0].start_time, Some(0.0));
          assert_eq!(chunks[0].end_time, Some(6.0));
      }

      #[test]
      fn splits_long_transcript_into_multiple_chunks_with_overlap() {
          // Each segment is ~40 tokens (rough_token_count on ~115 chars), so
          // a target of 100 tokens should force a split around segment 3,
          // with segment 3 repeated as the overlap turn at the start of
          // chunk 2.
          let long_text = "word ".repeat(23); // ~115 chars -> ~40 tokens
          let segments: Vec<_> = (0..6)
              .map(|i| segment(&format!("t-{i}"), long_text.trim(), i as f64 * 5.0, i as f64 * 5.0 + 4.0))
              .collect();

          let chunks = chunk_transcript_segments(&segments, 100, 1);

          assert!(chunks.len() >= 2, "expected the transcript to split into multiple chunks");
          let first_chunk_last_id = chunks[0].segment_ids.last().cloned().unwrap();
          let second_chunk_first_id = chunks[1].segment_ids.first().cloned().unwrap();
          assert_eq!(
              first_chunk_last_id, second_chunk_first_id,
              "the last segment of a chunk should be repeated as the first segment of the next (one turn of overlap)"
          );
      }

      #[test]
      fn empty_segments_produce_no_chunks() {
          assert!(chunk_transcript_segments(&[], 300, 1).is_empty());
      }

      #[test]
      fn a_single_oversized_segment_becomes_its_own_chunk() {
          // A pathological single segment far over budget must still be
          // returned whole rather than silently dropped or panicking.
          let huge_text = "word ".repeat(2000);
          let segments = vec![segment("t-1", huge_text.trim(), 0.0, 600.0)];
          let chunks = chunk_transcript_segments(&segments, 300, 1);
          assert_eq!(chunks.len(), 1);
          assert_eq!(chunks[0].segment_ids, vec!["t-1"]);
      }
  }
  ```

- [ ] **Step 2: Run test to verify it fails**

  ```bash
  cd frontend/src-tauri && cargo test --lib chunking::tests
  ```
  Expected: compile error — `chunk_transcript_segments` doesn't exist yet.

- [ ] **Step 3: Implement the chunker**

  ```rust
  use crate::summary::processor::rough_token_count;

  #[derive(Debug, Clone)]
  pub struct TranscriptSegmentInput {
      pub id: String,
      pub text: String,
      pub start_time: Option<f64>,
      pub end_time: Option<f64>,
  }

  #[derive(Debug, Clone)]
  pub struct PendingChunk {
      pub text: String,
      pub segment_ids: Vec<String>,
      pub start_time: Option<f64>,
      pub end_time: Option<f64>,
      pub token_count: usize,
  }

  /// Groups consecutive transcript segments (each already one speaker-turn/
  /// VAD segment - see `transcripts` table) into chunks of roughly
  /// `target_tokens`, repeating the last `overlap_turns` segments of each
  /// chunk as the start of the next so a decision straddling a chunk
  /// boundary isn't cut in half (docs/rag-token-research.md section 2.2). A
  /// single segment that alone exceeds `target_tokens` still becomes its own
  /// chunk whole, rather than being split mid-segment or dropped.
  pub fn chunk_transcript_segments(
      segments: &[TranscriptSegmentInput],
      target_tokens: usize,
      overlap_turns: usize,
  ) -> Vec<PendingChunk> {
      if segments.is_empty() {
          return Vec::new();
      }

      let mut chunks = Vec::new();
      let mut start_idx = 0usize;

      while start_idx < segments.len() {
          let mut end_idx = start_idx;
          let mut token_total = 0usize;

          loop {
              token_total += rough_token_count(&segments[end_idx].text);
              end_idx += 1;
              if end_idx >= segments.len() {
                  break;
              }
              let next_tokens = rough_token_count(&segments[end_idx].text);
              // Stop *before* adding a segment that would push a
              // multi-segment chunk over budget; a lone first segment always
              // goes in whole even if it's already over budget.
              if end_idx > start_idx + 1 && token_total + next_tokens > target_tokens {
                  break;
              }
              if token_total >= target_tokens {
                  break;
              }
          }

          let window = &segments[start_idx..end_idx];
          chunks.push(PendingChunk {
              text: window.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join(" "),
              segment_ids: window.iter().map(|s| s.id.clone()).collect(),
              start_time: window.first().and_then(|s| s.start_time),
              end_time: window.last().and_then(|s| s.end_time),
              token_count: token_total,
          });

          if end_idx >= segments.len() {
              break;
          }
          // Next chunk starts `overlap_turns` segments back into this one.
          start_idx = end_idx.saturating_sub(overlap_turns).max(start_idx + 1);
      }

      chunks
  }
  ```

  In `frontend/src-tauri/src/rag/mod.rs`:
  ```rust
  pub mod chunking;
  pub mod vector_store;
  ```

- [ ] **Step 4: Add the ordered full-transcript fetch Task 6 will need**

  In `frontend/src-tauri/src/database/repositories/transcript.rs`, after `search_transcripts_fts` (Task 1): existing fetch helpers are either character-budget-bounded (`MeetingsRepository::get_recent_transcript_text`) or paginated (`MeetingsRepository::get_meeting_transcripts_paginated`) — neither returns the full, structured, chronologically ordered segment list the chunker needs, so this is a genuinely new query, not a duplicate of either:
  ```rust
  /// Every transcript segment for a meeting, oldest first - the input shape
  /// `rag::chunking::chunk_transcript_segments` needs. Distinct from
  /// `MeetingsRepository::get_recent_transcript_text` (character-budget
  /// bounded, returns one joined string) and
  /// `MeetingsRepository::get_meeting_transcripts_paginated` (paginated) -
  /// indexing needs the whole meeting, unbounded, as structured rows.
  pub async fn get_all_for_meeting_ordered(
      pool: &SqlitePool,
      meeting_id: &str,
  ) -> Result<Vec<crate::database::models::Transcript>, SqlxError> {
      sqlx::query_as::<_, crate::database::models::Transcript>(
          "SELECT id, meeting_id, transcript, timestamp, summary, action_items, \
                  key_points, audio_start_time, audio_end_time, duration \
           FROM transcripts \
           WHERE meeting_id = ? \
           ORDER BY audio_start_time ASC, id ASC",
      )
      .bind(meeting_id)
      .fetch_all(pool)
      .await
  }
  ```

- [ ] **Step 5: Run test to verify it passes**

  ```bash
  cd frontend/src-tauri && cargo test --lib chunking::tests
  ```
  Expected: `test result: ok. 4 passed`.

- [ ] **Step 6: Commit**

  ```bash
  git add frontend/src-tauri/src/rag/chunking.rs frontend/src-tauri/src/rag/mod.rs frontend/src-tauri/src/database/repositories/transcript.rs
  git commit -m "feat(rag): chunk transcripts on speaker-turn boundaries with overlap"
  ```

---

### Task 5: Local embedding service (`fastembed-rs`)

**Files:**
- Modify: `frontend/src-tauri/Cargo.toml` (add `fastembed`)
- Create: `frontend/src-tauri/src/rag/embedding.rs`
- Modify: `frontend/src-tauri/src/rag/mod.rs` (add `pub mod embedding;`)
- Test: `frontend/src-tauri/src/rag/embedding.rs` (inline `#[cfg(test)]`, downloads the model once on CI/dev — mark `#[ignore]` if the environment has no network, per Step 4)

**Interfaces:**
- Consumes: nothing from earlier tasks except the module layout.
- Produces: `pub const EMBEDDING_MODEL_ID: &str = "bge-small-en-v1.5"; pub const EMBEDDING_DIMS: usize = 384;`, `pub struct EmbeddingService`, `pub fn EmbeddingService::new(cache_dir: &std::path::Path) -> anyhow::Result<Self>`, `pub fn EmbeddingService::embed_documents(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>>`, `pub fn EmbeddingService::embed_query(&self, text: &str) -> anyhow::Result<Vec<f32>>`. Task 6 constructs one `EmbeddingService` and reuses it for the life of the indexing task; Task 7 constructs one per retrieval call (or shares Task 6's — see Task 7 Step 3).

Per the research doc section 4.3/7: fastembed-rs's exact Tokio-dependency and default-model/batch-size behavior scored only 1–2/3 in verification ("re-verify the Tokio question on docs.rs before you code against it"). Step 1 below is that verification, done as an explicit step rather than assumed.

- [ ] **Step 1: Verify the fastembed-rs API before coding against it**

  Before writing `embedding.rs`, check https://docs.rs/fastembed (pinned to the version added in Step 2) for: the exact `TextEmbedding`/`InitOptions` construction API, whether any `TextEmbedding` method requires a Tokio runtime to be active (the research doc's unresolved question — if it does, `embed_documents`/`embed_query` below must be called via `tokio::task::spawn_blocking` from async callers rather than called directly), and the correct `EmbeddingModel` enum variant name for `bge-small-en-v1.5`. Adjust the signatures below if the verified API differs from what's assumed here.

- [ ] **Step 2: Add the dependency**

  In `frontend/src-tauri/Cargo.toml`, after the `sqlite-vec`/`libsqlite3-sys` lines added in Task 2:
  ```toml
  # Local, offline embedding generation (ONNX via ort) + local cross-encoder
  # reranking (Task 11) - one dependency covers both RAG stages, entirely
  # in-process, no network at query time (docs/rag-token-research.md 4.3).
  fastembed = "5"
  ```

- [ ] **Step 3: Write the failing test**

  `frontend/src-tauri/src/rag/embedding.rs`:
  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      use tempfile::TempDir;

      // Downloads ~130MB of ONNX weights on first run - not something CI
      // without network access can run unattended. Marked #[ignore]; run
      // explicitly with `cargo test --lib embedding::tests -- --ignored`
      // when network access is available, and unconditionally before
      // shipping Task 5.
      #[test]
      #[ignore]
      fn embed_query_and_documents_produce_expected_dims() {
          let cache_dir = TempDir::new().expect("tempdir failed");
          let service = EmbeddingService::new(cache_dir.path()).expect("model load failed");

          let query_vec = service.embed_query("what did we decide about the launch date")
              .expect("embed_query failed");
          assert_eq!(query_vec.len(), EMBEDDING_DIMS);

          let doc_vecs = service
              .embed_documents(&["we moved the launch to next quarter".to_string()])
              .expect("embed_documents failed");
          assert_eq!(doc_vecs.len(), 1);
          assert_eq!(doc_vecs[0].len(), EMBEDDING_DIMS);
      }

      #[test]
      #[ignore]
      fn semantically_similar_text_has_higher_cosine_similarity() {
          let cache_dir = TempDir::new().expect("tempdir failed");
          let service = EmbeddingService::new(cache_dir.path()).expect("model load failed");

          let query = service.embed_query("when is the product launching").unwrap();
          let docs = service
              .embed_documents(&[
                  "we're launching the product next quarter".to_string(),
                  "the office coffee machine is broken again".to_string(),
              ])
              .unwrap();

          let sim = |a: &[f32], b: &[f32]| -> f32 {
              let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
              let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
              let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
              dot / (norm_a * norm_b)
          };

          assert!(sim(&query, &docs[0]) > sim(&query, &docs[1]));
      }
  }
  ```

- [ ] **Step 2: Run test to verify it fails**

  ```bash
  cd frontend/src-tauri && cargo test --lib embedding::tests -- --ignored
  ```
  Expected: compile error — `EmbeddingService`/`EMBEDDING_DIMS` don't exist yet.

- [ ] **Step 3: Implement `EmbeddingService`**

  ```rust
  use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
  use std::path::Path;

  pub const EMBEDDING_MODEL_ID: &str = "bge-small-en-v1.5";
  pub const EMBEDDING_DIMS: usize = 384;

  /// Wraps fastembed-rs's `TextEmbedding` (ONNX via `ort`, CPU/local) so the
  /// rest of the RAG pipeline never touches the fastembed API directly.
  /// Models are cached in `cache_dir` after the first load - mirrors the
  /// existing Whisper model storage convention (per CLAUDE.md: dev under
  /// `frontend/models/`, prod under the platform app-data models dir) so
  /// embedding models live alongside the app's other local models rather
  /// than a new ad-hoc cache location.
  pub struct EmbeddingService {
      model: TextEmbedding,
  }

  impl EmbeddingService {
      pub fn new(cache_dir: &Path) -> anyhow::Result<Self> {
          let model = TextEmbedding::try_new(
              InitOptions::new(EmbeddingModel::BGESmallENV15)
                  .with_cache_dir(cache_dir.to_path_buf())
                  .with_show_download_progress(true),
          )?;
          Ok(Self { model })
      }

      /// Embeds passages being indexed (chunk text). Uses the
      /// `search_document:` task prefix internally if the underlying model
      /// requires the asymmetric query/document prefix convention (verify
      /// during Step 1 whether fastembed's BGE preset already applies this,
      /// since the research doc flags the prefix as "mandatory... omit them
      /// and quality drops with no error" for nomic-embed-text specifically).
      pub fn embed_documents(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
          Ok(self.model.embed(texts.to_vec(), None)?)
      }

      /// Embeds a user question at query time - kept as a separate method
      /// from `embed_documents` even though both currently call the same
      /// underlying `embed()`, so the query/document asymmetry noted above
      /// has exactly one call site to fix if the verified model needs it.
      pub fn embed_query(&self, text: &str) -> anyhow::Result<Vec<f32>> {
          let mut result = self.model.embed(vec![text.to_string()], None)?;
          result.pop().ok_or_else(|| anyhow::anyhow!("embed_query returned no vector"))
      }
  }
  ```

  In `frontend/src-tauri/src/rag/mod.rs`, add `pub mod embedding;`.

- [ ] **Step 4: Run test to verify it passes**

  ```bash
  cd frontend/src-tauri && cargo test --lib embedding::tests -- --ignored
  ```
  Expected: `test result: ok. 2 passed` (requires network for the first run to download the ONNX weights into the temp cache dir).

- [ ] **Step 5: Commit**

  ```bash
  git add frontend/src-tauri/Cargo.toml frontend/src-tauri/Cargo.lock frontend/src-tauri/src/rag/embedding.rs frontend/src-tauri/src/rag/mod.rs
  git commit -m "feat(rag): add local embedding service via fastembed-rs"
  ```

---

### Task 6: Background indexing pipeline

**Files:**
- Create: `frontend/src-tauri/src/rag/indexer.rs`
- Modify: `frontend/src-tauri/src/rag/mod.rs` (add `pub mod indexer;`)
- Modify: `frontend/src-tauri/src/summary/service.rs:294,565-583` (trigger indexing after a summary completes)
- Create: `frontend/src-tauri/src/rag/commands.rs` (manual `reindex_meeting`/`reindex_all_meetings` Tauri commands, for model upgrades)
- Modify: `frontend/src-tauri/src/lib.rs:679-681` (register the new commands)
- Test: `frontend/src-tauri/src/rag/indexer.rs` (inline `#[cfg(test)]`)

**Interfaces:**
- Consumes: `TranscriptsRepository::get_all_for_meeting_ordered` (Task 4), `chunk_transcript_segments` (Task 4), `EmbeddingService` (Task 5), `RagChunksRepository::insert_chunk_with_vector`/`delete_chunks_for_meeting` (Task 3).
- Produces: `pub async fn index_meeting(pool: &SqlitePool, embedder: &EmbeddingService, meeting_id: &str) -> anyhow::Result<usize>` (returns chunk count), `#[tauri::command] pub async fn reindex_meeting(...) -> Result<usize, String>`, `#[tauri::command] pub async fn reindex_all_meetings(...) -> Result<usize, String>`. Task 7 depends only on `rag_chunks`/`rag_chunk_vectors` being populated by this task, not on its internals.

Indexing must not run on the UI thread or block the summary-completion path — mirrors the existing `tauri::async_runtime::spawn` pattern already used for the first-launch event in [frontend/src-tauri/src/database/setup.rs:20](../../../frontend/src-tauri/src/database/setup.rs).

- [ ] **Step 1: Write the failing test**

  `frontend/src-tauri/src/rag/indexer.rs`:
  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      use crate::database::repositories::test_support::{insert_meeting, setup_pool};
      use crate::rag::embedding::EmbeddingService;
      use tempfile::TempDir;

      async fn insert_transcript(pool: &sqlx::SqlitePool, id: &str, meeting_id: &str, text: &str, start: f64) {
          sqlx::query(
              "INSERT INTO transcripts (id, meeting_id, transcript, timestamp, audio_start_time, audio_end_time) \
               VALUES (?, ?, ?, ?, ?, ?)",
          )
          .bind(id).bind(meeting_id).bind(text).bind("00:00:00").bind(start).bind(start + 4.0)
          .execute(pool).await.expect("insert failed");
      }

      // Requires network on first run to download the embedding model - see
      // the same #[ignore] rationale as Task 5's tests.
      #[tokio::test]
      #[ignore]
      async fn index_meeting_creates_chunks_and_vectors() {
          crate::rag::vector_store::register_sqlite_vec_extension();
          let pool = setup_pool().await;
          insert_meeting(&pool, "meeting-1").await;
          insert_transcript(&pool, "t-1", "meeting-1", "let's start the meeting", 0.0).await;
          insert_transcript(&pool, "t-2", "meeting-1", "first item is the Q3 roadmap", 4.0).await;

          let cache_dir = TempDir::new().unwrap();
          let embedder = EmbeddingService::new(cache_dir.path()).unwrap();

          let chunk_count = index_meeting(&pool, &embedder, "meeting-1").await.unwrap();
          assert_eq!(chunk_count, 1);

          let stored = crate::database::repositories::rag_chunk::RagChunksRepository::count_chunks_for_model(
              &pool, crate::rag::embedding::EMBEDDING_MODEL_ID,
          ).await.unwrap();
          assert_eq!(stored, 1);
      }

      #[tokio::test]
      #[ignore]
      async fn reindexing_a_meeting_replaces_old_chunks() {
          crate::rag::vector_store::register_sqlite_vec_extension();
          let pool = setup_pool().await;
          insert_meeting(&pool, "meeting-1").await;
          insert_transcript(&pool, "t-1", "meeting-1", "original text", 0.0).await;

          let cache_dir = TempDir::new().unwrap();
          let embedder = EmbeddingService::new(cache_dir.path()).unwrap();

          index_meeting(&pool, &embedder, "meeting-1").await.unwrap();
          index_meeting(&pool, &embedder, "meeting-1").await.unwrap();

          let stored = crate::database::repositories::rag_chunk::RagChunksRepository::count_chunks_for_model(
              &pool, crate::rag::embedding::EMBEDDING_MODEL_ID,
          ).await.unwrap();
          // Re-indexing must delete-then-reinsert, not accumulate duplicates.
          assert_eq!(stored, 1);
      }
  }
  ```

- [ ] **Step 2: Run test to verify it fails**

  ```bash
  cd frontend/src-tauri && cargo test --lib indexer::tests -- --ignored
  ```
  Expected: compile error — `index_meeting` doesn't exist yet.

- [ ] **Step 3: Implement `index_meeting`**

  ```rust
  use crate::database::repositories::rag_chunk::RagChunksRepository;
  use crate::database::repositories::transcript::TranscriptsRepository;
  use crate::rag::chunking::{chunk_transcript_segments, TranscriptSegmentInput};
  use crate::rag::embedding::{EmbeddingService, EMBEDDING_MODEL_ID};
  use sqlx::SqlitePool;

  const TARGET_CHUNK_TOKENS: usize = 300;
  const OVERLAP_TURNS: usize = 1;

  /// Re-chunks and re-embeds one meeting's whole transcript, replacing any
  /// chunks it already had (so this is safe to call again after a
  /// transcript edit, or in bulk from `reindex_all_meetings` after an
  /// embedding-model change). Returns the number of chunks created.
  pub async fn index_meeting(
      pool: &SqlitePool,
      embedder: &EmbeddingService,
      meeting_id: &str,
  ) -> anyhow::Result<usize> {
      let segments = TranscriptsRepository::get_all_for_meeting_ordered(pool, meeting_id).await?;
      if segments.is_empty() {
          return Ok(0);
      }

      let inputs: Vec<TranscriptSegmentInput> = segments
          .into_iter()
          .map(|s| TranscriptSegmentInput {
              id: s.id,
              text: s.transcript,
              start_time: s.audio_start_time,
              end_time: s.audio_end_time,
          })
          .collect();

      let pending_chunks = chunk_transcript_segments(&inputs, TARGET_CHUNK_TOKENS, OVERLAP_TURNS);
      if pending_chunks.is_empty() {
          return Ok(0);
      }

      let texts: Vec<String> = pending_chunks.iter().map(|c| c.text.clone()).collect();
      let embeddings = embedder.embed_documents(&texts)?;
      anyhow::ensure!(
          embeddings.len() == pending_chunks.len(),
          "embedder returned {} vectors for {} chunks",
          embeddings.len(),
          pending_chunks.len()
      );

      // Delete-then-reinsert rather than diffing: transcripts are edited
      // rarely and whole-meeting re-embedding is cheap at this scale, so
      // there's no reuse-vs-simplicity tradeoff worth a diffing algorithm
      // here.
      RagChunksRepository::delete_chunks_for_meeting(pool, meeting_id).await?;

      for (chunk, embedding) in pending_chunks.iter().zip(embeddings.iter()) {
          RagChunksRepository::insert_chunk_with_vector(
              pool,
              meeting_id,
              &chunk.text,
              &chunk.segment_ids,
              chunk.start_time,
              chunk.end_time,
              chunk.token_count as i64,
              EMBEDDING_MODEL_ID,
              embedding,
          )
          .await?;
      }

      Ok(pending_chunks.len())
  }
  ```

  In `frontend/src-tauri/src/rag/mod.rs`, add `pub mod indexer;`.

- [ ] **Step 4: Add the manual reindex commands**

  `frontend/src-tauri/src/rag/commands.rs`:
  ```rust
  use crate::rag::embedding::EmbeddingService;
  use crate::rag::indexer::index_meeting;
  use crate::state::AppState;
  use tauri::{AppHandle, Manager, Runtime};

  fn embedding_cache_dir<R: Runtime>(app: &AppHandle<R>) -> Result<std::path::PathBuf, String> {
      let mut dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
      dir.push("models");
      dir.push("embeddings");
      Ok(dir)
  }

  /// Re-chunks and re-embeds a single meeting on demand - exposed for
  /// manual "fix my index" recovery from Settings; normal indexing happens
  /// automatically after summary generation (see summary::service).
  #[tauri::command]
  pub async fn reindex_meeting<R: Runtime>(
      app: AppHandle<R>,
      state: tauri::State<'_, AppState>,
      meeting_id: String,
  ) -> Result<usize, String> {
      let cache_dir = embedding_cache_dir(&app)?;
      let embedder = EmbeddingService::new(&cache_dir).map_err(|e| e.to_string())?;
      index_meeting(state.db_manager.pool(), &embedder, &meeting_id)
          .await
          .map_err(|e| e.to_string())
  }

  /// Re-indexes every meeting - the migration path for an embedding-model
  /// upgrade (docs/rag-token-research.md: "you *will* change embedding
  /// models, and re-embedding is a migration"). Sequential, not parallel:
  /// this reuses the same single `EmbeddingService`/ONNX session rather than
  /// spinning up one per meeting, and re-indexing is a rare, user-initiated
  /// action rather than a hot path worth parallelizing.
  #[tauri::command]
  pub async fn reindex_all_meetings<R: Runtime>(
      app: AppHandle<R>,
      state: tauri::State<'_, AppState>,
  ) -> Result<usize, String> {
      let cache_dir = embedding_cache_dir(&app)?;
      let embedder = EmbeddingService::new(&cache_dir).map_err(|e| e.to_string())?;
      let pool = state.db_manager.pool();

      let meetings = crate::database::repositories::meeting::MeetingsRepository::get_meetings(pool)
          .await
          .map_err(|e| e.to_string())?;

      let mut total = 0usize;
      for meeting in meetings {
          total += index_meeting(pool, &embedder, &meeting.id)
              .await
              .map_err(|e| e.to_string())?;
      }
      Ok(total)
  }
  ```

  In `frontend/src-tauri/src/rag/mod.rs`, add `pub mod commands;`.

  In `frontend/src-tauri/src/lib.rs`, in the `tauri::generate_handler![...]` list (currently lines 679-681, right after `summary::commands::ask_about_live_transcript,`):
  ```rust
              summary::commands::ask_about_live_transcript,
              rag::commands::reindex_meeting,
              rag::commands::reindex_all_meetings,
  ```

- [ ] **Step 5: Trigger indexing after a summary completes**

  In `frontend/src-tauri/src/summary/service.rs`, rename the unused `_app` parameter to `app` in `process_transcript_background`'s signature (currently line 294-295), then in the success branch after `info!("Summary saved successfully for meeting_id: {}", meeting_id);` (currently line 578-582):
  ```rust
              } else {
                  info!(
                      "Summary saved successfully for meeting_id: {}",
                      meeting_id
                  );

                  // Index for cross-meeting RAG search in the background -
                  // never block the summary-completion path on embedding
                  // generation. A failure here is logged, not surfaced: the
                  // meeting is still fully usable without a RAG index, it
                  // just won't show up in ask_across_meetings results until
                  // the next successful (re)index.
                  let app_for_index = app.clone();
                  let meeting_id_for_index = meeting_id.clone();
                  tauri::async_runtime::spawn(async move {
                      let state = app_for_index.state::<crate::state::AppState>();
                      let cache_dir = match app_for_index.path().app_data_dir() {
                          Ok(mut dir) => {
                              dir.push("models");
                              dir.push("embeddings");
                              dir
                          }
                          Err(e) => {
                              error!("rag indexing: failed to resolve app data dir: {}", e);
                              return;
                          }
                      };
                      match crate::rag::embedding::EmbeddingService::new(&cache_dir) {
                          Ok(embedder) => {
                              match crate::rag::indexer::index_meeting(
                                  state.db_manager.pool(),
                                  &embedder,
                                  &meeting_id_for_index,
                              )
                              .await
                              {
                                  Ok(count) => info!(
                                      "rag indexing: {} chunks indexed for meeting_id: {}",
                                      count, meeting_id_for_index
                                  ),
                                  Err(e) => error!(
                                      "rag indexing: failed for meeting_id {}: {}",
                                      meeting_id_for_index, e
                                  ),
                              }
                          }
                          Err(e) => error!("rag indexing: failed to load embedding model: {}", e),
                      }
                  });
              }
  ```
  Note: check every other call site of `process_transcript_background` after this rename compiles cleanly against the now-used `app` parameter (the leading underscore was presumably there because it used to be unused).

- [ ] **Step 6: Run test to verify it passes**

  ```bash
  cd frontend/src-tauri && cargo test --lib indexer::tests -- --ignored
  cd frontend/src-tauri && cargo check
  ```
  Expected: `test result: ok. 2 passed`, and `cargo check` succeeds with the `service.rs` changes.

- [ ] **Step 7: Commit**

  ```bash
  git add frontend/src-tauri/src/rag/indexer.rs frontend/src-tauri/src/rag/commands.rs frontend/src-tauri/src/rag/mod.rs frontend/src-tauri/src/summary/service.rs frontend/src-tauri/src/lib.rs
  git commit -m "feat(rag): background-index meetings for cross-meeting search after summary completion"
  ```

---

### Task 7: Hybrid retrieval — FTS5 + vector, fused with RRF

**Files:**
- Create: `frontend/src-tauri/src/rag/retrieval.rs`
- Modify: `frontend/src-tauri/src/rag/mod.rs` (add `pub mod retrieval;`)
- Test: `frontend/src-tauri/src/rag/retrieval.rs` (inline `#[cfg(test)]`)

**Interfaces:**
- Consumes: `TranscriptsRepository::search_transcripts_fts`/`sanitize_fts_query` (Task 1), `rag_chunks`/`rag_chunk_vectors` (Task 3), `EmbeddingService::embed_query` (Task 5).
- Produces: `pub struct RetrievedChunk { pub chunk_id: i64, pub meeting_id: String, pub meeting_title: String, pub meeting_created_at: chrono::DateTime<chrono::Utc>, pub chunk_text: String, pub start_time: Option<f64>, pub end_time: Option<f64> }`, `pub async fn retrieve_relevant_chunks(pool: &SqlitePool, embedder: &EmbeddingService, query: &str, top_k: usize) -> anyhow::Result<Vec<RetrievedChunk>>`, `pub fn reciprocal_rank_fusion(ranked_lists: &[Vec<i64>], k: f64) -> Vec<(i64, f64)>` (pure, unit-testable alone). Task 9 calls `retrieve_relevant_chunks` directly in place of `build_cross_meeting_context`.

This directly replaces the research doc's flagged broken behavior: "`build_cross_meeting_context` packs meeting summaries until it hits 100,000 chars, then drops whole meetings" (section 0). Per section 2.4, fusion uses RRF (rank-only, `Σ 1/(k + rank)`, k≈60) specifically because it needs no score normalization between BM25 and cosine similarity — do not blend raw scores.

- [ ] **Step 1: Write the failing test for RRF (pure function, no DB)**

  `frontend/src-tauri/src/rag/retrieval.rs`:
  ```rust
  #[cfg(test)]
  mod rrf_tests {
      use super::*;

      #[test]
      fn item_ranked_first_in_both_lists_wins() {
          let keyword_ranked = vec![1, 2, 3];
          let vector_ranked = vec![1, 3, 2];
          let fused = reciprocal_rank_fusion(&[keyword_ranked, vector_ranked], 60.0);
          assert_eq!(fused[0].0, 1);
      }

      #[test]
      fn item_only_in_one_list_still_scores() {
          let keyword_ranked = vec![1, 2];
          let vector_ranked: Vec<i64> = vec![]; // e.g. no vector index yet
          let fused = reciprocal_rank_fusion(&[keyword_ranked, vector_ranked], 60.0);
          assert_eq!(fused.len(), 2);
          assert_eq!(fused[0].0, 1);
      }

      #[test]
      fn agreement_across_lists_outranks_a_single_first_place() {
          // Ranked 2nd in both lists should beat ranked 1st in only one -
          // this is RRF's whole point over picking either list alone.
          let keyword_ranked = vec![10, 20];
          let vector_ranked = vec![30, 20];
          let fused = reciprocal_rank_fusion(&[keyword_ranked, vector_ranked], 60.0);
          assert_eq!(fused[0].0, 20);
      }
  }
  ```

- [ ] **Step 2: Run test to verify it fails**

  ```bash
  cd frontend/src-tauri && cargo test --lib retrieval::rrf_tests
  ```
  Expected: compile error — `reciprocal_rank_fusion` doesn't exist yet.

- [ ] **Step 3: Implement RRF and the hybrid retrieval query**

  ```rust
  use crate::database::repositories::transcript::TranscriptsRepository;
  use crate::rag::embedding::EmbeddingService;
  use chrono::{DateTime, Utc};
  use sqlx::SqlitePool;
  use std::collections::HashMap;

  /// Fuses N independently-ranked lists of the same kind of id (here, always
  /// `rag_chunks.id`) using Reciprocal Rank Fusion: each id scores
  /// `Σ 1/(k + rank)` across every list it appears in (1-indexed rank; an id
  /// absent from a list simply contributes 0 from that list). Uses only
  /// ranks, never the lists' underlying scores, so a BM25 rank and a cosine-
  /// similarity rank fuse without normalizing one against the other
  /// (docs/rag-token-research.md section 2.4). Returns ids sorted by fused
  /// score, descending.
  pub fn reciprocal_rank_fusion(ranked_lists: &[Vec<i64>], k: f64) -> Vec<(i64, f64)> {
      let mut scores: HashMap<i64, f64> = HashMap::new();
      for list in ranked_lists {
          for (idx, id) in list.iter().enumerate() {
              let rank = (idx + 1) as f64;
              *scores.entry(*id).or_insert(0.0) += 1.0 / (k + rank);
          }
      }
      let mut fused: Vec<(i64, f64)> = scores.into_iter().collect();
      fused.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
      fused
  }

  const RRF_K: f64 = 60.0;
  /// How many candidates each retriever contributes before fusion - kept
  /// generous relative to `top_k` since RRF needs the full ranked lists, not
  /// just the eventual top results, to fuse correctly.
  const CANDIDATES_PER_RETRIEVER: i64 = 50;

  #[derive(Debug, Clone)]
  pub struct RetrievedChunk {
      pub chunk_id: i64,
      pub meeting_id: String,
      pub meeting_title: String,
      pub meeting_created_at: DateTime<Utc>,
      pub chunk_text: String,
      pub start_time: Option<f64>,
      pub end_time: Option<f64>,
  }

  fn embedding_to_blob(embedding: &[f32]) -> Vec<u8> {
      embedding.iter().flat_map(|f| f.to_le_bytes()).collect()
  }

  /// Retrieves the `top_k` chunks most relevant to `query` across every
  /// indexed meeting: keyword search (Task 1's `transcripts_fts`, mapped
  /// from transcript segment ids to their owning `rag_chunks` rows) and
  /// vector KNN (`rag_chunk_vectors`, brute-force - verified fast enough at
  /// this corpus size per docs/rag-token-research.md section 4.2) each
  /// contribute a ranked candidate list, fused with RRF.
  pub async fn retrieve_relevant_chunks(
      pool: &SqlitePool,
      embedder: &EmbeddingService,
      query: &str,
      top_k: usize,
  ) -> anyhow::Result<Vec<RetrievedChunk>> {
      // --- Keyword candidates: FTS5 hit -> owning chunk id(s) ---
      let fts_hits = TranscriptsRepository::search_transcripts_fts(pool, query, CANDIDATES_PER_RETRIEVER).await?;
      let mut keyword_ranked: Vec<i64> = Vec::new();
      for hit in &fts_hits {
          let chunk_ids: Vec<(i64,)> = sqlx::query_as(
              "SELECT id FROM rag_chunks \
               WHERE meeting_id = ? AND segment_ids LIKE '%' || ? || '%'",
          )
          .bind(&hit.meeting_id)
          .bind(&hit.transcript_id)
          .fetch_all(pool)
          .await?;
          for (id,) in chunk_ids {
              if !keyword_ranked.contains(&id) {
                  keyword_ranked.push(id);
              }
          }
      }

      // --- Vector candidates: query embedding -> KNN over rag_chunk_vectors ---
      let query_embedding = embedder.embed_query(query)?;
      let vector_hits: Vec<(i64,)> = sqlx::query_as(
          "SELECT rowid FROM rag_chunk_vectors WHERE embedding MATCH ? AND k = ? ORDER BY distance",
      )
      .bind(embedding_to_blob(&query_embedding))
      .bind(CANDIDATES_PER_RETRIEVER)
      .fetch_all(pool)
      .await?;
      let vector_ranked: Vec<i64> = vector_hits.into_iter().map(|(id,)| id).collect();

      let fused = reciprocal_rank_fusion(&[keyword_ranked, vector_ranked], RRF_K);

      // --- Hydrate the top-k fused ids with chunk + meeting metadata ---
      let mut results = Vec::with_capacity(top_k);
      for (chunk_id, _score) in fused.into_iter().take(top_k) {
          let row: Option<(String, String, String, DateTime<Utc>, Option<f64>, Option<f64>)> = sqlx::query_as(
              "SELECT rc.chunk_text, rc.meeting_id, m.title, m.created_at, rc.start_time, rc.end_time \
               FROM rag_chunks rc JOIN meetings m ON m.id = rc.meeting_id \
               WHERE rc.id = ?",
          )
          .bind(chunk_id)
          .fetch_optional(pool)
          .await?
          .map(|(text, meeting_id, title, created_at, start, end)| {
              (text, meeting_id, title, created_at, start, end)
          });

          if let Some((chunk_text, meeting_id, meeting_title, meeting_created_at, start_time, end_time)) = row {
              results.push(RetrievedChunk {
                  chunk_id,
                  meeting_id,
                  meeting_title,
                  meeting_created_at,
                  chunk_text,
                  start_time,
                  end_time,
              });
          }
      }
      Ok(results)
  }
  ```

  In `frontend/src-tauri/src/rag/mod.rs`, add `pub mod retrieval;`.

- [ ] **Step 4: Run RRF test to verify it passes**

  ```bash
  cd frontend/src-tauri && cargo test --lib retrieval::rrf_tests
  ```
  Expected: `test result: ok. 3 passed`.

- [ ] **Step 5: Write and run an end-to-end retrieval test**

  Add to `frontend/src-tauri/src/rag/retrieval.rs`:
  ```rust
  #[cfg(test)]
  mod retrieval_tests {
      use super::*;
      use crate::database::repositories::test_support::{insert_meeting, setup_pool};
      use crate::rag::indexer::index_meeting;
      use tempfile::TempDir;

      #[tokio::test]
      #[ignore] // needs network to download the embedding model on first run
      async fn retrieves_chunks_relevant_to_the_query_across_meetings() {
          crate::rag::vector_store::register_sqlite_vec_extension();
          let pool = setup_pool().await;
          insert_meeting(&pool, "meeting-1").await;
          insert_meeting(&pool, "meeting-2").await;

          sqlx::query("INSERT INTO transcripts (id, meeting_id, transcript, timestamp, audio_start_time) VALUES (?, ?, ?, ?, ?)")
              .bind("t-1").bind("meeting-1").bind("we decided to launch the redesign in October").bind("00:00:00").bind(0.0)
              .execute(&pool).await.unwrap();
          sqlx::query("INSERT INTO transcripts (id, meeting_id, transcript, timestamp, audio_start_time) VALUES (?, ?, ?, ?, ?)")
              .bind("t-2").bind("meeting-2").bind("the quarterly budget review went fine").bind("00:00:00").bind(0.0)
              .execute(&pool).await.unwrap();

          let cache_dir = TempDir::new().unwrap();
          let embedder = crate::rag::embedding::EmbeddingService::new(cache_dir.path()).unwrap();
          index_meeting(&pool, &embedder, "meeting-1").await.unwrap();
          index_meeting(&pool, &embedder, "meeting-2").await.unwrap();

          let results = retrieve_relevant_chunks(&pool, &embedder, "when is the redesign launching", 5)
              .await
              .unwrap();

          assert!(!results.is_empty());
          assert_eq!(results[0].meeting_id, "meeting-1");
      }
  }
  ```

  ```bash
  cd frontend/src-tauri && cargo test --lib retrieval::retrieval_tests -- --ignored
  ```
  Expected: `test result: ok. 1 passed`.

- [ ] **Step 6: Commit**

  ```bash
  git add frontend/src-tauri/src/rag/retrieval.rs frontend/src-tauri/src/rag/mod.rs
  git commit -m "feat(rag): hybrid FTS5 + vector retrieval fused with RRF"
  ```

---

### Task 8: Contextual query rewriting for cross-meeting follow-ups

**Files:**
- Modify: `frontend/src-tauri/src/summary/commands.rs` (add near `ASK_ACROSS_MEETINGS_SYSTEM_PROMPT`, currently line 501-504)
- Test: `frontend/src-tauri/src/summary/commands.rs` (inline `#[cfg(test)]`, alongside the existing `ask_ai_tests` module at line 1141)

**Interfaces:**
- Consumes: `ask_configured_llm` (existing, [frontend/src-tauri/src/summary/commands.rs:788-892](../../../frontend/src-tauri/src/summary/commands.rs)).
- Produces: `pub struct AskHistoryTurn { pub question: String, pub answer: String }`, `async fn rewrite_followup_query<R: Runtime>(app: &AppHandle<R>, history: &[AskHistoryTurn], question: &str) -> String` (never fails outward — falls back to the original question on any LLM error, since a broken rewrite must never block the ask). Task 9 calls this before retrieval.

Per the research doc section 4.5 (its most directly applicable, highest-confidence finding): retrieval recall halves after the first question in a conversation (0.89 → 0.47, MTRAG/TACL 2025) specifically because `useAskAI` already threads conversation history (`AskTurn[]`) and this UI is "designed for follow-ups... your dominant failure mode, not an edge case." The mitigation is a **decontextualizing** rewrite (turn the follow-up into a standalone question), not a keyword-list/HyDE-style rewrite — section 4.5 explicitly flags the latter as measured *harmful* in a separate source.

- [ ] **Step 1: Write the failing test**

  In `frontend/src-tauri/src/summary/commands.rs`, inside (or alongside) `mod ask_ai_tests` (line 1141):
  ```rust
  #[test]
  fn build_rewrite_user_prompt_includes_history_and_question() {
      let history = vec![
          AskHistoryTurn { question: "Tell me about the API redesign".to_string(), answer: "The team discussed moving to GraphQL.".to_string() },
      ];
      let prompt = build_rewrite_user_prompt(&history, "What did they decide?");
      assert!(prompt.contains("Tell me about the API redesign"));
      assert!(prompt.contains("GraphQL"));
      assert!(prompt.contains("What did they decide?"));
  }

  #[test]
  fn build_rewrite_user_prompt_with_no_history_still_includes_question() {
      let prompt = build_rewrite_user_prompt(&[], "What was decided?");
      assert!(prompt.contains("What was decided?"));
  }
  ```

- [ ] **Step 2: Run test to verify it fails**

  ```bash
  cd frontend/src-tauri && cargo test --lib build_rewrite_user_prompt
  ```
  Expected: compile error — `AskHistoryTurn`/`build_rewrite_user_prompt` don't exist yet.

- [ ] **Step 3: Implement the rewrite prompt and call**

  Near `ASK_ACROSS_MEETINGS_SYSTEM_PROMPT` (line 501-504):
  ```rust
  /// One prior question/answer exchange, for `rewrite_followup_query` below.
  /// Mirrors the frontend's `AskTurn` shape (`frontend/src/hooks/useAskAI.ts`)
  /// - the frontend already tracks this per-panel, it's just never sent to
  /// the backend today.
  pub struct AskHistoryTurn {
      pub question: String,
      pub answer: String,
  }

  const REWRITE_QUERY_SYSTEM_PROMPT: &str = "Rewrite the user's latest question into a \
  standalone question that makes sense with no prior context, using the conversation history \
  only to resolve pronouns and implicit references. Do not answer the question. Do not add \
  keywords or context that wasn't implied by the conversation. If the question is already \
  standalone, return it unchanged. Return ONLY the rewritten question, nothing else.";

  /// Builds the user-turn prompt for `rewrite_followup_query`: the
  /// conversation so far, then the question to rewrite. Pure/sync -
  /// unit-testable without a DB or network.
  fn build_rewrite_user_prompt(history: &[AskHistoryTurn], question: &str) -> String {
      let mut sections = Vec::new();
      if !history.is_empty() {
          let transcript = history
              .iter()
              .map(|t| format!("Q: {}\nA: {}", t.question, t.answer))
              .collect::<Vec<_>>()
              .join("\n\n");
          sections.push(format!("Conversation so far:\n{}", transcript));
      }
      sections.push(format!("Latest question to rewrite: {}", question));
      sections.join("\n\n")
  }

  /// Rewrites a follow-up question into a standalone one before retrieval,
  /// per docs/rag-token-research.md section 4.5 (MTRAG: retrieval recall
  /// halves on follow-up turns without this). Falls back to the original
  /// question - never propagates an error - since a broken rewrite must
  /// never block the ask itself; the worst case is retrieval quality
  /// reverting to today's (already-shipped) behavior, not a failed request.
  async fn rewrite_followup_query<R: Runtime>(
      app: &AppHandle<R>,
      history: &[AskHistoryTurn],
      question: &str,
  ) -> String {
      if history.is_empty() {
          // Nothing to resolve pronouns against - an opening question is
          // already standalone (MTRAG measures 0.89 recall here vs 0.47 on
          // later turns), so skip the LLM round-trip entirely.
          return question.to_string();
      }
      let user_prompt = build_rewrite_user_prompt(history, question);
      match ask_configured_llm(app, REWRITE_QUERY_SYSTEM_PROMPT, &user_prompt).await {
          Ok(rewritten) if !rewritten.trim().is_empty() => rewritten.trim().to_string(),
          _ => question.to_string(),
      }
  }
  ```

- [ ] **Step 4: Run test to verify it passes**

  ```bash
  cd frontend/src-tauri && cargo test --lib build_rewrite_user_prompt
  ```
  Expected: `test result: ok. 2 passed`.

- [ ] **Step 5: Commit**

  ```bash
  git add frontend/src-tauri/src/summary/commands.rs
  git commit -m "feat(rag): add decontextualizing query rewriting for cross-meeting follow-ups"
  ```

---

### Task 9: Rewire `ask_across_meetings` onto retrieval with structured citations

**Files:**
- Modify: `frontend/src-tauri/src/summary/commands.rs:1079-1139` (`ask_across_meetings` body)
- Test: `frontend/src-tauri/src/summary/commands.rs` (inline `#[cfg(test)]`)

**Interfaces:**
- Consumes: `rewrite_followup_query`/`AskHistoryTurn` (Task 8), `retrieve_relevant_chunks`/`RetrievedChunk` (Task 7), `EmbeddingService` (Task 5).
- Produces: replaces the command's return type from `Result<String, String>` to `Result<AskAcrossMeetingsResult, String>`:
  ```rust
  #[derive(serde::Serialize)]
  pub struct CrossMeetingCitation {
      pub index: usize,
      pub meeting_id: String,
      pub meeting_title: String,
      pub start_time: Option<f64>,
  }
  #[derive(serde::Serialize)]
  pub struct AskAcrossMeetingsResult {
      pub answer: String,
      pub citations: Vec<CrossMeetingCitation>,
  }
  ```
  This is a breaking change to `ask_across_meetings`'s frontend contract — Task 10 updates every caller.

The existing `[MM:SS]`-only citation format (used by `ask_about_meeting`/`ask_about_live_transcript`, resolved client-side by `askCitations.ts`) is ambiguous across meetings — a bare timestamp doesn't say *which* meeting. Rather than ask the LLM to echo meeting titles/ids inline (fragile: it can paraphrase a title or invent an id), each retrieved chunk is given a stable `[C<n>]` index in the prompt, the LLM is asked to cite by index, and the citation metadata (meeting id/title/timestamp) is resolved server-side from what was actually retrieved — never from the LLM's own text.

- [ ] **Step 1: Write the failing test**

  In `frontend/src-tauri/src/summary/commands.rs`, alongside `mod ask_ai_tests`:
  ```rust
  #[test]
  fn build_indexed_chunk_context_labels_each_chunk_and_includes_meeting_metadata() {
      let chunks = vec![
          crate::rag::retrieval::RetrievedChunk {
              chunk_id: 1,
              meeting_id: "meeting-1".to_string(),
              meeting_title: "Weekly Sync".to_string(),
              meeting_created_at: chrono::Utc::now(),
              chunk_text: "we decided to ship Friday".to_string(),
              start_time: Some(72.0),
              end_time: Some(80.0),
          },
      ];
      let context = build_indexed_chunk_context(&chunks);
      assert!(context.contains("[C1]"));
      assert!(context.contains("Weekly Sync"));
      assert!(context.contains("we decided to ship Friday"));
  }

  #[test]
  fn extract_cited_indices_parses_bracketed_c_tags() {
      let answer = "The team decided to ship Friday [C1], per an earlier discussion [C3].";
      assert_eq!(extract_cited_indices(answer), vec![1, 3]);
  }

  #[test]
  fn extract_cited_indices_ignores_out_of_range_and_malformed_tags() {
      let answer = "See [C1] and [Cx] and [C0].";
      assert_eq!(extract_cited_indices(answer), vec![1]);
  }
  ```

- [ ] **Step 2: Run test to verify it fails**

  ```bash
  cd frontend/src-tauri && cargo test --lib build_indexed_chunk_context extract_cited_indices
  ```
  Expected: compile error — neither function exists yet.

- [ ] **Step 3: Implement the indexed-chunk context, citation extraction, and rewire the command**

  Replace the `ASK_ACROSS_MEETINGS_SYSTEM_PROMPT` constant (line 501-504) with:
  ```rust
  const ASK_ACROSS_MEETINGS_SYSTEM_PROMPT: &str = "You are answering a question that may span \
  multiple meetings, using retrieved excerpts from those meetings as context. Each excerpt is \
  labeled with a bracketed tag like [C1]. Cite every claim you make by appending the tag(s) of \
  the excerpt(s) it came from, in that exact bracketed form. Cite only tags that appear in the \
  context, and do not invent new ones. If the answer isn't in the provided excerpts, say so \
  plainly.";
  ```

  Add near it:
  ```rust
  /// Builds the LLM context block for the retrieval-backed
  /// `ask_across_meetings`: each retrieved chunk gets a stable `[C<n>]` tag
  /// (1-indexed, matching `extract_cited_indices` below) plus its owning
  /// meeting's title and date, so the LLM can cite a specific excerpt and
  /// the frontend can resolve that tag back to a clickable
  /// meeting+timestamp without trusting the LLM to echo either correctly.
  fn build_indexed_chunk_context(chunks: &[crate::rag::retrieval::RetrievedChunk]) -> String {
      chunks
          .iter()
          .enumerate()
          .map(|(i, chunk)| {
              format!(
                  "[C{}] Meeting: {} ({})\n{}",
                  i + 1,
                  chunk.meeting_title,
                  chunk.meeting_created_at.format("%Y-%m-%d"),
                  chunk.chunk_text
              )
          })
          .collect::<Vec<_>>()
          .join("\n\n")
  }

  /// Extracts every `[C<n>]` tag the LLM cited, de-duplicated, in first-seen
  /// order, 1-indexed. A tag pointing past the number of chunks that were
  /// actually sent (or non-numeric, e.g. a hallucinated `[Cx]`) is dropped
  /// rather than causing an out-of-bounds lookup at citation-resolution
  /// time - this function is called with the chunk count implicitly bounded
  /// by its caller filtering with `citations_within(chunks.len())` below.
  fn extract_cited_indices(answer: &str) -> Vec<usize> {
      let re = regex::Regex::new(r"\[C(\d+)\]").expect("static regex is valid");
      let mut seen = std::collections::HashSet::new();
      let mut indices = Vec::new();
      for cap in re.captures_iter(answer) {
          if let Ok(n) = cap[1].parse::<usize>() {
              if n >= 1 && seen.insert(n) {
                  indices.push(n);
              }
          }
      }
      indices
  }
  ```

  Replace the `ask_across_meetings` body (lines 1079-1139):
  ```rust
  /// Answers a free-text question that may span every stored meeting, using
  /// hybrid retrieval (Task 7) over chunked transcripts instead of packing
  /// meeting summaries until a length budget is hit - see
  /// docs/rag-token-research.md section 0 for why the old approach silently
  /// couldn't see meetings past roughly the 65th most recent.
  #[tauri::command]
  pub async fn ask_across_meetings<R: Runtime>(
      app: AppHandle<R>,
      state: tauri::State<'_, AppState>,
      question: String,
      history: Vec<AskHistoryTurn>,
  ) -> Result<AskAcrossMeetingsResult, String> {
      log_info!("ask_across_meetings called");

      let question = validate_ask_question(&question)?;
      let pool = state.db_manager.pool();

      let rewritten_question = rewrite_followup_query(&app, &history, &question).await;
      log_info!(
          "ask_across_meetings: retrieval query {:?} (original: {:?})",
          rewritten_question, question
      );

      let cache_dir = app
          .path()
          .app_data_dir()
          .map(|mut dir| {
              dir.push("models");
              dir.push("embeddings");
              dir
          })
          .map_err(|e| {
              log_error!("ask_across_meetings: failed to resolve app data dir: {}", e);
              "Internal error preparing search.".to_string()
          })?;
      let embedder = crate::rag::embedding::EmbeddingService::new(&cache_dir).map_err(|e| {
          log_error!("ask_across_meetings: failed to load embedding model: {}", e);
          "Search index isn't ready yet - try again in a moment.".to_string()
      })?;

      let chunks = crate::rag::retrieval::retrieve_relevant_chunks(
          pool,
          &embedder,
          &rewritten_question,
          ASK_ACROSS_MEETINGS_TOP_K,
      )
      .await
      .map_err(|e| {
          log_error!("ask_across_meetings: retrieval failed: {}", e);
          "Failed to search past meetings.".to_string()
      })?;

      if chunks.is_empty() {
          log_info!("ask_across_meetings: no indexed chunks matched the question");
          return Ok(AskAcrossMeetingsResult {
              answer: "No relevant meeting content found yet to answer from.".to_string(),
              citations: Vec::new(),
          });
      }

      let context = build_indexed_chunk_context(&chunks);
      let user_prompt = format!("{}\n\nQuestion: {}", context, question);

      let answer = ask_configured_llm(&app, ASK_ACROSS_MEETINGS_SYSTEM_PROMPT, &user_prompt).await?;

      let citations = extract_cited_indices(&answer)
          .into_iter()
          .filter_map(|i| chunks.get(i - 1))
          .enumerate()
          .map(|(display_index, chunk)| CrossMeetingCitation {
              index: display_index + 1,
              meeting_id: chunk.meeting_id.clone(),
              meeting_title: chunk.meeting_title.clone(),
              start_time: chunk.start_time,
          })
          .collect();

      Ok(AskAcrossMeetingsResult { answer, citations })
  }
  ```

  Add the new constant near the other `ASK_*_MAX_CHARS` constants (line 484):
  ```rust
  /// How many retrieved chunks `ask_across_meetings` sends to the LLM as
  /// context - replaces `ASK_ACROSS_MEETINGS_CONTEXT_MAX_CHARS`'s
  /// char-budget packing now that context is retrieved chunks, not
  /// summaries; a fixed chunk count is the natural budget for retrieval.
  const ASK_ACROSS_MEETINGS_TOP_K: usize = 8;
  ```

  Remove `ASK_ACROSS_MEETINGS_CONTEXT_MAX_CHARS` (line 478-484) and `build_cross_meeting_context` (line 634-745) along with their now-orphaned unit tests, since nothing calls them anymore. Add `use regex;` to the file's imports if not already present (check `Cargo.toml` — `regex = "1.11.0"` is already a dependency per the existing `[dependencies]` block, so no `Cargo.toml` change is needed here).

- [ ] **Step 4: Run test to verify it passes**

  ```bash
  cd frontend/src-tauri && cargo test --lib build_indexed_chunk_context extract_cited_indices
  cd frontend/src-tauri && cargo check
  ```
  Expected: `test result: ok. 3 passed`, and `cargo check` succeeds (confirms the removed `build_cross_meeting_context` had no other callers, and the changed return/argument types compile).

- [ ] **Step 5: Commit**

  ```bash
  git add frontend/src-tauri/src/summary/commands.rs
  git commit -m "feat(rag): rewire ask_across_meetings onto hybrid retrieval with structured citations"
  ```

---

### Task 10: Frontend — structured cross-meeting citations and navigate-to-timestamp

**Files:**
- Modify: `frontend/src/hooks/useAskAI.ts` (generalize the result type)
- Modify: `frontend/src/components/Sidebar/GlobalAskPanel.tsx` (send history, render structured citations)
- Create: `frontend/src/lib/crossMeetingCitations.ts`
- Create: `frontend/src/components/Sidebar/CrossMeetingCitationChip.tsx`
- Modify: `frontend/src/app/meeting-details/page-content.tsx` (read a `focusTime` search param on mount)
- Test: `frontend/src/lib/crossMeetingCitations.test.ts` (new, matching the existing test setup used for `askCitations.ts` if one exists — otherwise a plain Jest/Vitest unit test file per the project's existing frontend test runner)

**Interfaces:**
- Consumes: `AskAcrossMeetingsResult`/`CrossMeetingCitation` shape produced by Task 9's Rust command (mirrored as a TS type below).
- Produces: `export interface AskAcrossMeetingsResult { answer: string; citations: CrossMeetingCitation[] }`, `export function parseIndexedCitations(answer: string, citations: CrossMeetingCitation[]): AnswerToken[]` (reusing the existing `AnswerToken` union from `askCitations.ts` so downstream rendering code doesn't need a second token type).

`AskSidebar`'s citation contract (`segments: TranscriptSegmentData[]`, scroll-and-highlight *within the current page's already-loaded transcript*) genuinely doesn't fit here: a cross-meeting citation points at a *different* meeting's transcript, which isn't loaded on the current page at all — it has to navigate, not scroll. Rather than force that mismatch into `AskSidebar`, `GlobalAskPanel` gets its own citation rendering, reusing only the pieces that do fit as-is (`useAskAI`'s state machine, the `AnswerToken` type, the "split answer into text/citation runs" shape `parseAnswerCitations` already established).

- [ ] **Step 1: Write the failing test**

  `frontend/src/lib/crossMeetingCitations.test.ts`:
  ```typescript
  import { describe, expect, it } from 'vitest'; // match the project's actual test runner import if different - check an existing *.test.ts file's imports first
  import { parseIndexedCitations, type CrossMeetingCitation } from './crossMeetingCitations';

  describe('parseIndexedCitations', () => {
    const citations: CrossMeetingCitation[] = [
      { index: 1, meetingId: 'meeting-1', meetingTitle: 'Weekly Sync', startTime: 72 },
      { index: 2, meetingId: 'meeting-2', meetingTitle: 'Q3 Planning', startTime: null },
    ];

    it('splits text around [C1]-style tags into text and citation tokens', () => {
      const tokens = parseIndexedCitations('We shipped Friday [C1] after planning in Q3 [C2].', citations);
      expect(tokens).toEqual([
        { kind: 'text', text: 'We shipped Friday ' },
        { kind: 'citation', citation: citations[0] },
        { kind: 'text', text: ' after planning in Q3 ' },
        { kind: 'citation', citation: citations[1] },
        { kind: 'text', text: '.' },
      ]);
    });

    it('leaves an unresolvable tag as literal text', () => {
      const tokens = parseIndexedCitations('See [C9] for details.', citations);
      expect(tokens).toEqual([{ kind: 'text', text: 'See [C9] for details.' }]);
    });

    it('returns a single text token when there are no citations', () => {
      const tokens = parseIndexedCitations('No citations here.', []);
      expect(tokens).toEqual([{ kind: 'text', text: 'No citations here.' }]);
    });
  });
  ```
  Check an existing test file (e.g. alongside `frontend/src/lib/askCitations.ts`, if a sibling `.test.ts` exists) first to match this project's actual test runner import and file-naming convention before finalizing this file.

- [ ] **Step 2: Run test to verify it fails**

  ```bash
  cd frontend && pnpm test crossMeetingCitations
  ```
  Expected: failure — `./crossMeetingCitations` doesn't exist yet.

- [ ] **Step 3: Implement `crossMeetingCitations.ts`**

  ```typescript
  /**
   * Citation plumbing for `GlobalAskPanel`'s cross-meeting answers. Distinct
   * from `askCitations.ts`: that module resolves `[MM:SS]` timestamps back
   * to segments already loaded on the *current* page (a single meeting's
   * transcript); a cross-meeting answer cites a *different* meeting
   * entirely, so a citation here always means "navigate to that meeting",
   * never "scroll the transcript in place." The backend (`ask_across_meetings`
   * in frontend/src-tauri/src/summary/commands.rs) resolves each `[C<n>]` tag
   * to real meeting/timestamp metadata server-side, from what was actually
   * retrieved - never parsed out of the LLM's own prose - so this module only
   * has to split text around tags it's handed, not re-derive their meaning.
   */

  export interface CrossMeetingCitation {
    index: number;
    meetingId: string;
    meetingTitle: string;
    startTime: number | null;
  }

  export interface AskAcrossMeetingsResult {
    answer: string;
    citations: CrossMeetingCitation[];
  }

  export type CrossMeetingAnswerToken =
    | { kind: 'text'; text: string }
    | { kind: 'citation'; citation: CrossMeetingCitation };

  const CITATION_TAG_PATTERN = /\[C(\d+)\]/g;

  /** Splits an answer into text runs and resolved citation tokens. */
  export function parseIndexedCitations(
    answer: string,
    citations: CrossMeetingCitation[]
  ): CrossMeetingAnswerToken[] {
    const byIndex = new Map(citations.map(c => [c.index, c]));
    const tokens: CrossMeetingAnswerToken[] = [];
    let cursor = 0;

    for (const match of answer.matchAll(CITATION_TAG_PATTERN)) {
      const citation = byIndex.get(Number(match[1]));
      if (!citation) continue; // unresolvable tag stays literal text below

      const start = match.index!;
      if (start > cursor) {
        tokens.push({ kind: 'text', text: answer.slice(cursor, start) });
      }
      tokens.push({ kind: 'citation', citation });
      cursor = start + match[0].length;
    }

    if (cursor < answer.length) {
      tokens.push({ kind: 'text', text: answer.slice(cursor) });
    }
    return tokens;
  }
  ```

- [ ] **Step 4: Generalize `useAskAI`'s result type**

  In `frontend/src/hooks/useAskAI.ts`, change the `answer`/`turns` typing to a generic defaulting to `string` so `ask_about_meeting`/`ask_about_live_transcript` callers are unaffected while `GlobalAskPanel` can opt into `AskAcrossMeetingsResult`:
  ```typescript
  export interface AskTurn<TResult = string> {
    id: string;
    question: string;
    answer: TResult;
  }

  export interface UseAskAIResult<TResult = string> {
    question: string;
    setQuestion: (value: string) => void;
    answer: TResult | null;
    turns: AskTurn<TResult>[];
    pendingQuestion: string | null;
    isLoading: boolean;
    error: string | null;
    ask: () => void;
    handleKeyDown: (e: KeyboardEvent<HTMLInputElement>) => void;
    isSubmitDisabled: boolean;
  }

  export function useAskAI<TResult = string>(
    command: string,
    buildArgs: (question: string) => Record<string, unknown>,
    options: UseAskAIOptions = {}
  ): UseAskAIResult<TResult> {
  ```
  and change the internal state/`invoke` call sites (`useState<string | null>` → `useState<TResult | null>`, `useState<AskTurn[]>` → `useState<AskTurn<TResult>[]>`, `invoke<string>(command, ...)` → `invoke<TResult>(command, ...)`) accordingly. `AskMeetingPanel`/`LiveAskPanel`/`AskSidebar` call `useAskAI(...)` without a type argument today, so `TResult` defaults to `string` and their behavior is unchanged — confirm this with a type-check, not just a read.

- [ ] **Step 5: Rewrite `GlobalAskPanel`**

  ```typescript
  'use client';

  import { useCallback } from 'react';
  import { useRouter } from 'next/navigation';
  import { Loader2, Sparkles } from 'lucide-react';
  import { Button } from '@/components/ui/button';
  import { Input } from '@/components/ui/input';
  import { useAskAI } from '@/hooks/useAskAI';
  import { parseIndexedCitations, type AskAcrossMeetingsResult } from '@/lib/crossMeetingCitations';
  import { CrossMeetingCitationChip } from './CrossMeetingCitationChip';

  /**
   * Free-form Q&A across all meetings, shown in the sidebar next to meeting
   * search. Calls the single-shot `ask_across_meetings` Tauri command, which
   * now retrieves relevant chunks across every indexed meeting (hybrid FTS5 +
   * vector search, see frontend/src-tauri/src/rag/retrieval.rs) rather than
   * packing meeting summaries - and returns structured citations pointing at
   * specific meetings/timestamps instead of unstructured prose.
   */
  export function GlobalAskPanel() {
    const router = useRouter();
    const { question, setQuestion, answer, turns, isLoading, error, ask, handleKeyDown, isSubmitDisabled } =
      useAskAI<AskAcrossMeetingsResult>('ask_across_meetings', question => ({
        question,
        // Sends the conversation so far so the backend can rewrite a
        // follow-up ("What did they decide?") into a standalone query
        // before retrieval - see rewrite_followup_query in
        // frontend/src-tauri/src/summary/commands.rs and
        // docs/rag-token-research.md section 4.5.
        history: turns.map(t => ({ question: t.question, answer: t.answer.answer })),
      }));

    const handleFocusMeeting = useCallback(
      (meetingId: string, startTime: number | null) => {
        const suffix = startTime !== null ? `&focusTime=${startTime}` : '';
        router.push(`/meeting-details?id=${meetingId}${suffix}`);
      },
      [router]
    );

    return (
      <div className="mt-2 space-y-2">
        <div className="flex items-center gap-2">
          <Input
            placeholder="Ask across all meetings..."
            value={question}
            onChange={e => setQuestion(e.target.value)}
            onKeyDown={handleKeyDown}
            disabled={isLoading}
            className="h-8 text-xs"
          />
          <Button onClick={ask} disabled={isSubmitDisabled} size="sm" variant="outline" aria-label="Ask">
            {isLoading ? <Loader2 className="w-3.5 h-3.5 animate-spin" /> : <Sparkles className="w-3.5 h-3.5" />}
          </Button>
        </div>
        {error && (
          <p
            className="text-xs text-amber-400 bg-amber-500/10 border border-amber-500/20 rounded-md px-2 py-1.5"
            aria-live="polite"
          >
            {error}
          </p>
        )}
        {answer && (
          <div
            className="text-xs text-foreground/80 bg-secondary/5 border border-border/10 rounded-md px-2 py-1.5 max-h-40 overflow-y-auto"
            aria-live="polite"
          >
            {parseIndexedCitations(answer.answer, answer.citations).map((token, i) =>
              token.kind === 'text' ? (
                <span key={i} className="whitespace-pre-wrap">{token.text}</span>
              ) : (
                <CrossMeetingCitationChip key={i} citation={token.citation} onNavigate={handleFocusMeeting} />
              )
            )}
          </div>
        )}
      </div>
    );
  }
  ```

- [ ] **Step 6: Add `CrossMeetingCitationChip`**

  `frontend/src/components/Sidebar/CrossMeetingCitationChip.tsx`:
  ```typescript
  'use client';

  import { formatRecordingTimeLabel } from '@/lib/transcriptTime';
  import type { CrossMeetingCitation } from '@/lib/crossMeetingCitations';
  import { cn } from '@/lib/utils';

  /**
   * A `[C<n>]` citation from a cross-meeting answer, rendered as a chip that
   * navigates to the cited meeting (and, if the excerpt has a timestamp,
   * scrolls to it there) - unlike `CitationChip.tsx`, which scrolls within
   * the current page's already-loaded transcript, this always navigates,
   * since the cited meeting is never the one currently open.
   */
  export function CrossMeetingCitationChip({
    citation,
    onNavigate,
  }: {
    citation: CrossMeetingCitation;
    onNavigate: (meetingId: string, startTime: number | null) => void;
  }) {
    const label =
      citation.startTime !== null
        ? `${citation.meetingTitle} @ ${formatRecordingTimeLabel(citation.startTime)}`
        : citation.meetingTitle;

    return (
      <button
        type="button"
        onClick={() => onNavigate(citation.meetingId, citation.startTime)}
        title={`Open "${citation.meetingTitle}"`}
        className={cn(
          'mx-0.5 rounded px-1.5 align-super font-mono text-[10.5px]',
          'bg-primary/15 text-primary hover:bg-primary/25'
        )}
      >
        {label}
      </button>
    );
  }
  ```

- [ ] **Step 7: Focus a timestamp on meeting-details load**

  In `frontend/src/app/meeting-details/page-content.tsx`, near the existing `focusSegment` state (line 70): add a `useSearchParams()`-driven effect that, once transcript segments have loaded, reads a `focusTime` query param and resolves it to the nearest segment via the same `findSegmentAtTime` helper `askCitations.ts` already exports, then calls the existing `setFocusSegment({ id })` (already wired to `onFocusSegment` at line 273) - reusing the exact same focus/scroll path a same-meeting citation chip already uses, rather than building a second one:
  ```typescript
  import { useSearchParams } from 'next/navigation';
  import { findSegmentAtTime } from '@/lib/askCitations';

  // ... inside the component, after segments are available:
  const searchParams = useSearchParams();
  useEffect(() => {
    const focusTimeParam = searchParams.get('focusTime');
    if (focusTimeParam === null || segments.length === 0) return;
    const seconds = Number(focusTimeParam);
    if (Number.isNaN(seconds)) return;
    const segment = findSegmentAtTime(segments, seconds);
    if (segment) setFocusSegment({ id: segment.id });
  }, [searchParams, segments]);
  ```
  Match this effect's placement to wherever `segments` first becomes available in this file (check the existing state/effect ordering around line 70-90 before inserting, so it runs after segments load rather than racing them).

- [ ] **Step 8: Run test to verify it passes**

  ```bash
  cd frontend && pnpm test crossMeetingCitations
  cd frontend && pnpm exec tsc --noEmit
  ```
  Expected: the `crossMeetingCitations` tests pass, and the type-check confirms `AskMeetingPanel`/`LiveAskPanel`/`AskSidebar` still compile unchanged against the generalized `useAskAI<TResult = string>`.

- [ ] **Step 9: Commit**

  ```bash
  git add frontend/src/hooks/useAskAI.ts frontend/src/components/Sidebar/GlobalAskPanel.tsx frontend/src/lib/crossMeetingCitations.ts frontend/src/lib/crossMeetingCitations.test.ts frontend/src/components/Sidebar/CrossMeetingCitationChip.tsx frontend/src/app/meeting-details/page-content.tsx
  git commit -m "feat(rag): render structured cross-meeting citations and navigate to cited timestamps"
  ```

---

### Task 11: Local cross-encoder reranking

**Files:**
- Modify: `frontend/src-tauri/src/rag/embedding.rs` (add reranking alongside the existing embedding model)
- Modify: `frontend/src-tauri/src/rag/retrieval.rs` (rerank fused candidates before truncating to `top_k`)
- Modify: `frontend/src-tauri/src/summary/commands.rs` (`ask_across_meetings`, updated in Task 9 — construct a `RerankerService` and pass it through)
- Test: `frontend/src-tauri/src/rag/embedding.rs` and `frontend/src-tauri/src/rag/retrieval.rs` (inline `#[cfg(test)]`)

**Interfaces:**
- Consumes: `fastembed`'s reranking API (same crate as Task 5 — "one dependency covers both stages," per the research doc section 4.3).
- Produces: `pub struct RerankerService`, `pub fn RerankerService::new(cache_dir: &Path) -> anyhow::Result<Self>`, `pub fn RerankerService::rerank(&self, query: &str, documents: &[String]) -> anyhow::Result<Vec<(usize, f32)>>` (document index + relevance score, sorted descending). `retrieve_relevant_chunks` (Task 7) gains an optional reranking pass.

Per the research doc section 4.4, this is "the biggest measured quality win in the whole research" (Recall@5 +17.4% relative, MRR@3 +39.7% relative) — but heavily qualified: measured on financial documents with numerical answers and whole-document (not chunked) retrieval, and "no open or local reranker was evaluated... zero direct evidence for `bge-reranker` running locally." Build it, but the plan treats it as an additive, disable-able Phase 2 pass, not a load-bearing assumption — consistent with the research doc's own framing ("worth building, worth measuring").

- [ ] **Step 1: Write the failing test**

  In `frontend/src-tauri/src/rag/embedding.rs`, in the existing `#[cfg(test)] mod tests`:
  ```rust
  #[test]
  #[ignore]
  fn rerank_orders_documents_by_relevance_to_the_query() {
      let cache_dir = TempDir::new().expect("tempdir failed");
      let reranker = RerankerService::new(cache_dir.path()).expect("reranker load failed");

      let documents = vec![
          "the office coffee machine is broken again".to_string(),
          "we're launching the product next quarter".to_string(),
      ];
      let ranked = reranker.rerank("when is the product launching", &documents).expect("rerank failed");

      assert_eq!(ranked[0].0, 1, "the launch-relevant document should rank first");
  }
  ```

- [ ] **Step 2: Run test to verify it fails**

  ```bash
  cd frontend/src-tauri && cargo test --lib embedding::tests::rerank_orders -- --ignored
  ```
  Expected: compile error — `RerankerService` doesn't exist yet.

- [ ] **Step 3: Implement `RerankerService`**

  In `frontend/src-tauri/src/rag/embedding.rs`, after `EmbeddingService`:
  ```rust
  use fastembed::{RerankInitOptions, RerankerModel, TextRerank};

  /// Local cross-encoder reranking via fastembed-rs's `TextRerank` - a
  /// second, slower, more accurate pass over a small candidate set (see
  /// docs/rag-token-research.md section 2.5/4.4). Never run over the full
  /// corpus, only over the ~50 candidates `retrieve_relevant_chunks` already
  /// fetched per retriever.
  pub struct RerankerService {
      model: TextRerank,
  }

  impl RerankerService {
      pub fn new(cache_dir: &Path) -> anyhow::Result<Self> {
          let model = TextRerank::try_new(
              RerankInitOptions::new(RerankerModel::BGERerankerBase)
                  .with_cache_dir(cache_dir.to_path_buf())
                  .with_show_download_progress(true),
          )?;
          Ok(Self { model })
      }

      /// Scores every document against `query` and returns
      /// `(original_index, score)` pairs sorted by score, descending -
      /// callers map the returned indices back to whatever richer struct
      /// (e.g. `RetrievedChunk`) the plain-text `documents` were drawn from.
      pub fn rerank(&self, query: &str, documents: &[String]) -> anyhow::Result<Vec<(usize, f32)>> {
          let doc_refs: Vec<&str> = documents.iter().map(String::as_str).collect();
          let results = self.model.rerank(query, doc_refs, false, None)?;
          Ok(results.into_iter().map(|r| (r.index, r.score)).collect())
      }
  }
  ```
  Verify `fastembed::{RerankInitOptions, RerankerModel, TextRerank}`'s exact API shape on docs.rs (same caveat as Task 5 Step 1 — this wasn't independently checked here) before finalizing; adjust method/field names if they differ.

- [ ] **Step 4: Wire reranking into `retrieve_relevant_chunks`**

  In `frontend/src-tauri/src/rag/retrieval.rs`, change the signature to accept an optional reranker and apply it after fusion, before hydration:
  ```rust
  pub async fn retrieve_relevant_chunks(
      pool: &SqlitePool,
      embedder: &EmbeddingService,
      reranker: Option<&crate::rag::embedding::RerankerService>,
      query: &str,
      top_k: usize,
  ) -> anyhow::Result<Vec<RetrievedChunk>> {
      // ... existing keyword_ranked / vector_ranked / fused logic unchanged ...

      let fused_ids: Vec<i64> = fused.into_iter().map(|(id, _)| id).collect();

      let candidate_ids: Vec<i64> = if let Some(reranker) = reranker {
          // Over-fetch beyond top_k for reranking to have something to
          // reorder - keep every fused candidate, not just top_k, since
          // RRF's ranking and the reranker's cross-encoder scoring can
          // disagree meaningfully within the fused list.
          let candidates = hydrate_chunk_texts(pool, &fused_ids).await?;
          if candidates.is_empty() {
              Vec::new()
          } else {
              let texts: Vec<String> = candidates.iter().map(|(_, text)| text.clone()).collect();
              let scored = reranker.rerank(query, &texts)?;
              scored.into_iter().map(|(idx, _score)| candidates[idx].0).collect()
          }
      } else {
          fused_ids
      };

      let mut results = Vec::with_capacity(top_k);
      for chunk_id in candidate_ids.into_iter().take(top_k) {
          // ... existing hydration-by-id logic unchanged, now looped over
          // candidate_ids instead of fused directly ...
      }
      Ok(results)
  }

  /// Fetches just `(chunk_id, chunk_text)` for a set of ids, in the order
  /// given - the minimal read `rerank` needs before the fuller
  /// `RetrievedChunk` hydration happens for the final, reranked order.
  async fn hydrate_chunk_texts(pool: &SqlitePool, ids: &[i64]) -> anyhow::Result<Vec<(i64, String)>> {
      let mut out = Vec::with_capacity(ids.len());
      for &id in ids {
          if let Some((text,)) = sqlx::query_as::<_, (String,)>("SELECT chunk_text FROM rag_chunks WHERE id = ?")
              .bind(id)
              .fetch_optional(pool)
              .await?
          {
              out.push((id, text));
          }
      }
      Ok(out)
  }
  ```
  `retrieve_relevant_chunks` now takes one more parameter, so its two existing call sites need updating:

  In `frontend/src-tauri/src/rag/retrieval.rs`, Task 7's own `retrieval_tests::retrieves_chunks_relevant_to_the_query_across_meetings` test (added in Task 7 Step 5) calls it with the old 4-argument form. Update that call to:
  ```rust
  let results = retrieve_relevant_chunks(&pool, &embedder, None, "when is the redesign launching", 5)
      .await
      .unwrap();
  ```

  In `frontend/src-tauri/src/summary/commands.rs`, `ask_across_meetings` (rewired in Task 9) currently constructs only an `EmbeddingService` before calling `retrieve_relevant_chunks`. Add a `RerankerService` alongside it, built from the same `cache_dir`, and pass it through:
  ```rust
      let embedder = crate::rag::embedding::EmbeddingService::new(&cache_dir).map_err(|e| {
          log_error!("ask_across_meetings: failed to load embedding model: {}", e);
          "Search index isn't ready yet - try again in a moment.".to_string()
      })?;
      let reranker = crate::rag::embedding::RerankerService::new(&cache_dir).map_err(|e| {
          log_error!("ask_across_meetings: failed to load reranker model: {}", e);
          "Search index isn't ready yet - try again in a moment.".to_string()
      })?;

      let chunks = crate::rag::retrieval::retrieve_relevant_chunks(
          pool,
          &embedder,
          Some(&reranker),
          &rewritten_question,
          ASK_ACROSS_MEETINGS_TOP_K,
      )
      .await
      .map_err(|e| {
          log_error!("ask_across_meetings: retrieval failed: {}", e);
          "Failed to search past meetings.".to_string()
      })?;
  ```
  This replaces the `let embedder = ...` / `let chunks = crate::rag::retrieval::retrieve_relevant_chunks(pool, &embedder, &rewritten_question, ASK_ACROSS_MEETINGS_TOP_K)...` block Task 9 wrote.

- [ ] **Step 5: Run test to verify it passes**

  ```bash
  cd frontend/src-tauri && cargo test --lib embedding::tests::rerank_orders -- --ignored
  cd frontend/src-tauri && cargo check
  ```
  Expected: `test result: ok. 1 passed`, and `cargo check` succeeds with every `retrieve_relevant_chunks` call site updated for the new parameter.

- [ ] **Step 6: Commit**

  ```bash
  git add frontend/src-tauri/src/rag/embedding.rs frontend/src-tauri/src/rag/retrieval.rs frontend/src-tauri/src/summary/commands.rs
  git commit -m "feat(rag): add optional local cross-encoder reranking to hybrid retrieval"
  ```
