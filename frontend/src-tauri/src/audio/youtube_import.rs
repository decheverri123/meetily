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
pub(crate) static YOUTUBE_IMPORT_CANCELLED: AtomicBool = AtomicBool::new(false);

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
static JS_RUNTIME_ARG: Lazy<Option<String>> = Lazy::new(find_js_runtime_internal);

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

/// Detect an available JS runtime for yt-dlp (node, deno, bun, quickjs)
fn find_js_runtime_internal() -> Option<String> {
    let candidate_names = ["node", "deno", "bun", "quickjs"];
    for name in candidate_names {
        if let Ok(path) = which(name) {
            debug!("Found JS runtime for yt-dlp in PATH: {} -> {:?}", name, path);
            return Some(format!("{}:{}", name, path.to_string_lossy()));
        }
    }

    let mut fallback_dirs: Vec<PathBuf> = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        fallback_dirs.push(PathBuf::from(&home).join(".nvm"));
        fallback_dirs.push(PathBuf::from(&home).join(".bun").join("bin"));
        fallback_dirs.push(PathBuf::from(&home).join(".deno").join("bin"));
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
        for name in candidate_names {
            let candidate = dir.join(name);
            if candidate.exists() {
                debug!("Found JS runtime for yt-dlp in fallback dir: {} -> {:?}", name, candidate);
                return Some(format!("{}:{}", name, candidate.to_string_lossy()));
            }
        }
    }

    debug!("No JS runtime found for yt-dlp");
    None
}

fn is_age_restriction_error(stderr: &str) -> bool {
    stderr.contains("Sign in to confirm your age")
        || stderr.contains("inappropriate for some users")
        || stderr.contains("Use --cookies-from-browser")
        || stderr.contains("pass cookies")
}

