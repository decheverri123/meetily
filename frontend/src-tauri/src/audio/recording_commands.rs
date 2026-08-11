// audio/recording_commands.rs
//
// Slim Tauri command layer for recording functionality.
// Delegates to transcription and recording modules for actual implementation.

use anyhow::Result;
use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::{
    parse_audio_device,
    default_input_device,   // Get default microphone
    default_output_device,  // Get default system audio
    RecordingManager,
    DeviceEvent,
    DeviceMonitorType
};

// Import transcription modules
use super::transcription::{
    self,
    reset_speech_detected_flag,
};

// Re-export TranscriptUpdate for backward compatibility
pub use super::transcription::TranscriptUpdate;

// Used by the live insights / action chips features below (kept as a single
// hoisted import rather than inline fully-qualified paths mid-function).
use crate::summary::summary_engine::{
    builtin_ai_get_available_summary_model, builtin_ai_is_model_ready, generate_with_builtin,
    ModelManagerState,
};
use crate::summary::llm_client::{generate_summary, provider_name, LLMProvider};

// ============================================================================
// GLOBAL STATE
// ============================================================================

// Simple recording state tracking
static IS_RECORDING: AtomicBool = AtomicBool::new(false);

// Global recording manager and transcription task to keep them alive during recording
static RECORDING_MANAGER: Mutex<Option<RecordingManager>> = Mutex::new(None);
static TRANSCRIPTION_TASK: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);

// Listener ID for proper cleanup - prevents microphone from staying active after recording stops
static TRANSCRIPT_LISTENER_ID: Mutex<Option<tauri::EventId>> = Mutex::new(None);

// Guards concurrent calls to `generate_live_insights` AND `generate_live_action_chip`
// - both ultimately drive the same single-flight LLM call, so they share this
// one flag (see `LiveInsightsGuard`'s doc comment). `generate_live_insights` is
// polled periodically by the frontend and wants to skip a tick rather than
// queue up work if a previous generation is still running; chips reuse the
// same guard so a click can't race a poll (or another chip click) on it.
static LIVE_INSIGHTS_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

// Timestamp of the last accepted `generate_live_insights` call, used for the
// backend-enforced minimum-interval check below. `generate_live_action_chip`
// rate-limits independently via its own per-kind statics
// (`LIVE_ACTION_CHIP_RECAP_LAST_CALL` / `LIVE_ACTION_CHIP_QUESTIONS_LAST_CALL`).
static LIVE_INSIGHTS_LAST_CALL: Mutex<Option<std::time::Instant>> = Mutex::new(None);

// Cache of the resolved builtin summary model name, shared by the BuiltInAI
// route of `generate_bounded_live_llm_text` for both `generate_live_insights`
// and `generate_live_action_chip` (see `LIVE_INSIGHTS_MODEL_CACHE_TTL`).
static LIVE_INSIGHTS_MODEL_CACHE: Mutex<Option<(Option<String>, std::time::Instant)>> =
    Mutex::new(None);

// ============================================================================
// PUBLIC TYPES
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct RecordingArgs {
    pub save_path: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct TranscriptionStatus {
    pub chunks_in_queue: usize,
    pub is_processing: bool,
    pub last_activity_ms: u64,
}

// ============================================================================
// RECORDING COMMANDS
// ============================================================================

/// Start recording with default devices
pub async fn start_recording<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    start_recording_with_meeting_name(app, None).await
}

/// Start recording with default devices and optional meeting name
pub async fn start_recording_with_meeting_name<R: Runtime>(
    app: AppHandle<R>,
    meeting_name: Option<String>,
) -> Result<(), String> {
    info!(
        "Starting recording with default devices, meeting: {:?}",
        meeting_name
    );

    let engine_lifecycle_guard = super::common::acquire_engine_lifecycle_lock().await;

    // Check if already recording
    let current_recording_state = IS_RECORDING.load(Ordering::SeqCst);
    info!("🔍 IS_RECORDING state check: {}", current_recording_state);
    if current_recording_state {
        return Err("Recording already in progress".to_string());
    }

    // Validate that transcription models are available before starting recording
    info!("🔍 Validating transcription model availability before starting recording...");
    if let Err(validation_error) = transcription::validate_transcription_model_ready(&app).await {
        error!("Model validation failed: {}", validation_error);

        // Emit error event for frontend - actionable: false to show toast instead of modal
        // (download progress is already shown in top-right toast)
        let _ = app.emit("transcription-error", serde_json::json!({
            "error": validation_error,
            "userMessage": "Recording cannot start: Transcription model is still downloading. Please wait for the download to complete.",
            "actionable": false
        }));

        return Err(validation_error);
    }
    info!("✅ Transcription model validation passed");

    // Async-first approach - no more blocking operations!
    info!("🚀 Starting async recording initialization");

    // Create new recording manager
    let mut manager = RecordingManager::new();

    // Load recording preferences to get auto_save AND device preferences
    let (auto_save, preferred_mic_name, preferred_system_name) =
        match super::recording_preferences::load_recording_preferences(&app).await {
            Ok(prefs) => {
                info!("📋 Loaded recording preferences: auto_save={}, preferred_mic={:?}, preferred_system={:?}",
                      prefs.auto_save, prefs.preferred_mic_device, prefs.preferred_system_device);
                (prefs.auto_save, prefs.preferred_mic_device, prefs.preferred_system_device)
            }
            Err(e) => {
                warn!("Failed to load recording preferences, using defaults: {}", e);
                (true, None, None)
            }
        };

    // ============================================================================
    // MICROPHONE DEVICE RESOLUTION: Preference → Default → Error
    // ============================================================================
    let microphone_device = match preferred_mic_name {
        Some(pref_name) => {
            info!("🎤 Attempting to use preferred microphone: '{}'", pref_name);
            match parse_audio_device(&pref_name) {
                Ok(device) => {
                    info!("✅ Using preferred microphone: '{}'", device.name);
                    Some(Arc::new(device))
                }
                Err(e) => {
                    warn!("⚠️ Preferred microphone '{}' not available: {}", pref_name, e);
                    warn!("   Falling back to system default microphone...");
                    match default_input_device() {
                        Ok(device) => {
                            info!("✅ Using default microphone: '{}'", device.name);
                            Some(Arc::new(device))
                        }
                        Err(default_err) => {
                            error!("❌ No microphone available (preferred and default both failed)");
                            return Err(format!(
                                "No microphone device available. Preferred device '{}' not found, and default microphone unavailable: {}",
                                pref_name, default_err
                            ));
                        }
                    }
                }
            }
        }
        None => {
            info!("🎤 No microphone preference set, using system default");
            match default_input_device() {
                Ok(device) => {
                    info!("✅ Using default microphone: '{}'", device.name);
                    Some(Arc::new(device))
                }
                Err(e) => {
                    error!("❌ No default microphone available");
                    return Err(format!("No microphone device available: {}", e));
                }
            }
        }
    };

    // ============================================================================
    // SYSTEM AUDIO DEVICE RESOLUTION: Preference → Default → None (optional)
    // ============================================================================
    let system_device = match preferred_system_name {
        Some(pref_name) => {
            info!("🔊 Attempting to use preferred system audio: '{}'", pref_name);
            match parse_audio_device(&pref_name) {
                Ok(device) => {
                    info!("✅ Using preferred system audio: '{}'", device.name);
                    Some(Arc::new(device))
                }
                Err(e) => {
                    warn!("⚠️ Preferred system audio '{}' not available: {}", pref_name, e);
                    warn!("   Falling back to system default...");
                    match default_output_device() {
                        Ok(device) => {
                            info!("✅ Using default system audio: '{}'", device.name);
                            Some(Arc::new(device))
                        }
                        Err(default_err) => {
                            warn!("⚠️ No system audio available (preferred and default both failed): {}", default_err);
                            warn!("   Recording will continue with microphone only");
                            None // System audio is optional
                        }
                    }
                }
            }
        }
        None => {
            info!("🔊 No system audio preference set, using system default");
            match default_output_device() {
                Ok(device) => {
                    info!("✅ Using default system audio: '{}'", device.name);
                    Some(Arc::new(device))
                }
                Err(e) => {
                    warn!("⚠️ No default system audio available: {}", e);
                    warn!("   Recording will continue with microphone only");
                    None // System audio is optional
                }
            }
        }
    };

    // Always ensure a meeting name is set so incremental saver initializes
    let effective_meeting_name = meeting_name.clone().unwrap_or_else(|| {
        // Example: Meeting 2025-10-03_08-25-23
        let now = chrono::Local::now();
        format!(
            "Meeting {}",
            now.format("%Y-%m-%d_%H-%M-%S")
        )
    });
    manager.set_meeting_name(Some(effective_meeting_name));

    // Set up error callback
    let app_for_error = app.clone();
    manager.set_error_callback(move |error| {
        let _ = app_for_error.emit("recording-error", error.user_message());
    });

    // Start recording with resolved devices (replaces start_recording_with_defaults_and_auto_save call)
    let transcription_receiver = manager
        .start_recording(microphone_device, system_device, auto_save)
        .await
        .map_err(|e| format!("Failed to start recording: {}", e))?;

    // Store the manager globally to keep it alive
    {
        let mut global_manager = RECORDING_MANAGER.lock().unwrap();
        *global_manager = Some(manager);
    }

    // Set recording flag and reset speech detection flag
    info!("🔍 Setting IS_RECORDING to true and resetting SPEECH_DETECTED_EMITTED");
    IS_RECORDING.store(true, Ordering::SeqCst);
    drop(engine_lifecycle_guard);
    reset_speech_detected_flag(); // Reset for new recording session

    // Start optimized parallel transcription task and store handle
    let task_handle = transcription::start_transcription_task(app.clone(), transcription_receiver);
    {
        let mut global_task = TRANSCRIPTION_TASK.lock().unwrap();
        *global_task = Some(task_handle);
    }

    // CRITICAL: Listen for transcript-update events and save to recording manager
    // This enables transcript history persistence for page reload sync
    // Store listener ID for cleanup during stop_recording to ensure microphone is released
    {
        use tauri::Listener;
        let listener_id = app.listen("transcript-update", move |event: tauri::Event| {
            // Parse the transcript update from the event payload
            if let Ok(update) = serde_json::from_str::<TranscriptUpdate>(event.payload()) {
                // Create structured transcript segment
                let segment = crate::audio::recording_saver::TranscriptSegment {
                    id: format!("seg_{}", update.sequence_id),
                    text: update.text.clone(),
                    audio_start_time: update.audio_start_time,
                    audio_end_time: update.audio_end_time,
                    duration: update.duration,
                    display_time: update.timestamp.clone(), // Use wall-clock timestamp for display
                    confidence: update.confidence,
                    sequence_id: update.sequence_id,
                };

                // Save to recording manager
                if let Ok(manager_guard) = RECORDING_MANAGER.lock() {
                    if let Some(manager) = manager_guard.as_ref() {
                        manager.add_transcript_segment(segment);
                    }
                }
            }
        });
        let mut global_listener = TRANSCRIPT_LISTENER_ID.lock().unwrap();
        *global_listener = Some(listener_id);
        info!("✅ Transcript-update event listener registered for history persistence");
    }

    // Emit success event
    app.emit("recording-started", serde_json::json!({
        "message": "Recording started successfully with parallel processing",
        "devices": ["Default Microphone", "Default System Audio"],
        "workers": 3
    })).map_err(|e| e.to_string())?;

    // Update tray menu to reflect recording state
    crate::tray::update_tray_menu(&app);

    info!("✅ Recording started successfully with async-first approach");

    Ok(())
}

/// Start recording with specific devices
pub async fn start_recording_with_devices<R: Runtime>(
    app: AppHandle<R>,
    mic_device_name: Option<String>,
    system_device_name: Option<String>,
) -> Result<(), String> {
    start_recording_with_devices_and_meeting(app, mic_device_name, system_device_name, None).await
}

/// Start recording with specific devices and optional meeting name
pub async fn start_recording_with_devices_and_meeting<R: Runtime>(
    app: AppHandle<R>,
    mic_device_name: Option<String>,
    system_device_name: Option<String>,
    meeting_name: Option<String>,
) -> Result<(), String> {
    info!(
        "Starting recording with specific devices: mic={:?}, system={:?}, meeting={:?}",
        mic_device_name, system_device_name, meeting_name
    );

    let engine_lifecycle_guard = super::common::acquire_engine_lifecycle_lock().await;

    // Check if already recording
    let current_recording_state = IS_RECORDING.load(Ordering::SeqCst);
    info!("🔍 IS_RECORDING state check: {}", current_recording_state);
    if current_recording_state {
        return Err("Recording already in progress".to_string());
    }

    // Validate that transcription models are available before starting recording
    info!("🔍 Validating transcription model availability before starting recording...");
    if let Err(validation_error) = transcription::validate_transcription_model_ready(&app).await {
        error!("Model validation failed: {}", validation_error);

        // Emit error event for frontend - actionable: false to show toast instead of modal
        // (download progress is already shown in top-right toast)
        let _ = app.emit("transcription-error", serde_json::json!({
            "error": validation_error,
            "userMessage": "Recording cannot start: Transcription model is still downloading. Please wait for the download to complete.",
            "actionable": false
        }));

        return Err(validation_error);
    }
    info!("✅ Transcription model validation passed");

    // Parse devices
    let mic_device = if let Some(ref name) = mic_device_name {
        Some(Arc::new(parse_audio_device(name).map_err(|e| {
            format!("Invalid microphone device '{}': {}", name, e)
        })?))
    } else {
        None
    };

    let system_device = if let Some(ref name) = system_device_name {
        Some(Arc::new(parse_audio_device(name).map_err(|e| {
            format!("Invalid system device '{}': {}", name, e)
        })?))
    } else {
        None
    };

    // Async-first approach for custom devices - no more blocking operations!
    info!("🚀 Starting async recording initialization with custom devices");

    // Create new recording manager
    let mut manager = RecordingManager::new();

    // Load recording preferences to check auto_save setting
    let auto_save = match super::recording_preferences::load_recording_preferences(&app).await {
        Ok(prefs) => {
            info!("📋 Loaded recording preferences: auto_save={}", prefs.auto_save);
            prefs.auto_save
        }
        Err(e) => {
            warn!("Failed to load recording preferences, defaulting to auto_save=true: {}", e);
            true // Default to saving if preferences can't be loaded
        }
    };

    // Always ensure a meeting name is set so incremental saver initializes
    let effective_meeting_name = meeting_name.clone().unwrap_or_else(|| {
        let now = chrono::Local::now();
        format!(
            "Meeting {}",
            now.format("%Y-%m-%d_%H-%M-%S")
        )
    });
    manager.set_meeting_name(Some(effective_meeting_name));

    // Set up error callback
    let app_for_error = app.clone();
    manager.set_error_callback(move |error| {
        let _ = app_for_error.emit("recording-error", error.user_message());
    });

    // Start recording with specified devices and auto_save setting
    let transcription_receiver = manager
        .start_recording(mic_device, system_device, auto_save)
        .await
        .map_err(|e| format!("Failed to start recording: {}", e))?;

    // Store the manager globally to keep it alive
    {
        let mut global_manager = RECORDING_MANAGER.lock().unwrap();
        *global_manager = Some(manager);
    }

    // Set recording flag and reset speech detection flag
    info!("🔍 Setting IS_RECORDING to true and resetting SPEECH_DETECTED_EMITTED");
    IS_RECORDING.store(true, Ordering::SeqCst);
    drop(engine_lifecycle_guard);
    reset_speech_detected_flag(); // Reset for new recording session

    // Start optimized parallel transcription task and store handle
    let task_handle = transcription::start_transcription_task(app.clone(), transcription_receiver);
    {
        let mut global_task = TRANSCRIPTION_TASK.lock().unwrap();
        *global_task = Some(task_handle);
    }

    // CRITICAL: Listen for transcript-update events and save to recording manager
    // This enables transcript history persistence for page reload sync
    // Store listener ID for cleanup during stop_recording to ensure microphone is released
    {
        use tauri::Listener;
        let listener_id = app.listen("transcript-update", move |event: tauri::Event| {
            // Parse the transcript update from the event payload
            if let Ok(update) = serde_json::from_str::<TranscriptUpdate>(event.payload()) {
                // Create structured transcript segment
                let segment = crate::audio::recording_saver::TranscriptSegment {
                    id: format!("seg_{}", update.sequence_id),
                    text: update.text.clone(),
                    audio_start_time: update.audio_start_time,
                    audio_end_time: update.audio_end_time,
                    duration: update.duration,
                    display_time: update.timestamp.clone(), // Use wall-clock timestamp for display
                    confidence: update.confidence,
                    sequence_id: update.sequence_id,
                };

                // Save to recording manager
                if let Ok(manager_guard) = RECORDING_MANAGER.lock() {
                    if let Some(manager) = manager_guard.as_ref() {
                        manager.add_transcript_segment(segment);
                    }
                }
            }
        });
        let mut global_listener = TRANSCRIPT_LISTENER_ID.lock().unwrap();
        *global_listener = Some(listener_id);
        info!("✅ Transcript-update event listener registered for history persistence");
    }

    // Emit success event
    app.emit("recording-started", serde_json::json!({
        "message": "Recording started with custom devices and parallel processing",
        "devices": [
            mic_device_name.unwrap_or_else(|| "Default Microphone".to_string()),
            system_device_name.unwrap_or_else(|| "Default System Audio".to_string())
        ],
        "workers": 3
    })).map_err(|e| e.to_string())?;

    // Update tray menu to reflect recording state
    crate::tray::update_tray_menu(&app);

    info!("✅ Recording started with custom devices using async-first approach");

    Ok(())
}

