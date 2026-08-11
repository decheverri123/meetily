// Shared transcription pipeline for audio imports (local-file and YouTube).
//
// Both import sources acquire audio differently (copy a local file vs.
// download+extract via yt-dlp), but everything after that is identical:
// decode -> resample -> VAD -> segment-split -> transcribe -> persist to
// SQLite -> write transcripts.json. This module owns that shared portion so
// it isn't duplicated between `audio::import` and `audio::youtube_import`.

use crate::api::TranscriptSegment;
use crate::audio::decoder::decode_audio_file_with_progress;
use crate::audio::vad::get_speech_chunks_with_progress;
use crate::config::{DEFAULT_PARAKEET_MODEL, DEFAULT_WHISPER_MODEL};
use crate::parakeet_engine::ParakeetEngine;
use crate::state::AppState;
use crate::whisper_engine::WhisperEngine;
use anyhow::{anyhow, Result};
use log::{debug, info, warn};
use serde::Serialize;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, Runtime};
use uuid::Uuid;

use super::common::{create_transcript_segments, split_segment_at_silence, write_transcripts_json};

/// VAD redemption time in milliseconds - bridges natural pauses in speech.
/// Batch processing needs longer redemption (2000ms) than live pipeline (400ms)
/// because the entire file is processed at once by VAD, and 400ms fragments
/// speech at every natural sentence/topic pause (500ms-2s).
const VAD_REDEMPTION_TIME_MS: u32 = 2000;

/// Segments longer than this are split at silence boundaries before transcription,
/// since a hard cut at an arbitrary sample position loses words at the boundary.
const MAX_SEGMENT_SAMPLES: usize = 25 * 16000; // 25 seconds at 16kHz

/// Event names an import kind uses to report progress/warnings. Both events share the
/// exact payload shape of local-file import's `import-progress` / `import-warning`
/// events (`PipelineProgress` / `PipelineWarning` below), so the frontend can reuse one
/// renderer regardless of which event name it's listening for.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PipelineEvents {
    pub progress: &'static str,
    pub warning: &'static str,
}

/// Payload for a `*-progress` event.
#[derive(Debug, Clone, Serialize)]
struct PipelineProgress {
    stage: String,
    progress_percentage: u32,
    message: String,
}

/// Payload for a `*-warning` event.
#[derive(Debug, Clone, Serialize)]
struct PipelineWarning {
    warning: String,
    details: Option<String>,
}

/// Result of running the shared decode -> transcribe -> persist pipeline.
pub(crate) struct TranscriptionPipelineOutput {
    pub meeting_id: String,
    pub segments: Vec<TranscriptSegment>,
    pub duration_seconds: f64,
}

/// Emit a progress update on `event`.
pub(crate) fn emit_progress<R: Runtime>(
    app: &AppHandle<R>,
    event: &'static str,
    stage: &str,
    progress: u32,
    message: &str,
) {
    let _ = app.emit(
        event,
        PipelineProgress {
            stage: stage.to_string(),
            progress_percentage: progress,
            message: message.to_string(),
        },
    );
}

fn emit_warning<R: Runtime>(app: &AppHandle<R>, event: &'static str, warning: &str, details: Option<String>) {
    let _ = app.emit(
        event,
        PipelineWarning {
            warning: warning.to_string(),
            details,
        },
    );
}