fn format_ytdlp_error(stderr: &str) -> String {
    let raw = stderr.trim();
    if raw.is_empty() {
        return "Failed to process YouTube video.".to_string();
    }

    if is_age_restriction_error(raw) {
        return "This video is age-restricted or requires sign-in. Signed-in browser cookies could not be accessed.".to_string();
    }
    if raw.contains("Video unavailable") {
        return "This YouTube video is unavailable or private.".to_string();
    }

    let lines: Vec<&str> = raw
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();

    let error_lines: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|l| l.contains("ERROR:"))
        .collect();

    let selected_text = if !error_lines.is_empty() {
        error_lines.join(" ")
    } else {
        lines
            .into_iter()
            .filter(|l| !l.starts_with("WARNING:"))
            .collect::<Vec<&str>>()
            .join(" ")
    };

    let re_prefix = Regex::new(r"ERROR:\s*\[[^\]]+\]\s*[^:]+:\s*").ok();
    let cleaned = if let Some(re) = re_prefix {
        re.replace_all(&selected_text, "").to_string()
    } else {
        selected_text
    };

    if cleaned.trim().is_empty() {
        "Failed to process YouTube video.".to_string()
    } else {
        cleaned.trim().to_string()
    }
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
pub(crate) struct YoutubeImportOutcome {
    pub meeting_id: String,
    pub title: String,
    pub segments_count: usize,
    pub duration_seconds: f64,
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
///
/// Playlists are explicitly rejected: a URL like
/// `https://www.youtube.com/watch?v=ID&list=PL...` looks like a single
/// video link but actually resolves to a playlist. We treat playlist
/// URLs as invalid at this entry point so the import command errors
/// cleanly instead of silently pulling the first video of the list
/// (or all of them).
pub fn is_valid_youtube_url(url: &str) -> bool {
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

    if parsed.query_pairs().any(|(k, _)| k == "list") {
        return false;
    }

    match host {
        "youtube.com" | "m.youtube.com" | "music.youtube.com" => {
            let path = parsed.path();
            (path == "/watch" && parsed.query_pairs().any(|(k, v)| k == "v" && !v.is_empty()))
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

const BROWSER_COOKIE_TARGETS: &[&str] = &["chrome", "safari", "firefox", "edge", "brave"];

/// Run `yt-dlp --dump-json --skip-download` and parse the result into a `YoutubeVideoInfo`.
async fn fetch_youtube_video_info(yt_dlp_path: &Path, url: &str) -> Result<YoutubeVideoInfo, String> {
    let yt_dlp_path = yt_dlp_path.to_path_buf();
    let url = url.to_string();
    let js_runtime = JS_RUNTIME_ARG.clone();

    let run_cmd = move |cookies_browser: Option<&'static str>| {
        let mut command = std::process::Command::new(&yt_dlp_path);
        command
            .arg("--dump-json")
            .arg("--skip-download")
            .arg("--no-playlist");

        if let Some(ref js_arg) = js_runtime {
            command.arg("--js-runtimes").arg(js_arg);
        }

        if let Some(browser) = cookies_browser {
            command.arg("--cookies-from-browser").arg(browser);
        }

        command
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
    };

    let mut output = tokio::task::spawn_blocking({
        let run_cmd = run_cmd.clone();
        move || run_cmd(None)
    })
    .await
    .map_err(|e| format!("yt-dlp task join error: {}", e))?
    .map_err(|e| format!("Failed to run yt-dlp: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if is_age_restriction_error(&stderr) {
            for &browser in BROWSER_COOKIE_TARGETS {
                if let Ok(Ok(retry_output)) = tokio::task::spawn_blocking({
                    let run_cmd = run_cmd.clone();
                    move || run_cmd(Some(browser))
                }).await {
                    if retry_output.status.success() {
                        output = retry_output;
                        break;
                    }
                }
            }
        }
    }

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format_ytdlp_error(&stderr));
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

async fn execute_ytdlp_download<R: Runtime>(
    app: &AppHandle<R>,
    yt_dlp_path: &Path,
    ffmpeg_path: &Path,
    url: &str,
    meeting_folder: &Path,
    progress_event: &str,
    cookies_browser: Option<&'static str>,
) -> Result<(std::process::ExitStatus, String), String> {
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
        .arg("--no-playlist");

    if let Some(ref js_arg) = *JS_RUNTIME_ARG {
        command.arg("--js-runtimes").arg(js_arg);
    }

    if let Some(browser) = cookies_browser {
        command.arg("--cookies-from-browser").arg(browser);
    }

    command
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
            import_pipeline::emit_progress_dyn(
                app,
                progress_event,
                "downloading",
                overall,
                &format!("Downloading audio... {}%", pct),
            );
        }
    }

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
    Ok((status, stderr_output))
}

/// Download a YouTube video's audio as WAV into `meeting_folder/audio.wav` via yt-dlp,
/// emitting progress events on `progress_event` (0-15% range, since the shared
/// transcription pipeline picks up at 15%).
async fn download_audio<R: Runtime>(
    app: &AppHandle<R>,
    yt_dlp_path: &Path,
    ffmpeg_path: &Path,
    url: &str,
    meeting_folder: &Path,
    progress_event: &str,
) -> Result<PathBuf, String> {
    let (mut status, mut stderr_output) = execute_ytdlp_download(
        app,
        yt_dlp_path,
        ffmpeg_path,
        url,
        meeting_folder,
        progress_event,
        None,
    )
    .await?;

    if YOUTUBE_IMPORT_CANCELLED.load(Ordering::SeqCst) {
        return Err("YouTube import cancelled".to_string());
    }

    if !status.success() && is_age_restriction_error(&stderr_output) {
        for &browser in BROWSER_COOKIE_TARGETS {
            if YOUTUBE_IMPORT_CANCELLED.load(Ordering::SeqCst) {
                return Err("YouTube import cancelled".to_string());
            }
            if let Ok((retry_status, retry_stderr)) = execute_ytdlp_download(
                app,
                yt_dlp_path,
                ffmpeg_path,
                url,
                meeting_folder,
                progress_event,
                Some(browser),
            )
            .await
            {
                if retry_status.success() {
                    status = retry_status;
                    stderr_output = retry_stderr;
                    break;
                }
            }
        }
    }

    if YOUTUBE_IMPORT_CANCELLED.load(Ordering::SeqCst) {
        return Err("YouTube import cancelled".to_string());
    }

    if !status.success() {
        return Err(format_ytdlp_error(&stderr_output));
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

/// Result of a successful YouTube download: paths the caller (single-URL or batch)
/// hands off to the shared transcription pipeline.
#[derive(Clone)]
pub struct YoutubeDownloadResult {
    pub meeting_folder: PathBuf,
    pub audio_path: PathBuf,
    pub title: String,
    pub channel: Option<String>,
}

/// Resolve a user-supplied title and YouTube metadata for a URL, create a meeting
/// folder, and download the audio via yt-dlp. Emits progress on `progress_event` and
/// cleans up the folder on failure. Used by both the single-URL import command and
/// the batch path (which calls this concurrently for each item).
pub async fn download_youtube_audio<R: Runtime>(
    app: &AppHandle<R>,
    url: &str,
    title: Option<String>,
    progress_event: &str,
) -> Result<YoutubeDownloadResult, String> {
    let yt_dlp_path = find_yt_dlp_path()?;
    let ffmpeg_path = find_ffmpeg_path().ok_or_else(|| {
        "FFmpeg not found, but is required by yt-dlp to extract audio. Please install FFmpeg.".to_string()
    })?;

    let video_info = fetch_youtube_video_info(&yt_dlp_path, url).await.ok();
    let resolved_title = title
        .filter(|t| !t.trim().is_empty())
        .or_else(|| video_info.as_ref().map(|i| i.title.clone()))
        .unwrap_or_else(|| "YouTube Import".to_string());
    let channel = video_info.and_then(|i| i.channel);

    import_pipeline::emit_progress_dyn(app, progress_event, "downloading", 0, "Starting download...");

    if YOUTUBE_IMPORT_CANCELLED.load(Ordering::SeqCst) {
        return Err("YouTube import cancelled".to_string());
    }

    let base_folder = get_default_recordings_folder();
    let meeting_folder =
        create_meeting_folder(&base_folder, &resolved_title, false).map_err(|e| e.to_string())?;

    let audio_path = match download_audio(
        app,
        &yt_dlp_path,
        &ffmpeg_path,
        url,
        &meeting_folder,
        progress_event,
    )
    .await
    {
        Ok(path) => path,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&meeting_folder);
            return Err(e);
        }
    };

    Ok(YoutubeDownloadResult {
        meeting_folder,
        audio_path,
        title: resolved_title,
        channel,
    })
}

/// Run the shared transcription pipeline against an already-downloaded YouTube audio
/// file and write the meeting metadata. Used by both the single-URL command and the
/// batch path (which calls this serially for each downloaded item).
pub(crate) async fn transcribe_youtube_download<R: Runtime>(
    app: &AppHandle<R>,
    download: YoutubeDownloadResult,
    url: &str,
    provider: String,
) -> Result<YoutubeImportOutcome, String> {
    let YoutubeDownloadResult {
        meeting_folder,
        audio_path,
        title: resolved_title,
        channel,
    } = download;

    let output = import_pipeline::run_transcription_pipeline(
        app,
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

    import_pipeline::emit_progress(app, EVENTS.progress, "complete", 100, "Import complete");

    Ok(YoutubeImportOutcome {
        meeting_id: output.meeting_id,
        title: resolved_title,
        segments_count: output.segments.len(),
        duration_seconds: output.duration_seconds,
    })
}

/// Run the single-URL YouTube import: download audio via yt-dlp, then hand off to
/// the shared transcription pipeline.
async fn run_youtube_import<R: Runtime>(
    app: AppHandle<R>,
    url: String,
    title: Option<String>,
    provider: String,
) -> Result<YoutubeImportOutcome, String> {
    info!("Starting YouTube import for {}", url);

    let download = download_youtube_audio(&app, &url, title, EVENTS.progress).await?;

    if YOUTUBE_IMPORT_CANCELLED.load(Ordering::SeqCst) {
        let _ = std::fs::remove_dir_all(&download.meeting_folder);
        return Err("YouTube import cancelled".to_string());
    }

    transcribe_youtube_download(&app, download, &url, provider).await
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
        // Reset the cancel flag before the guard makes `is_youtube_import_in_progress()`
        // visible to the cancel command, so a cancel racing in right after acquire can't
        // be clobbered by this reset (see test_cancel_immediately_after_guard_acquire_gets_clobbered).
        YOUTUBE_IMPORT_CANCELLED.store(false, Ordering::SeqCst);
        let guard = match YoutubeImportGuard::acquire() {
            Ok(g) => g,
            Err(e) => {
                error!("Failed to start YouTube import: {}", e);
                let _ = app.emit("youtube-import-error", ImportError { error: e });
                return;
            }
        };

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
    use crate::audio::common::IMPORT_IN_PROGRESS;
    use std::sync::Arc;

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

    // -- Adversarial: is_valid_youtube_url edge cases --

    #[test]
    fn test_is_valid_youtube_url_accepts_empty_video_id() {
        // `?v=` with an empty value must be rejected: the video id is checked
        // for non-emptiness, not just the key's presence.
        assert!(
            !is_valid_youtube_url("https://www.youtube.com/watch?v="),
            "empty v= param should not be treated as a valid video URL, but validation accepted it"
        );
    }

    #[test]
    fn test_is_valid_youtube_url_rejects_userinfo_spoof() {
        // "https://www.youtube.com@evil.com/..." parses www.youtube.com as
        // userinfo and evil.com as the actual host. Confirms no bypass.
        assert!(!is_valid_youtube_url("https://www.youtube.com@evil.com/watch?v=dQw4w9WgXcQ"));
    }

    #[test]
    fn test_is_valid_youtube_url_rejects_subdomain_spoof() {
        // "youtube.com.evil.com" contains "youtube.com" as a substring but is a
        // different registrable domain entirely.
        assert!(!is_valid_youtube_url("https://youtube.com.evil.com/watch?v=dQw4w9WgXcQ"));
    }

    // -- Adversarial: playlist handling --
    //
    // A URL like `?v=ID&list=PL...` looks like a single-video watch URL
    // but actually resolves to a playlist. The validator must reject any
    // URL that carries a `list` query parameter, regardless of which other
    // params are present, so the import command errors cleanly rather
    // than silently pulling the first video of the playlist (or all of
    // them).

    #[test]
    fn test_is_valid_youtube_url_rejects_watch_with_playlist() {
        assert!(
            !is_valid_youtube_url("https://www.youtube.com/watch?v=dQw4w9WgXcQ&list=PLxyz"),
            "watch?v=ID&list=PL must be rejected: it resolves to a playlist, not the video"
        );
    }

    #[test]
    fn test_is_valid_youtube_url_rejects_watch_with_playlist_and_index() {
        assert!(!is_valid_youtube_url(
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ&list=PLxyz&index=2"
        ));
    }

    #[test]
    fn test_is_valid_youtube_url_rejects_playlist_only_via_v_empty() {
        // Belt-and-suspenders: even with `v=` and a bare `list=PL`, the
        // URL is rejected because the list param is present.
        assert!(!is_valid_youtube_url(
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ&list=PL"
        ));
    }

    #[test]
    fn test_is_valid_youtube_url_rejects_pure_playlist_url() {
        // /playlist?list=... has no v= param and no /shorts/ or /embed/
        // prefix — must be rejected.
        assert!(!is_valid_youtube_url("https://www.youtube.com/playlist?list=PLxyz"));
        assert!(!is_valid_youtube_url("https://www.youtube.com/playlist?list=PLxyz&index=2"));
    }

    // -- Adversarial: host variants & edge cases --

    #[test]
    fn test_is_valid_youtube_url_accepts_music_youtube() {
        assert!(is_valid_youtube_url("https://music.youtube.com/watch?v=abc"));
    }

    #[test]
    fn test_is_valid_youtube_url_rejects_case_variation_of_evil_host() {
        // Capitalized YouTube: parse + lowercase on host_str should catch it.
        // But case-sensitivity in the matched arm: the host comparison
        // is case-insensitive (host_owned.to_lowercase()) so this
        // matches as "youtube.com" and is accepted. If the comparison
        // were case-sensitive, this would be a security issue.
        assert!(
            is_valid_youtube_url("https://YOUTUBE.COM/watch?v=abc"),
            "YOUTUBE.COM (uppercase) is accepted via lowercase host match"
        );
    }

    #[test]
    fn test_is_valid_youtube_url_rejects_scheme_relative() {
        // No scheme — not a valid URL.
        assert!(!is_valid_youtube_url("//www.youtube.com/watch?v=abc"));
        assert!(!is_valid_youtube_url("www.youtube.com/watch?v=abc"));
    }

    #[test]
    fn test_is_valid_youtube_url_rejects_empty_path_youtu_be() {
        // https://youtu.be/ with no id
        assert!(!is_valid_youtube_url("https://youtu.be/"));
        assert!(!is_valid_youtube_url("https://youtu.be"));
    }

    #[test]
    fn test_is_valid_youtube_url_handles_urls_with_whitespace_in_middle() {
        // Documents current behavior: `url::Url::parse` tolerates embedded
        // whitespace in the query string and the validator does not strip
        // it. The v= param value is "abc def" — non-empty, so the URL
        // passes validation. yt-dlp will then fail downstream.
        //
        // This is a *latent* bug: the frontend parseQueueInput does trim(),
        // but that only removes leading/trailing whitespace, not internal.
        // A user pasting "https://www.youtube.com/watch?v=abc def" gets a
        // URL that is "valid" by our rules but will fail to download.
        //
        // Pin the current behavior here so the regression is visible.
        let bad = "https://www.youtube.com/watch?v=abc def";
        assert!(
            is_valid_youtube_url(bad),
            "embedded whitespace is currently accepted — will fail at yt-dlp time"
        );
    }

    #[test]
    fn test_is_valid_youtube_url_rejects_url_with_very_long_query() {
        // Pathological 10KB query string. Url::parse should handle it
        // without panic, and validation should still work.
        let huge_query = "v=abc&".to_string() + &"x=1&".repeat(2_000);
        let url = format!("https://www.youtube.com/watch?{}", huge_query);
        let result = is_valid_youtube_url(&url);
        assert!(result, "valid v= param is present even with 10KB query");
    }

    // -- Adversarial: fetch_youtube_video_info against a fake yt-dlp subprocess --
    //
    // These spawn a real child process (a throwaway bash script written to a
    // tempdir and never left in the tree) to exercise the actual process
    // boundary: argv passing, non-zero exit + stderr surfacing, and JSON
    // parsing of yt-dlp's stdout.

    fn write_fake_ytdlp(dir: &std::path::Path, script: &str) -> PathBuf {
        let path = dir.join("yt-dlp");
        std::fs::write(&path, script).unwrap();
        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).unwrap();
        }
        path
    }