/// Stop recording with optimized graceful shutdown ensuring NO transcript chunks are lost
pub async fn stop_recording<R: Runtime>(
    app: AppHandle<R>,
    _args: RecordingArgs,
) -> Result<(), String> {
    info!(
        "🛑 Starting optimized recording shutdown - ensuring ALL transcript chunks are preserved"
    );

    // Check if recording is active
    if !IS_RECORDING.load(Ordering::SeqCst) {
        info!("Recording was not active");
        return Ok(());
    }

    // Emit shutdown progress to frontend
    let _ = app.emit(
        "recording-shutdown-progress",
        serde_json::json!({
            "stage": "stopping_audio",
            "message": "Stopping audio capture...",
            "progress": 20
        }),
    );

    // Step 1: Stop audio capture immediately (no more new chunks) with proper error handling
    let manager_for_cleanup = {
        let mut global_manager = RECORDING_MANAGER.lock().unwrap();
        global_manager.take()
    };

    let stop_result = if let Some(mut manager) = manager_for_cleanup {
        // Use FORCE FLUSH to immediately process all accumulated audio - eliminates 30s delay!
        info!("🚀 Using FORCE FLUSH to eliminate pipeline accumulation delays");
        let result = manager.stop_streams_and_force_flush().await;
        // Store manager back for later cleanup
        let manager_for_cleanup = Some(manager);
        (result, manager_for_cleanup)
    } else {
        warn!("No recording manager found to stop");
        (Ok(()), None)
    };

    let (stop_result, manager_for_cleanup) = stop_result;

    match stop_result {
        Ok(_) => {
            info!("✅ Audio streams stopped successfully - no more chunks will be created");
        }
        Err(e) => {
            error!("❌ Failed to stop audio streams: {}", e);
            return Err(format!("Failed to stop audio streams: {}", e));
        }
    }

    // Step 1.5: Clean up transcript listener to release microphone
    // Unlisten transcript-update event to prevent lingering references
    {
        use tauri::Listener;
        if let Some(listener_id) = TRANSCRIPT_LISTENER_ID.lock().unwrap().take() {
            app.unlisten(listener_id);
            info!("✅ Transcript-update listener removed");
        }
    }

    // Step 2: Signal transcription workers to finish processing ALL queued chunks
    let _ = app.emit(
        "recording-shutdown-progress",
        serde_json::json!({
            "stage": "processing_transcripts",
            "message": "Processing remaining transcript chunks...",
            "progress": 40
        }),
    );

    // Wait for transcription task with enhanced progress monitoring (NO TIMEOUT - we must process all chunks)
    let transcription_task = {
        let mut global_task = TRANSCRIPTION_TASK.lock().unwrap();
        global_task.take()
    };

    if let Some(task_handle) = transcription_task {
        info!("⏳ Waiting for ALL transcription chunks to be processed (no timeout - preserving every chunk)");

        // Enhanced progress monitoring during shutdown
        let progress_app = app.clone();
        let progress_task = tokio::spawn(async move {
            let last_update = std::time::Instant::now();

            loop {
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

                // Emit periodic progress updates during shutdown
                let elapsed = last_update.elapsed().as_secs();
                let _ = progress_app.emit(
                    "recording-shutdown-progress",
                    serde_json::json!({
                        "stage": "processing_transcripts",
                        "message": format!("Processing transcripts... ({}s elapsed)", elapsed),
                        "progress": 40,
                        "detailed": true,
                        "elapsed_seconds": elapsed
                    }),
                );
            }
        });

        // Wait up to 10 minutes for transcription completion to prevent indefinite hangs
        match tokio::time::timeout(
            tokio::time::Duration::from_secs(600), // 10 minutes max
            task_handle
        ).await {
            Ok(Ok(())) => {
                info!("✅ ALL transcription chunks processed successfully - no data lost");
            }
            Ok(Err(e)) => {
                warn!("⚠️ Transcription task completed with error: {:?}", e);
                // Continue anyway - the worker may have processed most chunks
            }
            Err(_) => {
                warn!("⏱️ Transcription timeout (10 minutes) reached, continuing shutdown to prevent indefinite hang");
                // Continue shutdown even on timeout - better to lose some chunks than hang forever
            }
        }

        // Stop progress monitoring
        progress_task.abort();
    } else {
        info!("ℹ️ No transcription task found to wait for");
    }

    // Step 3: Now safely unload Whisper model after ALL chunks are processed
    let _ = app.emit(
        "recording-shutdown-progress",
        serde_json::json!({
            "stage": "unloading_model",
            "message": "Unloading speech recognition model...",
            "progress": 70
        }),
    );

    info!("🧠 All transcript chunks processed. Now safely unloading transcription model...");

    // Determine which provider was used and unload the appropriate model (with timeout)
    let config = match tokio::time::timeout(
        tokio::time::Duration::from_secs(30), // 30 seconds max for DB operation
        crate::api::api::api_get_transcript_config(
            app.clone(),
            app.clone().state(),
            None,
        )
    )
    .await
    {
        Ok(Ok(Some(config))) => Some(config.provider),
        Ok(Ok(None)) => None,
        Ok(Err(e)) => {
            warn!("⚠️ Failed to get transcript config: {:?}", e);
            None
        }
        Err(_) => {
            warn!("⏱️ Transcript config timeout (30s), continuing shutdown");
            None
        }
    };

    match config.as_deref() {
        Some("parakeet") => {
            info!("🦜 Unloading Parakeet model...");
            let engine_clone = {
                let engine_guard = crate::parakeet_engine::commands::PARAKEET_ENGINE
                    .lock()
                    .unwrap();
                engine_guard.as_ref().cloned()
            };

            if let Some(engine) = engine_clone {
                let current_model = engine
                    .get_current_model()
                    .await
                    .unwrap_or_else(|| "unknown".to_string());
                info!("Current Parakeet model before unload: '{}'", current_model);

                if engine.unload_model().await {
                    info!("✅ Parakeet model '{}' unloaded successfully", current_model);
                } else {
                    warn!("⚠️ Failed to unload Parakeet model '{}'", current_model);
                }
            } else {
                warn!("⚠️ No Parakeet engine found to unload model");
            }
        }
        _ => {
            // Default to Whisper
            info!("🎤 Unloading Whisper model...");
            let engine_clone = {
                let engine_guard = crate::whisper_engine::commands::WHISPER_ENGINE
                    .lock()
                    .unwrap();
                engine_guard.as_ref().cloned()
            };

            if let Some(engine) = engine_clone {
                let current_model = engine
                    .get_current_model()
                    .await
                    .unwrap_or_else(|| "unknown".to_string());
                info!("Current Whisper model before unload: '{}'", current_model);

                if engine.unload_model().await {
                    info!("✅ Whisper model '{}' unloaded successfully", current_model);
                } else {
                    warn!("⚠️ Failed to unload Whisper model '{}'", current_model);
                }
            } else {
                warn!("⚠️ No Whisper engine found to unload model");
            }
        }
    }

    // Step 3.5: Track meeting ended analytics with privacy-safe metadata
    // Extract all data from manager BEFORE any async operations to avoid Send issues
    let analytics_data = if let Some(ref manager) = manager_for_cleanup {
        let state = manager.get_state();
        let stats = state.get_stats();

        Some((
            manager.get_recording_duration(),
            manager.get_active_recording_duration().unwrap_or(0.0),
            manager.get_total_pause_duration(),
            manager.get_transcript_segments().len() as u64,
            state.has_fatal_error(),
            state.get_microphone_device().map(|d| d.name.clone()),
            state.get_system_device().map(|d| d.name.clone()),
            stats.chunks_processed,
        ))
    } else {
        None
    };

    // Now perform async analytics tracking without holding manager reference
    if let Some((total_duration, active_duration, pause_duration, transcript_segments_count, had_fatal_error, mic_device_name, sys_device_name, chunks_processed)) = analytics_data {
        info!("📊 Collecting analytics for meeting end");

        // Helper function to classify device type from device name (privacy-safe)
        fn classify_device_type(device_name: &str) -> &'static str {
            let name_lower = device_name.to_lowercase();
            // Check for Bluetooth keywords
            if name_lower.contains("bluetooth")
                || name_lower.contains("airpods")
                || name_lower.contains("beats")
                || name_lower.contains("headphones")
                || name_lower.contains("bt ")
                || name_lower.contains("wireless") {
                "Bluetooth"
            } else {
                "Wired"
            }
        }

        // Get transcription model info (already loaded above for model unload)
        let transcription_config = match crate::api::api::api_get_transcript_config(
            app.clone(),
            app.clone().state(),
            None,
        )
        .await
        {
            Ok(Some(config)) => Some((config.provider, config.model)),
            _ => None,
        };

        let (transcription_provider, transcription_model) = transcription_config
            .unwrap_or_else(|| ("unknown".to_string(), "unknown".to_string()));

        // Get summary model info from API
        let summary_config = match crate::api::api::api_get_model_config(
            app.clone(),
            app.clone().state(),
            None,
        )
        .await
        {
            Ok(Some(config)) => Some((config.provider, config.model)),
            _ => None,
        };

        let (summary_provider, summary_model) = summary_config
            .unwrap_or_else(|| ("unknown".to_string(), "unknown".to_string()));

        // Classify device types (privacy-safe)
        let microphone_device_type = mic_device_name
            .as_ref()
            .map(|name| classify_device_type(name))
            .unwrap_or("Unknown");

        let system_audio_device_type = sys_device_name
            .as_ref()
            .map(|name| classify_device_type(name))
            .unwrap_or("Unknown");

        // Track meeting ended event with privacy-safe data
        match crate::analytics::commands::track_meeting_ended(
            transcription_provider.clone(),
            transcription_model.clone(),
            summary_provider.clone(),
            summary_model.clone(),
            total_duration,
            active_duration,
            pause_duration,
            microphone_device_type.to_string(),
            system_audio_device_type.to_string(),
            chunks_processed,
            transcript_segments_count,
            had_fatal_error,
        )
        .await
        {
            Ok(_) => info!("✅ Analytics tracked successfully for meeting end"),
            Err(e) => warn!("⚠️ Failed to track analytics: {}", e),
        }
    }

    // Step 4: Finalize recording state and cleanup resources safely
    let _ = app.emit(
        "recording-shutdown-progress",
        serde_json::json!({
            "stage": "finalizing",
            "message": "Finalizing recording and cleaning up resources...",
            "progress": 90
        }),
    );

    // Perform final cleanup with the manager if available
    let (meeting_folder, meeting_name) = if let Some(mut manager) = manager_for_cleanup {
        info!("🧹 Performing final cleanup and saving recording data");

        // Extract meeting info BEFORE async operations
        let meeting_folder = manager.get_meeting_folder();
        let meeting_name = manager.get_meeting_name();

        match tokio::time::timeout(
            tokio::time::Duration::from_secs(300), // 5 minutes max for file I/O
            manager.save_recording_only(&app)
        ).await {
            Ok(Ok(_)) => {
                info!("✅ Recording data saved successfully during cleanup");
            }
            Ok(Err(e)) => {
                warn!(
                    "⚠️ Error during recording cleanup (transcripts preserved): {}",
                    e
                );
                // Don't fail shutdown - transcripts are already preserved
            }
            Err(_) => {
                warn!("⏱️ File I/O timeout (5 minutes) reached during save, continuing shutdown");
                // Don't fail shutdown - transcripts are already preserved
            }
        }

        (meeting_folder, meeting_name)
    } else {
        info!("ℹ️ No recording manager available for cleanup");
        (None, None)
    };

    // Set recording flag to false
    info!("🔍 Setting IS_RECORDING to false");
    IS_RECORDING.store(false, Ordering::SeqCst);

    // Step 4.5: Prepare metadata for frontend (NO database save)
    // NOTE: We do NOT save to database here. The frontend will save after all transcripts are displayed.
    // This ensures the user sees all transcripts streaming in before the database save happens.
    let (folder_path_str, meeting_name_str) = match (&meeting_folder, &meeting_name) {
        (Some(path), Some(name)) => (
            Some(path.to_string_lossy().to_string()),
            Some(name.clone()),
        ),
        _ => (None, None),
    };

    info!("📤 Preparing recording metadata for frontend save");
    info!("   folder_path: {:?}", folder_path_str);
    info!("   meeting_name: {:?}", meeting_name_str);

    // Database save removed - frontend will handle this after receiving all transcripts
    info!("ℹ️ Skipping database save in Rust - frontend will save after all transcripts received");

    // Step 5: Complete shutdown
    let _ = app.emit(
        "recording-shutdown-progress",
        serde_json::json!({
            "stage": "complete",
            "message": "Recording stopped successfully",
            "progress": 100
        }),
    );

    // Emit final stop event with folder_path and meeting_name for frontend to save
    app.emit(
        "recording-stopped",
        serde_json::json!({
            "message": "Recording stopped - frontend will save after all transcripts received",
            "folder_path": folder_path_str,
            "meeting_name": meeting_name_str
        }),
    )
    .map_err(|e| e.to_string())?;

    // Update tray menu to reflect stopped state
    crate::tray::update_tray_menu(&app);

    info!("🎉 Recording stopped successfully with ZERO transcript chunks lost");
    Ok(())
}

/// Check if recording is active
pub async fn is_recording() -> bool {
    IS_RECORDING.load(Ordering::SeqCst)
}

/// Get recording statistics
pub async fn get_transcription_status() -> TranscriptionStatus {
    TranscriptionStatus {
        chunks_in_queue: 0,
        is_processing: IS_RECORDING.load(Ordering::SeqCst),
        last_activity_ms: 0,
    }
}

/// Pause the current recording
#[tauri::command]
pub async fn pause_recording<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    info!("Pausing recording");

    // Check if currently recording
    if !IS_RECORDING.load(Ordering::SeqCst) {
        return Err("No recording is currently active".to_string());
    }

    // Access the recording manager and pause it
    let manager_guard = RECORDING_MANAGER.lock().unwrap();
    if let Some(manager) = manager_guard.as_ref() {
        manager.pause_recording().map_err(|e| e.to_string())?;

        // Emit pause event to frontend
        app.emit(
            "recording-paused",
            serde_json::json!({
                "message": "Recording paused"
            }),
        )
        .map_err(|e| e.to_string())?;

        // Update tray menu to reflect paused state
        crate::tray::update_tray_menu(&app);

        info!("Recording paused successfully");
        Ok(())
    } else {
        Err("No recording manager found".to_string())
    }
}

/// Resume the current recording
#[tauri::command]
pub async fn resume_recording<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    info!("Resuming recording");

    // Check if currently recording
    if !IS_RECORDING.load(Ordering::SeqCst) {
        return Err("No recording is currently active".to_string());
    }

    // Access the recording manager and resume it
    let manager_guard = RECORDING_MANAGER.lock().unwrap();
    if let Some(manager) = manager_guard.as_ref() {
        manager.resume_recording().map_err(|e| e.to_string())?;

        // Emit resume event to frontend
        app.emit(
            "recording-resumed",
            serde_json::json!({
                "message": "Recording resumed"
            }),
        )
        .map_err(|e| e.to_string())?;

        // Update tray menu to reflect resumed state
        crate::tray::update_tray_menu(&app);

        info!("Recording resumed successfully");
        Ok(())
    } else {
        Err("No recording manager found".to_string())
    }
}

/// Check if recording is currently paused
#[tauri::command]
pub async fn is_recording_paused() -> bool {
    let manager_guard = RECORDING_MANAGER.lock().unwrap();
    if let Some(manager) = manager_guard.as_ref() {
        manager.is_paused()
    } else {
        false
    }
}

/// Get detailed recording state
#[tauri::command]
pub async fn get_recording_state() -> serde_json::Value {
    let is_recording = IS_RECORDING.load(Ordering::SeqCst);
    let manager_guard = RECORDING_MANAGER.lock().unwrap();

    if let Some(manager) = manager_guard.as_ref() {
        serde_json::json!({
            "is_recording": is_recording,
            "is_paused": manager.is_paused(),
            "is_active": manager.is_active(),
            "recording_duration": manager.get_recording_duration(),
            "active_duration": manager.get_active_recording_duration(),
            "total_pause_duration": manager.get_total_pause_duration(),
            "current_pause_duration": manager.get_current_pause_duration()
        })
    } else {
        serde_json::json!({
            "is_recording": is_recording,
            "is_paused": false,
            "is_active": false,
            "recording_duration": null,
            "active_duration": null,
            "total_pause_duration": 0.0,
            "current_pause_duration": null
        })
    }
}

