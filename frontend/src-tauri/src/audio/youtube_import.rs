// YouTube audio import module - downloads a video's audio via the yt-dlp CLI
// (required to be pre-installed on PATH; never bundled or auto-installed) and
// runs it through the same decode -> VAD -> transcribe -> persist pipeline
// used by local-file import (see `audio::import_pipeline`).

use log::{debug, error, info, warn};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex as StdMutex;
use tauri::{AppHandle, Emitter, Runtime};
use tokio::io::{AsyncBufReadExt, BufReader};
use url::Url;
use which::which;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

use super::audio_processing::create_meeting_folder;
use super::common::{
    release_batch_import, try_acquire_batch_import, unload_engine_after_batch,
    write_import_metadata, YOUTUBE_IMPORT_IN_PROGRESS,
};
use super::ffmpeg::find_ffmpeg_path;
use super::import::{ImportError, ImportStarted};
use super::import_pipeline::{self, get_configured_provider, PipelineEvents};
use super::recording_preferences::get_default_recordings_folder;

/// Progress/warning events this import kind reports on. Payload shapes are defined by
/// `import_pipeline::run_transcription_pipeline`; identical to local-file import's
/// `import-progress` / `import-warning` shapes, just under different event names.
const EVENTS: PipelineEvents = PipelineEvents {
    progress: "youtube-import-progress",
    warning: "youtube-import-warning",
};

/// Signal cancellation of an in-progress YouTube import.
static YOUTUBE_IMPORT_CANCELLED: AtomicBool = AtomicBool::new(false);

/// Handle to the currently-running yt-dlp child process, if any. Lets
/// `cancel_youtube_import_command` kill it directly — killing the process closes its
/// stdout, which unblocks the progress-reading loop in `download_audio` naturally.
static YOUTUBE_IMPORT_CHILD: Lazy<StdMutex<Option<tokio::process::Child>>> =
    Lazy::new(|| StdMutex::new(None));

#[cfg(not(windows))]
const YT_DLP_EXECUTABLE_NAME: &str = "yt-dlp";
#[cfg(windows)]
const YT_DLP_EXECUTABLE_NAME: &str = "yt-dlp.exe";

const YT_DLP_NOT_FOUND_MSG: &str = "yt-dlp not found. Install it and ensure it's on your PATH (e.g. 'brew install yt-dlp' on macOS). See https://github.com/yt-dlp/yt-dlp#installation";

static YT_DLP_PATH: Lazy<Option<PathBuf>> = Lazy::new(find_yt_dlp_path_internal);

/// Find yt-dlp on PATH (or common fallback install dirs). yt-dlp is a required
/// external dependency: it is never bundled or auto-installed by this app, unlike
/// `audio::ffmpeg::find_ffmpeg_path`, which this mirrors minus the build-time
/// bundling/auto-download behavior.
pub fn find_yt_dlp_path() -> Result<PathBuf, String> {
    YT_DLP_PATH.clone().ok_or_else(|| YT_DLP_NOT_FOUND_MSG.to_string())
}

fn find_yt_dlp_path_internal() -> Option<PathBuf> {
    if let Ok(path) = which(YT_DLP_EXECUTABLE_NAME) {
        debug!("Found yt-dlp in PATH: {:?}", path);
        return Some(path);
    }
    debug!("yt-dlp not found in PATH");

    let mut fallback_dirs: Vec<PathBuf> = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        fallback_dirs.push(PathBuf::from(home).join(".local").join("bin"));
    }
    #[cfg(target_os = "macos")]
    {
        fallback_dirs.push(PathBuf::from("/opt/homebrew/bin"));
        fallback_dirs.push(PathBuf::from("/usr/local/bin"));
    }
    #[cfg(target_os = "linux")]
    {
        fallback_dirs.push(PathBuf::from("/usr/local/bin"));
        fallback_dirs.push(PathBuf::from("/usr/bin"));
    }

    for dir in fallback_dirs {
        let candidate = dir.join(YT_DLP_EXECUTABLE_NAME);
        if candidate.exists() {
            debug!("Found yt-dlp in fallback dir: {:?}", candidate);
            return Some(candidate);
        }
    }

    debug!("yt-dlp not found in PATH or any fallback directory");
    None
}

/// Metadata about a YouTube video, fetched without downloading it.
#[derive(Debug, Clone, Serialize)]
pub struct YoutubeVideoInfo {
    pub title: String,
    pub duration_seconds: Option<u64>,
    pub channel: Option<String>,
    pub thumbnail_url: Option<String>,
}