/// Run the shared decode -> resample -> VAD -> segment-split -> transcribe -> persist
/// pipeline against an already-acquired local audio file.
///
/// `audio_path` must already exist on disk (a copied local file, or a completed yt-dlp
/// download). `cancelled` is checked between stages so the caller can cooperatively
/// cancel a running batch job; on cancellation, `meeting_folder` is removed. The caller
/// is responsible for writing its own `metadata.json` (fields differ per import source)
/// and emitting its own completion event.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_transcription_pipeline<R: Runtime>(
    app: &AppHandle<R>,
    meeting_folder: &Path,
    audio_path: &Path,
    title: &str,
    language: Option<String>,
    model: Option<String>,
    provider: Option<String>,
    events: PipelineEvents,
    cancelled: &'static AtomicBool,
) -> Result<TranscriptionPipelineOutput> {
    let use_parakeet = provider.as_deref() == Some("parakeet");

    emit_progress(app, events.progress, "decoding", 15, "Decoding audio file...");

    let app_for_decode = app.clone();
    let decode_progress_event = events.progress;
    let decode_progress = Box::new(move |progress: u32, msg: &str| {
        // Map decode progress: 15% + (progress * 0.05) to go from 15% to 20%
        let overall_progress = 15 + ((progress as f32 * 0.05) as u32);
        emit_progress(&app_for_decode, decode_progress_event, "decoding", overall_progress, msg);
    });

    let path_for_decode = audio_path.to_path_buf();
    let decoded = tokio::task::spawn_blocking(move || {
        decode_audio_file_with_progress(&path_for_decode, Some(decode_progress))
    })
    .await
    .map_err(|e| anyhow!("Decode task join error: {}", e))??;
    let duration_seconds = decoded.duration_seconds;

    info!(
        "Decoded audio: {:.2}s, {}Hz, {} channels",
        duration_seconds, decoded.sample_rate, decoded.channels
    );

    emit_progress(app, events.progress, "resampling", 20, "Converting audio format...");

    if cancelled.load(Ordering::SeqCst) {
        let _ = std::fs::remove_dir_all(meeting_folder);
        return Err(anyhow!("Import cancelled"));
    }

    // Convert to 16kHz mono format with progress updates
    let app_for_resample = app.clone();
    let resample_progress_event = events.progress;
    let resample_progress = Box::new(move |progress: u32, msg: &str| {
        // Map resample progress: 20% + (progress * 0.05) to go from 20% to 25%
        let overall_progress = 20 + ((progress as f32 * 0.05) as u32);
        emit_progress(&app_for_resample, resample_progress_event, "resampling", overall_progress, msg);
    });

    let audio_samples = tokio::task::spawn_blocking(move || {
        decoded.to_whisper_format_with_progress(Some(resample_progress))
    })
    .await
    .map_err(|e| anyhow!("Resample task join error: {}", e))?;
    info!("Converted to 16kHz mono format: {} samples", audio_samples.len());

    emit_progress(app, events.progress, "vad", 25, "Detecting speech segments...");

    if cancelled.load(Ordering::SeqCst) {
        let _ = std::fs::remove_dir_all(meeting_folder);
        return Err(anyhow!("Import cancelled"));
    }

    let app_for_vad = app.clone();
    let vad_progress_event = events.progress;
    let speech_segments = tokio::task::spawn_blocking(move || {
        get_speech_chunks_with_progress(
            &audio_samples,
            VAD_REDEMPTION_TIME_MS,
            |vad_progress, segments_found| {
                let overall_progress = 25 + (vad_progress as f32 * 0.05) as u32;
                emit_progress(
                    &app_for_vad,
                    vad_progress_event,
                    "vad",
                    overall_progress,
                    &format!(
                        "Detecting speech segments... {}% ({} found)",
                        vad_progress, segments_found
                    ),
                );
                !cancelled.load(Ordering::SeqCst)
            },
        )
    })
    .await
    .map_err(|e| anyhow!("VAD task panicked: {}", e))?
    .map_err(|e| anyhow!("VAD processing failed: {}", e))?;

    let total_segments = speech_segments.len();
    info!("VAD detected {} speech segments (redemption_time={}ms)", total_segments, VAD_REDEMPTION_TIME_MS);

    // Diagnostic: log segment duration distribution
    if !speech_segments.is_empty() {
        let durations_ms: Vec<f64> = speech_segments.iter()
            .map(|s| s.end_timestamp_ms - s.start_timestamp_ms)
            .collect();
        let total_speech_ms: f64 = durations_ms.iter().sum();
        let avg_duration = total_speech_ms / durations_ms.len() as f64;
        let min_duration = durations_ms.iter().cloned().fold(f64::INFINITY, f64::min);
        let max_duration = durations_ms.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        info!(
            "VAD segment stats: avg={:.0}ms, min={:.0}ms, max={:.0}ms, total_speech={:.1}s/{:.1}s ({:.0}%)",
            avg_duration, min_duration, max_duration,
            total_speech_ms / 1000.0, duration_seconds,
            (total_speech_ms / 1000.0 / duration_seconds) * 100.0
        );
        for (i, seg) in speech_segments.iter().take(10).enumerate() {
            let dur = seg.end_timestamp_ms - seg.start_timestamp_ms;
            debug!("  Segment {}: {:.0}ms-{:.0}ms ({:.0}ms, {} samples)",
                i, seg.start_timestamp_ms, seg.end_timestamp_ms, dur, seg.samples.len());
        }
        if total_segments > 10 {
            debug!("  ... and {} more segments", total_segments - 10);
        }
    }

    if total_segments == 0 {
        warn!("No speech detected in audio");
        emit_warning(
            app,
            events.warning,
            "No speech detected in audio file",
            Some(
                "The file was imported successfully, but VAD did not detect any speech. \
                 The meeting was created but contains no transcripts.".to_string()
            ),
        );
        // Still create the meeting, just with no transcripts
    }

    if cancelled.load(Ordering::SeqCst) {
        let _ = std::fs::remove_dir_all(meeting_folder);
        return Err(anyhow!("Import cancelled"));
    }

    emit_progress(app, events.progress, "transcribing", 30, "Loading transcription engine...");

    let whisper_engine = if !use_parakeet && total_segments > 0 {
        Some(get_or_init_whisper(app, model.as_deref()).await?)
    } else {
        None
    };
    let parakeet_engine = if use_parakeet && total_segments > 0 {
        Some(get_or_init_parakeet(app, model.as_deref()).await?)
    } else {
        None
    };

    let mut processable_segments: Vec<crate::audio::vad::SpeechSegment> = Vec::new();
    for segment in &speech_segments {
        if segment.samples.len() > MAX_SEGMENT_SAMPLES {
            debug!(
                "Splitting large segment ({:.0}ms, {} samples) at silence boundaries",
                segment.end_timestamp_ms - segment.start_timestamp_ms,
                segment.samples.len()
            );
            let sub_segments = split_segment_at_silence(segment, MAX_SEGMENT_SAMPLES);
            debug!("Split into {} sub-segments", sub_segments.len());
            processable_segments.extend(sub_segments);
        } else {
            processable_segments.push(segment.clone());
        }
    }

    let processable_count = processable_segments.len();
    info!("Processing {} segments (after splitting)", processable_count);

    let mut all_transcripts: Vec<(String, f64, f64)> = Vec::new();
    let mut total_confidence = 0.0f32;

    for (i, segment) in processable_segments.iter().enumerate() {
        if cancelled.load(Ordering::SeqCst) {
            let _ = std::fs::remove_dir_all(meeting_folder);
            return Err(anyhow!("Import cancelled"));
        }

        let progress = 30 + ((i as f32 / processable_count.max(1) as f32) * 50.0) as u32;
        let segment_duration_sec = (segment.end_timestamp_ms - segment.start_timestamp_ms) / 1000.0;
        emit_progress(
            app,
            events.progress,
            "transcribing",
            progress,
            &format!(
                "Transcribing segment {} of {} ({:.1}s)...",
                i + 1, processable_count, segment_duration_sec
            ),
        );

        // Skip very short segments
        if segment.samples.len() < 1600 {
            debug!("Skipping short segment {} with {} samples", i, segment.samples.len());
            continue;
        }

        // Transcribe
        let (text, conf) = if use_parakeet {
            let engine = parakeet_engine
                .as_ref()
                .ok_or_else(|| anyhow!("Parakeet engine not initialized"))?;
            let text = engine
                .transcribe_audio(segment.samples.clone())
                .await
                .map_err(|e| anyhow!("Parakeet transcription failed on segment {}: {}", i, e))?;
            (text, 0.9f32)
        } else {
            let engine = whisper_engine
                .as_ref()
                .ok_or_else(|| anyhow!("Whisper engine not initialized"))?;
            let (text, conf, _) = engine
                .transcribe_audio_with_confidence(segment.samples.clone(), language.clone())
                .await
                .map_err(|e| anyhow!("Whisper transcription failed on segment {}: {}", i, e))?;
            (text, conf)
        };

        let trimmed = text.trim();
        if !trimmed.is_empty() {
            debug!(
                "Segment {}/{}: {:.1}s, conf={:.2}, text='{}'",
                i + 1, processable_count, segment_duration_sec, conf,
                if trimmed.len() > 80 { let mut end = 80; while !trimmed.is_char_boundary(end) { end -= 1; } &trimmed[..end] } else { trimmed }
            );
            all_transcripts.push((text, segment.start_timestamp_ms, segment.end_timestamp_ms));
            total_confidence += conf;
        } else {
            debug!("Segment {}/{}: {:.1}s — empty transcription", i + 1, processable_count, segment_duration_sec);
        }
    }

    let transcribed_count = all_transcripts.len();
    let avg_confidence = if transcribed_count > 0 {
        total_confidence / transcribed_count as f32
    } else {
        0.0
    };
    info!(
        "Transcription complete: {} segments transcribed out of {}, avg confidence: {:.2}",
        transcribed_count, processable_count, avg_confidence
    );

    if cancelled.load(Ordering::SeqCst) {
        let _ = std::fs::remove_dir_all(meeting_folder);
        return Err(anyhow!("Import cancelled"));
    }

    emit_progress(app, events.progress, "saving", 85, "Creating meeting...");

    let segments = create_transcript_segments(&all_transcripts);

    let app_state = app
        .try_state::<AppState>()
        .ok_or_else(|| anyhow!("App state not available"))?;

    let meeting_id = create_meeting_with_transcripts(
        app_state.db_manager.pool(),
        title,
        &segments,
        meeting_folder.to_string_lossy().to_string(),
    )
    .await?;

    emit_progress(app, events.progress, "saving", 90, "Writing transcript files...");
    if let Err(e) = write_transcripts_json(meeting_folder, &segments) {
        warn!("Failed to write transcripts.json: {}", e);
    }

    Ok(TranscriptionPipelineOutput {
        meeting_id,
        segments,
        duration_seconds,
    })
}