/// Get the meeting folder path for the current recording
/// Returns the path if a meeting name was set and folder structure initialized
#[tauri::command]
pub async fn get_meeting_folder_path() -> Result<Option<String>, String> {
    let manager_guard = RECORDING_MANAGER.lock().unwrap();
    if let Some(manager) = manager_guard.as_ref() {
        Ok(manager.get_meeting_folder().map(|p| p.to_string_lossy().to_string()))
    } else {
        Ok(None)
    }
}

/// Get accumulated transcript segments from current recording session
/// Used for syncing frontend state after page reload during active recording
#[tauri::command]
pub async fn get_transcript_history() -> Result<Vec<crate::audio::recording_saver::TranscriptSegment>, String> {
    let manager_guard = RECORDING_MANAGER.lock().unwrap();

    if let Some(manager) = manager_guard.as_ref() {
        Ok(manager.get_transcript_segments())
    } else {
        Ok(Vec::new()) // No recording active, return empty
    }
}

/// Get meeting name from current recording session
/// Used for syncing frontend state after page reload during active recording
#[tauri::command]
pub async fn get_recording_meeting_name() -> Result<Option<String>, String> {
    let manager_guard = RECORDING_MANAGER.lock().unwrap();

    if let Some(manager) = manager_guard.as_ref() {
        Ok(manager.get_meeting_name())
    } else {
        Ok(None)
    }
}

// ============================================================================
// DEVICE MONITORING COMMANDS (AirPods/Bluetooth disconnect/reconnect support)
// ============================================================================

/// Response structure for device events
#[derive(Debug, Serialize, Clone)]
#[serde(tag = "type")]
pub enum DeviceEventResponse {
    DeviceDisconnected {
        device_name: String,
        device_type: String,
    },
    DeviceReconnected {
        device_name: String,
        device_type: String,
    },
    DeviceListChanged,
}

impl From<DeviceEvent> for DeviceEventResponse {
    fn from(event: DeviceEvent) -> Self {
        match event {
            DeviceEvent::DeviceDisconnected { device_name, device_type } => {
                DeviceEventResponse::DeviceDisconnected {
                    device_name,
                    device_type: format!("{:?}", device_type),
                }
            }
            DeviceEvent::DeviceReconnected { device_name, device_type } => {
                DeviceEventResponse::DeviceReconnected {
                    device_name,
                    device_type: format!("{:?}", device_type),
                }
            }
            DeviceEvent::DeviceListChanged => DeviceEventResponse::DeviceListChanged,
        }
    }
}

/// Reconnection status information
#[derive(Debug, Serialize, Clone)]
pub struct ReconnectionStatus {
    pub is_reconnecting: bool,
    pub disconnected_device: Option<DisconnectedDeviceInfo>,
}

/// Information about a disconnected device
#[derive(Debug, Serialize, Clone)]
pub struct DisconnectedDeviceInfo {
    pub name: String,
    pub device_type: String,
}

/// Poll for audio device events (disconnect/reconnect)
/// Should be called periodically (every 1-2 seconds) by frontend during recording
#[tauri::command]
pub async fn poll_audio_device_events() -> Result<Option<DeviceEventResponse>, String> {
    let mut manager_guard = RECORDING_MANAGER.lock().unwrap();

    if let Some(manager) = manager_guard.as_mut() {
        if let Some(event) = manager.poll_device_events() {
            info!("📱 Device event polled: {:?}", event);
            Ok(Some(event.into()))
        } else {
            Ok(None)
        }
    } else {
        // Not recording, no events
        Ok(None)
    }
}

/// Get current reconnection status
/// Returns whether the system is attempting to reconnect and which device
#[tauri::command]
pub async fn get_reconnection_status() -> Result<ReconnectionStatus, String> {
    let manager_guard = RECORDING_MANAGER.lock().unwrap();

    if let Some(manager) = manager_guard.as_ref() {
        let state = manager.get_state();
        let disconnected_device = state.get_disconnected_device().map(|(device, device_type)| {
            DisconnectedDeviceInfo {
                name: device.name.clone(),
                device_type: format!("{:?}", device_type),
            }
        });

        Ok(ReconnectionStatus {
            is_reconnecting: manager.is_reconnecting(),
            disconnected_device,
        })
    } else {
        // Not recording, no reconnection in progress
        Ok(ReconnectionStatus {
            is_reconnecting: false,
            disconnected_device: None,
        })
    }
}

/// Get information about the active audio output device
/// Used to warn users about Bluetooth playback issues
#[tauri::command]
pub async fn get_active_audio_output() -> Result<super::playback_monitor::AudioOutputInfo, String> {
    super::playback_monitor::get_active_audio_output()
        .await
        .map_err(|e| format!("Failed to get audio output info: {}", e))
}

/// Manually trigger device reconnection attempt
/// Useful for UI "Retry" button
#[tauri::command]
pub async fn attempt_device_reconnect(
    device_name: String,
    device_type: String,
) -> Result<bool, String> {
    // Parse device type first
    let monitor_type = match device_type.as_str() {
        "Microphone" => DeviceMonitorType::Microphone,
        "SystemAudio" => DeviceMonitorType::SystemAudio,
        _ => return Err(format!("Invalid device type: {}", device_type)),
    };

    // Check if recording is active
    {
        let manager_guard = RECORDING_MANAGER.lock().unwrap();
        if manager_guard.is_none() {
            return Err("Recording not active".to_string());
        }
    } // Release lock

    // Spawn blocking task to handle the async reconnection
    let result = tokio::task::spawn_blocking(move || {
        tokio::runtime::Handle::current().block_on(async {
            let mut manager_guard = RECORDING_MANAGER.lock().unwrap();
            if let Some(manager) = manager_guard.as_mut() {
                manager.attempt_device_reconnect(&device_name, monitor_type).await
            } else {
                Err(anyhow::anyhow!("Recording not active"))
            }
        })
    })
    .await
    .map_err(|e| format!("Task join error: {}", e))?;

    match result {
        Ok(success) => {
            if success {
                info!("✅ Manual reconnection successful");
            } else {
                warn!("❌ Manual reconnection failed - device not available");
            }
            Ok(success)
        }
        Err(e) => {
            error!("Manual reconnection error: {}", e);
            Err(e.to_string())
        }
    }
}

// ============================================================================
// LIVE INSIGHTS (lightweight running summary during an ACTIVE recording)
//
// This is intentionally separate from the post-meeting summary pipeline
// (chunk + synthesize + template in `summary::processor`/`summary::service`).
// It is a single-shot generation over a bounded recent window of the
// transcript-so-far, meant to be polled periodically (e.g. every ~45s)
// while recording is live.
// ============================================================================

/// Maximum number of Unicode characters (not bytes) of transcript to send to
/// the LLM per call (the configured builtin/local model, or a remote provider
/// - see `resolve_live_llm_provider`). Counting by `.chars().count()` rather than
/// `.len()` matters here - `.len()` counts UTF-8 bytes, which would silently
/// shrink the effective window for non-Latin transcripts (e.g. a CJK
/// character is 3 bytes but 1 char).
const LIVE_INSIGHTS_MAX_CHARS: usize = 6000;

/// Minimum transcript length (after windowing), in Unicode characters, required
/// before bothering to call the LLM. Below this, there's nothing meaningful to
/// summarize yet.
const LIVE_INSIGHTS_MIN_CHARS: usize = 50;

/// Minimum interval enforced between successive `generate_live_insights`
/// calls (and, via `live_action_chip_last_call_static`'s own per-kind
/// statics, `generate_live_action_chip` calls of each kind). This is
/// defense-in-depth against a buggy/runaway frontend polling loop, not a fix
/// for an actual vulnerability - the app is local/single-user. It just bounds
/// worst-case load on the local LLM sidecar - or a configured remote
/// provider's API - if the intended cadence were ever violated.
const LIVE_INSIGHTS_MIN_CALL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

/// How long a resolved builtin summary model name stays cached before
/// `resolve_cached_live_insights_model` re-scans the models directory via
/// `builtin_ai_get_available_summary_model`. Live insights are polled every
/// ~45s while a meeting is active, so without this cache every poll (plus
/// every action-chip click, which shares this same cache) pays for a
/// filesystem `scan_models()` walk. A short TTL (vs. caching indefinitely)
/// means a model added/removed mid-meeting is still picked up within a few
/// minutes rather than requiring an app restart - this cache only affects
/// model selection for the live-insights/chips convenience features, not the
/// canonical model-selection source of truth used elsewhere.
const LIVE_INSIGHTS_MODEL_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(5 * 60);

/// Error returned when `LiveInsightsGuard` fails to claim because a previous
/// `generate_live_insights` or `generate_live_action_chip` call is still
/// running.
///
/// IMPORTANT: `frontend/src/hooks/useLiveInsights.ts` has a matching TS
/// constant that MUST use this exact same string value - the frontend
/// pattern-matches on it to decide whether to silently skip a poll tick.
pub(crate) const LIVE_INSIGHTS_IN_PROGRESS_ERROR: &str = "insights generation already in progress";

/// Error returned by the backend-enforced minimum-interval check when
/// `generate_live_insights` or `generate_live_action_chip` is called again
/// too soon after the previous call (see `LIVE_INSIGHTS_MIN_CALL_INTERVAL`).
pub(crate) const LIVE_INSIGHTS_RATE_LIMITED_ERROR: &str =
    "insights generation requested too soon - please retry shortly";

/// Local timeout for a single `generate_live_insights` or
/// `generate_live_action_chip` call, deliberately much shorter than the
/// shared `summary::summary_engine::models::GENERATION_TIMEOUT_SECS` (900s)
/// used by the post-meeting summary pipeline. That constant is left
/// untouched - it's out of scope and other long-meeting summaries rely on the
/// full budget. Live insights/chips are just retried on the next poll/click if
/// this is hit, so a short local timeout is safe and keeps the UI responsive.
///
/// This is enforced via a `CancellationToken` passed into `generate_with_builtin`
/// rather than a bare `tokio::time::timeout` wrapper. `generate_with_builtin`
/// holds a lock on the shared `SIDECAR_MANAGER` singleton across an in-flight
/// read from the llama-helper sidecar's stdout, and that stdin/stdout protocol
/// has no request-ID correlation - it relies on strict one-request-in-flight
/// ordering. A bare `timeout` would just drop the future on expiry, leaving the
/// sidecar process running; its eventual (now-abandoned) response would then be
/// read by the *next* call on the shared pipe - whether another live-insights
/// poll or the real post-meeting summary pipeline - silently corrupting that
/// call's output. Cancelling via the token instead runs `generate_with_builtin`'s
/// internal `tokio::select!` cancellation arm, which calls `manager.shutdown()`
/// to kill the sidecar and reset its state before returning, so the pipe is
/// never left desynced.
const LIVE_INSIGHTS_GENERATION_TIMEOUT_SECS: u64 = 60;

const LIVE_INSIGHTS_SYSTEM_PROMPT: &str = "You are assisting with a meeting that is still in progress. \
Given the transcript so far, write a brief running summary (2-4 sentences) of what's been discussed, \
followed by a bulleted list titled \"Action items so far\" covering any action items or decisions \
mentioned so far. If nothing actionable has come up yet, say so briefly under that heading. Keep it \
concise and in markdown - this will be shown live during the meeting, so it must be fast to generate \
and quick to read.";

/// RAII guard for `LIVE_INSIGHTS_IN_PROGRESS`. Releases the flag on drop -
/// including on panic - so a single missed manual cleanup path can't wedge
/// live insights off for the rest of the app session.
struct LiveInsightsGuard;

impl LiveInsightsGuard {
    /// Attempts to claim the in-progress flag. Returns `None` if another call
    /// already holds it.
    fn try_claim() -> Option<Self> {
        LIVE_INSIGHTS_IN_PROGRESS
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| Self)
    }
}

impl Drop for LiveInsightsGuard {
    fn drop(&mut self) {
        LIVE_INSIGHTS_IN_PROGRESS.store(false, Ordering::SeqCst);
    }
}

/// Join transcript segments into a single string, bounded to `max_chars`
/// Unicode characters (counted via `.chars().count()`, not UTF-8 bytes -
/// `.len()` would undercount the remaining budget for non-Latin transcripts).
///
/// Walks backward from the most recent segment, including whole segments
/// until adding the next one would exceed the budget - never cutting a
/// segment's text in half. If the total transcript is already shorter than
/// `max_chars`, the whole thing is returned. The single most recent segment
/// is always included in full even if it alone exceeds `max_chars`. Segments
/// are only ever trimmed (`str::trim`, which is char-boundary safe) or kept
/// whole - never sliced mid-string - so byte/char boundaries are never a
/// concern here.
fn build_recent_window(
    segments: &[crate::audio::recording_saver::TranscriptSegment],
    max_chars: usize,
) -> String {
    let mut selected: Vec<&str> = Vec::new();
    let mut total_len = 0usize;

    for segment in segments.iter().rev() {
        let text = segment.text.trim();
        if text.is_empty() {
            continue;
        }

        // +1 accounts for the newline that will join this segment to the ones
        // already selected (no separator needed before the very first one).
        let added_len = text.chars().count() + if selected.is_empty() { 0 } else { 1 };
        if !selected.is_empty() && total_len + added_len > max_chars {
            break;
        }

        selected.push(text);
        total_len += added_len;
    }

    selected.reverse();
    selected.join("\n")
}

/// Shown instead of a raw sidecar/llama.cpp error string when the builtin AI
/// model backing live insights/action chips turns out to be missing or
/// corrupted - e.g. a real user hit the raw sidecar message "Generation
/// failed: Failed to load model: unable to load model at
/// '.../Qwen3.5-4B-Q4_K_M.gguf'", which is meaningless to a non-technical
/// user and gives no actionable next step. Mirrors the distinct not-
/// downloaded/downloading/corrupted messaging
/// `SummaryGeneratorButtonGroup.checkBuiltInAIModelsAndGenerate` already shows
/// for this same class of failure in the post-meeting summary flow, collapsed
/// into one message here since live insights/chips have far less UI real
/// estate than that flow's toast+modal combo.
const LIVE_LLM_MODEL_UNAVAILABLE_ERROR: &str = "The builtin AI model appears to be missing or \
corrupted — open Settings → Model Settings to re-download it.";

/// Whether an error message returned by the sidecar looks like a model-load
/// failure (missing/corrupted model file) rather than a transient failure
/// (rate limit, cancellation/timeout, in-progress) that already has its own
/// sentinel handling elsewhere. Used by `map_generation_outcome` as a
/// reactive fallback for the case where the sidecar's own load fails despite
/// the proactive `builtin_ai_is_model_ready` check in
/// `generate_bounded_live_llm_text` having passed moments earlier (e.g. the
/// model file was deleted/corrupted in the gap between the two).
fn is_model_load_failure(message: &str) -> bool {
    let lower = message.to_lowercase();
    lower.contains("unable to load model") || lower.contains("failed to load model")
}

/// Maps a generation outcome (the raw `Result` from `generate_with_builtin`
/// plus whether the shared cancellation token ended up cancelled) to the
/// final `Result<String, String>` returned by `generate_live_insights`.
/// Extracted so this mapping - in particular, distinguishing a
/// cancellation/timeout from any other failure - can be unit tested directly
/// instead of being duplicated inline across tests.
fn map_generation_outcome(
    result: Result<String, impl ToString>,
    was_cancelled: bool,
) -> Result<String, String> {
    match result {
        Ok(text) => Ok(text),
        Err(_e) if was_cancelled => {
            Err("Live insights generation timed out — will retry on the next update".to_string())
        }
        Err(e) => {
            let message = e.to_string();
            if is_model_load_failure(&message) {
                Err(LIVE_LLM_MODEL_UNAVAILABLE_ERROR.to_string())
            } else {
                Err(message)
            }
        }
    }
}

/// Whether a value cached at `resolved_at` is still within `ttl`. Pulled out
/// of `resolve_cached_live_insights_model` so the cache-freshness check can
/// be unit tested without needing a full Tauri `AppHandle`/`State`.
fn is_cache_fresh(resolved_at: std::time::Instant, ttl: std::time::Duration) -> bool {
    resolved_at.elapsed() < ttl
}