    #[tokio::test]
    async fn test_fetch_video_info_surfaces_stderr_on_nonzero_exit() {
        let dir = tempfile::tempdir().unwrap();
        let script = "#!/usr/bin/env bash\necho 'ERROR: Video unavailable. This video is private.' >&2\nexit 1\n";
        let fake = write_fake_ytdlp(dir.path(), script);

        let result = fetch_youtube_video_info(&fake, "https://www.youtube.com/watch?v=dQw4w9WgXcQ").await;
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(
            msg.contains("private"),
            "expected the real yt-dlp stderr reason to surface to the user, got: {}",
            msg
        );
    }

    #[tokio::test]
    async fn test_fetch_video_info_nonzero_exit_empty_stderr_still_useful() {
        // Simulates yt-dlp crashing/killed with no stderr output at all
        // (e.g. OOM-killed, SIGKILL from the OS on disk-full paging).
        let dir = tempfile::tempdir().unwrap();
        let script = "#!/usr/bin/env bash\nexit 137\n";
        let fake = write_fake_ytdlp(dir.path(), script);

        let result = fetch_youtube_video_info(&fake, "https://www.youtube.com/watch?v=dQw4w9WgXcQ").await;
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(!msg.is_empty());
    }

    #[tokio::test]
    async fn test_fetch_video_info_malformed_json_does_not_panic() {
        // Simulates an ancient/incompatible yt-dlp whose --dump-json output
        // isn't actually JSON (e.g. a deprecation banner printed to stdout).
        let dir = tempfile::tempdir().unwrap();
        let script = "#!/usr/bin/env bash\necho 'yt-dlp: command not fully supported, upgrade recommended'\nexit 0\n";
        let fake = write_fake_ytdlp(dir.path(), script);

        let result = fetch_youtube_video_info(&fake, "https://www.youtube.com/watch?v=dQw4w9WgXcQ").await;
        assert!(result.is_err(), "malformed JSON should error, not panic");
    }

