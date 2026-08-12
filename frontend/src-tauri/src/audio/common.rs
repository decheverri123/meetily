use crate::api::TranscriptSegment;
use anyhow::Result;
use log::{debug, info};
use once_cell::sync::Lazy;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};
use uuid::Uuid;

static ENGINE_LIFECYCLE_LOCK: Lazy<Arc<AsyncMutex<()>>> =
    Lazy::new(|| Arc::new(AsyncMutex::new(())));

// ============================================================================
// Batch-import mutual exclusion
// ============================================================================

/// Set while a local-file import batch job is running (see `import::ImportGuard`).
pub(crate) static IMPORT_IN_PROGRESS: AtomicBool = AtomicBool::new(false);
/// Set while a YouTube import batch job is running (see `youtube_import::YoutubeImportGuard`).
pub(crate) static YOUTUBE_IMPORT_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// Serializes acquisition of the two batch-import flags above so the check-then-set
/// below is atomic across both flags, not just within one.
static BATCH_IMPORT_ACQUIRE_LOCK: StdMutex<()> = StdMutex::new(());

/// Acquire a batch-import flag, refusing if either import kind is already running.
///
/// Local-file and YouTube imports track their in-progress/cancel state with separate
/// flags (so their status/cancel commands stay independent), but starting one is
/// blocked while the other is active: both funnel through the same singleton
/// Whisper/Parakeet engine, and `unload_engine_after_batch` unloads that engine when
/// a batch job finishes — running two batch jobs concurrently risks one unloading the
/// engine the other is still using mid-transcription.
pub(crate) fn try_acquire_batch_import(flag: &'static AtomicBool) -> Result<(), String> {
    let _lock = BATCH_IMPORT_ACQUIRE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if IMPORT_IN_PROGRESS.load(Ordering::SeqCst) || YOUTUBE_IMPORT_IN_PROGRESS.load(Ordering::SeqCst) {
        return Err("An import is already in progress".to_string());
    }
    flag.store(true, Ordering::SeqCst);
    Ok(())
}

/// Release a batch-import flag acquired via `try_acquire_batch_import`.
pub(crate) fn release_batch_import(flag: &'static AtomicBool) {
    flag.store(false, Ordering::SeqCst);
}

pub(crate) async fn acquire_engine_lifecycle_lock() -> OwnedMutexGuard<()> {
    ENGINE_LIFECYCLE_LOCK.clone().lock_owned().await
}

/// Unload the transcription engine after a batch job (import or retranscription).
/// Skips unloading if a live recording is currently in progress, since recording
/// uses the same global engine instances.
pub(crate) async fn unload_engine_after_batch(use_parakeet: bool) {
    let _engine_lifecycle_guard = acquire_engine_lifecycle_lock().await;

    if crate::audio::recording_commands::is_recording().await {
        log::info!("Skipping model unload after batch: recording in progress");
        return;
    }

    if use_parakeet {
        use crate::parakeet_engine::commands::PARAKEET_ENGINE;
        let engine = {
            let guard = PARAKEET_ENGINE.lock().unwrap_or_else(|e| e.into_inner());
            guard.as_ref().cloned()
        };
        if let Some(e) = engine {
            e.unload_model().await;
        }
    } else {
        use crate::whisper_engine::commands::WHISPER_ENGINE;
        let engine = {
            let guard = WHISPER_ENGINE.lock().unwrap_or_else(|e| e.into_inner());
            guard.as_ref().cloned()
        };
        if let Some(e) = engine {
            e.unload_model().await;
        }
    }
}