/// Resolves the builtin summary model to use for live insights, reusing a
/// cached result (see `LIVE_INSIGHTS_MODEL_CACHE_TTL`) instead of rescanning
/// the models directory via `builtin_ai_get_available_summary_model` on every
/// poll.
async fn resolve_cached_live_insights_model(
    app: &tauri::AppHandle,
) -> Result<Option<String>, String> {
    if let Some((cached, resolved_at)) = LIVE_INSIGHTS_MODEL_CACHE.lock().unwrap().clone() {
        if is_cache_fresh(resolved_at, LIVE_INSIGHTS_MODEL_CACHE_TTL) {
            return Ok(cached);
        }
    }

    let resolved =
        builtin_ai_get_available_summary_model(app.clone(), app.state::<ModelManagerState>())
            .await?;

    *LIVE_INSIGHTS_MODEL_CACHE.lock().unwrap() =
        Some((resolved.clone(), std::time::Instant::now()));

    Ok(resolved)
}

/// Whether a new `generate_live_insights` call arriving now, given the
/// timestamp of the previous accepted call, should be rejected as too soon
/// (see `LIVE_INSIGHTS_MIN_CALL_INTERVAL`). Pulled out into a pure function so
/// the interval check can be unit tested without sleeping in real time.
fn is_rate_limited(
    last_call: Option<std::time::Instant>,
    min_interval: std::time::Duration,
) -> bool {
    match last_call {
        Some(prev) => prev.elapsed() < min_interval,
        None => false,
    }
}

/// Rejects with `LIVE_INSIGHTS_RATE_LIMITED_ERROR` if `last_call` was set too
/// recently. Deliberately does NOT stamp `last_call` itself - see
/// `commit_rate_limit_slot` below, which callers must invoke separately once
/// they know the call is actually going to run. Splitting "check" from
/// "commit" like this (rather than stamping here unconditionally on success)
/// is what lets a caller that passes this check but then loses the
/// `LiveInsightsGuard` single-flight race avoid burning the rate-limit
/// window for a call that never actually generated anything.
///
/// Shared by `generate_live_insights` and `generate_live_action_chip`, each
/// passing their own `last_call` static so the commands rate-limit
/// independently (see `LIVE_ACTION_CHIP_RECAP_LAST_CALL` for why the
/// timestamps aren't shared even though the interval is - including between
/// the two chip kinds themselves).
fn claim_rate_limit_slot(
    last_call: &Mutex<Option<std::time::Instant>>,
    min_interval: std::time::Duration,
) -> Result<(), String> {
    let last_call = last_call.lock().unwrap();
    if is_rate_limited(*last_call, min_interval) {
        return Err(LIVE_INSIGHTS_RATE_LIMITED_ERROR.to_string());
    }
    Ok(())
}

/// Stamps `last_call` with `Instant::now()`, marking a rate-limit window as
/// consumed. Callers must only invoke this *after* `claim_rate_limit_slot`
/// has passed AND `LiveInsightsGuard::try_claim()` has actually succeeded -
/// committing any earlier (e.g. right after `claim_rate_limit_slot` alone)
/// would burn the window even for a call that goes on to lose the guard race
/// and never generates anything.
fn commit_rate_limit_slot(last_call: &Mutex<Option<std::time::Instant>>) {
    *last_call.lock().unwrap() = Some(std::time::Instant::now());
}

/// Determines which `LLMProvider` `generate_bounded_live_llm_text` should use,
/// given the raw `provider` string from the user's saved model config (or
/// `None` if no config row exists yet, or the lookup itself failed). Pulled
/// out into its own pure function - mirroring `live_action_chip_last_call_static`
/// / `is_model_load_failure` elsewhere in this file - so this branch point is
/// unit-testable without a `ModelConfig`/`AppHandle`.
///
/// Falls back to `LLMProvider::BuiltInAI` for `None`, an unparseable provider
/// string, or (by construction, since callers pass `.ok().flatten()` results
/// from `api_get_model_config`) a failed config lookup - preserving the
/// pre-existing "always use the local sidecar" behavior as the safe default
/// whenever the user hasn't (successfully) configured anything else.
// `pub(crate)` on this function and the three below (plus
// `LiveLlmProviderInvocation`) so `summary::commands::ask_about_meeting` /
// `ask_across_meetings` can resolve the same configured-provider/model
// invocation this live-insights flow uses, instead of reimplementing
// provider branching a second time - see their doc comments in
// `summary/commands.rs` for how they're reused.
pub(crate) fn resolve_live_llm_provider(provider_str: Option<&str>) -> LLMProvider {
    provider_str
        .and_then(|s| LLMProvider::from_str(s).ok())
        .unwrap_or(LLMProvider::BuiltInAI)
}

/// Resolves the effective model name `generate_bounded_live_llm_text` should
/// use for a non-builtin provider call: `model_name_override` wins when
/// present (verbatim, ignoring whatever `config_model` says), otherwise falls
/// back to the Settings-configured `config_model`. `None` means neither was
/// available - the caller must treat that as an error rather than passing an
/// empty model name through to the provider.
pub(crate) fn resolve_effective_model_name(
    model_name_override: Option<&str>,
    config_model: Option<&str>,
) -> Option<String> {
    model_name_override
        .or(config_model)
        .map(str::to_string)
}

/// Whether `generate_bounded_live_llm_text` must fetch a fresh API key for
/// `provider_override` rather than reusing the key already loaded alongside
/// the Settings-configured `ModelConfig` (`model_config.api_key`).
///
/// That api_key is always scoped to `settings_provider` specifically (see
/// `api_get_model_config`'s provider-keyed join in `api/api.rs`) - reusing it
/// for a *different* overridden provider would silently send one provider's
/// credential to a different provider's API. True whenever an override is
/// present and it does not match the Settings-configured provider string,
/// including when there is no Settings-configured provider at all (an ad-hoc
/// override with no saved config). Pure/unit-testable like
/// `resolve_live_llm_provider` above; the actual async key fetch
/// (`api_get_api_key`) is left to the caller.
fn provider_override_needs_fresh_key(
    provider_override: Option<&str>,
    settings_provider: Option<&str>,
) -> bool {
    match provider_override {
        Some(overridden) => Some(overridden) != settings_provider,
        None => false,
    }
}

/// Resolves which builtin model name `generate_bounded_live_llm_text`'s
/// BuiltInAI branch should request from `builtin_ai_is_model_ready`/
/// `generate_with_builtin`: `model_name_override` (threaded through from
/// `generate_live_action_chip`'s `model_name` param, e.g. a user's explicit
/// pick via `LiveActionChipModelPicker`) wins verbatim when present, mirroring
/// `resolve_effective_model_name`'s override precedence for the non-builtin
/// branch above - and, crucially, `resolve_cached` (standing in for
/// `resolve_cached_live_insights_model(app)`) is only invoked when there is
/// NO override. This is the fix for the bug where an explicit override was
/// silently ignored in favor of whatever `resolve_cached_live_insights_model`
/// (via `builtin_ai_get_available_summary_model`) auto-picked instead: taking
/// `resolve_cached` as a parameter (rather than calling
/// `resolve_cached_live_insights_model` directly) makes that "never even
/// consult the auto-pick when overridden" behavior itself unit-testable
/// without a real `AppHandle`.
async fn resolve_effective_builtin_model_name<F, Fut>(
    model_name_override: Option<&str>,
    resolve_cached: F,
) -> Result<Option<String>, String>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<Option<String>, String>>,
{
    match model_name_override {
        Some(explicit) => Ok(Some(explicit.to_string())),
        None => resolve_cached().await,
    }
}

/// Selects the API key `generate_bounded_live_llm_text`'s non-builtin branch
/// sends to `resolve_provider_invocation`: `fresh_override_key` (the result of
/// an `api_get_api_key` lookup scoped to the override provider) when
/// `needs_fresh_key` is true, or `settings_api_key` (already loaded alongside
/// `model_config`) when false. Pulled out of the inline
/// `if needs_fresh_key { ... } else { ... }` at the call site into its own
/// pure function - mirroring `resolve_provider_invocation`/
/// `resolve_live_llm_provider`/`resolve_effective_model_name` above - so the
/// mux itself, not just `provider_override_needs_fresh_key`'s boolean, is
/// directly unit-testable and directly exercised by production code, instead
/// of being hand-reproduced separately in tests.
fn resolve_live_llm_api_key(
    needs_fresh_key: bool,
    fresh_override_key: Option<String>,
    settings_api_key: Option<String>,
) -> Option<String> {
    if needs_fresh_key {
        fresh_override_key
    } else {
        settings_api_key
    }
}

/// A resolved plan for calling a configured *non-builtin* summary provider
/// from `generate_bounded_live_llm_text`, produced by
/// `resolve_provider_invocation`. Kept separate from the `LLMProvider` enum
/// itself since it also carries the provider-specific connection details
/// (endpoint, credentials, CustomOpenAI generation params) that
/// `llm_client::generate_summary` needs.
#[derive(Debug, PartialEq)]
pub(crate) struct LiveLlmProviderInvocation {
    pub(crate) provider: LLMProvider,
    pub(crate) model_name: String,
    pub(crate) api_key: String,
    pub(crate) ollama_endpoint: Option<String>,
    pub(crate) custom_openai_endpoint: Option<String>,
    pub(crate) custom_openai_max_tokens: Option<u32>,
    pub(crate) custom_openai_temperature: Option<f32>,
    pub(crate) custom_openai_top_p: Option<f32>,
}

/// Resolves how to call a configured non-builtin summary `provider`, mirroring
/// the same provider-driven branching `SummaryService::process_transcript_background`
/// (`summary/service.rs`) already uses for the post-meeting summary pipeline:
/// `Ollama` and `LmStudio` need no API key (just an optional custom endpoint,
/// since both are local servers), `CustomOpenAI` is configured entirely
/// separately (its own endpoint/key/generation params, stored as JSON - see
/// `custom_openai_config`), and every other provider requires a non-empty API
/// key from `api_key`.
///
/// A pure function deliberately: `custom_openai_config` must be fetched by the
/// caller beforehand (it's async/DB-backed), so this branching logic itself
/// stays unit-testable without any `AppHandle`/DB access. Must not be called
/// with `provider == LLMProvider::BuiltInAI` - that path is handled entirely
/// separately by `generate_bounded_live_llm_text` to keep the existing builtin
/// behavior (cached model resolution + readiness check) untouched.
pub(crate) fn resolve_provider_invocation(
    provider: &LLMProvider,
    model_name: &str,
    api_key: Option<&str>,
    ollama_endpoint: Option<&str>,
    custom_openai_config: Option<&crate::summary::CustomOpenAIConfig>,
) -> Result<LiveLlmProviderInvocation, String> {
    match provider {
        LLMProvider::BuiltInAI => unreachable!(
            "resolve_provider_invocation must not be called for LLMProvider::BuiltInAI - \
             generate_bounded_live_llm_text handles that path separately"
        ),
        LLMProvider::Ollama | LLMProvider::LmStudio => Ok(LiveLlmProviderInvocation {
            provider: provider.clone(),
            model_name: model_name.to_string(),
            api_key: String::new(),
            ollama_endpoint: ollama_endpoint.map(str::to_string),
            custom_openai_endpoint: None,
            custom_openai_max_tokens: None,
            custom_openai_temperature: None,
            custom_openai_top_p: None,
        }),
        LLMProvider::CustomOpenAI => {
            let config = custom_openai_config.ok_or_else(|| {
                "Custom OpenAI provider selected but no endpoint configured — add one in \
                 Settings → Model Settings."
                    .to_string()
            })?;
            Ok(LiveLlmProviderInvocation {
                provider: provider.clone(),
                model_name: model_name.to_string(),
                api_key: config.api_key.clone().unwrap_or_default(),
                ollama_endpoint: None,
                custom_openai_endpoint: Some(config.endpoint.clone()),
                custom_openai_max_tokens: config.max_tokens.map(|t| t.max(0) as u32),
                custom_openai_temperature: config.temperature,
                custom_openai_top_p: config.top_p,
            })
        }
        other => {
            let key = api_key.map(str::trim).filter(|k| !k.is_empty());
            match key {
                Some(key) => Ok(LiveLlmProviderInvocation {
                    provider: other.clone(),
                    model_name: model_name.to_string(),
                    api_key: key.to_string(),
                    ollama_endpoint: None,
                    custom_openai_endpoint: None,
                    custom_openai_max_tokens: None,
                    custom_openai_temperature: None,
                    custom_openai_top_p: None,
                }),
                None => Err(format!(
                    "No API key configured for {} — add it in Settings → Model Settings.",
                    provider_name(other)
                )),
            }
        }
    }
}

/// Generate a lightweight running summary + action items from the transcript
/// accumulated so far during an ACTIVE recording, using the app's local
/// builtin LLM (llama-helper sidecar). Reads the transcript-so-far itself via
/// the global recording manager - the frontend does not need to pass it in.
///
/// Returns:
/// - `Ok(markdown)` with a short running summary + "Action items so far" list
/// - `Ok("")` if there's no active recording or not enough transcript yet
/// - `Err(LIVE_INSIGHTS_IN_PROGRESS_ERROR)` if a previous call is still
///   running (frontend should treat this as "skip this tick")
/// - `Err(LIVE_INSIGHTS_RATE_LIMITED_ERROR)` if called again within
///   `LIVE_INSIGHTS_MIN_CALL_INTERVAL` of the previous call
/// - `Err(...)` for other failures (e.g. no builtin model configured/ready)
#[tauri::command]
pub async fn generate_live_insights(app: tauri::AppHandle) -> Result<String, String> {
    claim_rate_limit_slot(&LIVE_INSIGHTS_LAST_CALL, LIVE_INSIGHTS_MIN_CALL_INTERVAL)?;

    let _guard = LiveInsightsGuard::try_claim()
        .ok_or_else(|| LIVE_INSIGHTS_IN_PROGRESS_ERROR.to_string())?;
    commit_rate_limit_slot(&LIVE_INSIGHTS_LAST_CALL);

    generate_bounded_live_llm_text(&app, LIVE_INSIGHTS_SYSTEM_PROMPT, None, None).await
}