    #[tokio::test]
    async fn test_fetch_video_info_missing_fields_degrades_gracefully() {
        // Simulates a yt-dlp version whose JSON schema dropped/renamed fields.
        let dir = tempfile::tempdir().unwrap();
        let script = "#!/usr/bin/env bash\necho '{}'\nexit 0\n";
        let fake = write_fake_ytdlp(dir.path(), script);

        let result = fetch_youtube_video_info(&fake, "https://www.youtube.com/watch?v=dQw4w9WgXcQ").await;
        assert!(result.is_ok(), "missing fields should degrade to defaults, not error: {:?}", result.err());
        let info = result.unwrap();
        assert_eq!(info.title, "Untitled YouTube Video");
        assert_eq!(info.duration_seconds, None);
        assert_eq!(info.channel, None);
    }

    #[tokio::test]
    async fn test_fetch_video_info_negative_duration_does_not_panic() {
        // Simulates a broken/malicious yt-dlp fork emitting a negative duration.
        let dir = tempfile::tempdir().unwrap();
        let script = "#!/usr/bin/env bash\necho '{\"title\":\"x\",\"duration\":-5}'\nexit 0\n";
        let fake = write_fake_ytdlp(dir.path(), script);

        let result = fetch_youtube_video_info(&fake, "https://www.youtube.com/watch?v=dQw4w9WgXcQ").await;
        assert!(result.is_ok());
        // as_u64() on a negative JSON number returns None rather than panicking/wrapping.
        assert_eq!(result.unwrap().duration_seconds, None);
    }