/// Result of a completed YouTube import, used to build the `youtube-import-complete`
/// event payload (shaped like local-file import's `import-complete` payload).
struct YoutubeImportOutcome {
    meeting_id: String,
    title: String,
    segments_count: usize,
    duration_seconds: f64,
}

/// Parse the download percentage out of a yt-dlp progress line, e.g.
/// `"[download]  42.3% of ~10.00MiB at 1.20MiB/s"` -> `Some(42)`. Returns `None` for
/// lines with no percentage (destination lines, merger lines, error lines, etc).
fn parse_ytdlp_progress_percentage(line: &str) -> Option<u32> {
    static PROGRESS_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"\[download\]\s+(\d+(?:\.\d+)?)%").expect("static regex is valid"));

    let captures = PROGRESS_RE.captures(line)?;
    let raw: f64 = captures.get(1)?.as_str().parse().ok()?;
    Some(raw.round().clamp(0.0, 100.0) as u32)
}

/// Check whether a string looks like a YouTube video URL (watch/shorts/youtu.be/embed/
/// live). Validation only — does not check that the video exists or is reachable.
fn is_valid_youtube_url(url: &str) -> bool {
    let parsed = match Url::parse(url) {
        Ok(u) => u,
        Err(_) => return false,
    };

    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return false;
    }

    let host_owned = match parsed.host_str() {
        Some(h) => h.to_lowercase(),
        None => return false,
    };
    let host = host_owned.strip_prefix("www.").unwrap_or(&host_owned);

    match host {
        "youtube.com" | "m.youtube.com" | "music.youtube.com" => {
            let path = parsed.path();
            (path == "/watch" && parsed.query_pairs().any(|(k, _)| k == "v"))
                || path.starts_with("/shorts/")
                || path.starts_with("/embed/")
                || path.starts_with("/live/")
        }
        "youtu.be" => !parsed.path().trim_start_matches('/').is_empty(),
        _ => false,
    }
}

pub fn is_youtube_import_in_progress() -> bool {
    YOUTUBE_IMPORT_IN_PROGRESS.load(Ordering::SeqCst)
}

/// Cancel an ongoing YouTube import: sets the cancellation flag and kills the yt-dlp
/// child process if the import is still in its download phase.
pub fn cancel_youtube_import() {
    YOUTUBE_IMPORT_CANCELLED.store(true, Ordering::SeqCst);
    if let Ok(mut guard) = YOUTUBE_IMPORT_CHILD.lock() {
        if let Some(child) = guard.as_mut() {
            let _ = child.start_kill();
        }
    }
}

/// RAII guard for the YOUTUBE_IMPORT_IN_PROGRESS flag. See
/// `common::try_acquire_batch_import` for why local-file and YouTube imports are
/// mutually exclusive despite tracking separate flags.
struct YoutubeImportGuard;

impl YoutubeImportGuard {
    fn acquire() -> Result<Self, String> {
        try_acquire_batch_import(&YOUTUBE_IMPORT_IN_PROGRESS)?;
        Ok(Self)
    }
}

impl Drop for YoutubeImportGuard {
    fn drop(&mut self) {
        release_batch_import(&YOUTUBE_IMPORT_IN_PROGRESS);
    }
}