/// Shared implementation behind both `generate_live_insights` and
/// `generate_live_action_chip`: reads the transcript-so-far, builds the
/// bounded recent window, resolves which summary provider/model to use, and
/// generates `system_prompt` against it. Callers are responsible for their
/// own rate-limit check and for holding `LiveInsightsGuard` before calling
/// this - it does not claim the guard itself, since callers need to validate
/// their own arguments (e.g. `kind`) before deciding whether to consume a
/// rate-limit slot/guard claim at all.
///
/// `provider_override`/`model_name_override` let a caller ask for an ad-hoc
/// provider/model for just this one call, overriding (rather than replacing)
/// the user's Settings-configured default - `generate_live_insights` always
/// passes `None`/`None` here, preserving its pre-existing "always use
/// whatever's configured in Settings → Model Settings" behavior exactly.
/// `generate_live_action_chip` is the only caller that can pass `Some(..)`.
/// The underlying lookup (via `api_get_model_config` - the same one
/// `stop_recording`'s analytics call already uses at this file's
/// `summary_config` call site) still always runs, both as the fallback
/// when no override is given and as the source of the API key when the
/// override's provider matches it (see `provider_override_needs_fresh_key`).
///
/// Routing:
/// - `LLMProvider::BuiltInAI` (including no override + no config / an
///   unparseable provider string - see `resolve_live_llm_provider`):
///   unchanged pre-existing behavior - resolves the cached builtin model,
///   confirms it's ready, and calls the local llama-helper sidecar via
///   `generate_with_builtin`. An explicit `provider_override` of
///   `"builtin-ai"` also lands here, behaving identically to the no-config
///   default.
/// - Any other resolved provider: validates an API key is present where
///   required (see `resolve_provider_invocation`) and makes a single
///   non-chunked call via `llm_client::generate_summary` - the same one-shot
///   call the post-meeting summary pipeline uses per-chunk, just called once
///   here against the bounded transcript window instead of looping.
///
/// Both routes share the same 60s `CancellationToken`-based timeout wrapper
/// below - it is *not* duplicated per-route, since building a second, looser
/// timeout mechanism for the provider path would defeat the reason this one
/// exists in the first place (see `LIVE_INSIGHTS_GENERATION_TIMEOUT_SECS`).
async fn generate_bounded_live_llm_text(
    app: &tauri::AppHandle,
    system_prompt: &str,
    provider_override: Option<&str>,
    model_name_override: Option<&str>,
) -> Result<String, String> {
    let segments = {
        let manager_guard = RECORDING_MANAGER.lock().unwrap();
        match manager_guard.as_ref() {
            Some(manager) => manager.get_transcript_segments(),
            None => Vec::new(),
        }
    };

    let window = build_recent_window(&segments, LIVE_INSIGHTS_MAX_CHARS);
    if window.trim().chars().count() < LIVE_INSIGHTS_MIN_CHARS {
        return Ok(String::new());
    }

    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to resolve app data directory: {}", e))?;

    let user_prompt = format!("Transcript so far:\n\n{}", window);

    // Same lookup (and call pattern) `stop_recording`'s analytics tracking
    // already uses to read the user's configured summary provider server-side
    // - a failed/missing lookup falls back to `None`, which
    // `resolve_live_llm_provider` treats as BuiltInAI (the pre-existing
    // default behavior).
    let model_config = crate::api::api::api_get_model_config(app.clone(), app.clone().state(), None)
        .await
        .ok()
        .flatten();

    // `provider_override` takes priority over the Settings config, which in
    // turn takes priority over the None/BuiltInAI default - see
    // `resolve_live_llm_provider`.
    let provider = resolve_live_llm_provider(
        provider_override.or_else(|| model_config.as_ref().map(|c| c.provider.as_str())),
    );

    /// Which call `generate_bounded_live_llm_text` will make once the shared
    /// timeout wrapper below is set up. Resolved *before* that wrapper so
    /// pre-flight validation failures (no model configured, model not ready,
    /// no API key, no CustomOpenAI endpoint) return immediately without ever
    /// spinning up the cancellation timer - mirroring how the pre-existing
    /// builtin checks already worked.
    enum Plan {
        Builtin { model_name: String },
        Provider(LiveLlmProviderInvocation),
    }

    let plan = if provider == LLMProvider::BuiltInAI {
        // `model_name_override` (e.g. a specific model picked via
        // `LiveActionChipModelPicker`) wins verbatim and, crucially, is
        // resolved WITHOUT ever consulting `resolve_cached_live_insights_model`'s
        // auto-pick - otherwise a user's explicit choice would be silently
        // swapped for whichever model `builtin_ai_get_available_summary_model`
        // happens to auto-pick. See `resolve_effective_builtin_model_name`.
        let model_name = resolve_effective_builtin_model_name(model_name_override, || {
            resolve_cached_live_insights_model(app)
        })
        .await?
        .ok_or_else(|| {
            "No local model configured or ready — configure a builtin AI model in Settings"
                .to_string()
        })?;

        // Proactively confirm the resolved model - the override when one was
        // given, otherwise whatever the cache auto-picked - is actually ready
        // before paying for a sidecar round-trip, reusing the same underlying
        // readiness check (`ModelManager::is_model_ready`, via the
        // `builtin_ai_is_model_ready` Tauri command) that
        // `SummaryGeneratorButtonGroup` already uses for the equivalent
        // pre-flight check in the post-meeting summary flow, rather than
        // reinventing a second readiness check here. This is also what makes
        // an explicit `model_name_override` fail loudly instead of silently:
        // if the specifically requested model isn't ready/available, this
        // returns `LIVE_LLM_MODEL_UNAVAILABLE_ERROR` below rather than
        // falling back to a different model. Deliberately does NOT pass
        // `refresh: true` (unlike that flow's call): rescanning the models
        // directory on every live-insights poll/chip click would defeat the
        // point of `resolve_cached_live_insights_model`'s cache (see
        // `LIVE_INSIGHTS_MODEL_CACHE_TTL`), and an override still benefits
        // from the manager's already in-memory scan results. This still
        // catches the common "model missing/corrupted" case via that
        // in-memory status, and `map_generation_outcome` below is the
        // reactive backstop for the rarer case where the sidecar's own load
        // fails anyway (e.g. a race between this check and the actual load).
        let is_ready = builtin_ai_is_model_ready(
            app.clone(),
            app.state::<ModelManagerState>(),
            model_name.clone(),
            None,
        )
        .await?;
        if !is_ready {
            return Err(LIVE_LLM_MODEL_UNAVAILABLE_ERROR.to_string());
        }

        Plan::Builtin { model_name }
    } else {
        // `provider != BuiltInAI` here happens either because `provider_override`
        // named a non-builtin provider directly, or because `model_config`
        // parsed to one (see `resolve_live_llm_provider`) - unlike before
        // overrides existed, `model_config` itself may still be `None` (e.g.
        // an ad-hoc override with no Settings config saved at all yet), so it
        // can no longer be unconditionally unwrapped here.
        let effective_model_name = resolve_effective_model_name(
            model_name_override,
            model_config.as_ref().map(|c| c.model.as_str()),
        )
        .ok_or_else(|| {
            "No model configured for the selected provider — configure one in Settings → \
             Model Settings."
                .to_string()
        })?;

        // CustomOpenAI's endpoint/key/generation params live in their own
        // JSON-backed settings row, not on `ModelConfig` - only fetch it when
        // actually needed, mirroring `SummaryService::process_transcript_background`.
        // Keyed off the *resolved* provider, so an override to CustomOpenAI
        // fetches this fresh regardless of what `model_config.provider` was.
        let custom_openai_config = if provider == LLMProvider::CustomOpenAI {
            crate::api::api::api_get_custom_openai_config(app.clone(), app.clone().state())
                .await
                .ok()
                .flatten()
        } else {
            None
        };

        // `model_config.api_key` (when present) is scoped to
        // `model_config.provider` specifically - see `api_get_model_config`'s
        // provider-keyed join in `api/api.rs`. It's only safe to reuse when
        // the resolved provider matches that provider; otherwise it would
        // silently send the *wrong* provider's credential over the wire.
        // CustomOpenAI/Ollama never read this value anyway (see
        // `resolve_provider_invocation`), so skip the extra DB round-trip for
        // those regardless of whether the provider was overridden.
        let needs_fresh_key = provider_override_needs_fresh_key(
            provider_override,
            model_config.as_ref().map(|c| c.provider.as_str()),
        ) && !matches!(provider, LLMProvider::Ollama | LLMProvider::CustomOpenAI);

        let fresh_override_key = if needs_fresh_key {
            let override_provider = provider_override
                .expect("needs_fresh_key implies provider_override is Some")
                .to_string();
            crate::api::api::api_get_api_key(app.clone(), app.clone().state(), override_provider, None)
                .await
                .ok()
        } else {
            None
        };

        let api_key = resolve_live_llm_api_key(
            needs_fresh_key,
            fresh_override_key,
            model_config.as_ref().and_then(|c| c.api_key.clone()),
        );

        let invocation = resolve_provider_invocation(
            &provider,
            &effective_model_name,
            api_key.as_deref(),
            model_config.as_ref().and_then(|c| c.ollama_endpoint.as_deref()),
            custom_openai_config.as_ref(),
        )?;

        Plan::Provider(invocation)
    };

    // Live insights/chips are polled or clicked frequently and shown while the
    // meeting is still running, so we cap generation to a much shorter budget
    // than the shared `GENERATION_TIMEOUT_SECS` (900s) used by the
    // post-meeting summary pipeline - that constant stays untouched since long
    // meetings legitimately need the full 15 minutes there. Here, a slow
    // generation just means the caller retries on the next poll/click.
    //
    // Deliberately NOT a bare `tokio::time::timeout(...)` around the call: see
    // the doc comment on `LIVE_INSIGHTS_GENERATION_TIMEOUT_SECS` for why that's
    // unsafe against the shared sidecar. Instead, a `CancellationToken` is
    // cancelled by a background timer once the budget elapses, which drives
    // `generate_with_builtin`'s own `tokio::select!` cancellation arm - that
    // arm shuts the sidecar down cleanly before returning, so the shared
    // stdin/stdout pipe can never be left desynced for a later caller.
    // `llm_client::generate_summary` honors the same token for non-builtin
    // providers (it's a plain HTTP call, so cancellation there is just
    // dropping the in-flight request - no shared-pipe desync risk).
    let cancellation_token = CancellationToken::new();
    let timeout_token = cancellation_token.clone();
    let timeout_task: JoinHandle<()> = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(
            LIVE_INSIGHTS_GENERATION_TIMEOUT_SECS,
        ))
        .await;
        timeout_token.cancel();
    });

    let result: Result<String, String> = match plan {
        Plan::Builtin { model_name } => generate_with_builtin(
            &app_data_dir,
            &model_name,
            system_prompt,
            &user_prompt,
            Some(&cancellation_token),
        )
        .await
        .map_err(|e| e.to_string()),
        Plan::Provider(invocation) => {
            let client = reqwest::Client::new();
            generate_summary(
                &client,
                &invocation.provider,
                &invocation.model_name,
                &invocation.api_key,
                system_prompt,
                &user_prompt,
                invocation.ollama_endpoint.as_deref(),
                invocation.custom_openai_endpoint.as_deref(),
                invocation.custom_openai_max_tokens,
                invocation.custom_openai_temperature,
                invocation.custom_openai_top_p,
                Some(&app_data_dir),
                Some(&cancellation_token),
            )
            .await
        }
    };

    // The timer task is only useful until the call above returns - abort it
    // now so a fast (successful or failed) generation doesn't leave a
    // dangling 60s sleep task running in the background.
    timeout_task.abort();

    map_generation_outcome(result, cancellation_token.is_cancelled())
}

// ============================================================================
// LIVE ACTION CHIPS (short, on-demand "recap" / "questions" suggestions)
//
// Distinct from `generate_live_insights` above: these are short, user-
// triggered (e.g. by clicking a chip button), single-purpose generations
// rather than a periodically-polled running summary. They reuse the same
// transcript windowing, cached model resolution, and sidecar call as live
// insights - see the design note on `LiveInsightsGuard` below for why they
// also share its single-flight lock.
// ============================================================================

/// System prompt for the "recap" live action chip. Deliberately much terser
/// than `LIVE_INSIGHTS_SYSTEM_PROMPT` - this is quick-glance chip/tooltip
/// copy, not a running summary with an action-items section.
const LIVE_ACTION_CHIP_RECAP_PROMPT: &str = "You are assisting with a meeting that is still in progress. \
Given the transcript so far, write an extremely short recap - 1 to 3 sentences, plain prose, no heading, no \
bullet points - of what's been discussed so far. This is quick-glance chip copy meant to be read in a couple \
of seconds, not a running summary, so be as terse as possible while still being useful. Markdown is fine but \
keep formatting minimal.";

const LIVE_ACTION_CHIP_QUESTIONS_PROMPT: &str = "You are assisting with a meeting that is still in progress. \
Given the transcript so far, suggest 2 to 3 short, concrete clarifying or follow-up questions the user could \
ask next. Return ONLY a markdown bullet list of the questions - no heading, no preamble, no numbering beyond \
the bullets themselves. Each question should be a single short sentence.";

/// Maps a chip `kind` to its system prompt, or an error naming the invalid
/// kind. That error is not a sentinel the frontend needs to pattern-match on
/// (unlike the in-progress/rate-limit errors) - it only fires for a
/// programmer error on the calling side, since the frontend is expected to
/// only ever pass `"recap"` or `"questions"`.
fn live_action_chip_system_prompt(kind: &str) -> Result<&'static str, String> {
    match kind {
        "recap" => Ok(LIVE_ACTION_CHIP_RECAP_PROMPT),
        "questions" => Ok(LIVE_ACTION_CHIP_QUESTIONS_PROMPT),
        other => Err(format!(
            "Invalid live action chip kind: '{}' (expected \"recap\" or \"questions\")",
            other
        )),
    }
}

/// Timestamps of the last accepted `generate_live_action_chip` call, kept
/// *per chip kind* rather than as one shared timestamp for both "recap" and
/// "questions".
///
/// Deliberately separate from `LIVE_INSIGHTS_LAST_CALL` too, even though all
/// three (`LIVE_INSIGHTS_LAST_CALL` and these two) reuse the same
/// `LIVE_INSIGHTS_MIN_CALL_INTERVAL` value and the same
/// `LIVE_INSIGHTS_RATE_LIMITED_ERROR` sentinel: `generate_live_insights` is
/// driven by a background ~45s poll loop, while chips are triggered
/// on-demand by an explicit user click. Sharing a timestamp across any of
/// these would let one incorrectly rate-limit another even though they're
/// logically independent user-facing actions.
///
/// The "recap" and "questions" chips in particular are two separately
/// rendered buttons with fully independent UI/loading state - a user can
/// click "Recap" (which resolves quickly) and then immediately click
/// "Questions" for the very first time. With a single shared timestamp, that
/// "questions" click would be wrongly rejected as rate-limited for a chip it
/// never touched. Two known kinds only, so two plain statics (rather than a
/// `HashMap`) match the existing single-static style used throughout this
/// file. Only the *single-flight guard* (`LiveInsightsGuard`) needs to stay
/// shared across all of these - that's what actually protects the one shared
/// sidecar process from concurrent requests; see its doc comment.
static LIVE_ACTION_CHIP_RECAP_LAST_CALL: Mutex<Option<std::time::Instant>> = Mutex::new(None);
static LIVE_ACTION_CHIP_QUESTIONS_LAST_CALL: Mutex<Option<std::time::Instant>> = Mutex::new(None);

/// Selects the per-kind rate-limit timestamp static for a given chip `kind`.
/// Pulled out into its own function so the "recap"/"questions" -> static
/// mapping can be unit tested directly, mirroring `live_action_chip_system_prompt`.
///
/// The fallback arm is unreachable in practice: `generate_live_action_chip`
/// always calls `live_action_chip_system_prompt(&kind)?` first, which already
/// rejects any `kind` other than `"recap"`/`"questions"` before this function
/// is ever reached.
fn live_action_chip_last_call_static(kind: &str) -> &'static Mutex<Option<std::time::Instant>> {
    match kind {
        "questions" => &LIVE_ACTION_CHIP_QUESTIONS_LAST_CALL,
        _ => &LIVE_ACTION_CHIP_RECAP_LAST_CALL,
    }
}

/// Generate short, actionable suggestion-chip content from the transcript
/// accumulated so far during an ACTIVE recording, using the same local
/// builtin LLM (llama-helper sidecar) as `generate_live_insights`.
///
/// `kind` must be `"recap"` (a very short 1-3 sentence recap, phrased for a
/// chip/tooltip - terser than `generate_live_insights`' running summary) or
/// `"questions"` (a markdown bullet list of 2-3 short clarifying/follow-up
/// questions).
///
/// `provider`/`model_name` are optional ad-hoc overrides that let the caller
/// generate this one chip against a different model than whatever's
/// configured in Settings → Model Settings, without changing that default for
/// anything else (`generate_live_insights`'s running summary keeps using the
/// Settings default regardless). Both are `None` for "use the Settings
/// default", matching pre-existing behavior exactly - see
/// `generate_bounded_live_llm_text` for the precedence rules once either is
/// `Some`.
///
/// Deliberately shares `LiveInsightsGuard` - and therefore
/// `LIVE_INSIGHTS_IN_PROGRESS_ERROR` - with `generate_live_insights` rather
/// than using an independent lock. Both commands ultimately call
/// `generate_with_builtin` against the same single llama-helper sidecar
/// process (see the doc comment on `LIVE_INSIGHTS_GENERATION_TIMEOUT_SECS`
/// for why that shared pipe requires strict one-request-in-flight ordering).
/// A second, independent lock scoped only to chip generation would not
/// prevent a chip call and an insights poll from racing against each other
/// on that shared pipe, so the two commands must contend for the *same* lock.
///
/// Returns:
/// - `Ok(markdown)` with the requested chip content
/// - `Ok("")` if there's no active recording or not enough transcript yet
/// - `Err(...)` with a message naming the invalid kind if `kind` is neither
///   `"recap"` nor `"questions"`
/// - `Err(LIVE_INSIGHTS_IN_PROGRESS_ERROR)` if a `generate_live_insights` or
///   `generate_live_action_chip` call is still running
/// - `Err(LIVE_INSIGHTS_RATE_LIMITED_ERROR)` if called again within
///   `LIVE_INSIGHTS_MIN_CALL_INTERVAL` of the previous accepted
///   `generate_live_action_chip` call
/// - `Err(...)` for other failures (e.g. no builtin model configured/ready)
#[tauri::command]
pub async fn generate_live_action_chip(
    app: tauri::AppHandle,
    kind: String,
    provider: Option<String>,
    model_name: Option<String>,
) -> Result<String, String> {
    // Validate first, before touching the rate limiter or the shared guard -
    // an invalid `kind` shouldn't consume either.
    let system_prompt = live_action_chip_system_prompt(&kind)?;
    let last_call = live_action_chip_last_call_static(&kind);

    claim_rate_limit_slot(last_call, LIVE_INSIGHTS_MIN_CALL_INTERVAL)?;

    let _guard = LiveInsightsGuard::try_claim()
        .ok_or_else(|| LIVE_INSIGHTS_IN_PROGRESS_ERROR.to_string())?;
    commit_rate_limit_slot(last_call);

    generate_bounded_live_llm_text(&app, system_prompt, provider.as_deref(), model_name.as_deref())
        .await
}

#[cfg(test)]
mod live_insights_window_tests {
    use super::*;
    use crate::audio::recording_saver::TranscriptSegment;