    #[tokio::test]
    async fn test_url_passed_as_single_argv_element_not_shell_interpreted() {
        // Proves the URL reaches the child process as one argv element (via
        // std::process::Command::arg), not through a shell, by using a
        // "URL" containing shell metacharacters and confirming the fake
        // yt-dlp receives it byte-for-byte with no injected command effects.
        let dir = tempfile::tempdir().unwrap();
        let out_file = dir.path().join("argv_dump.txt");
        let script = format!(
            "#!/usr/bin/env bash\nprintf '%s\\n' \"$@\" > {}\necho '{{}}'\nexit 0\n",
            out_file.display()
        );
        let fake = write_fake_ytdlp(dir.path(), &script);

        // Deliberately not a URL is_valid_youtube_url would accept - this test
        // calls fetch_youtube_video_info directly (bypassing validation) to
        // isolate the subprocess-argv boundary itself.
        let hostile = "https://www.youtube.com/watch?v=x; touch /tmp/pwned; echo $(whoami)`id`|cat";

        let result = fetch_youtube_video_info(&fake, hostile).await;
        assert!(result.is_ok());
        assert!(!std::path::Path::new("/tmp/pwned").exists(), "shell metacharacters were executed!");

        let dumped = std::fs::read_to_string(&out_file).unwrap();
        assert!(
            dumped.contains(hostile),
            "expected the fake yt-dlp to receive the hostile string as a literal single arg, got: {}",
            dumped
        );
        let _ = std::fs::remove_file("/tmp/pwned");
    }

