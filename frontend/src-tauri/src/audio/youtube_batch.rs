// YouTube batch import: parallel downloads via yt-dlp, then serial transcription
// through the shared Whisper/Parakeet engine.
//
// Reuses `youtube_import::download_youtube_audio` for each item and the shared
// `import_pipeline::run_transcription_pipeline` for the serial transcribe pass.
// Batch state is in-memory only - v1 does not persist batches across restarts.

use futures_util::stream::{self, StreamExt};
use log::info;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Runtime};
use tokio::sync::Mutex as AsyncMutex;
use uuid::Uuid;

use super::common::{
    release_batch_import, try_acquire_batch_import, unload_engine_after_batch,
    YOUTUBE_IMPORT_IN_PROGRESS,
};
use super::import::ImportStarted;
use super::import_pipeline::get_configured_provider;
use super::youtube_import::{
    cancel_youtube_import, download_youtube_audio, is_youtube_import_in_progress,
    is_valid_youtube_url, transcribe_youtube_download, YoutubeDownloadResult,
    YOUTUBE_IMPORT_CANCELLED,
};

/// Maximum number of in-flight yt-dlp downloads at once. The serial transcription
/// phase that follows is bottlenecked on a single engine, so downloads should
/// stay ahead but not saturate the network/disk.
pub const BATCH_DOWNLOAD_CONCURRENCY: usize = 4;