    /// Serializes tests that manipulate the shared `LIVE_INSIGHTS_IN_PROGRESS`
    /// / `LIVE_INSIGHTS_LAST_CALL` / `LIVE_ACTION_CHIP_RECAP_LAST_CALL` /
    /// `LIVE_ACTION_CHIP_QUESTIONS_LAST_CALL` statics.
    /// Cargo runs tests in this module concurrently by default, and those
    /// statics are shared process-wide state (mirroring the real singleton
    /// guard/rate-limiter), so two such tests running in parallel can observe
    /// each other's writes and flake. Every test that touches those statics
    /// must acquire this lock for its full body. `unwrap_or_else` recovers
    /// from a poisoned lock (an earlier test panicking while holding it)
    /// rather than cascading that panic into every other serialized test.
    static TEST_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn seg(id: &str, text: &str) -> TranscriptSegment {
        TranscriptSegment {
            id: id.to_string(),
            text: text.to_string(),
            audio_start_time: 0.0,
            audio_end_time: 0.0,
            duration: 0.0,
            display_time: "[00:00]".to_string(),
            confidence: 1.0,
            sequence_id: 0,
        }
    }

    /// Test double for `generate_with_builtin`'s cancellation-aware shape:
    /// resolves after `delay`, but bails out early with the same cancellation
    /// error as the real sidecar call if `token` is cancelled first. Shared by
    /// the cancellation tests below instead of being redefined per-test.
    async fn simulated_slow_generation(
        token: &CancellationToken,
        delay: std::time::Duration,
    ) -> anyhow::Result<String> {
        tokio::select! {
            _ = tokio::time::sleep(delay) => Ok("should not get here".to_string()),
            _ = token.cancelled() => Err(anyhow::anyhow!("Generation cancelled by user")),
        }
    }