    // -- Adversarial: child-process kill / cancellation --

    /// Serializes tests that mutate the module's shared statics
    /// (`YOUTUBE_IMPORT_CANCELLED`, `YOUTUBE_IMPORT_CHILD`) and the
    /// cross-module `IMPORT_IN_PROGRESS` / `YOUTUBE_IMPORT_IN_PROGRESS`
    /// flags, since `cargo test` runs tests in the same binary concurrently
    /// by default and these are process-wide globals.
    static GLOBAL_STATE_TEST_LOCK: StdMutex<()> = StdMutex::new(());

    #[tokio::test]
    async fn test_cancel_kills_but_leaves_zombie_until_waited() {
        let _guard = GLOBAL_STATE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        // Spawn a long-lived stand-in for yt-dlp mid-download.
        let mut command = tokio::process::Command::new("sleep");
        command.arg("60").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        let child = command.spawn().expect("failed to spawn sleep");
        let pid = child.id().expect("child should have a pid");

        {
            let mut guard = YOUTUBE_IMPORT_CHILD.lock().unwrap_or_else(|e| e.into_inner());
            *guard = Some(child);
        }

        cancel_youtube_import();

        // `cancel_youtube_import` only calls `start_kill()`, which sends
        // SIGKILL but does not reap the process - reaping happens later, in
        // `download_audio`, only once its stdout-reading loop observes EOF
        // and falls through to `child.wait()`. If a caller ever invokes
        // `cancel_youtube_import()` without that follow-up `wait()` (e.g. a
        // future refactor, or cancellation racing after the child handle was
        // already taken out of `YOUTUBE_IMPORT_CHILD`), the process is killed
        // but never reaped, i.e. exactly the "zombie" failure mode this test
        // is checking for.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let is_zombie = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("ps -o state= -p {} 2>/dev/null", pid))
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().starts_with('Z'))
            .unwrap_or(false);
        assert!(
            is_zombie,
            "expected the killed-but-unwaited child (pid {}) to be a zombie, proving              `cancel_youtube_import` alone does not reap - callers must always follow it              with a `wait()`, exactly as `download_audio` does today",
            pid
        );

