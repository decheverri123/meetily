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
                    agg.set_item_status(idx, BatchItemStatus::Failed, 0, Some(err), None);
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
}