    /// Spawns the same "cancel the token after a delay" timer shape used by
    /// `generate_live_insights` itself, so tests can trigger cancellation
    /// deterministically without duplicating the spawn boilerplate.
    fn spawn_cancel_after(token: &CancellationToken, delay: std::time::Duration) -> JoinHandle<()> {
        let token = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            token.cancel();
        })
    }

    #[test]
    fn build_recent_window_empty_input_returns_empty_string() {
        let segments: Vec<TranscriptSegment> = Vec::new();
        assert_eq!(build_recent_window(&segments, 6000), "");
    }

    #[test]
    fn build_recent_window_shorter_than_budget_returns_everything() {
        let segments = vec![seg("1", "hello there"), seg("2", "general kenobi")];
        let result = build_recent_window(&segments, 6000);
        assert!(result.contains("hello there"));
        assert!(result.contains("general kenobi"));
    }

    #[test]
    fn build_recent_window_longer_than_budget_snaps_to_segment_boundary() {
        // Each segment is 10 chars. Budget of 25 naively would cut segment "ccccccccc2" in half.
        let segments = vec![
            seg("1", "aaaaaaaaa1"),
            seg("2", "bbbbbbbbb2"),
            seg("3", "ccccccccc3"),
        ];
        let result = build_recent_window(&segments, 25);

        // Never a partial segment: every included segment's full text appears intact.
        for included_text in ["bbbbbbbbb2", "ccccccccc3"] {
            assert!(
                result.contains(included_text),
                "expected full segment '{}' in result '{}'",
                included_text,
                result
            );
        }
        // The oldest segment must have been dropped rather than truncated.
        assert!(!result.contains("aaaaaaaaa1"));
    }

    #[test]
    fn build_recent_window_never_returns_partial_segment_text() {
        let segments = vec![seg("1", "0123456789"), seg("2", "abcdefghij")];
        // Budget (5) only fits part of the most recent segment - naive char-slicing
        // would yield "fghij". The whole most-recent segment must be kept intact
        // instead, and the older segment dropped rather than truncated.
        let result = build_recent_window(&segments, 5);

        assert_eq!(result, "abcdefghij");
    }

    #[test]
    fn build_recent_window_counts_unicode_chars_not_utf8_bytes() {
        // "こんにちは" is 5 Unicode scalar values but 15 UTF-8 bytes (3 bytes/char).
        // With a budget of 20 *characters*, both segments (5 + 10 = 15 chars,
        // well under 20) should fit. Byte-based counting would instead see
        // 15 + 10 = 25 bytes > 20 and wrongly drop the older segment.
        let segments = vec![seg("1", "bbbbbbbbbb"), seg("2", "こんにちは")];
        let result = build_recent_window(&segments, 20);

        assert!(
            result.contains("bbbbbbbbbb"),
            "older ASCII segment should fit within a 20-char budget once counting is \
             char-based rather than byte-based; got '{}'",
            result
        );
        assert!(result.contains("こんにちは"));
    }

    #[test]
    fn live_insights_guard_prevents_concurrent_claims_and_releases_on_drop() {
        let _serial = TEST_SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner());

        // Ensure clean starting state in case another test left it claimed.
        LIVE_INSIGHTS_IN_PROGRESS.store(false, Ordering::SeqCst);

        let first = LiveInsightsGuard::try_claim();
        assert!(first.is_some(), "first claim should succeed");
        assert!(
            LiveInsightsGuard::try_claim().is_none(),
            "second claim should fail while first is held"
        );

        drop(first);

        let second = LiveInsightsGuard::try_claim();
        assert!(second.is_some(), "claim should succeed again after release");
    }

    /// Mirrors the `CancellationToken` + timer setup in
    /// `generate_live_insights`: once the background timer cancels the token,
    /// the in-flight call must map to a retry-friendly error. Calls the real
    /// `map_generation_outcome` production function directly rather than
    /// duplicating its match arms inline. `simulated_slow_generation` stands
    /// in for `generate_with_builtin`'s own cancellation arm, which shuts the
    /// sidecar down and returns an error rather than being silently dropped.
    #[tokio::test]
    async fn generate_live_insights_cancellation_wrapper_maps_cancelled_to_actionable_error() {
        let cancellation_token = CancellationToken::new();
        let timer_task =
            spawn_cancel_after(&cancellation_token, std::time::Duration::from_millis(5));

        let generation_result =
            simulated_slow_generation(&cancellation_token, std::time::Duration::from_millis(200))
                .await;
        timer_task.abort();

        let result = map_generation_outcome(generation_result, cancellation_token.is_cancelled());

        assert_eq!(
            result,
            Err("Live insights generation timed out — will retry on the next update".to_string())
        );
    }

    /// The `LiveInsightsGuard` is bound in the outer function's scope (`let
    /// _guard = ...`), so it must release the concurrency flag regardless of
    /// which branch of the cancellation match executes - including the
    /// cancelled branch itself. Guards against reintroducing a stuck-forever
    /// lock now that the timeout is driven by a `CancellationToken` instead of
    /// a bare `tokio::time::timeout`.
    #[tokio::test]
    async fn live_insights_guard_releases_even_when_cancellation_wrapper_fires() {
        let _serial = TEST_SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner());

        LIVE_INSIGHTS_IN_PROGRESS.store(false, Ordering::SeqCst);

        async fn run_and_get_cancelled() {
            let _guard = LiveInsightsGuard::try_claim().expect("should claim cleanly");

            let cancellation_token = CancellationToken::new();
            let timer_task =
                spawn_cancel_after(&cancellation_token, std::time::Duration::from_millis(5));

            let generation_result = simulated_slow_generation(
                &cancellation_token,
                std::time::Duration::from_millis(200),
            )
            .await;
            timer_task.abort();

            let _ = map_generation_outcome(generation_result, cancellation_token.is_cancelled());
            // `_guard` drops here, at the end of this scope - same as at the end
            // of `generate_live_insights` itself.
        }

        run_and_get_cancelled().await;

        assert!(
            LiveInsightsGuard::try_claim().is_some(),
            "guard must have released the flag after the wrapped call was cancelled"
        );
    }

    /// A fast, successful generation must not leave the 60s timer task running
    /// in the background - `generate_live_insights` calls `.abort()` on its
    /// `JoinHandle` as soon as the call returns. Verifies both that the abort
    /// actually cancels the still-sleeping timer task (rather than letting it
    /// run to completion) and that the token is never marked cancelled on this
    /// path, so a fast success can't be misreported as a timeout.
    #[tokio::test]
    async fn generate_live_insights_fast_success_aborts_timer_without_misreporting_cancellation() {
        async fn fast_generation(_token: &CancellationToken) -> anyhow::Result<String> {
            Ok("done".to_string())
        }

        let cancellation_token = CancellationToken::new();
        let timer_task = spawn_cancel_after(
            &cancellation_token,
            std::time::Duration::from_secs(LIVE_INSIGHTS_GENERATION_TIMEOUT_SECS),
        );

        let generation_result = fast_generation(&cancellation_token).await;
        timer_task.abort();

        let result = map_generation_outcome(generation_result, cancellation_token.is_cancelled());

        assert_eq!(result, Ok("done".to_string()));
        assert!(
            !cancellation_token.is_cancelled(),
            "token must not be cancelled on the fast-success path"
        );

        // If abort() didn't actually take effect, awaiting the handle would
        // hang for the full 60s sleep; bound it tightly so a regression fails
        // fast instead of timing out the test suite.
        let join_result = tokio::time::timeout(std::time::Duration::from_millis(500), timer_task)
            .await
            .expect("aborted timer task should resolve promptly, not run its full 60s sleep");
        assert!(
            join_result.is_err() && join_result.unwrap_err().is_cancelled(),
            "timer task should have been cancelled by abort(), not left running/completed"
        );
    }

    /// A failure that is *not* due to cancellation (e.g. sidecar startup
    /// failure, malformed response) must be surfaced as-is rather than being
    /// misreported as the "timed out" message - only `was_cancelled == true`
    /// should trigger that mapping.
    #[test]
    fn map_generation_outcome_passes_through_non_cancellation_errors() {
        let result: Result<String, anyhow::Error> = Err(anyhow::anyhow!("sidecar exited early"));
        let mapped = map_generation_outcome(result, false);

        assert_eq!(mapped, Err("sidecar exited early".to_string()));
    }

    #[test]
    fn map_generation_outcome_passes_through_success() {
        let result: Result<String, anyhow::Error> = Ok("summary text".to_string());
        let mapped = map_generation_outcome(result, false);

        assert_eq!(mapped, Ok("summary text".to_string()));
    }

    /// REGRESSION TEST (bug): a raw llama.cpp-sidecar "unable to load model"
    /// error - forwarded verbatim via `Err(e) => Err(e.to_string())` - used to
    /// reach the user completely unfiltered. A real user hit exactly this:
    /// "Generation failed: Failed to load model: unable to load model at
    /// '.../Qwen3.5-4B-Q4_K_M.gguf'". That raw sidecar/llama.cpp string is
    /// meaningless to a non-technical user and gives no actionable next step,
    /// unlike the dedicated not-downloaded/downloading/corrupted messaging the
    /// post-meeting summary flow (`SummaryGeneratorButtonGroup`) already shows
    /// for this same underlying class of failure. This class of error must
    /// instead be replaced with `LIVE_LLM_MODEL_UNAVAILABLE_ERROR`.
    #[test]
    fn map_generation_outcome_replaces_model_load_failure_with_friendly_message() {
        let result: Result<String, anyhow::Error> = Err(anyhow::anyhow!(
            "Failed to load model: unable to load model at '/Users/x/Library/Application \
             Support/Meetily/models/summary/Qwen3.5-4B-Q4_K_M.gguf'"
        ));

        let mapped = map_generation_outcome(result, false);

        assert_eq!(mapped, Err(LIVE_LLM_MODEL_UNAVAILABLE_ERROR.to_string()));
    }

    #[test]
    fn is_model_load_failure_detects_unable_to_load_model_message() {
        assert!(is_model_load_failure(
            "Failed to load model: unable to load model at '/some/path/model.gguf'"
        ));
    }

    #[test]
    fn is_model_load_failure_ignores_unrelated_errors() {
        assert!(!is_model_load_failure("sidecar exited early"));
        assert!(!is_model_load_failure(
            "Live insights generation timed out — will retry on the next update"
        ));
        assert!(!is_model_load_failure(LIVE_INSIGHTS_RATE_LIMITED_ERROR));
        assert!(!is_model_load_failure(LIVE_INSIGHTS_IN_PROGRESS_ERROR));
    }

    #[test]
    fn is_cache_fresh_true_within_ttl() {
        let now = std::time::Instant::now();
        assert!(is_cache_fresh(now, std::time::Duration::from_secs(60)));
    }

    #[test]
    fn is_cache_fresh_false_once_ttl_elapsed() {
        let resolved_at = std::time::Instant::now() - std::time::Duration::from_secs(10);
        assert!(!is_cache_fresh(
            resolved_at,
            std::time::Duration::from_millis(1)
        ));
    }

    #[test]
    fn is_rate_limited_false_when_no_previous_call() {
        assert!(!is_rate_limited(None, std::time::Duration::from_secs(5)));
    }

    #[test]
    fn is_rate_limited_true_within_min_interval() {
        let last_call = std::time::Instant::now();
        assert!(is_rate_limited(
            Some(last_call),
            std::time::Duration::from_secs(5)
        ));
    }

    #[test]
    fn is_rate_limited_false_after_min_interval_elapsed() {
        let last_call = std::time::Instant::now() - std::time::Duration::from_secs(10);
        assert!(!is_rate_limited(
            Some(last_call),
            std::time::Duration::from_secs(5)
        ));
    }

    #[test]
    fn live_action_chip_system_prompt_selects_recap_prompt() {
        assert_eq!(
            live_action_chip_system_prompt("recap"),
            Ok(LIVE_ACTION_CHIP_RECAP_PROMPT)
        );
    }

    #[test]
    fn live_action_chip_system_prompt_selects_questions_prompt() {
        assert_eq!(
            live_action_chip_system_prompt("questions"),
            Ok(LIVE_ACTION_CHIP_QUESTIONS_PROMPT)
        );
    }

    #[test]
    fn live_action_chip_system_prompt_rejects_unknown_kind() {
        let result = live_action_chip_system_prompt("summary");
        assert!(
            result.is_err(),
            "unrecognized kind must be rejected rather than silently falling back"
        );
        assert!(
            result.unwrap_err().contains("summary"),
            "error message should name the invalid kind so a caller can debug it"
        );

        // Empty input and wrong-case variants of valid kinds hit the same
        // fallback arm - no implicit case-folding, so a frontend typo can't be
        // silently masked.
        assert!(live_action_chip_system_prompt("").is_err());
        assert!(live_action_chip_system_prompt("Recap").is_err());
        assert!(live_action_chip_system_prompt("QUESTIONS").is_err());
    }

    #[test]
    fn live_action_chip_prompts_are_distinct_from_each_other_and_from_live_insights() {
        // The recap/questions chip prompts must read as terser, single-purpose
        // chip copy - not duplicates of each other or of the fuller running
        // summary + action items prompt used by `generate_live_insights`.
        assert_ne!(LIVE_ACTION_CHIP_RECAP_PROMPT, LIVE_ACTION_CHIP_QUESTIONS_PROMPT);
        assert_ne!(LIVE_ACTION_CHIP_RECAP_PROMPT, LIVE_INSIGHTS_SYSTEM_PROMPT);
        assert_ne!(LIVE_ACTION_CHIP_QUESTIONS_PROMPT, LIVE_INSIGHTS_SYSTEM_PROMPT);
    }

    #[test]
    fn live_action_chip_last_call_static_selects_distinct_statics_per_kind() {
        assert!(std::ptr::eq(
            live_action_chip_last_call_static("recap"),
            &LIVE_ACTION_CHIP_RECAP_LAST_CALL
        ));
        assert!(std::ptr::eq(
            live_action_chip_last_call_static("questions"),
            &LIVE_ACTION_CHIP_QUESTIONS_LAST_CALL
        ));
        assert!(!std::ptr::eq(
            live_action_chip_last_call_static("recap"),
            live_action_chip_last_call_static("questions")
        ));
    }

    /// REGRESSION TEST (bug): `generate_live_action_chip` used to track the
    /// rate-limit window for BOTH the "recap" and "questions" chip kinds in a
    /// single shared timestamp, even though the two kinds are independent,
    /// separately-clickable UI elements with fully independent loading state.
    /// A user who clicks "Recap" (which resolves) and then immediately clicks
    /// "Questions" for the first time was wrongly rejected with the
    /// rate-limited sentinel for a chip they never touched.
    ///
    /// This test mirrors `generate_live_action_chip`'s exact
    /// kind -> static selection -> check-then-commit sequence, via
    /// `live_action_chip_last_call_static` (the same production function the
    /// real command calls), rather than reaching into a single hardcoded
    /// static directly - so it actually exercises the fix, not just the
    /// existence of two separate statics.
    #[test]
    fn recap_resolving_must_not_rate_limit_an_immediate_first_questions_click() {
        let _serial = TEST_SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner());

        // Clean slate.
        *LIVE_ACTION_CHIP_RECAP_LAST_CALL.lock().unwrap() = None;
        *LIVE_ACTION_CHIP_QUESTIONS_LAST_CALL.lock().unwrap() = None;

        // Simulate a "recap" click that resolves successfully just now -
        // mirrors `generate_live_action_chip`'s exact
        // claim_rate_limit_slot -> commit_rate_limit_slot sequence.
        let recap_last_call = live_action_chip_last_call_static("recap");
        claim_rate_limit_slot(recap_last_call, LIVE_INSIGHTS_MIN_CALL_INTERVAL)
            .expect("first recap click should pass the rate limiter");
        commit_rate_limit_slot(recap_last_call);

        // Immediately afterwards, the user clicks "Questions" for the very
        // first time. Because the two kinds are logically independent chips,
        // this must be accepted - "questions" has never been called before.
        let questions_last_call = live_action_chip_last_call_static("questions");
        let questions_check =
            claim_rate_limit_slot(questions_last_call, LIVE_INSIGHTS_MIN_CALL_INTERVAL);

        assert!(
            questions_check.is_ok(),
            "an immediate first click on a DIFFERENT chip kind must not be rejected as \
             rate-limited just because the OTHER kind's chip happened to resolve moments ago - \
             the two kinds must track their rate-limit windows independently"
        );
    }

    // `generate_live_action_chip` deliberately reuses `LiveInsightsGuard` rather
    // than an independent lock, so it contends for the same single-flight slot
    // as `generate_live_insights` (both ultimately drive the one shared
    // llama-helper sidecar process). That single-flight behavior is already
    // covered by `live_insights_guard_prevents_concurrent_claims_and_releases_on_drop`
    // above, since both commands claim the same `LiveInsightsGuard`.

    /// REGRESSION TEST: reproduces the sequence a real user hits when they
    /// click a chip while another live-insights/chip call is still running,
    /// get the "still busy" (`LIVE_INSIGHTS_IN_PROGRESS_ERROR`) message, wait
    /// for it to clear, and immediately retry - exactly the "rapid
    /// double-click a single chip" scenario called out for this review.
    ///
    /// Guards against a bug where `claim_rate_limit_slot` unconditionally
    /// stamped the rate-limit timestamp on success, before
    /// `LiveInsightsGuard::try_claim()` was attempted. A call that passed the
    /// rate-limit check but then lost the race for the single-flight guard
    /// would still burn the rate-limit window despite never actually running
    /// a generation, wrongly rate-limiting an immediate legitimate retry once
    /// the guard freed up. Fixed by splitting the check
    /// (`claim_rate_limit_slot`, side-effect-free) from the commit
    /// (`commit_rate_limit_slot`, called only after the guard is actually
    /// claimed) - see their doc comments.
    #[test]
    fn rate_limit_slot_is_burned_by_a_call_that_only_loses_the_guard_race() {
        let _serial = TEST_SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner());

        // Clean slate: no previous chip call, and simulate some other
        // long-running call (a live-insights poll tick, or another chip
        // click) currently holding the single-flight guard.
        *LIVE_ACTION_CHIP_RECAP_LAST_CALL.lock().unwrap() = None;
        LIVE_INSIGHTS_IN_PROGRESS.store(true, Ordering::SeqCst);

        // --- Call A: user's first click -------------------------------------------------
        // Passes the rate limiter (no prior call recorded)...
        let rate_check_a = claim_rate_limit_slot(
            &LIVE_ACTION_CHIP_RECAP_LAST_CALL,
            LIVE_INSIGHTS_MIN_CALL_INTERVAL,
        );
        assert!(rate_check_a.is_ok(), "first click should pass the rate limiter");

        // ...but loses the race for the single-flight guard, because the
        // simulated other call is still in progress. `claim_rate_limit_slot`
        // is side-effect-free (see its doc comment), so this loss doesn't
        // stamp the rate-limit timestamp - `generate_live_action_chip` only
        // does that via `commit_rate_limit_slot`, after the guard claim
        // below actually succeeds.
        let guard_a = LiveInsightsGuard::try_claim();
        assert!(
            guard_a.is_none(),
            "guard should be busy - simulating a concurrent in-flight call"
        );
        // Call A returns Err(LIVE_INSIGHTS_IN_PROGRESS_ERROR) to the user here
        // in the real command; no guard was ever held by A.

        // The other in-flight call now finishes and releases the guard.
        LIVE_INSIGHTS_IN_PROGRESS.store(false, Ordering::SeqCst);

        // --- Call B: user's immediate retry, guard is now free --------------------------
        let rate_check_b = claim_rate_limit_slot(
            &LIVE_ACTION_CHIP_RECAP_LAST_CALL,
            LIVE_INSIGHTS_MIN_CALL_INTERVAL,
        );

        // The retry is allowed through: the guard is free and Call A never
        // stamped the rate-limit timestamp (it lost the guard race before
        // `commit_rate_limit_slot` would have run), so the user doing exactly
        // what the "still busy, try again" UI copy told them to do succeeds.
        assert!(
            rate_check_b.is_ok(),
            "a retry immediately after the single-flight guard freed up should not be \
             rate-limited just because an earlier attempt that never actually generated \
             anything (it lost the guard race) had already stamped the rate-limit clock"
        );
    }

    /// REGRESSION/ADVERSARIAL TEST (round 2): reproduces "app/window torn down
    /// mid-generation" by aborting a tokio task that is holding the guard
    /// partway through a simulated slow generation, instead of letting the
    /// call finish and drop the guard normally. If `LiveInsightsGuard` only
    /// released on normal scope-exit (and not on the future simply being
    /// dropped by `.abort()`), this would leave `LIVE_INSIGHTS_IN_PROGRESS`
    /// stuck `true` forever, permanently wedging both `generate_live_insights`
    /// and `generate_live_action_chip` for the rest of the process lifetime
    /// (they share this single guard).
    #[tokio::test]
    async fn guard_releases_when_holding_task_is_aborted_mid_generation() {
        let _serial = TEST_SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        LIVE_INSIGHTS_IN_PROGRESS.store(false, Ordering::SeqCst);

        let task: JoinHandle<()> = tokio::spawn(async move {
            let _guard = LiveInsightsGuard::try_claim().expect("should claim cleanly");
            // Simulate a slow in-flight generation call (e.g. waiting on the
            // llama-helper sidecar) that is still running when the app/window
            // is torn down.
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        });

        // Give the spawned task a chance to actually claim the guard before we
        // tear it down.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(
            LiveInsightsGuard::try_claim().is_none(),
            "sanity check: guard should be held by the spawned task at this point"
        );

        // Simulate app/window teardown aborting the in-flight command task
        // (e.g. Tauri dropping the future rather than letting it run to
        // completion).
        task.abort();
        let _ = task.await;

        assert!(
            LiveInsightsGuard::try_claim().is_some(),
            "aborting the task mid-generation must still run the guard's Drop impl and \
             release LIVE_INSIGHTS_IN_PROGRESS - otherwise live insights/action chips would \
             be permanently wedged off for the rest of the app session after any teardown \
             that happens to land mid-generation"
        );
    }

    /// ADVERSARIAL TEST (round 2): hammers `claim_rate_limit_slot` /
    /// `LiveInsightsGuard::try_claim` / `commit_rate_limit_slot` from many real
    /// OS threads at once (not just a hand-sequenced simulation), to check for
    /// any TOCTOU introduced by splitting the rate-limit "check" from "commit"
    /// into two separate lock acquisitions with a gap between them. Asserts
    /// two invariants that must hold under real concurrent execution:
    ///   1. At most one thread ever believes it is "inside" the guarded
    ///      section at a time (no double-claim).
    ///   2. Exactly as many rate-limit commits happen as successful guard
    ///      claims - a thread that loses the guard race never commits the
    ///      rate-limit slot (this is the round-1 fix; this test re-verifies it
    ///      holds under genuine thread-level parallelism, not just a
    ///      hand-ordered sequence).
    #[test]
    fn concurrent_threads_never_double_claim_guard_or_double_commit_rate_limit() {
        let _serial = TEST_SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        LIVE_INSIGHTS_IN_PROGRESS.store(false, Ordering::SeqCst);
        *LIVE_ACTION_CHIP_RECAP_LAST_CALL.lock().unwrap() = None;

        static INSIDE_GUARDED_SECTION: AtomicBool = AtomicBool::new(false);
        static MAX_CONCURRENT_HOLDERS_SEEN: std::sync::atomic::AtomicUsize =
            std::sync::atomic::AtomicUsize::new(0);
        static SUCCESSFUL_GUARD_CLAIMS: std::sync::atomic::AtomicUsize =
            std::sync::atomic::AtomicUsize::new(0);
        static RATE_LIMIT_COMMITS: std::sync::atomic::AtomicUsize =
            std::sync::atomic::AtomicUsize::new(0);

        INSIDE_GUARDED_SECTION.store(false, Ordering::SeqCst);
        MAX_CONCURRENT_HOLDERS_SEEN.store(0, Ordering::SeqCst);
        SUCCESSFUL_GUARD_CLAIMS.store(0, Ordering::SeqCst);
        RATE_LIMIT_COMMITS.store(0, Ordering::SeqCst);

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(16));
        let mut handles = Vec::new();

        for _ in 0..16 {
            let barrier = barrier.clone();
            handles.push(std::thread::spawn(move || {
                // Line every thread up so they all hit the check/claim/commit
                // sequence as close to simultaneously as possible.
                barrier.wait();

                // Mirrors generate_live_action_chip's exact sequence.
                if claim_rate_limit_slot(&LIVE_ACTION_CHIP_RECAP_LAST_CALL, std::time::Duration::from_millis(0))
                    .is_err()
                {
                    return;
                }

                let guard = LiveInsightsGuard::try_claim();
                if guard.is_none() {
                    return;
                }
                SUCCESSFUL_GUARD_CLAIMS.fetch_add(1, Ordering::SeqCst);

                // Detect any overlap: if another thread is concurrently inside
                // this section too, the guard failed to provide exclusion.
                let was_already_inside = INSIDE_GUARDED_SECTION.swap(true, Ordering::SeqCst);
                assert!(!was_already_inside, "two threads inside the guarded section at once");
                MAX_CONCURRENT_HOLDERS_SEEN.fetch_add(1, Ordering::SeqCst);

                commit_rate_limit_slot(&LIVE_ACTION_CHIP_RECAP_LAST_CALL);
                RATE_LIMIT_COMMITS.fetch_add(1, Ordering::SeqCst);

                // Hold the section briefly to widen the window for a racing
                // thread to (incorrectly) slip in, if the guard were broken.
                std::thread::sleep(std::time::Duration::from_millis(5));

                INSIDE_GUARDED_SECTION.store(false, Ordering::SeqCst);
                drop(guard);
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        let claims = SUCCESSFUL_GUARD_CLAIMS.load(Ordering::SeqCst);
        let commits = RATE_LIMIT_COMMITS.load(Ordering::SeqCst);
        assert_eq!(
            claims, 1,
            "exactly one of the 16 racing threads should win the single-flight guard \
             (all fired at once with a fresh rate-limit slot, so all 16 pass the rate-limit \
             check, but only one may actually claim the guard)"
        );
        assert_eq!(
            commits, claims,
            "the number of committed rate-limit slots must exactly equal the number of \
             successful guard claims - any thread that lost the guard race must not have \
             committed (this is the round-1 fix; verifies it holds under real thread \
             parallelism, not just a hand-ordered simulation)"
        );

        // Clean up the shared guard for any tests that run after this one.
        LIVE_INSIGHTS_IN_PROGRESS.store(false, Ordering::SeqCst);
    }

    // ========================================================================
    // Provider routing (`resolve_live_llm_provider` / `resolve_provider_invocation`)
    //
    // `generate_bounded_live_llm_text` itself needs a real `AppHandle` (it
    // calls `api_get_model_config`, `builtin_ai_is_model_ready`, etc.), so
    // these tests exercise the pure routing/validation logic it delegates to
    // instead - same pattern as `map_generation_outcome` and
    // `live_action_chip_last_call_static` above being tested directly rather
    // than through the full Tauri command.
    // ========================================================================

    /// (a) No saved config (fresh install) must resolve to BuiltInAI, so the
    /// pre-existing sidecar-only behavior is exactly preserved by default.
    #[test]
    fn resolve_live_llm_provider_none_defaults_to_builtin() {
        assert_eq!(resolve_live_llm_provider(None), LLMProvider::BuiltInAI);
    }

    /// (a) An explicit builtin-ai selection (and its legacy aliases) must
    /// stay on the builtin route.
    #[test]
    fn resolve_live_llm_provider_explicit_builtin_stays_builtin() {
        assert_eq!(
            resolve_live_llm_provider(Some("builtin-ai")),
            LLMProvider::BuiltInAI
        );
        assert_eq!(
            resolve_live_llm_provider(Some("local-llama")),
            LLMProvider::BuiltInAI
        );
    }

    /// (a) An unparseable/unknown provider string (e.g. a stale or corrupted
    /// settings row) must fall back to BuiltInAI rather than erroring out or
    /// silently treating it as some other provider.
    #[test]
    fn resolve_live_llm_provider_unparseable_string_defaults_to_builtin() {
        assert_eq!(
            resolve_live_llm_provider(Some("not-a-real-provider")),
            LLMProvider::BuiltInAI
        );
    }

    /// A configured Groq provider must resolve to the Groq route, not
    /// builtin - this is the core regression this change fixes.
    #[test]
    fn resolve_live_llm_provider_groq_resolves_to_groq() {
        assert_eq!(resolve_live_llm_provider(Some("groq")), LLMProvider::Groq);
    }

    #[test]
    fn resolve_live_llm_provider_is_case_insensitive() {
        assert_eq!(resolve_live_llm_provider(Some("GROQ")), LLMProvider::Groq);
    }

    /// (b) A configured non-builtin provider (Groq) with a valid API key must
    /// resolve to a `Provider` invocation carrying that key/model through
    /// unchanged - proving the routing decision picks `generate_summary`
    /// over the sidecar rather than erroring or silently falling back.
    #[test]
    fn resolve_provider_invocation_groq_with_key_routes_to_provider_call() {
        let invocation = resolve_provider_invocation(
            &LLMProvider::Groq,
            "llama-3.3-70b-versatile",
            Some("gsk_test_key"),
            None,
            None,
        )
        .expect("valid API key should resolve successfully");

        assert_eq!(
            invocation,
            LiveLlmProviderInvocation {
                provider: LLMProvider::Groq,
                model_name: "llama-3.3-70b-versatile".to_string(),
                api_key: "gsk_test_key".to_string(),
                ollama_endpoint: None,
                custom_openai_endpoint: None,
                custom_openai_max_tokens: None,
                custom_openai_temperature: None,
                custom_openai_top_p: None,
            }
        );
    }

    /// (c) A configured non-builtin provider (Groq) with NO API key must
    /// return a clear, actionable error - not a builtin-style "model
    /// missing/corrupted" message, and not a silent fallthrough that would
    /// attempt the call anyway.
    #[test]
    fn resolve_provider_invocation_groq_without_key_returns_clear_error() {
        let err = resolve_provider_invocation(&LLMProvider::Groq, "llama-3.3-70b-versatile", None, None, None)
            .expect_err("missing API key must be rejected");

        assert_eq!(
            err,
            "No API key configured for Groq — add it in Settings → Model Settings."
        );
        // Must not resemble the builtin "model missing/corrupted" error path
        // (`LIVE_LLM_MODEL_UNAVAILABLE_ERROR`) - a missing key is a distinct,
        // actionable problem from a missing/corrupted model file.
        assert!(!err.to_lowercase().contains("missing or corrupted"));
    }

    /// An empty (rather than absent) API key must be treated the same as a
    /// missing one - guards against a blank string in the settings DB
    /// silently passing validation.
    #[test]
    fn resolve_provider_invocation_empty_key_is_treated_as_missing() {
        let err = resolve_provider_invocation(&LLMProvider::OpenAI, "gpt-4o", Some("   "), None, None)
            .expect_err("whitespace-only API key must be rejected");
        assert!(err.contains("No API key configured for OpenAI"));
    }

    /// Ollama doesn't require an API key (mirrors
    /// `SummaryService::process_transcript_background` in `summary/service.rs`)
    /// - only its optional custom endpoint should be threaded through.
    #[test]
    fn resolve_provider_invocation_ollama_needs_no_api_key() {
        let invocation = resolve_provider_invocation(
            &LLMProvider::Ollama,
            "llama3.2:latest",
            None,
            Some("http://custom-host:11434"),
            None,
        )
        .expect("Ollama must not require an API key");

        assert_eq!(invocation.api_key, "");
        assert_eq!(
            invocation.ollama_endpoint.as_deref(),
            Some("http://custom-host:11434")
        );
    }

    /// LM Studio's local server doesn't require an API key either (mirrors
    /// `llm_client::generate_summary`, which sends no Authorization header for
    /// `LLMProvider::LmStudio` - see the `provider != &LLMProvider::LmStudio`
    /// check in that function's header-building logic).
    #[test]
    fn resolve_provider_invocation_lmstudio_needs_no_api_key() {
        let invocation = resolve_provider_invocation(
            &LLMProvider::LmStudio,
            "local-model",
            None,
            Some("http://localhost:1234/v1"),
            None,
        )
        .expect("LM Studio must not require an API key");

        assert_eq!(invocation.api_key, "");
        assert_eq!(
            invocation.ollama_endpoint.as_deref(),
            Some("http://localhost:1234/v1")
        );
    }

    /// CustomOpenAI with a saved config must thread through its endpoint,
    /// key, and generation params from that config (not from the top-level
    /// `ModelConfig.api_key`, which is separate).
    #[test]
    fn resolve_provider_invocation_custom_openai_with_config_uses_its_fields() {
        let config = crate::summary::CustomOpenAIConfig {
            endpoint: "http://localhost:8000/v1".to_string(),
            api_key: Some("local-key".to_string()),
            model: "mistral-7b".to_string(),
            max_tokens: Some(2048),
            temperature: Some(0.7),
            top_p: Some(0.9),
        };

        let invocation = resolve_provider_invocation(
            &LLMProvider::CustomOpenAI,
            "mistral-7b",
            None,
            None,
            Some(&config),
        )
        .expect("CustomOpenAI with a saved config should resolve successfully");

        assert_eq!(invocation.api_key, "local-key");
        assert_eq!(
            invocation.custom_openai_endpoint.as_deref(),
            Some("http://localhost:8000/v1")
        );
        assert_eq!(invocation.custom_openai_max_tokens, Some(2048));
        assert_eq!(invocation.custom_openai_temperature, Some(0.7));
        assert_eq!(invocation.custom_openai_top_p, Some(0.9));
    }

    /// CustomOpenAI selected but with no saved endpoint config must return a
    /// clear error rather than attempting a call with an empty endpoint.
    #[test]
    fn resolve_provider_invocation_custom_openai_without_config_returns_clear_error() {
        let err = resolve_provider_invocation(&LLMProvider::CustomOpenAI, "mistral-7b", None, None, None)
            .expect_err("missing CustomOpenAI config must be rejected");
        assert!(err.contains("Custom OpenAI"));
        assert!(err.contains("no endpoint configured"));
    }

    /// CustomOpenAI's API key is optional even when a config exists (some
    /// self-hosted OpenAI-compatible servers don't require one) - `None`
    /// there must resolve to an empty key, not an error.
    #[test]
    fn resolve_provider_invocation_custom_openai_optional_key_defaults_to_empty() {
        let config = crate::summary::CustomOpenAIConfig {
            endpoint: "http://localhost:8000/v1".to_string(),
            api_key: None,
            model: "mistral-7b".to_string(),
            max_tokens: None,
            temperature: None,
            top_p: None,
        };

        let invocation = resolve_provider_invocation(
            &LLMProvider::CustomOpenAI,
            "mistral-7b",
            None,
            None,
            Some(&config),
        )
        .expect("a config with no api_key should still resolve");

        assert_eq!(invocation.api_key, "");
    }

    #[test]
    #[should_panic(expected = "must not be called for LLMProvider::BuiltInAI")]
    fn resolve_provider_invocation_panics_if_called_with_builtin_ai() {
        let _ = resolve_provider_invocation(&LLMProvider::BuiltInAI, "any-model", None, None, None);
    }

    // ========================================================================
    // Ad-hoc provider/model overrides (`generate_live_action_chip`'s
    // `provider`/`model_name` params, threaded through
    // `generate_bounded_live_llm_text`)
    //
    // Like the section above, `generate_bounded_live_llm_text` itself needs a
    // real `AppHandle`, so these exercise the pure helpers it delegates to -
    // `resolve_effective_model_name` and `provider_override_needs_fresh_key` -
    // plus, where the precedence is just an inline `.or_else()` at the call
    // site rather than its own function, the exact same combinator chain
    // reproduced here so a regression in that one-liner still fails a test.
    // ========================================================================

    /// (a) No override: an override-shaped `None.or_else(settings)` chain
    /// must resolve identically to the pre-existing "just use whatever
    /// Settings has" lookup.
    #[test]
    fn no_override_falls_back_to_settings_configured_provider() {
        let provider_override: Option<&str> = None;
        let settings_provider = Some("groq");
        assert_eq!(
            resolve_live_llm_provider(provider_override.or(settings_provider)),
            LLMProvider::Groq
        );
    }

    /// (a) No override and no Settings config at all (fresh install): must
    /// still default to BuiltInAI, exactly like today.
    #[test]
    fn no_override_and_no_settings_config_defaults_to_builtin() {
        let provider_override: Option<&str> = None;
        let settings_provider: Option<&str> = None;
        assert_eq!(
            resolve_live_llm_provider(provider_override.or(settings_provider)),
            LLMProvider::BuiltInAI
        );
    }

    /// An explicit override takes priority over a *different*
    /// Settings-configured provider.
    #[test]
    fn override_provider_takes_priority_over_settings_configured_provider() {
        let provider_override = Some("groq");
        let settings_provider = Some("openai");
        assert_eq!(
            resolve_live_llm_provider(provider_override.or(settings_provider)),
            LLMProvider::Groq
        );
    }

    /// An override of `"builtin-ai"` is not just "same as no override" by
    /// coincidence of both defaulting somewhere - it must resolve to the
    /// exact same route (BuiltInAI) as the no-config default, even when
    /// Settings has some *other* provider configured (the override still
    /// wins).
    #[test]
    fn explicit_builtin_override_resolves_to_builtin_even_over_a_different_settings_provider() {
        let provider_override = Some("builtin-ai");
        let settings_provider = Some("groq");
        assert_eq!(
            resolve_live_llm_provider(provider_override.or(settings_provider)),
            LLMProvider::BuiltInAI
        );
    }

    /// (e) An override model name is used verbatim, ignoring whatever
    /// `model_config.model` says.
    #[test]
    fn resolve_effective_model_name_override_wins_verbatim() {
        assert_eq!(
            resolve_effective_model_name(Some("gpt-4o-mini"), Some("gpt-3.5-turbo")),
            Some("gpt-4o-mini".to_string())
        );
    }

    /// No override: falls back to the Settings-configured model, unchanged.
    #[test]
    fn resolve_effective_model_name_falls_back_to_settings_model_when_no_override() {
        assert_eq!(
            resolve_effective_model_name(None, Some("gpt-3.5-turbo")),
            Some("gpt-3.5-turbo".to_string())
        );
    }

    /// Neither an override nor a Settings-configured model exist (e.g. an
    /// ad-hoc override with no saved config at all): the caller must treat
    /// this as an error rather than silently passing an empty model name to
    /// the provider.
    #[test]
    fn resolve_effective_model_name_none_when_neither_override_nor_config_present() {
        assert_eq!(resolve_effective_model_name(None, None), None);
    }

    // ========================================================================
    // Builtin model override (`resolve_effective_builtin_model_name`) - Fix 1
    // regression coverage: `generate_bounded_live_llm_text`'s BuiltInAI branch
    // used to resolve the model via `resolve_cached_live_insights_model`
    // unconditionally, never consulting `model_name_override` at all, so a
    // user who explicitly picked a specific, available builtin model via
    // `LiveActionChipModelPicker` would silently get whatever
    // `resolve_cached_live_insights_model` (i.e.
    // `builtin_ai_get_available_summary_model`'s auto-pick) resolved to
    // instead. These call the same async helper production now uses, with the
    // cache-lookup closure standing in for `resolve_cached_live_insights_model(app)`.
    // ========================================================================

    /// The core regression this fixes: an explicit override must be used
    /// verbatim, and the cache's auto-pick closure must never even be invoked
    /// - proving the override isn't silently swapped for whatever
    /// `resolve_cached_live_insights_model` would have auto-picked.
    #[tokio::test]
    async fn resolve_effective_builtin_model_name_override_wins_without_consulting_cache() {
        let cache_lookup_called = Arc::new(AtomicBool::new(false));
        let cache_lookup_called_clone = cache_lookup_called.clone();

        let result = resolve_effective_builtin_model_name(Some("gemma3:1b"), move || {
            let flag = cache_lookup_called_clone.clone();
            async move {
                flag.store(true, Ordering::SeqCst);
                // A different, non-default model - stands in for whatever
                // `builtin_ai_get_available_summary_model` would auto-pick,
                // to prove the override wins over it rather than the two
                // simply coinciding.
                Ok(Some("qwen3.5:4b".to_string()))
            }
        })
        .await
        .expect("an override should resolve successfully");

        assert_eq!(result, Some("gemma3:1b".to_string()));
        assert!(
            !cache_lookup_called.load(Ordering::SeqCst),
            "must not consult resolve_cached_live_insights_model's auto-pick when an override is \
             present - this is the exact Fix 1 regression: an explicit override was previously \
             silently ignored in favor of whatever the cache auto-picked"
        );
    }

    /// No override: falls back to whatever the cache resolves to, unchanged
    /// from the pre-existing behavior.
    #[tokio::test]
    async fn resolve_effective_builtin_model_name_falls_back_to_cache_when_no_override() {
        let result = resolve_effective_builtin_model_name(None, || async {
            Ok(Some("qwen3.5:4b".to_string()))
        })
        .await
        .expect("no override should fall back to the cache");

        assert_eq!(result, Some("qwen3.5:4b".to_string()));
    }

    /// No override, and the cache lookup itself fails (or resolves to no
    /// available model): the error/`None` must propagate unchanged, matching
    /// pre-existing behavior exactly.
    #[tokio::test]
    async fn resolve_effective_builtin_model_name_propagates_cache_error_when_no_override() {
        let err = resolve_effective_builtin_model_name(None, || async {
            Err("boom: model manager not initialized".to_string())
        })
        .await
        .expect_err("cache lookup errors must propagate when there is no override");

        assert_eq!(err, "boom: model manager not initialized");
    }

    /// (a) No override: never needs a fresh key fetch, regardless of what
    /// Settings has configured - the existing `model_config.api_key` is
    /// always safe to reuse since it's for the very provider being used.
    #[test]
    fn provider_override_needs_fresh_key_false_when_no_override() {
        assert!(!provider_override_needs_fresh_key(None, Some("openai")));
        assert!(!provider_override_needs_fresh_key(None, None));
    }

    /// (b) Override provider matches the Settings-configured provider: must
    /// reuse `model_config.api_key` rather than triggering a redundant fetch.
    #[test]
    fn provider_override_needs_fresh_key_false_when_override_matches_settings_provider() {
        assert!(!provider_override_needs_fresh_key(
            Some("openai"),
            Some("openai")
        ));
    }

    /// (c) Override provider differs from the Settings-configured provider:
    /// must require a fresh fetch rather than reusing the wrong provider's
    /// key.
    #[test]
    fn provider_override_needs_fresh_key_true_when_override_differs_from_settings_provider() {
        assert!(provider_override_needs_fresh_key(
            Some("claude"),
            Some("openai")
        ));
    }

    /// (c)/(d) Override provider present but there is no Settings config at
    /// all: must still require a fresh fetch - there is no
    /// `model_config.api_key` to reuse in the first place.
    #[test]
    fn provider_override_needs_fresh_key_true_when_override_present_and_no_settings_config() {
        assert!(provider_override_needs_fresh_key(Some("claude"), None));
    }

    // ========================================================================
    // `resolve_live_llm_api_key` - the extracted mux `generate_bounded_live_llm_text`'s
    // non-builtin branch uses at its api_key selection call site (Fix 2).
    // ========================================================================

    #[test]
    fn resolve_live_llm_api_key_uses_fresh_key_when_needed() {
        assert_eq!(
            resolve_live_llm_api_key(
                true,
                Some("fresh-key".to_string()),
                Some("settings-key".to_string())
            ),
            Some("fresh-key".to_string())
        );
    }

    #[test]
    fn resolve_live_llm_api_key_uses_settings_key_when_not_needed() {
        assert_eq!(
            resolve_live_llm_api_key(
                false,
                Some("fresh-key".to_string()),
                Some("settings-key".to_string())
            ),
            Some("settings-key".to_string())
        );
    }

    /// Needs a fresh key, but the fresh fetch came back empty (e.g. no key
    /// saved for the override provider) - must resolve to `None`, not
    /// silently fall back to the (wrong-provider) settings key.
    #[test]
    fn resolve_live_llm_api_key_needs_fresh_but_fetch_returned_none() {
        assert_eq!(
            resolve_live_llm_api_key(true, None, Some("settings-key".to_string())),
            None
        );
    }

    /// (c) The core regression this override feature must not introduce:
    /// when the override provider differs from the Settings-configured
    /// provider, the invocation actually sent to `llm_client::generate_summary`
    /// must carry the *override* provider's own key - never the
    /// Settings-configured provider's key that happened to already be loaded
    /// on `model_config`. Calls the actual `resolve_live_llm_api_key` function
    /// `generate_bounded_live_llm_text`'s non-builtin branch uses for this mux
    /// (rather than hand-reproducing its `if needs_fresh_key { .. } else { .. }`
    /// inline here), so a regression there (inverted condition, forgotten
    /// `provider_override_needs_fresh_key` call, swapped if/else arms) fails
    /// this test directly instead of silently passing a hand-copied twin.
    #[test]
    fn override_provider_differing_from_settings_uses_its_own_key_not_settings_key() {
        let provider_override = Some("claude");
        let settings_provider = Some("openai");
        let settings_api_key = Some("openai-settings-key".to_string());
        // Stands in for `api_get_api_key(app, state, "claude".to_string(), None)`.
        let freshly_fetched_override_key = Some("claude-override-key".to_string());

        let needs_fresh_key =
            provider_override_needs_fresh_key(provider_override, settings_provider);
        assert!(needs_fresh_key);

        let api_key = resolve_live_llm_api_key(
            needs_fresh_key,
            freshly_fetched_override_key,
            settings_api_key,
        );

        let resolved_provider = resolve_live_llm_provider(provider_override.or(settings_provider));
        assert_eq!(resolved_provider, LLMProvider::Claude);

        let invocation = resolve_provider_invocation(
            &resolved_provider,
            "claude-3-5-sonnet",
            api_key.as_deref(),
            None,
            None,
        )
        .expect("a freshly fetched key for the override provider should resolve successfully");

        assert_eq!(invocation.api_key, "claude-override-key");
        assert_ne!(
            invocation.api_key, "openai-settings-key",
            "must never send the Settings-configured provider's key to a different overridden provider"
        );
    }

    /// (b) Companion to the above: when the override provider *matches* the
    /// Settings-configured provider, `model_config.api_key` must be reused
    /// as-is - proving the "differs" branch above isn't just always fetching
    /// fresh regardless of the comparison. Also calls the real
    /// `resolve_live_llm_api_key` rather than a hand-reimplemented mux (see
    /// the test above).
    #[test]
    fn override_provider_matching_settings_reuses_settings_key() {
        let provider_override = Some("openai");
        let settings_provider = Some("openai");
        let settings_api_key = Some("openai-settings-key".to_string());

        let needs_fresh_key =
            provider_override_needs_fresh_key(provider_override, settings_provider);
        assert!(!needs_fresh_key);

        // A fresh-fetch stand-in that must never be used, given `needs_fresh_key`
        // is `false` here - proving `resolve_live_llm_api_key` genuinely branches
        // on `needs_fresh_key` rather than e.g. always preferring a `Some(..)`
        // fresh key when one happens to be provided.
        let unused_fresh_key = Some("should-never-be-used".to_string());

        let api_key = resolve_live_llm_api_key(needs_fresh_key, unused_fresh_key, settings_api_key);

        let resolved_provider = resolve_live_llm_provider(provider_override.or(settings_provider));
        let invocation = resolve_provider_invocation(
            &resolved_provider,
            "gpt-4o",
            api_key.as_deref(),
            None,
            None,
        )
        .expect("the reused Settings key should resolve successfully");

        assert_eq!(invocation.api_key, "openai-settings-key");
    }

    /// (d) Override provider has no saved key at all (fresh fetch returns
    /// nothing, mirroring `api_get_api_key`'s `Ok(String::new())` default for
    /// an unset key, or a failed lookup) - must produce the exact same
    /// "No API key configured for {provider}" error as the non-override case
    /// in `resolve_provider_invocation_groq_without_key_returns_clear_error`
    /// above, not some override-specific error message.
    #[test]
    fn override_provider_with_no_saved_key_returns_same_error_as_non_override_case() {
        let provider_override = Some("claude");
        let settings_provider: Option<&str> = None;

        let needs_fresh_key =
            provider_override_needs_fresh_key(provider_override, settings_provider);
        assert!(needs_fresh_key);

        // No key configured for "claude" - `api_get_api_key` would resolve to
        // an empty string, which `resolve_provider_invocation` already treats
        // as missing (see `resolve_provider_invocation_empty_key_is_treated_as_missing`).
        let api_key: Option<String> = None;

        let resolved_provider = resolve_live_llm_provider(provider_override.or(settings_provider));
        let err = resolve_provider_invocation(
            &resolved_provider,
            "claude-3-5-sonnet",
            api_key.as_deref(),
            None,
            None,
        )
        .expect_err("missing key for the override provider must be rejected");

        assert_eq!(
            err,
            "No API key configured for Claude — add it in Settings → Model Settings."
        );
    }
}