pub const BATCH_PROGRESS_EVENT: &str = "youtube-batch-progress";
pub const BATCH_COMPLETE_EVENT: &str = "youtube-batch-complete";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BatchItemStatus {
    Pending,
    Downloading,
    Downloaded,
    Transcribing,
    Complete,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
pub struct BatchItem {
    pub index: usize,
    pub url: String,
    pub title: Option<String>,
    pub status: BatchItemStatus,
    pub progress_percentage: u32,
    pub meeting_id: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BatchImportStatus {
    pub id: String,
    pub total: usize,
    pub completed: usize,
    pub failed: usize,
    pub items: Vec<BatchItem>,
    pub finished: bool,
    pub cancelled: bool,
}

#[derive(Debug, Clone, Serialize)]
struct BatchProgressPayload {
    id: String,
    total: usize,
    completed: usize,
    failed: usize,
    item: BatchItem,
    finished: bool,
    cancelled: bool,
}

#[derive(Debug, Clone, Serialize)]
struct BatchCompletePayload {
    id: String,
    total: usize,
    completed: usize,
    failed: usize,
    cancelled: bool,
}

#[derive(Default)]
struct BatchAggregator {
    items: Vec<BatchItem>,
    completed: usize,
    failed: usize,
    cancelled: bool,
}

impl BatchAggregator {
    fn new(requests: Vec<(String, Option<String>)>) -> Self {
        let items = requests
            .into_iter()
            .enumerate()
            .map(|(index, (url, title))| BatchItem {
                index,
                url,
                title,
                status: BatchItemStatus::Pending,
                progress_percentage: 0,
                meeting_id: None,
                error: None,
            })
            .collect();
        Self {
            items,
            completed: 0,
            failed: 0,
            cancelled: false,
        }
    }

    fn set_item_status(
        &mut self,
        index: usize,
        status: BatchItemStatus,
        progress_percentage: u32,
        error: Option<String>,
        meeting_id: Option<String>,
    ) {
        if let Some(item) = self.items.get_mut(index) {
            item.status = status;
            item.progress_percentage = progress_percentage;
            item.error = error;
            item.meeting_id = meeting_id;
        }
    }

    fn recompute_counts(&mut self) {
        self.completed = self
            .items
            .iter()
            .filter(|i| i.status == BatchItemStatus::Complete)
            .count();
        self.failed = self
            .items
            .iter()
            .filter(|i| {
                matches!(
                    i.status,
                    BatchItemStatus::Failed | BatchItemStatus::Cancelled
                )
            })
            .count();
    }

    fn snapshot(&self, id: &str, finished: bool) -> BatchImportStatus {
        BatchImportStatus {
            id: id.to_string(),
            total: self.items.len(),
            completed: self.completed,
            failed: self.failed,
            items: self.items.clone(),
            finished,
            cancelled: self.cancelled,
        }
    }

    fn mark_remaining_cancelled(&mut self) {
        for item in self.items.iter_mut() {
            if matches!(
                item.status,
                BatchItemStatus::Pending
                    | BatchItemStatus::Downloading
                    | BatchItemStatus::Downloaded
                    | BatchItemStatus::Transcribing
            ) {
                item.status = BatchItemStatus::Cancelled;
                if item.error.is_none() {
                    item.error = Some("Batch cancelled".to_string());
                }
            }
        }
        self.cancelled = true;
        self.recompute_counts();
    }
}

struct BatchState {
    aggregator: AsyncMutex<BatchAggregator>,
    id: String,
    cancel_flag: Arc<AtomicBool>,
}

static BATCH_STATE: Lazy<AsyncMutex<Option<Arc<BatchState>>>> =
    Lazy::new(|| AsyncMutex::new(None));

fn progress_event_for_item(batch_id: &str, index: usize) -> String {
    format!("youtube-batch-item-{}-{}", batch_id, index)
}

/// Classify a transcription error as cancellation vs. real failure. Three
/// signals are checked, in order: the global YOUTUBE_IMPORT_CANCELLED
/// atomic (set by the user-facing cancel command), the per-batch cancel
/// flag (set internally), and a case-insensitive "cancel" substring in
/// the error message itself. Any of the three means the user (or the
/// pipeline) wanted this item stopped, not that the engine crashed.
fn is_cancellation_error(err: &str, batch_cancel_flag: &AtomicBool) -> bool {
    YOUTUBE_IMPORT_CANCELLED.load(Ordering::SeqCst)
        || batch_cancel_flag.load(Ordering::SeqCst)
        || err.to_lowercase().contains("cancel")
}

fn progress_payload(id: &str, agg: &BatchAggregator, item: BatchItem) -> BatchProgressPayload {
    BatchProgressPayload {
        id: id.to_string(),
        total: agg.items.len(),
        completed: agg.completed,
        failed: agg.failed,
        item,
        finished: false,
        cancelled: agg.cancelled,
    }
}

async fn emit_progress<R: Runtime>(
    app: &AppHandle<R>,
    state: &BatchState,
    agg: &BatchAggregator,
    item: BatchItem,
) {
    let payload = progress_payload(&state.id, agg, item);
    let _ = app.emit(BATCH_PROGRESS_EVENT, payload);
}

async fn emit_complete<R: Runtime>(app: &AppHandle<R>, state: &BatchState, agg: &BatchAggregator) {
    let payload = BatchCompletePayload {
        id: state.id.clone(),
        total: agg.items.len(),
        completed: agg.completed,
        failed: agg.failed,
        cancelled: agg.cancelled,
    };
    let _ = app.emit(BATCH_COMPLETE_EVENT, payload);
}

pub fn parse_batch_url_input(raw: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    raw.lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter(|s| seen.insert(s.to_string()))
        .map(|s| s.to_string())
        .collect()
}

pub fn partition_valid_urls(
    urls: Vec<String>,
) -> (Vec<(String, Option<String>)>, Vec<(String, String)>) {
    let mut valid = Vec::new();
    let mut invalid = Vec::new();
    for url in urls {
        let trimmed = url.trim().to_string();
        if is_valid_youtube_url(&trimmed) {
            valid.push((trimmed, None));
        } else {
            invalid.push((url, "Not a valid YouTube URL".to_string()));
        }
    }
    (valid, invalid)
}

pub fn parse_and_validate_batch_input(
    raw: &str,
    titles: Option<Vec<String>>,
) -> (Vec<(String, Option<String>)>, Vec<(String, String)>) {
    let urls = parse_batch_url_input(raw);
    let (mut valid, invalid) = partition_valid_urls(urls);
    if let Some(t) = titles {
        for (i, title) in t.into_iter().enumerate() {
            if let Some(slot) = valid.get_mut(i) {
                let trimmed = title.trim();
                if !trimmed.is_empty() {
                    slot.1 = Some(trimmed.to_string());
                }
            }
        }
    }
    (valid, invalid)
}

#[tauri::command]
pub async fn start_youtube_batch_import_command<R: Runtime>(
    app: AppHandle<R>,
    urls: Vec<String>,
    titles: Option<Vec<String>>,
) -> Result<ImportStarted, String> {
    let joined = urls.join("\n");
    let (valid, invalid) = parse_and_validate_batch_input(&joined, titles);

    if valid.is_empty() {
        let msg = if invalid.is_empty() {
            "No URLs provided".to_string()
        } else {
            format!(
                "No valid YouTube URLs ({} invalid). {}",
                invalid.len(),
                invalid
                    .iter()
                    .map(|(u, why)| format!("'{}': {}", u, why))
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        };
        return Err(msg);
    }

    if is_youtube_import_in_progress() {
        return Err("An import is already in progress".to_string());
    }

    let total = valid.len();
    let batch_id = Uuid::new_v4().to_string();

    try_acquire_batch_import(&YOUTUBE_IMPORT_IN_PROGRESS)?;
    YOUTUBE_IMPORT_CANCELLED.store(false, Ordering::SeqCst);

    let aggregator = BatchAggregator::new(valid.clone());
    let state = Arc::new(BatchState {
        aggregator: AsyncMutex::new(aggregator),
        id: batch_id.clone(),
        cancel_flag: Arc::new(AtomicBool::new(false)),
    });

    {
        let mut guard = BATCH_STATE.lock().await;
        *guard = Some(state.clone());
    }

    tauri::async_runtime::spawn(async move {
        let provider = get_configured_provider(&app)
            .await
            .unwrap_or_else(|_| "whisper".to_string());

        let download_inputs: Vec<(usize, String, Option<String>)> = {
            let agg = state.aggregator.lock().await;
            agg.items
                .iter()
                .map(|i| (i.index, i.url.clone(), i.title.clone()))
                .collect()
        };

        let mut downloads = stream::iter(download_inputs.into_iter().map(|(index, url, title)| {
            let app = app.clone();
            let state = state.clone();
            async move {
                {
                    let mut agg = state.aggregator.lock().await;
                    agg.set_item_status(index, BatchItemStatus::Downloading, 0, None, None);
                    agg.recompute_counts();
                    let item = agg.items[index].clone();
                    emit_progress(&app, &state, &agg, item).await;
                }

                if state.cancel_flag.load(Ordering::SeqCst) {
                    let mut agg = state.aggregator.lock().await;
                    agg.set_item_status(
                        index,
                        BatchItemStatus::Cancelled,
                        0,
                        Some("Batch cancelled".to_string()),
                        None,
                    );
                    agg.recompute_counts();
                    return None;
                }

                let event_name = progress_event_for_item(&state.id, index);
                let result = download_youtube_audio(&app, &url, title, &event_name).await;
                let mut agg = state.aggregator.lock().await;
                match result {
                    Ok(download) => {
                        agg.set_item_status(
                            index,
                            BatchItemStatus::Downloaded,
                            15,
                            None,
                            None,
                        );
                        agg.recompute_counts();
                        let item = agg.items[index].clone();
                        emit_progress(&app, &state, &agg, item).await;
                        Some((index, url, download))
                    }
                    Err(err) => {
                        agg.set_item_status(
                            index,
                            BatchItemStatus::Failed,
                            0,
                            Some(err),
                            None,
                        );
                        agg.recompute_counts();
                        let item = agg.items[index].clone();
                        emit_progress(&app, &state, &agg, item).await;
                        None
                    }
                }
            }
        }))
        .buffer_unordered(BATCH_DOWNLOAD_CONCURRENCY);

        let mut ordered: Vec<Option<(String, YoutubeDownloadResult)>> = vec![None; total];
        while let Some(result) = downloads.next().await {
            if let Some((idx, url, download)) = result {
                ordered[idx] = Some((url, download));
            }
        }

        for (idx, slot) in ordered.into_iter().enumerate() {
            if state.cancel_flag.load(Ordering::SeqCst) {
                if let Some((_, download)) = slot {
                    let _ = std::fs::remove_dir_all(&download.meeting_folder);
                }
                continue;
            }
            let Some((url, download)) = slot else { continue };

            {
                let mut agg = state.aggregator.lock().await;
                agg.set_item_status(idx, BatchItemStatus::Transcribing, 30, None, None);
                agg.recompute_counts();
                let item = agg.items[idx].clone();
                emit_progress(&app, &state, &agg, item).await;
            }

            let result = transcribe_youtube_download(&app, download, &url, provider.clone()).await;

            let mut agg = state.aggregator.lock().await;
            match result {
                Ok(outcome) => {
                    agg.set_item_status(
                        idx,
                        BatchItemStatus::Complete,
                        100,
                        None,
                        Some(outcome.meeting_id),
                    );
                }
                Err(err) => {
                    let was_cancelled = is_cancellation_error(&err, state.cancel_flag.as_ref());
                    let status = if was_cancelled {
                        BatchItemStatus::Cancelled
                    } else {
                        BatchItemStatus::Failed
                    };
                    agg.set_item_status(idx, status, 0, Some(err), None);
                }
            }
            agg.recompute_counts();
            let item = agg.items[idx].clone();
            emit_progress(&app, &state, &agg, item).await;
        }

        {
            let mut agg = state.aggregator.lock().await;
            if state.cancel_flag.load(Ordering::SeqCst) {
                agg.mark_remaining_cancelled();
            }
            emit_complete(&app, &state, &agg).await;
        }

        {
            let mut guard = BATCH_STATE.lock().await;
            *guard = None;
        }
        release_batch_import(&YOUTUBE_IMPORT_IN_PROGRESS);

        let use_parakeet = provider == "parakeet";
        unload_engine_after_batch(use_parakeet).await;

        info!(
            "YouTube batch import {} finished: {} total, {} completed, {} failed, cancelled={}",
            state.id,
            total,
            state.aggregator.lock().await.completed,
            state.aggregator.lock().await.failed,
            state.aggregator.lock().await.cancelled,
        );
    });

    Ok(ImportStarted {
        message: format!("YouTube batch import started ({} URLs)", total),
    })
}

#[tauri::command]
pub async fn cancel_youtube_batch_import_command() -> Result<(), String> {
    let state_opt = {
        let guard = BATCH_STATE.lock().await;
        guard.as_ref().cloned()
    };
    let Some(state) = state_opt else {
        return Err("No YouTube batch import in progress".to_string());
    };
    state.cancel_flag.store(true, Ordering::SeqCst);
    YOUTUBE_IMPORT_CANCELLED.store(true, Ordering::SeqCst);
    cancel_youtube_import();
    Ok(())
}

#[tauri::command]
pub async fn get_youtube_batch_status_command(batch_id: String) -> Result<BatchImportStatus, String> {
    let state_opt = {
        let guard = BATCH_STATE.lock().await;
        guard.as_ref().cloned()
    };
    let Some(state) = state_opt else {
        return Err(format!("No active YouTube batch import with id {}", batch_id));
    };
    if state.id != batch_id {
        return Err(format!(
            "Batch import id mismatch: expected {}",
            state.id
        ));
    }
    let agg = state.aggregator.lock().await;
    Ok(agg.snapshot(&state.id, false))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url_with_title(s: &str) -> (String, Option<String>) {
        (s.to_string(), None)
    }

    // -- URL parsing helpers --

    #[test]
    fn test_parse_batch_url_input_splits_lines_and_trims() {
        let input = "  https://youtu.be/a\n\n https://youtu.be/b \n\n";
        let parsed = parse_batch_url_input(input);
        assert_eq!(
            parsed,
            vec![
                "https://youtu.be/a".to_string(),
                "https://youtu.be/b".to_string(),
            ]
        );
    }

    #[test]
    fn test_parse_batch_url_input_dedups_preserving_first_seen() {
        let input = "https://youtu.be/a\nhttps://youtu.be/b\nhttps://youtu.be/a\nhttps://youtu.be/c";
        let parsed = parse_batch_url_input(input);
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0], "https://youtu.be/a");
        assert_eq!(parsed[1], "https://youtu.be/b");
        assert_eq!(parsed[2], "https://youtu.be/c");
    }

    #[test]
    fn test_parse_batch_url_input_empty_input() {
        assert!(parse_batch_url_input("").is_empty());
        assert!(parse_batch_url_input("\n\n   \n").is_empty());
    }

    #[test]
    fn test_partition_valid_urls_splits_correctly() {
        let urls = vec![
            "https://www.youtube.com/watch?v=abc".to_string(),
            "not a url".to_string(),
            "https://youtu.be/xyz".to_string(),
            "ftp://youtube.com/watch?v=foo".to_string(),
        ];
        let (valid, invalid) = partition_valid_urls(urls);
        assert_eq!(valid.len(), 2);
        assert_eq!(valid[0].0, "https://www.youtube.com/watch?v=abc");
        assert_eq!(valid[1].0, "https://youtu.be/xyz");
        assert_eq!(invalid.len(), 2);
        assert_eq!(invalid[0].0, "not a url");
        assert_eq!(invalid[1].0, "ftp://youtube.com/watch?v=foo");
    }

    #[test]
    fn test_parse_and_validate_combined() {
        let (valid, invalid) = parse_and_validate_batch_input(
            "https://youtu.be/a\nnot_a_url\nhttps://youtu.be/b",
            Some(vec!["Title A".to_string(), "Title B".to_string()]),
        );
        assert_eq!(valid.len(), 2);
        assert_eq!(valid[0].0, "https://youtu.be/a");
        assert_eq!(valid[0].1.as_deref(), Some("Title A"));
        assert_eq!(valid[1].0, "https://youtu.be/b");
        assert_eq!(valid[1].1.as_deref(), Some("Title B"));
        assert_eq!(invalid.len(), 1);
        assert_eq!(invalid[0].0, "not_a_url");
    }

    #[test]
    fn test_parse_and_validate_drops_blank_titles() {
        let (valid, _) = parse_and_validate_batch_input(
            "https://youtu.be/a\nhttps://youtu.be/b",
            Some(vec!["   ".to_string(), "Real".to_string()]),
        );
        assert!(valid[0].1.is_none());
        assert_eq!(valid[1].1.as_deref(), Some("Real"));
    }

    // -- BatchAggregator state machine --

    #[test]
    fn test_aggregator_new_initializes_pending_items() {
        let agg = BatchAggregator::new(vec![url_with_title("a"), url_with_title("b")]);
        assert_eq!(agg.items.len(), 2);
        assert!(agg
            .items
            .iter()
            .all(|i| i.status == BatchItemStatus::Pending));
        assert_eq!(agg.completed, 0);
        assert_eq!(agg.failed, 0);
        assert!(!agg.cancelled);
    }

    #[test]
    fn test_aggregator_set_item_status_updates_one_item() {
        let mut agg = BatchAggregator::new(vec![url_with_title("a"), url_with_title("b")]);
        agg.set_item_status(1, BatchItemStatus::Downloading, 0, None, None);
        assert_eq!(agg.items[0].status, BatchItemStatus::Pending);
        assert_eq!(agg.items[1].status, BatchItemStatus::Downloading);
    }

    #[test]
    fn test_aggregator_recompute_counts_classifies_correctly() {
        let mut agg = BatchAggregator::new(vec![
            url_with_title("a"),
            url_with_title("b"),
            url_with_title("c"),
            url_with_title("d"),
            url_with_title("e"),
        ]);
        agg.set_item_status(0, BatchItemStatus::Complete, 100, None, Some("m1".into()));
        agg.set_item_status(1, BatchItemStatus::Complete, 100, None, Some("m2".into()));
        agg.set_item_status(2, BatchItemStatus::Failed, 0, Some("err".into()), None);
        agg.set_item_status(3, BatchItemStatus::Cancelled, 0, Some("c".into()), None);
        agg.set_item_status(4, BatchItemStatus::Transcribing, 30, None, None);
        agg.recompute_counts();
        assert_eq!(agg.completed, 2);
        assert_eq!(agg.failed, 2);
    }

    #[test]
    fn test_aggregator_snapshot_carries_id_and_finished_flag() {
        let agg = BatchAggregator::new(vec![url_with_title("a")]);
        let snap = agg.snapshot("batch-1", true);
        assert_eq!(snap.id, "batch-1");
        assert!(snap.finished);
        assert_eq!(snap.total, 1);
    }

    #[test]
    fn test_aggregator_set_item_status_ignores_out_of_range_index() {
        let mut agg = BatchAggregator::new(vec![url_with_title("a")]);
        agg.set_item_status(99, BatchItemStatus::Complete, 100, None, Some("m".into()));
        assert_eq!(agg.items[0].status, BatchItemStatus::Pending);
    }

    #[test]
    fn test_aggregator_mark_remaining_cancelled_handles_mixed_states() {
        let mut agg = BatchAggregator::new(vec![
            url_with_title("a"),
            url_with_title("b"),
            url_with_title("c"),
            url_with_title("d"),
        ]);
        agg.set_item_status(0, BatchItemStatus::Complete, 100, None, Some("m".into()));
        agg.set_item_status(1, BatchItemStatus::Downloading, 0, None, None);
        agg.set_item_status(2, BatchItemStatus::Downloaded, 15, None, None);
        agg.mark_remaining_cancelled();
        assert_eq!(agg.items[0].status, BatchItemStatus::Complete);
        assert_eq!(agg.items[1].status, BatchItemStatus::Cancelled);
        assert_eq!(agg.items[2].status, BatchItemStatus::Cancelled);
        assert_eq!(agg.items[3].status, BatchItemStatus::Cancelled);
        assert!(agg.cancelled);
        assert!(agg.items[1].error.is_some());
    }

    // -- Failure mode: empty after validation --

    #[test]
    fn test_parse_and_validate_empty_after_validation() {
        let (valid, invalid) = parse_and_validate_batch_input("nope\nstill-nope", None);
        assert!(valid.is_empty());
        assert_eq!(invalid.len(), 2);
    }

    #[test]
    fn test_batch_download_concurrency_is_positive_and_reasonable() {
        assert!(BATCH_DOWNLOAD_CONCURRENCY > 0);
        assert!(BATCH_DOWNLOAD_CONCURRENCY <= 32);
    }

    #[test]
    fn test_batch_item_status_equality() {
        assert_eq!(BatchItemStatus::Failed, BatchItemStatus::Failed);
        assert_ne!(BatchItemStatus::Failed, BatchItemStatus::Complete);
    }

    // =========================================================================
    // Adversarial tests (breaker agent pass 1)
    // =========================================================================
    // Focus: URL parsing edge cases (whitespace, control chars, dedup, very
    // long lists), playlist URLs, title alignment, and aggregator invariants
    // under load.

    #[test]
    fn test_parse_batch_url_input_handles_only_whitespace() {
        let parsed = parse_batch_url_input("   \n\t  \n   \r\n");
        assert!(parsed.is_empty(), "got {:?}", parsed);
    }

    #[test]
    fn test_parse_batch_url_input_treats_internal_whitespace_as_separators() {
        // "a b" stays as one URL (URLs shouldn't have spaces anyway, so
        // is_valid_youtube_url will reject it later).
        let parsed = parse_batch_url_input("https://youtu.be/a b\nhttps://youtu.be/c");
        // split happens only on \n, not spaces.
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0], "https://youtu.be/a b");
        assert_eq!(parsed[1], "https://youtu.be/c");
    }

    #[test]
    fn test_parse_batch_url_input_dedups_case_sensitive() {
        // Two URLs that differ only in case (e.g. scheme or path). The
        // current dedup is exact-match, so these are NOT deduped.
        let parsed = parse_batch_url_input(
            "https://youtu.be/abc\nhttps://Youtu.be/abc\nHTTPS://YOUTU.BE/abc",
        );
        assert_eq!(
            parsed.len(),
            3,
            "case-only variants are NOT deduped (got {:?})",
            parsed
        );
    }

    #[test]
    fn test_parse_batch_url_input_handles_100_url_list() {
        // 100 distinct valid URLs — exercises dedup/trim/partition on a
        // realistic batch size.
        let mut s = String::new();
        for i in 0..100 {
            if i > 0 {
                s.push('\n');
            }
            s.push_str(&format!("  https://youtu.be/id{i:03}  "));
        }
        let parsed = parse_batch_url_input(&s);
        assert_eq!(parsed.len(), 100);
        assert_eq!(parsed[0], "https://youtu.be/id000");
        assert_eq!(parsed[99], "https://youtu.be/id099");
    }

    #[test]
    fn test_parse_batch_url_input_handles_url_with_extra_query_params() {
        // Valid YouTube URLs with start time, feature, etc.
        let parsed = parse_batch_url_input(
            "https://www.youtube.com/watch?v=abc&t=42s\nhttps://www.youtube.com/watch?v=def&feature=share",
        );
        assert_eq!(parsed.len(), 2);
    }

    #[test]
    fn test_partition_valid_urls_rejects_bare_playlist_url() {
        // A pure playlist URL (no v= param) must be rejected.
        let urls = vec!["https://www.youtube.com/playlist?list=PLxyz".to_string()];
        let (valid, invalid) = partition_valid_urls(urls);
        assert!(valid.is_empty());
        assert_eq!(invalid.len(), 1);
        assert_eq!(invalid[0].0, "https://www.youtube.com/playlist?list=PLxyz");
    }

    #[test]
    fn test_partition_valid_urls_rejects_non_youtube_urls() {
        // Direct mp4, vimeo, twitch, dailymotion.
        let urls = vec![
            "https://example.com/video.mp4".to_string(),
            "https://vimeo.com/12345".to_string(),
            "https://www.twitch.tv/videos/12345".to_string(),
            "https://www.dailymotion.com/video/x7tg8e0".to_string(),
        ];
        let (valid, invalid) = partition_valid_urls(urls);
        assert!(valid.is_empty(), "got valid={:?}", valid);
        assert_eq!(invalid.len(), 4);
    }

    #[test]
    fn test_parse_and_validate_handles_more_titles_than_urls() {
        // More titles than URLs: extra titles are ignored.
        let (valid, invalid) = parse_and_validate_batch_input(
            "https://youtu.be/a\nhttps://youtu.be/b",
            Some(vec![
                "T1".to_string(),
                "T2".to_string(),
                "T3-extra".to_string(),
            ]),
        );
        assert_eq!(valid.len(), 2);
        assert_eq!(valid[0].1.as_deref(), Some("T1"));
        assert_eq!(valid[1].1.as_deref(), Some("T2"));
        assert!(invalid.is_empty());
    }

    #[test]
    fn test_parse_and_validate_handles_fewer_titles_than_urls() {
        // Fewer titles than URLs: extra URLs have no title.
        let (valid, _invalid) = parse_and_validate_batch_input(
            "https://youtu.be/a\nhttps://youtu.be/b\nhttps://youtu.be/c",
            Some(vec!["T1".to_string()]),
        );
        assert_eq!(valid.len(), 3);
        assert_eq!(valid[0].1.as_deref(), Some("T1"));
        assert!(valid[1].1.is_none());
        assert!(valid[2].1.is_none());
    }

    #[test]
    fn test_parse_and_validate_titles_align_only_to_valid_urls() {
        // Titles are aligned to *valid* URLs, not to raw input lines.
        // So an invalid URL at position 0 shifts the title mapping.
        let (valid, _invalid) = parse_and_validate_batch_input(
            "garbage\nhttps://youtu.be/real",
            Some(vec!["TitleForFirst".to_string(), "TitleForSecond".to_string()]),
        );
        assert_eq!(valid.len(), 1);
        // Title index 0 maps to the first valid slot — i.e. "TitleForFirst"
        assert_eq!(valid[0].1.as_deref(), Some("TitleForFirst"));
    }

    #[test]
    fn test_parse_and_validate_blank_titles_become_none() {
        // Whitespace-only titles are dropped, not stored as the whitespace.
        let (valid, _) = parse_and_validate_batch_input(
            "https://youtu.be/a\nhttps://youtu.be/b\nhttps://youtu.be/c",
            Some(vec!["   ".to_string(), "\t\n".to_string(), "Real".to_string()]),
        );
        assert!(valid[0].1.is_none());
        assert!(valid[1].1.is_none());
        assert_eq!(valid[2].1.as_deref(), Some("Real"));
    }

    #[test]
    fn test_aggregator_recompute_counts_cancelled_counted_as_failed() {
        // Cancelled items roll into the "failed" count by design. Verify
        // that explicit. The frontend uses "failed" to show the error
        // summary, so cancelled being lumped in is observable.
        let mut agg = BatchAggregator::new(vec![
            url_with_title("a"),
            url_with_title("b"),
            url_with_title("c"),
        ]);
        agg.set_item_status(0, BatchItemStatus::Complete, 100, None, Some("m".into()));
        agg.set_item_status(1, BatchItemStatus::Cancelled, 0, Some("c".into()), None);
        agg.set_item_status(2, BatchItemStatus::Failed, 0, Some("e".into()), None);
        agg.recompute_counts();
        assert_eq!(agg.completed, 1);
        assert_eq!(agg.failed, 2, "cancelled items contribute to failed count");
    }

    #[test]
    fn test_aggregator_completed_count_excludes_downloaded_only() {
        // Only "Complete" counts as completed. Downloaded-but-not-transcribed
        // items are still in-flight.
        let mut agg = BatchAggregator::new(vec![
            url_with_title("a"),
            url_with_title("b"),
            url_with_title("c"),
        ]);
        agg.set_item_status(0, BatchItemStatus::Downloaded, 15, None, None);
        agg.set_item_status(1, BatchItemStatus::Transcribing, 30, None, None);
        agg.set_item_status(2, BatchItemStatus::Complete, 100, None, Some("m".into()));
        agg.recompute_counts();
        assert_eq!(agg.completed, 1);
        assert_eq!(agg.failed, 0);
    }

    #[test]
    fn test_aggregator_100_items_does_not_panic() {
        let requests: Vec<(String, Option<String>)> = (0..100)
            .map(|i| (format!("https://youtu.be/id{i:03}"), None))
            .collect();
        let mut agg = BatchAggregator::new(requests);
        for i in 0..100 {
            let status = match i % 5 {
                0 => BatchItemStatus::Complete,
                1 => BatchItemStatus::Failed,
                2 => BatchItemStatus::Cancelled,
                3 => BatchItemStatus::Downloading,
                _ => BatchItemStatus::Transcribing,
            };
            agg.set_item_status(i, status, 0, None, None);
        }
        agg.recompute_counts();
        // 20 Complete (i % 5 == 0), 20 Failed, 20 Cancelled
        assert_eq!(agg.completed, 20);
        assert_eq!(agg.failed, 40, "Failed + Cancelled");
    }

    #[test]
    fn test_aggregator_mark_remaining_preserves_terminal_statuses() {
        // Items already in Complete, Failed, or Cancelled must NOT be
        // overwritten by mark_remaining_cancelled.
        let mut agg = BatchAggregator::new(vec![
            url_with_title("a"),
            url_with_title("b"),
            url_with_title("c"),
            url_with_title("d"),
        ]);
        agg.set_item_status(0, BatchItemStatus::Complete, 100, None, Some("m0".into()));
        agg.set_item_status(1, BatchItemStatus::Failed, 0, Some("original err".into()), None);
        agg.set_item_status(2, BatchItemStatus::Cancelled, 0, Some("orig cancel".into()), None);
        agg.set_item_status(3, BatchItemStatus::Downloading, 0, None, None);
        agg.mark_remaining_cancelled();
        assert_eq!(agg.items[0].status, BatchItemStatus::Complete);
        assert_eq!(agg.items[1].status, BatchItemStatus::Failed);
        assert_eq!(agg.items[2].status, BatchItemStatus::Cancelled);
        assert_eq!(agg.items[3].status, BatchItemStatus::Cancelled);
        // The Complete item must keep its meeting_id; failed/cancelled must
        // not be overwritten with "Batch cancelled".
        assert_eq!(agg.items[0].meeting_id.as_deref(), Some("m0"));
        assert_eq!(agg.items[1].error.as_deref(), Some("original err"));
        assert_eq!(agg.items[2].error.as_deref(), Some("orig cancel"));
    }

    #[test]
    fn test_snapshot_total_reflects_input_not_terminal_state() {
        // The snapshot's `total` is the input size, not the count of
        // completed/failed items. Note: snapshot returns the cached
        // completed/failed counters — recompute_counts() must be called
        // before snapshot for the counters to be accurate. This test
        // pins that contract.
        let mut agg = BatchAggregator::new(vec![
            url_with_title("a"),
            url_with_title("b"),
            url_with_title("c"),
        ]);
        agg.set_item_status(0, BatchItemStatus::Complete, 100, None, Some("m".into()));
        agg.set_item_status(1, BatchItemStatus::Failed, 0, Some("e".into()), None);
        agg.recompute_counts();
        let snap = agg.snapshot("batch-1", true);
        assert_eq!(snap.total, 3);
        assert_eq!(snap.completed, 1);
        assert_eq!(snap.failed, 1);
        assert_eq!(snap.items.len(), 3);
    }

    #[test]
    fn test_parse_batch_url_input_handles_urls_with_internal_newlines_after_trim() {
        // URLs themselves don't contain newlines (those are separators),
        // but trim() removes only outer whitespace. A URL with a newline
        // in the middle stays as one input "line" only if there were no
        // \n in the raw text — which there always are. So this is just
        // confirming the parser's input model: it splits on \n then trims.
        let parsed = parse_batch_url_input("https://youtu.be/a\n\n");
        assert_eq!(parsed, vec!["https://youtu.be/a".to_string()]);
    }

    // =========================================================================
    // Round 2 adversarial tests (breaker pass 2)
    // =========================================================================
    // Focus: cancel-during-transcription mis-classification, pending-item
    // visibility after cancel, stale tests from round 1.

    /// Simulates the batch's transcription loop (the for-loop at line 407
    /// of the real code) when an item's transcription fails *because the
    /// user cancelled* — `transcribe_youtube_download` returns Err, the
    /// loop calls `set_item_status(Failed, ...)` without checking whether
    /// the failure was a cancellation. So the cancelled item shows up
    /// with status `Failed` in the snapshot, not `Cancelled`.
    ///
    /// This test exercises the *aggregator* path the loop uses. The
    /// frontend's UI then displays "failed" rather than "cancelled" for
    /// the affected item.
    #[test]
    fn test_cancelled_item_marks_as_failed_not_cancelled() {
        let mut agg = BatchAggregator::new(vec![
            url_with_title("a"),
            url_with_title("b"),
            url_with_title("c"),
        ]);
        // Simulate the loop: item 1 is in Transcribing, the cancel flag
        // fires, the transcription returns Err("cancelled").
        agg.set_item_status(0, BatchItemStatus::Complete, 100, None, Some("m0".into()));
        agg.set_item_status(1, BatchItemStatus::Transcribing, 30, None, None);
        agg.recompute_counts();

        // The loop sees Err and writes Failed — *not* Cancelled. This
        // mirrors line 437-440 of youtube_batch.rs.
        let cancel_err: String = "Import cancelled".to_string();
        agg.set_item_status(1, BatchItemStatus::Failed, 0, Some(cancel_err), None);
        agg.recompute_counts();

        let snap = agg.snapshot("batch-1", true);
        let item1 = &snap.items[1];
        // BUG: the item that failed only because the user cancelled ends
        // up labeled "Failed" in the snapshot. The frontend has no way to
        // distinguish "user cancelled" from "download error" from this
        // status alone. (snapshot.cancelled is true only if the whole
        // batch's `cancelled` flag was set, which is set by
        // mark_remaining_cancelled, not by per-item transcription
        // failures.)
        assert_eq!(item1.status, BatchItemStatus::Failed);
        assert_eq!(item1.error.as_deref(), Some("Import cancelled"));
        // Pinning the observed (buggy) behavior: there's no Cancelled
        // status set on this item.
        assert_ne!(item1.status, BatchItemStatus::Cancelled);
    }

    /// When the batch is cancelled mid-download, the orchestrator drops
    /// the download stream. Items whose download closure never gets a
    /// chance to run (still in the buffer) never have their status
    /// updated. They remain in `Pending` (or `Downloading` if they had
    /// been picked up but hadn't run the cancel-check yet).
    ///
    /// In the UI this means a cancelled batch shows some items as
    /// "Pending" or "Downloading" forever, even though the batch is
    /// done. The end-of-batch `mark_remaining_cancelled` is only called
    /// if the batch's *own* `cancel_flag` is set, but it only marks
    /// items that are NOT already in a terminal state — which Pending
    /// and Downloading items satisfy, so they SHOULD be marked
    /// cancelled. Verify the mark function fixes them, and that the
    /// orchestrator actually calls it.
    #[test]
    fn test_mark_remaining_cancelled_fixes_pending_and_downloading() {
        let mut agg = BatchAggregator::new(vec![
            url_with_title("a"),
            url_with_title("b"),
            url_with_title("c"),
            url_with_title("d"),
        ]);
        // a completed, b still pending, c downloading, d downloaded-but-
        // not-yet-transcribed
        agg.set_item_status(0, BatchItemStatus::Complete, 100, None, Some("m".into()));
        agg.set_item_status(2, BatchItemStatus::Downloading, 0, None, None);
        agg.set_item_status(3, BatchItemStatus::Downloaded, 15, None, None);

        agg.mark_remaining_cancelled();

        // The orchestrator's call to mark_remaining_cancelled() does
        // catch the in-flight items. So the bug is actually that the
        // *orchestrator doesn't always call this for short-circuit
        // cancels* — but at minimum mark_remaining_cancelled itself
        // works correctly. Confirm that.
        assert_eq!(agg.items[0].status, BatchItemStatus::Complete);
        assert_eq!(agg.items[1].status, BatchItemStatus::Cancelled);
        assert_eq!(agg.items[2].status, BatchItemStatus::Cancelled);
        assert_eq!(agg.items[3].status, BatchItemStatus::Cancelled);
        assert!(agg.cancelled);
    }

    /// The actual orchestrator only calls `mark_remaining_cancelled` at
    /// the end of the transcription phase (line 448), inside the same
    /// `let mut agg = state.aggregator.lock().await;` block as the
    /// `emit_complete`. If cancel fires *before* the orchestrator even
    /// starts the transcription loop (e.g. cancel right after downloads
    /// finish, before any item enters Transcribing), the items that
    /// downloaded successfully but haven't been transcribed yet are in
    /// `Downloaded` state. The loop checks cancel_flag *per item* and
    /// cleans up the meeting folder but doesn't update the item status
    /// to Cancelled (the item stays in `Downloaded` with no error).
    ///
    /// Pin this observed behavior: the loop at line 407-444 removes the
    /// meeting folder for cancelled items but never writes
    /// BatchItemStatus::Cancelled to the aggregator for them.
    #[test]
    fn test_loop_short_circuit_on_cancel_leaves_items_in_downloaded() {
        // The actual loop in the production code is:
        //     for (idx, slot) in ordered.into_iter().enumerate() {
        //         if state.cancel_flag.load(...) {
        //             if let Some((_, download)) = slot {
        //                 let _ = std::fs::remove_dir_all(&download.meeting_folder);
        //             }
        //             continue;     // <-- skips without updating status
        //         }
        //         ...
        //     }
        //
        // The `continue` skips the rest of the loop body, including the
        // `set_item_status` calls. The aggregator's items stay in their
        // pre-loop state (Downloaded, in this scenario). The end-of-loop
        // mark_remaining_cancelled only runs if cancel_flag is set, but
        // it operates on the items as they are — Downloaded IS one of
        // the states it would mark as Cancelled. So in practice the
        // end-of-batch block *does* fix this. But: if the orchestrator
        // ever short-circuits before the loop, the items are stuck.
        let mut agg = BatchAggregator::new(vec![url_with_title("a"), url_with_title("b")]);
        agg.set_item_status(0, BatchItemStatus::Downloaded, 15, None, None);
        agg.set_item_status(1, BatchItemStatus::Downloaded, 15, None, None);

        // Simulate the loop body's cancel short-circuit: nothing happens
        // to the aggregator items.
        // (No set_item_status calls here.)

        // Now mark_remaining_cancelled runs at the end.
        agg.mark_remaining_cancelled();

        // Yes, mark_remaining_cancelled does fix this case. So the
        // bug depends on the orchestrator reaching the end-of-loop
        // block. If a panic or early return happens between the loop
        // and the mark_remaining_cancelled call, items would be stuck
        // in Downloaded state forever. Pin the *fixed* behavior.
        assert_eq!(agg.items[0].status, BatchItemStatus::Cancelled);
        assert_eq!(agg.items[1].status, BatchItemStatus::Cancelled);
    }

    /// Stale round-1 test: `test_partition_valid_urls_classifies_playlist_url_as_valid`
    /// was written when `is_valid_youtube_url` accepted watch URLs with
    /// a `list=` query parameter. The round-1 fix (commit 1ce4696) made
    /// `is_valid_youtube_url` reject any URL with a `list` query param.
    /// The stale test in this file now fails. Pin the *post-fix* behavior.
    #[test]
    fn test_partition_valid_urls_rejects_watch_url_with_list_param() {
        let urls = vec!["https://www.youtube.com/watch?v=abc&list=PLxyz".to_string()];
        let (valid, invalid) = partition_valid_urls(urls);
        assert!(valid.is_empty(), "post-fix: playlist URL must be rejected, got valid={:?}", valid);
        assert_eq!(invalid.len(), 1);
        assert_eq!(invalid[0].0, "https://www.youtube.com/watch?v=abc&list=PLxyz");
    }

    // =========================================================================
    // Round 2 regression tests (breaker pass 2)
    // =========================================================================
    // Focus: when the transcription loop sees Err, classify as Cancelled vs
    // Failed correctly. Three signals: global YOUTUBE_IMPORT_CANCELLED,
    // per-batch cancel_flag, and a "cancel" substring in the error message.

    /// Serializes tests that mutate `YOUTUBE_IMPORT_CANCELLED` (a process-wide
    /// atomic shared with `youtube_import.rs`) so they don't race with other
    /// tests in the same binary that read or set it.
    static GLOBAL_STATE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn test_is_cancellation_error_when_global_flag_set() {
        let _guard = GLOBAL_STATE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        YOUTUBE_IMPORT_CANCELLED.store(true, Ordering::SeqCst);
        let batch_flag = AtomicBool::new(false);
        // Error message says nothing about cancel; the global flag alone is
        // enough to classify this as a cancellation.
        assert!(is_cancellation_error("transcribe failed: out of memory", &batch_flag));
        YOUTUBE_IMPORT_CANCELLED.store(false, Ordering::SeqCst);
    }

    #[test]
    fn test_is_cancellation_error_when_batch_flag_set() {
        let _guard = GLOBAL_STATE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        YOUTUBE_IMPORT_CANCELLED.store(false, Ordering::SeqCst);
        let batch_flag = AtomicBool::new(true);
        assert!(is_cancellation_error("transcribe failed: out of memory", &batch_flag));
    }

    #[test]
    fn test_is_cancellation_error_when_message_mentions_cancel() {
        let _guard = GLOBAL_STATE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        YOUTUBE_IMPORT_CANCELLED.store(false, Ordering::SeqCst);
        let batch_flag = AtomicBool::new(false);
        // Any case-insensitive "cancel" substring wins, even without the flags.
        assert!(is_cancellation_error("Import cancelled", &batch_flag));
        assert!(is_cancellation_error("CANCELLED by user", &batch_flag));
        assert!(is_cancellation_error("audio pipeline: cancelled mid-decode", &batch_flag));
    }

    #[test]
    fn test_is_cancellation_error_false_for_real_failure() {
        let _guard = GLOBAL_STATE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        YOUTUBE_IMPORT_CANCELLED.store(false, Ordering::SeqCst);
        let batch_flag = AtomicBool::new(false);
        // No flag set, no "cancel" in the message — this is a real failure.
        assert!(!is_cancellation_error("whisper engine crashed: code 137", &batch_flag));
        assert!(!is_cancellation_error("audio decode failed: unsupported format", &batch_flag));
    }

    /// When `transcribe_youtube_download` returns Err and the global cancel
    /// flag is set, the loop's classification (mirrored here via the
    /// `is_cancellation_error` helper) must write `Cancelled` to the
    /// aggregator, not `Failed`. Without this fix the UI shows a red
    /// "failed" indicator that the user can't distinguish from a real
    /// engine crash.
    ///
    /// This is the "when cancellation flag is set before transcription
    /// starts, the item ends up Cancelled not Failed" regression test.
    #[test]
    fn test_transcription_loop_err_with_cancel_flag_marks_item_cancelled() {
        let _guard = GLOBAL_STATE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        YOUTUBE_IMPORT_CANCELLED.store(true, Ordering::SeqCst);

        let mut agg = BatchAggregator::new(vec![
            url_with_title("a"),
            url_with_title("b"),
        ]);
        agg.set_item_status(0, BatchItemStatus::Transcribing, 30, None, None);
        agg.recompute_counts();

        // Simulate transcribe_youtube_download returning Err with a message
        // that does NOT itself mention "cancel" — the classification must
        // come from the flag, not the message.
        let err: String = "engine returned non-zero exit".to_string();
        let batch_flag = AtomicBool::new(false);
        let was_cancelled = is_cancellation_error(&err, &batch_flag);
        let status = if was_cancelled {
            BatchItemStatus::Cancelled
        } else {
            BatchItemStatus::Failed
        };
        agg.set_item_status(0, status, 0, Some(err), None);
        agg.recompute_counts();

        let snap = agg.snapshot("batch-1", true);
        assert_eq!(
            snap.items[0].status,
            BatchItemStatus::Cancelled,
            "with cancel flag set, transcription err must classify as Cancelled, not Failed"
        );
        assert_ne!(snap.items[0].status, BatchItemStatus::Failed);

        YOUTUBE_IMPORT_CANCELLED.store(false, Ordering::SeqCst);
    }

    /// When `transcribe_youtube_download` returns Err with no cancel flag
    /// set and no "cancel" substring in the message, the loop must still
    /// mark the item as `Failed` — cancels must not over-match.
    #[test]
    fn test_transcription_loop_err_without_cancel_still_marks_failed() {
        let _guard = GLOBAL_STATE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        YOUTUBE_IMPORT_CANCELLED.store(false, Ordering::SeqCst);

        let mut agg = BatchAggregator::new(vec![url_with_title("a")]);
        agg.set_item_status(0, BatchItemStatus::Transcribing, 30, None, None);
        agg.recompute_counts();

        let err: String = "whisper: failed to load model".to_string();
        let batch_flag = AtomicBool::new(false);
        let was_cancelled = is_cancellation_error(&err, &batch_flag);
        let status = if was_cancelled {
            BatchItemStatus::Cancelled
        } else {
            BatchItemStatus::Failed
        };
        agg.set_item_status(0, status, 0, Some(err), None);
        agg.recompute_counts();

        let snap = agg.snapshot("batch-1", true);
        assert_eq!(snap.items[0].status, BatchItemStatus::Failed);
    }
}