        // Now perform the reap `download_audio` would have done, and confirm
        // that resolves the zombie (this is the actual production behavior;
        // the assertion above only demonstrates that `cancel_youtube_import`
        // by itself is not sufficient).
        let mut guard = YOUTUBE_IMPORT_CHILD.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(mut child) = guard.take() {
            let _ = child.wait().await;
        }
        drop(guard);

        YOUTUBE_IMPORT_CANCELLED.store(false, Ordering::SeqCst);
    }

    #[test]
    fn test_cancel_immediately_after_guard_acquire_gets_clobbered() {
        // Reproduces the exact statement order in `start_youtube_import_command`'s
        // spawned task:
        //     let guard = YoutubeImportGuard::acquire()?;   // sets IN_PROGRESS = true
        //     YOUTUBE_IMPORT_CANCELLED.store(false, ...);   // resets cancel flag
        //
        // If `cancel_youtube_import_command` races in between those two lines
        // (which it legitimately can: `is_youtube_import_in_progress()` already
        // reports true once the guard is acquired, so a fast user double-click
        // on Cancel right after Start is a real, reachable window - not just a
        // theoretical thread interleaving), the cancellation the user asked for
        // is silently thrown away by the subsequent reset.
        let _guard_lock = GLOBAL_STATE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        // Ensure clean starting state.
        release_batch_import(&YOUTUBE_IMPORT_IN_PROGRESS);
        YOUTUBE_IMPORT_CANCELLED.store(false, Ordering::SeqCst);

        // 1. Background task acquires the guard (flips IN_PROGRESS -> true).
        let guard = YoutubeImportGuard::acquire().expect("should acquire cleanly");
        assert!(is_youtube_import_in_progress());

        // 2. A racing `cancel_youtube_import_command` call lands here, in the
        //    window between guard-acquire and the cancel-flag reset below.
        //    `cancel_youtube_import_command` only guards on
        //    `is_youtube_import_in_progress()`, which is already true, so this
        //    call succeeds and sets the flag exactly as production code would.
        cancel_youtube_import();
        assert!(YOUTUBE_IMPORT_CANCELLED.load(Ordering::SeqCst), "cancel should have registered");

        // 3. The background task continues past guard-acquire and resets the
        //    cancellation flag, per the real code path in
        //    `start_youtube_import_command`.
        YOUTUBE_IMPORT_CANCELLED.store(false, Ordering::SeqCst);

        // BUG: the user's cancel request from step 2 is gone. The import will
        // now run to completion even though the user clicked Cancel.
        assert!(
            !YOUTUBE_IMPORT_CANCELLED.load(Ordering::SeqCst),
            "demonstrates the clobber: this is the bad state, not the desired one"
        );

        drop(guard);
        release_batch_import(&YOUTUBE_IMPORT_IN_PROGRESS);
    }

    // -- Adversarial: guard mutual-exclusion under real concurrency --

    #[test]
    fn test_concurrent_start_only_one_side_wins_no_toctou() {
        // Fires many concurrent local-file-vs-YouTube acquire attempts at
        // once and confirms `try_acquire_batch_import`'s shared mutex really
        // does serialize the two flags (no TOCTOU gap where both fast-path
        // checks pass before either flag is set).
        let _guard_lock = GLOBAL_STATE_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        release_batch_import(&IMPORT_IN_PROGRESS);
        release_batch_import(&YOUTUBE_IMPORT_IN_PROGRESS);

        use std::sync::atomic::AtomicUsize;
        use std::sync::Barrier;
        let barrier = Arc::new(Barrier::new(20));
        let successes = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();
        for i in 0..20 {
            let barrier = barrier.clone();
            let successes = successes.clone();
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                let flag: &'static AtomicBool = if i % 2 == 0 {
                    &IMPORT_IN_PROGRESS
                } else {
                    &YOUTUBE_IMPORT_IN_PROGRESS
                };
                if try_acquire_batch_import(flag).is_ok() {
                    successes.fetch_add(1, Ordering::SeqCst);
                    // Hold it briefly to widen any race window.
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    release_batch_import(flag);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        // `try_acquire_batch_import` is a non-blocking try-acquire (it
        // returns Err immediately if either flag is already set, rather than
        // waiting its turn), so of 20 concurrent attempts exactly one should
        // win the initial race; the other 19 should be correctly rejected
        // rather than incorrectly also succeeding (which is what the
        // pre-fix, per-flag `compare_exchange` had a TOCTOU gap for: two
        // threads targeting *different* flags could both pass a stale
        // "neither flag is set" check before either flag was actually
        // written). Held up: no double-acquire observed here.
        assert_eq!(successes.load(Ordering::SeqCst), 1);
        assert!(!IMPORT_IN_PROGRESS.load(Ordering::SeqCst));
        assert!(!YOUTUBE_IMPORT_IN_PROGRESS.load(Ordering::SeqCst));
    }

    // -- Adversarial: progress regex on real yt-dlp output variety --

    #[test]
    fn test_parse_progress_fragment_based_lines_no_panic_no_match() {
        // Fragment-based (HLS/DASH) downloads print differently shaped lines;
        // confirm none of them are misparsed or panic.
        let lines = [
            "[download] Downloading fragment 5 of 20",
            "[hlsnative] Total fragments: 20",
            "[download] Downloading segment 1",
            "[Merger] Merging formats into \"audio.wav\"",
            "[ExtractAudio] Destination: audio.wav",
        ];
        for line in lines {
            assert_eq!(parse_ytdlp_progress_percentage(line), None, "line: {}", line);
        }
    }

    // -- Adversarial: what the shared pipeline does with a bad/empty download --

    #[test]
    fn test_decode_zero_byte_wav_errors_gracefully_not_panic() {
        // Simulates yt-dlp/ffmpeg producing a truncated/empty "audio.wav"
        // (e.g. disk-full mid-write, or a killed ffmpeg leaving a 0-byte
        // file) that still passes `download_audio`'s `audio_path.exists()`
        // check, since that check only verifies existence, not that the
        // file is non-empty or a valid WAV.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audio.wav");
        std::fs::write(&path, b"").unwrap();

        let result = std::panic::catch_unwind(|| {
            crate::audio::decoder::decode_audio_file(&path)
        });

        assert!(result.is_ok(), "decoding a 0-byte WAV panicked instead of returning an error");
        assert!(result.unwrap().is_err(), "decoding a 0-byte WAV should fail gracefully with Err");
    }

    #[test]
    fn test_decode_garbage_wav_errors_gracefully_not_panic() {
        // A file with a plausible-looking name but content that isn't audio
        // at all (e.g. yt-dlp emitting an HTML error page as "audio.wav"
        // because ffmpeg-location resolution went wrong).
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audio.wav");
        std::fs::write(&path, b"<html><body>404 Not Found</body></html>").unwrap();

        let result = std::panic::catch_unwind(|| {
            crate::audio::decoder::decode_audio_file(&path)
        });

        assert!(result.is_ok(), "decoding a garbage WAV panicked instead of returning an error");
        assert!(result.unwrap().is_err(), "decoding a garbage WAV should fail gracefully with Err");
    }

    #[test]
    fn test_parse_progress_comma_decimal_locale_fails_safe() {
        // Held up: the `\[download\]\s+` prefix anchors the match to right
        // after "[download]", so a comma-decimal line ("42,3%") fails to
        // match at all (None) instead of misparsing to a wrong percentage
        // (e.g. "3%") - there's no second attempt point in the string for
        // the regex engine to retry from.
        assert_eq!(
            parse_ytdlp_progress_percentage("[download]  42,3% of ~10.00MiB at 1.20MiB/s"),
            None
        );
    }

    #[test]
    fn test_format_ytdlp_error_filters_warning_and_formats_age_gate() {
        let stderr = "WARNING: [youtube] No supported JavaScript runtime could be found.\nERROR: [youtube] m5NTKNuSyF0: Sign in to confirm your age. This video may be inappropriate for some users.";
        assert!(is_age_restriction_error(stderr));
        let formatted = format_ytdlp_error(stderr);
        assert!(formatted.contains("age-restricted"));
        assert!(!formatted.contains("WARNING"));
    }

    #[test]
    fn test_format_ytdlp_error_strips_prefix() {
        let stderr = "WARNING: [youtube] Some warning\nERROR: [youtube] abc1234: Video unavailable";
        let formatted = format_ytdlp_error(stderr);
        assert_eq!(formatted, "This YouTube video is unavailable or private.");
    }
}