/// Create transcript segments from transcription results.
/// Each tuple is (text, start_ms, end_ms) from VAD timestamps.
pub(crate) fn create_transcript_segments(transcripts: &[(String, f64, f64)]) -> Vec<TranscriptSegment> {
    transcripts
        .iter()
        .map(|(text, start_ms, end_ms)| {
            let start_seconds = start_ms / 1000.0;
            let end_seconds = end_ms / 1000.0;
            let duration = end_seconds - start_seconds;

            TranscriptSegment {
                id: format!("transcript-{}", Uuid::new_v4()),
                text: text.trim().to_string(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                audio_start_time: Some(start_seconds),
                audio_end_time: Some(end_seconds),
                duration: Some(duration),
            }
        })
        .collect()
}

/// Write metadata.json to a meeting folder (atomic write with temp file).
///
/// `extra_fields`, if given, must be a JSON object; its keys are merged into the base
/// metadata object. This lets callers attach source-specific fields (e.g. YouTube's
/// `source_url`/`video_title`/`channel`/`default_template`) without duplicating the
/// base shape shared by every import source.
pub(crate) fn write_import_metadata(
    folder: &Path,
    meeting_id: &str,
    title: &str,
    duration_seconds: f64,
    audio_filename: &str,
    source: &str,
    extra_fields: Option<serde_json::Value>,
) -> Result<()> {
    let metadata_path = folder.join("metadata.json");
    let temp_path = folder.join(".metadata.json.tmp");
    let now = chrono::Utc::now().to_rfc3339();

    let mut json = serde_json::json!({
        "version": "1.0",
        "meeting_id": meeting_id,
        "meeting_name": title,
        "created_at": now,
        "completed_at": now,
        "duration_seconds": duration_seconds,
        "audio_file": audio_filename,
        "transcript_file": "transcripts.json",
        "status": "completed",
        "source": source
    });

    if let Some(serde_json::Value::Object(extra_map)) = extra_fields {
        if let serde_json::Value::Object(base_map) = &mut json {
            base_map.extend(extra_map);
        }
    }

    let json_string = serde_json::to_string_pretty(&json)?;
    std::fs::write(&temp_path, &json_string)?;
    std::fs::rename(&temp_path, &metadata_path)?;

    info!("Wrote metadata.json to {}", metadata_path.display());
    Ok(())
}

/// Write transcripts.json to a meeting folder (atomic write with temp file)
pub(crate) fn write_transcripts_json(folder: &Path, segments: &[TranscriptSegment]) -> Result<()> {
    let transcript_path = folder.join("transcripts.json");
    let temp_path = folder.join(".transcripts.json.tmp");

    let json = serde_json::json!({
        "version": "1.0",
        "last_updated": chrono::Utc::now().to_rfc3339(),
        "total_segments": segments.len(),
        "segments": segments.iter().enumerate().map(|(i, s)| {
            serde_json::json!({
                "id": s.id,
                "text": s.text,
                "timestamp": s.timestamp,
                "audio_start_time": s.audio_start_time,
                "audio_end_time": s.audio_end_time,
                "duration": s.duration,
                "sequence_id": i
            })
        }).collect::<Vec<_>>()
    });

    let json_string = serde_json::to_string_pretty(&json)?;
    std::fs::write(&temp_path, &json_string)?;
    std::fs::rename(&temp_path, &transcript_path)?;

    info!(
        "Wrote transcripts.json with {} segments to {}",
        segments.len(),
        transcript_path.display()
    );
    Ok(())
}

/// Split a long speech segment at the lowest-energy (silence) point near the target size.
///
/// Scans for 100ms windows with minimal RMS energy within +/-3 seconds of each target
/// split point. If no clear silence is found, falls back to a 1-second overlap split
/// to avoid cutting words at boundaries.
pub(crate) fn split_segment_at_silence(
    segment: &crate::audio::vad::SpeechSegment,
    max_samples: usize,
) -> Vec<crate::audio::vad::SpeechSegment> {
    const SAMPLE_RATE: usize = 16000;
    // 100ms window for energy measurement (1600 samples at 16kHz)
    const ENERGY_WINDOW: usize = SAMPLE_RATE / 10;
    // Search +/-3 seconds around the target split point
    const SEARCH_RADIUS: usize = SAMPLE_RATE * 3;
    // RMS threshold below which we consider a window "silent"
    const SILENCE_RMS_THRESHOLD: f32 = 0.02;
    // Overlap to use when no silence boundary is found (1 second)
    const FALLBACK_OVERLAP: usize = SAMPLE_RATE;

    let total = segment.samples.len();
    if total <= max_samples {
        return vec![segment.clone()];
    }

    let ms_per_sample = (segment.end_timestamp_ms - segment.start_timestamp_ms)
        / segment.samples.len() as f64;
    let mut result = Vec::new();
    let mut pos = 0usize;

    while pos < total {
        let remaining = total - pos;
        if remaining <= max_samples {
            // Last chunk - take everything remaining
            let chunk_samples = segment.samples[pos..].to_vec();
            let chunk_start_ms = segment.start_timestamp_ms + (pos as f64 * ms_per_sample);
            let chunk_end_ms = segment.end_timestamp_ms;
            result.push(crate::audio::vad::SpeechSegment {
                samples: chunk_samples,
                start_timestamp_ms: chunk_start_ms,
                end_timestamp_ms: chunk_end_ms,
                confidence: segment.confidence,
            });
            break;
        }

        // Target split point
        let target = pos + max_samples;

        // Search window: [target - SEARCH_RADIUS, target + SEARCH_RADIUS]
        let search_start = target.saturating_sub(SEARCH_RADIUS).max(pos + SAMPLE_RATE);
        let search_end = (target + SEARCH_RADIUS).min(total.saturating_sub(ENERGY_WINDOW));

        // Find the lowest-energy 100ms window in the search range
        let mut best_split = target.min(total); // fallback: exact target
        let mut best_rms = f32::MAX;

        if search_start + ENERGY_WINDOW <= search_end {
            let mut idx = search_start;
            while idx + ENERGY_WINDOW <= search_end {
                let window = &segment.samples[idx..idx + ENERGY_WINDOW];
                let rms = (window.iter().map(|s| s * s).sum::<f32>() / ENERGY_WINDOW as f32).sqrt();
                if rms < best_rms {
                    best_rms = rms;
                    best_split = idx + ENERGY_WINDOW / 2; // split at center of quiet window
                }
                // Step by 10ms (160 samples) for efficiency
                idx += SAMPLE_RATE / 100;
            }
        }

        let split_at = best_split;
        if best_rms <= SILENCE_RMS_THRESHOLD {
            debug!(
                "Splitting at silence boundary: sample {} (RMS={:.4})",
                split_at, best_rms
            );
        } else {
            debug!(
                "No silence found near target (best RMS={:.4}), splitting with overlap at sample {}",
                best_rms, split_at
            );
        }

        // Determine the actual end of this chunk (with overlap if no silence)
        let chunk_end = if best_rms > SILENCE_RMS_THRESHOLD {
            (split_at + FALLBACK_OVERLAP).min(total)
        } else {
            split_at
        };

        let chunk_samples = segment.samples[pos..chunk_end].to_vec();
        let chunk_start_ms = segment.start_timestamp_ms + (pos as f64 * ms_per_sample);
        let chunk_end_ms = segment.start_timestamp_ms + (chunk_end as f64 * ms_per_sample);

        result.push(crate::audio::vad::SpeechSegment {
            samples: chunk_samples,
            start_timestamp_ms: chunk_start_ms,
            end_timestamp_ms: chunk_end_ms,
            confidence: segment.confidence,
        });

        // Advance position to where the current chunk actually ends
        // to avoid transcribing the overlap region twice
        pos = chunk_end;
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_engine_lifecycle_lock_serializes_acquirers() {
        let guard = acquire_engine_lifecycle_lock().await;
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (acquired_tx, mut acquired_rx) = tokio::sync::oneshot::channel();
        let waiter = tokio::spawn(async {
            started_tx.send(()).unwrap();
            let _guard = acquire_engine_lifecycle_lock().await;
            acquired_tx.send(()).unwrap();
        });

        started_rx.await.unwrap();
        assert!(acquired_rx.try_recv().is_err());
        drop(guard);

        acquired_rx.await.unwrap();
        waiter.await.unwrap();
    }

    #[test]
    fn test_write_import_metadata_base_fields() {
        let dir = tempfile::tempdir().unwrap();

        let result = write_import_metadata(
            dir.path(),
            "meeting-123",
            "Test Meeting",
            1800.0,
            "audio.mp4",
            "import",
            None,
        );
        assert!(result.is_ok(), "write_import_metadata failed: {:?}", result);

        let path = dir.path().join("metadata.json");
        assert!(path.exists());

        let content = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["version"], "1.0");
        assert_eq!(parsed["meeting_id"], "meeting-123");
        assert_eq!(parsed["meeting_name"], "Test Meeting");
        assert_eq!(parsed["duration_seconds"], 1800.0);
        assert_eq!(parsed["audio_file"], "audio.mp4");
        assert_eq!(parsed["status"], "completed");
        assert_eq!(parsed["source"], "import");
    }

    #[test]
    fn test_write_import_metadata_with_extra_fields() {
        let dir = tempfile::tempdir().unwrap();

        let result = write_import_metadata(
            dir.path(),
            "meeting-456",
            "My Video",
            600.0,
            "audio.wav",
            "youtube",
            Some(serde_json::json!({
                "source_url": "https://youtu.be/abc123",
                "video_title": "My Video",
                "channel": "Some Channel",
            })),
        );
        assert!(result.is_ok(), "write_import_metadata failed: {:?}", result);

        let path = dir.path().join("metadata.json");
        let content = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();

        // Base fields still present
        assert_eq!(parsed["meeting_id"], "meeting-456");
        assert_eq!(parsed["status"], "completed");
        // Extra fields merged in
        assert_eq!(parsed["source"], "youtube");
        assert_eq!(parsed["source_url"], "https://youtu.be/abc123");
        assert_eq!(parsed["channel"], "Some Channel");
    }
}