/// Create a new meeting with transcripts in the database.
async fn create_meeting_with_transcripts(
    pool: &sqlx::SqlitePool,
    title: &str,
    segments: &[TranscriptSegment],
    folder_path: String,
) -> Result<String> {
    let meeting_id = format!("meeting-{}", Uuid::new_v4());
    let now = chrono::Utc::now();

    let mut conn = pool.acquire().await.map_err(|e| anyhow!("DB error: {}", e))?;
    let mut tx = sqlx::Connection::begin(&mut *conn)
        .await
        .map_err(|e| anyhow!("Failed to start transaction: {}", e))?;

    sqlx::query(
        "INSERT INTO meetings (id, title, created_at, updated_at, folder_path)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&meeting_id)
    .bind(title)
    .bind(now)
    .bind(now)
    .bind(&folder_path)
    .execute(&mut *tx)
    .await
    .map_err(|e| anyhow!("Failed to create meeting: {}", e))?;

    for segment in segments {
        sqlx::query(
            "INSERT INTO transcripts (id, meeting_id, transcript, timestamp, audio_start_time, audio_end_time, duration)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&segment.id)
        .bind(&meeting_id)
        .bind(&segment.text)
        .bind(&segment.timestamp)
        .bind(segment.audio_start_time)
        .bind(segment.audio_end_time)
        .bind(segment.duration)
        .execute(&mut *tx)
        .await
        .map_err(|e| anyhow!("Failed to insert transcript: {}", e))?;
    }

    tx.commit()
        .await
        .map_err(|e| anyhow!("Failed to commit transaction: {}", e))?;

    info!("Created meeting '{}' with {} transcripts", meeting_id, segments.len());

    Ok(meeting_id)
}

/// Get or initialize the Whisper engine.
async fn get_or_init_whisper<R: Runtime>(
    app: &AppHandle<R>,
    requested_model: Option<&str>,
) -> Result<Arc<WhisperEngine>> {
    use crate::whisper_engine::commands::WHISPER_ENGINE;

    let engine = {
        let guard = WHISPER_ENGINE.lock().unwrap_or_else(|e| e.into_inner());
        guard.as_ref().cloned()
    };

    match engine {
        Some(e) => {
            let target_model = match requested_model {
                Some(model) => model.to_string(),
                None => get_configured_model(app, "whisper").await?,
            };

            let current_model = e.get_current_model().await;
            let needs_load = match &current_model {
                Some(loaded) => loaded != &target_model,
                None => true,
            };

            if needs_load {
                info!("Loading Whisper model '{}' (current: {:?})", target_model, current_model);
                if let Err(e) = e.discover_models().await {
                    warn!("Model discovery error (continuing): {}", e);
                }
                e.load_model(&target_model)
                    .await
                    .map_err(|e| anyhow!("Failed to load model '{}': {}", target_model, e))?;
            }

            Ok(e)
        }
        None => Err(anyhow!("Whisper engine not initialized")),
    }
}

/// Get or initialize the Parakeet engine.
async fn get_or_init_parakeet<R: Runtime>(
    app: &AppHandle<R>,
    requested_model: Option<&str>,
) -> Result<Arc<ParakeetEngine>> {
    use crate::parakeet_engine::commands::PARAKEET_ENGINE;

    let engine = {
        let guard = PARAKEET_ENGINE.lock().unwrap_or_else(|e| e.into_inner());
        guard.as_ref().cloned()
    };

    match engine {
        Some(e) => {
            let target_model = match requested_model {
                Some(model) => model.to_string(),
                None => get_configured_model(app, "parakeet").await?,
            };

            let current_model = e.get_current_model().await;
            let needs_load = match &current_model {
                Some(loaded) => loaded != &target_model,
                None => true,
            };

            if needs_load {
                info!("Loading Parakeet model '{}' (current: {:?})", target_model, current_model);
                if let Err(e) = e.discover_models().await {
                    warn!("Model discovery error (continuing): {}", e);
                }
                e.load_model(&target_model)
                    .await
                    .map_err(|e| anyhow!("Failed to load model '{}': {}", target_model, e))?;
            }

            Ok(e)
        }
        None => Err(anyhow!("Parakeet engine not initialized")),
    }
}

/// Fetch the raw (provider, model) pair from `transcript_settings`, if configured.
async fn fetch_transcript_settings<R: Runtime>(app: &AppHandle<R>) -> Result<Option<(String, String)>> {
    let app_state = app
        .try_state::<AppState>()
        .ok_or_else(|| anyhow!("App state not available"))?;

    sqlx::query_as("SELECT provider, model FROM transcript_settings WHERE id = '1'")
        .fetch_optional(app_state.db_manager.pool())
        .await
        .map_err(|e| anyhow!("Failed to query config: {}", e))
}

/// Get the configured model for a provider type ("whisper" or "parakeet") from the
/// database, falling back to that type's default model if unset or mismatched.
async fn get_configured_model<R: Runtime>(app: &AppHandle<R>, provider_type: &str) -> Result<String> {
    let default_model = if provider_type == "parakeet" {
        DEFAULT_PARAKEET_MODEL
    } else {
        DEFAULT_WHISPER_MODEL
    };

    Ok(match fetch_transcript_settings(app).await? {
        Some((provider, model))
            if (provider_type == "whisper" && (provider == "localWhisper" || provider == "whisper"))
                || (provider_type == "parakeet" && provider == "parakeet") =>
        {
            model
        }
        _ => default_model.to_string(),
    })
}

/// Get the DB-configured transcription provider kind ("parakeet" or "whisper"),
/// defaulting to "whisper" when unset. Import flows that don't take an explicit
/// `provider` argument (currently: YouTube import) use this to decide which engine
/// getter to call before running the shared pipeline.
pub(crate) async fn get_configured_provider<R: Runtime>(app: &AppHandle<R>) -> Result<String> {
    Ok(match fetch_transcript_settings(app).await? {
        Some((provider, _)) if provider == "parakeet" => "parakeet".to_string(),
        _ => "whisper".to_string(),
    })
}