/// Run `yt-dlp --dump-json --skip-download` and parse the result into a `YoutubeVideoInfo`.
async fn fetch_youtube_video_info(yt_dlp_path: &Path, url: &str) -> Result<YoutubeVideoInfo, String> {
    let yt_dlp_path = yt_dlp_path.to_path_buf();
    let url = url.to_string();

    let output = tokio::task::spawn_blocking(move || {
        let mut command = std::process::Command::new(&yt_dlp_path);
        command
            .arg("--dump-json")
            .arg("--skip-download")
            .arg("--no-playlist")
            .arg(&url)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        #[cfg(target_os = "windows")]
        {
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            command.creation_flags(CREATE_NO_WINDOW);
        }

        command.output()
    })
    .await
    .map_err(|e| format!("yt-dlp task join error: {}", e))?
    .map_err(|e| format!("Failed to run yt-dlp: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let trimmed = stderr.trim();
        return Err(if trimmed.is_empty() {
            format!("yt-dlp exited with status: {}", output.status)
        } else {
            format!("Could not fetch video info: {}", trimmed)
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(stdout.trim())
        .map_err(|e| format!("Failed to parse yt-dlp output: {}", e))?;

    Ok(YoutubeVideoInfo {
        title: json
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("Untitled YouTube Video")
            .to_string(),
        duration_seconds: json.get("duration").and_then(|v| v.as_u64()),
        channel: json
            .get("channel")
            .or_else(|| json.get("uploader"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        thumbnail_url: json.get("thumbnail").and_then(|v| v.as_str()).map(|s| s.to_string()),
    })
}

/// Download a YouTube video's audio as WAV into `meeting_folder/audio.wav` via yt-dlp,
/// emitting `youtube-import-progress` events for download percentage (scaled into the
/// 0-15% range, since the shared transcription pipeline picks up at 15%).
async fn download_audio<R: Runtime>(
    app: &AppHandle<R>,
    yt_dlp_path: &Path,
    ffmpeg_path: &Path,
    url: &str,
    meeting_folder: &Path,
) -> Result<PathBuf, String> {
    let output_template = meeting_folder.join("audio.%(ext)s");

    let mut command = tokio::process::Command::new(yt_dlp_path);
    command
        .arg("-x")
        .arg("--audio-format")
        .arg("wav")
        .arg("--audio-quality")
        .arg("0")
        .arg("--ffmpeg-location")
        .arg(ffmpeg_path)
        .arg("--newline")
        .arg("--no-playlist")
        .arg("-o")
        .arg(&output_template)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(target_os = "windows")]
    {
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = command
        .spawn()
        .map_err(|e| format!("Failed to spawn yt-dlp: {}", e))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Failed to capture yt-dlp stdout".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "Failed to capture yt-dlp stderr".to_string())?;

    {
        let mut guard = YOUTUBE_IMPORT_CHILD.lock().unwrap_or_else(|e| e.into_inner());
        *guard = Some(child);
    }

    let stderr_task = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        let mut collected = String::new();
        while let Ok(Some(line)) = lines.next_line().await {
            collected.push_str(&line);
            collected.push('\n');
        }
        collected
    });

    let mut stdout_lines = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = stdout_lines.next_line().await {
        if let Some(pct) = parse_ytdlp_progress_percentage(&line) {
            let overall = (pct as f32 * 0.15) as u32;
            import_pipeline::emit_progress(
                app,
                EVENTS.progress,
                "downloading",
                overall,
                &format!("Downloading audio... {}%", pct),
            );
        }
    }

    // yt-dlp's stdout has closed (process exited, or was killed by cancellation).
    let child_opt = {
        let mut guard = YOUTUBE_IMPORT_CHILD.lock().unwrap_or_else(|e| e.into_inner());
        guard.take()
    };

    let status = match child_opt {
        Some(mut child) => child
            .wait()
            .await
            .map_err(|e| format!("Failed waiting for yt-dlp: {}", e))?,
        None => return Err("yt-dlp process handle missing".to_string()),
    };

    let stderr_output = stderr_task.await.unwrap_or_default();

    if YOUTUBE_IMPORT_CANCELLED.load(Ordering::SeqCst) {
        return Err("YouTube import cancelled".to_string());
    }

    if !status.success() {
        let trimmed = stderr_output.trim();
        return Err(if trimmed.is_empty() {
            format!("yt-dlp exited with status: {}", status)
        } else {
            format!("yt-dlp failed: {}", trimmed)
        });
    }

    let audio_path = meeting_folder.join("audio.wav");
    if !audio_path.exists() {
        return Err(format!(
            "yt-dlp completed but expected output file not found: {}",
            audio_path.display()
        ));
    }

    Ok(audio_path)
}

/// Run the YouTube import: download audio via yt-dlp, then hand off to the shared
/// transcription pipeline (see `import_pipeline::run_transcription_pipeline`).
async fn run_youtube_import<R: Runtime>(
    app: AppHandle<R>,
    url: String,
    title: Option<String>,
    provider: String,
) -> Result<YoutubeImportOutcome, String> {
    let yt_dlp_path = find_yt_dlp_path()?;
    let ffmpeg_path = find_ffmpeg_path().ok_or_else(|| {
        "FFmpeg not found, but is required by yt-dlp to extract audio. Please install FFmpeg.".to_string()
    })?;

    info!("Starting YouTube import for {}", url);

    // Best-effort metadata lookup for the title/channel; a failure here shouldn't block
    // the import, since the user (or a fallback title) can still carry it through.
    let video_info = fetch_youtube_video_info(&yt_dlp_path, &url).await.ok();
    let resolved_title = title
        .filter(|t| !t.trim().is_empty())
        .or_else(|| video_info.as_ref().map(|i| i.title.clone()))
        .unwrap_or_else(|| "YouTube Import".to_string());
    let channel = video_info.and_then(|i| i.channel);

    import_pipeline::emit_progress(&app, EVENTS.progress, "downloading", 0, "Starting download...");

    if YOUTUBE_IMPORT_CANCELLED.load(Ordering::SeqCst) {
        return Err("YouTube import cancelled".to_string());
    }

    let base_folder = get_default_recordings_folder();
    let meeting_folder =
        create_meeting_folder(&base_folder, &resolved_title, false).map_err(|e| e.to_string())?;

    let audio_path = match download_audio(&app, &yt_dlp_path, &ffmpeg_path, &url, &meeting_folder).await {
        Ok(path) => path,
        Err(e) => {
            // Unlike a local-file copy, a failed download leaves no useful audio
            // artifact behind, so clean up immediately rather than leaving an empty
            // meeting folder around.
            let _ = std::fs::remove_dir_all(&meeting_folder);
            return Err(e);
        }
    };

    if YOUTUBE_IMPORT_CANCELLED.load(Ordering::SeqCst) {
        let _ = std::fs::remove_dir_all(&meeting_folder);
        return Err("YouTube import cancelled".to_string());
    }

    let output = import_pipeline::run_transcription_pipeline(
        &app,
        &meeting_folder,
        &audio_path,
        &resolved_title,
        None,
        None,
        Some(provider),
        EVENTS,
        &YOUTUBE_IMPORT_CANCELLED,
    )
    .await
    .map_err(|e| e.to_string())?;

    if let Err(e) = write_import_metadata(
        &meeting_folder,
        &output.meeting_id,
        &resolved_title,
        output.duration_seconds,
        "audio.wav",
        "youtube",
        Some(serde_json::json!({
            "source_url": url,
            "video_title": resolved_title,
            "channel": channel,
            "default_template": "youtube_summary",
        })),
    ) {
        warn!("Failed to write metadata.json: {}", e);
    }

    import_pipeline::emit_progress(&app, EVENTS.progress, "complete", 100, "Import complete");

    Ok(YoutubeImportOutcome {
        meeting_id: output.meeting_id,
        title: resolved_title,
        segments_count: output.segments.len(),
        duration_seconds: output.duration_seconds,
    })
}

// ============================================================================
// Tauri Commands
// ============================================================================

/// Validate a YouTube URL and fetch its video metadata (title/duration/channel/
/// thumbnail) without downloading it.
#[tauri::command]
pub async fn validate_youtube_url_command(url: String) -> Result<YoutubeVideoInfo, String> {
    if !is_valid_youtube_url(&url) {
        return Err("Not a valid YouTube URL. Expected a youtube.com or youtu.be link.".to_string());
    }
    let yt_dlp_path = find_yt_dlp_path()?;
    fetch_youtube_video_info(&yt_dlp_path, &url).await
}

/// Start importing a YouTube video's audio as a new meeting.
#[tauri::command]
pub async fn start_youtube_import_command<R: Runtime>(
    app: AppHandle<R>,
    url: String,
    title: Option<String>,
) -> Result<ImportStarted, String> {
    if !is_valid_youtube_url(&url) {
        return Err("Not a valid YouTube URL. Expected a youtube.com or youtu.be link.".to_string());
    }

    // Fast-path check; the authoritative check happens atomically inside
    // `YoutubeImportGuard::acquire` once the background task starts.
    if super::common::IMPORT_IN_PROGRESS.load(Ordering::SeqCst) || is_youtube_import_in_progress() {
        return Err("An import is already in progress".to_string());
    }

    tauri::async_runtime::spawn(async move {
        let guard = match YoutubeImportGuard::acquire() {
            Ok(g) => g,
            Err(e) => {
                error!("Failed to start YouTube import: {}", e);
                let _ = app.emit("youtube-import-error", ImportError { error: e });
                return;
            }
        };
        YOUTUBE_IMPORT_CANCELLED.store(false, Ordering::SeqCst);

        let provider = get_configured_provider(&app)
            .await
            .unwrap_or_else(|_| "whisper".to_string());
        let use_parakeet = provider == "parakeet";

        let result = run_youtube_import(app.clone(), url, title, provider).await;

        drop(guard);
        unload_engine_after_batch(use_parakeet).await;

        match result {
            Ok(outcome) => {
                let _ = app.emit(
                    "youtube-import-complete",
                    serde_json::json!({
                        "meeting_id": outcome.meeting_id,
                        "title": outcome.title,
                        "segments_count": outcome.segments_count,
                        "duration_seconds": outcome.duration_seconds,
                    }),
                );
            }
            Err(e) => {
                error!("YouTube import failed: {}", e);
                let _ = app.emit("youtube-import-error", ImportError { error: e });
            }
        }
    });

    Ok(ImportStarted {
        message: "YouTube import started".to_string(),
    })
}

#[tauri::command]
pub async fn cancel_youtube_import_command() -> Result<(), String> {
    if !is_youtube_import_in_progress() {
        return Err("No YouTube import in progress".to_string());
    }
    cancel_youtube_import();
    Ok(())
}

#[tauri::command]
pub async fn is_youtube_import_in_progress_command() -> Result<bool, String> {
    Ok(is_youtube_import_in_progress())
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- yt-dlp progress line parsing --

    #[test]
    fn test_parse_progress_percentage_basic() {
        assert_eq!(
            parse_ytdlp_progress_percentage("[download]  42.3% of ~10.00MiB at 1.20MiB/s"),
            Some(42)
        );
    }

    #[test]
    fn test_parse_progress_percentage_100_percent() {
        assert_eq!(
            parse_ytdlp_progress_percentage("[download] 100% of 10.00MiB in 00:00:05"),
            Some(100)
        );
    }

    #[test]
    fn test_parse_progress_percentage_with_eta() {
        assert_eq!(
            parse_ytdlp_progress_percentage("[download]  67.8% of ~15.00MiB at 2.00MiB/s ETA 00:03"),
            Some(68)
        );
    }

    #[test]
    fn test_parse_progress_percentage_no_percentage_present() {
        assert_eq!(
            parse_ytdlp_progress_percentage("[download] Destination: audio.webm"),
            None
        );
    }

    #[test]
    fn test_parse_progress_percentage_non_download_line() {
        assert_eq!(
            parse_ytdlp_progress_percentage("[Merger] Merging formats into \"audio.wav\""),
            None
        );
    }

    #[test]
    fn test_parse_progress_percentage_error_line() {
        assert_eq!(
            parse_ytdlp_progress_percentage("ERROR: [youtube] abc123: Video unavailable"),
            None
        );
    }

    #[test]
    fn test_parse_progress_percentage_empty_line() {
        assert_eq!(parse_ytdlp_progress_percentage(""), None);
    }

    // -- YouTube URL validation --

    #[test]
    fn test_is_valid_youtube_url_watch() {
        assert!(is_valid_youtube_url("https://www.youtube.com/watch?v=dQw4w9WgXcQ"));
        assert!(is_valid_youtube_url("https://youtube.com/watch?v=dQw4w9WgXcQ"));
        assert!(is_valid_youtube_url("http://youtube.com/watch?v=dQw4w9WgXcQ"));
    }

    #[test]
    fn test_is_valid_youtube_url_short_link() {
        assert!(is_valid_youtube_url("https://youtu.be/dQw4w9WgXcQ"));
    }

    #[test]
    fn test_is_valid_youtube_url_shorts() {
        assert!(is_valid_youtube_url("https://www.youtube.com/shorts/dQw4w9WgXcQ"));
    }

    #[test]
    fn test_is_valid_youtube_url_rejects_non_youtube() {
        assert!(!is_valid_youtube_url("https://vimeo.com/12345"));
        assert!(!is_valid_youtube_url("https://example.com/watch?v=dQw4w9WgXcQ"));
    }

    #[test]
    fn test_is_valid_youtube_url_rejects_malformed() {
        assert!(!is_valid_youtube_url("not a url"));
        assert!(!is_valid_youtube_url(""));
        assert!(!is_valid_youtube_url("ftp://youtube.com/watch?v=dQw4w9WgXcQ"));
    }

    #[test]
    fn test_is_valid_youtube_url_rejects_watch_without_video_id() {
        assert!(!is_valid_youtube_url("https://www.youtube.com/watch"));
    }

    #[test]
    fn test_is_valid_youtube_url_rejects_bare_youtu_be() {
        assert!(!is_valid_youtube_url("https://youtu.be/"));
    }
}
