use crate::database::repositories::{
    meeting::MeetingsRepository, setting::SettingsRepository, summary::SummaryProcessesRepository,
};
use crate::summary::llm_client::LLMProvider;
use crate::summary::language_detection::detect_summary_language;
use crate::summary::metadata::read_detected_summary_language_from_metadata;
use crate::summary::processor::{
    extract_meeting_name_from_markdown, generate_meeting_summary, language_name_from_code,
};
use crate::summary::template_commands;
use crate::summary::templates::{self, Template};
use crate::ollama::metadata::ModelMetadataCache;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use once_cell::sync::Lazy;

// Global cache for model metadata (5 minute TTL). `pub(crate)` so
// `summary::commands`'s `ask_across_meetings` context-budget resolution can
// reuse this same cache/TTL instead of instantiating a second, independent
// cache instance for the same Ollama endpoints.
pub(crate) static METADATA_CACHE: Lazy<ModelMetadataCache> = Lazy::new(|| {
    ModelMetadataCache::new(Duration::from_secs(300))
});

/// Number of tokens reserved for prompt overhead when sizing a context
/// budget off a resolved model's raw context window (Ollama, BuiltInAI).
const CONTEXT_BUDGET_RESERVED_TOKENS: u32 = 300;

/// Floor below which an Ollama-reported `context_size` is treated as bogus
/// metadata rather than a real (if unusually small) context window. Real
/// models' advertised windows run from the low thousands up; this sits far
/// below any of them so it only catches malformed/zeroed `model_info` (e.g.
/// a literal `context_length: 0`), never a legitimate small-context model.
/// `extract_context_from_model_info` (ollama::metadata) only recognizes its
/// own `ULTIMATE_FALLBACK` sentinel as "not found", so a server-reported `0`
/// sails through `fetch_model_info` unguarded - this is the backstop that
/// catches it before it becomes `num_ctx: 0` on the wire.
const MIN_SANE_OLLAMA_CONTEXT_TOKENS: u32 = 100;

/// Fallback token budget (already net of `CONTEXT_BUDGET_RESERVED_TOKENS`)
/// used for Ollama when a real context window can't be resolved - either the
/// metadata fetch failed, or the model reported an implausibly small
/// `context_size` (see `MIN_SANE_OLLAMA_CONTEXT_TOKENS`).
const OLLAMA_FALLBACK_BUDGET_TOKENS: usize = 4000;

/// Fallback token budget (already net of `CONTEXT_BUDGET_RESERVED_TOKENS`)
/// used for a BuiltInAI model name not found in the local model registry:
/// `2048 - 300`.
const BUILTIN_AI_FALLBACK_BUDGET_TOKENS: usize = 1748;

/// Flat token budget used for every other (cloud/custom) provider - OpenAI,
/// Claude, Groq, OpenRouter, CustomOpenAI, LmStudio - whose actual context
/// window this app doesn't resolve per-model. Meant as "effectively
/// unlimited" for this module's chunked, multi-call per-meeting
/// summarization (`process_transcript_background`, below). A caller with a
/// single-shot prompt budget instead of a chunking budget (e.g.
/// `summary::commands::ask_across_meetings`) should NOT feed this directly
/// through `tokens_to_chars` - see that command's own cap for why.
pub(crate) const CLOUD_PROVIDER_BUDGET_TOKENS: usize = 100_000;

/// The resolved LLM context budget for a given provider/model, produced by
/// `resolve_provider_context_budget`.
pub(crate) struct ProviderContextBudget {
    /// The model's raw resolved context window, in tokens - only ever `Some`
    /// for Ollama, and never `Some(0)` (see `MIN_SANE_OLLAMA_CONTEXT_TOKENS`).
    /// This is what should be sent as `num_ctx` on an actual Ollama call,
    /// since `num_ctx` should reflect the model's real window, not the
    /// reserved-overhead budget below.
    pub(crate) raw_context_tokens: Option<u32>,
    /// The token budget a prompt/chunking pass should actually be sized
    /// against: the raw context window minus `CONTEXT_BUDGET_RESERVED_TOKENS`
    /// of overhead for Ollama and BuiltInAI, where a real context window is
    /// resolved - or the flat, unreserved `CLOUD_PROVIDER_BUDGET_TOKENS`
    /// placeholder for every other (cloud) provider, where no real per-model
    /// context data is known.
    pub(crate) budget_tokens: usize,
}

/// Resolves `provider`/`model_name`'s `ProviderContextBudget`. Shared by
/// `process_transcript_background` (below) and
/// `summary::commands::ask_across_meetings`'s context-budget resolution -
/// previously duplicated in both places with the same magic numbers, which
/// had already drifted (the zero/bogus-context guard below once existed in
/// only one of the two copies).
///
/// For Ollama, fetches the model's real context window via `METADATA_CACHE`
/// (a network round-trip) and reserves `CONTEXT_BUDGET_RESERVED_TOKENS` of
/// overhead, guarding against a zero/bogus-small reported `context_size` by
/// falling back to `OLLAMA_FALLBACK_BUDGET_TOKENS`. For BuiltInAI, looks up
/// the model in the local registry with the same reservation, falling back
/// to `BUILTIN_AI_FALLBACK_BUDGET_TOKENS` for an unrecognized model name.
/// For every other provider, returns the flat `CLOUD_PROVIDER_BUDGET_TOKENS`
/// placeholder with no reservation applied.
pub(crate) async fn resolve_provider_context_budget(
    provider: &LLMProvider,
    model_name: &str,
    ollama_endpoint: Option<&str>,
) -> ProviderContextBudget {
    if *provider == LLMProvider::Ollama {
        match METADATA_CACHE.get_or_fetch(model_name, ollama_endpoint).await {
            Ok(metadata) if metadata.context_size >= MIN_SANE_OLLAMA_CONTEXT_TOKENS as usize => {
                let optimal = metadata.context_size.saturating_sub(CONTEXT_BUDGET_RESERVED_TOKENS as usize);
                info!(
                    "✓ Using dynamic context for {}: {} tokens (chunk size: {})",
                    model_name, metadata.context_size, optimal
                );
                ProviderContextBudget {
                    raw_context_tokens: Some(metadata.context_size as u32),
                    budget_tokens: optimal,
                }
            }
            Ok(metadata) => {
                warn!(
                    "resolve_provider_context_budget: Ollama reported an implausibly small \
                     context_size ({}) for {} - treating as unusable metadata. Using default {}",
                    metadata.context_size, model_name, OLLAMA_FALLBACK_BUDGET_TOKENS
                );
                ProviderContextBudget {
                    raw_context_tokens: None,
                    budget_tokens: OLLAMA_FALLBACK_BUDGET_TOKENS,
                }
            }
            Err(e) => {
                warn!(
                    "Failed to fetch context for {}: {}. Using default {}",
                    model_name, e, OLLAMA_FALLBACK_BUDGET_TOKENS
                );
                ProviderContextBudget {
                    raw_context_tokens: None,
                    budget_tokens: OLLAMA_FALLBACK_BUDGET_TOKENS,
                }
            }
        }
    } else if *provider == LLMProvider::BuiltInAI {
        use crate::summary::summary_engine::models;
        match models::get_model_by_name(model_name) {
            Some(model_def) => {
                let optimal = model_def.context_size.saturating_sub(CONTEXT_BUDGET_RESERVED_TOKENS) as usize;
                info!(
                    "✓ Using BuiltInAI context size: {} tokens (chunk size: {})",
                    model_def.context_size, optimal
                );
                ProviderContextBudget { raw_context_tokens: None, budget_tokens: optimal }
            }
            None => {
                warn!(
                    "Unknown model: {}, using default {}",
                    model_name, BUILTIN_AI_FALLBACK_BUDGET_TOKENS
                );
                ProviderContextBudget {
                    raw_context_tokens: None,
                    budget_tokens: BUILTIN_AI_FALLBACK_BUDGET_TOKENS,
                }
            }
        }
    } else {
        ProviderContextBudget { raw_context_tokens: None, budget_tokens: CLOUD_PROVIDER_BUDGET_TOKENS }
    }
}

// Global registry for cancellation tokens (thread-safe)
static CANCELLATION_REGISTRY: Lazy<Arc<Mutex<HashMap<String, CancellationToken>>>> =
    Lazy::new(|| Arc::new(Mutex::new(HashMap::new())));

/// Strips the first `#` heading line; returns "" if no `#` is found.
fn strip_leading_title(markdown: &str) -> String {
    if let Some(hash_pos) = markdown.find('#') {
        let body_start = markdown[hash_pos..]
            .find('\n')
            .map_or(markdown.len(), |line_end| hash_pos + line_end);
        markdown[body_start..].trim_start().to_string()
    } else {
        String::new()
    }
}

/// Strips the leading H1 (`# Title\n...`) only when the markdown starts with one.
/// No-op on already-stripped values, values starting with `## Subheading`, or values
/// without any heading. Avoids the silent-empty-return case where `strip_leading_title`
/// returns "" for input lacking a leading `#`.
fn strip_title_if_present(markdown: &str) -> String {
    if markdown.trim_start().starts_with("# ") {
        strip_leading_title(markdown)
    } else {
        markdown.to_string()
    }
}

const ENGLISH_CACHE_FIELD: &str = "english_cache";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct SummaryCacheSource {
    transcript_fingerprint: String,
    custom_prompt_fingerprint: String,
    template_id: String,
    template_fingerprint: String,
    token_threshold: usize,
    model_provider: String,
    model_name: String,
    ollama_endpoint: Option<String>,
    custom_openai_endpoint: Option<String>,
    max_tokens: Option<u32>,
    temperature: Option<f32>,
    top_p: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct EnglishSummaryCache {
    markdown: String,
    source: SummaryCacheSource,
    output_language: Option<String>,
}

fn stable_text_fingerprint(text: &str) -> String {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET;
    for byte in text.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("{:016x}:{}", hash, text.len())
}

#[allow(clippy::too_many_arguments)]
fn build_summary_cache_source(
    text: &str,
    custom_prompt: &str,
    template_id: &str,
    template_fingerprint: &str,
    token_threshold: usize,
    model_provider: &str,
    model_name: &str,
    ollama_endpoint: Option<&str>,
    custom_openai_endpoint: Option<&str>,
    max_tokens: Option<u32>,
    temperature: Option<f32>,
    top_p: Option<f32>,
) -> SummaryCacheSource {
    SummaryCacheSource {
        transcript_fingerprint: stable_text_fingerprint(text),
        custom_prompt_fingerprint: stable_text_fingerprint(custom_prompt),
        template_id: template_id.to_string(),
        template_fingerprint: template_fingerprint.to_string(),
        token_threshold,
        model_provider: model_provider.to_string(),
        model_name: model_name.to_string(),
        ollama_endpoint: ollama_endpoint.map(str::to_string),
        custom_openai_endpoint: custom_openai_endpoint.map(str::to_string),
        max_tokens,
        temperature,
        top_p,
    }
}

fn template_cache_fingerprint(template: &Template) -> String {
    let rendered_template = format!(
        "{}\n---SECTION-INSTRUCTIONS---\n{}",
        template.to_markdown_structure(),
        template.to_section_instructions()
    );
    stable_text_fingerprint(&rendered_template)
}

/// `template_id` value that requests LLM-driven template selection/generation
/// (via `templates::select_template`) rather than a concrete template.
const AUTO_TEMPLATE_ID: &str = "auto";

/// The three ways `process_transcript_background` can resolve a `Template`
/// for a summary run, decided from `custom_template_json`/`template_id`
/// alone (no IO). Split out so this decision is unit-testable without a
/// live LLM/template store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TemplateResolutionPlan {
    /// Reuse a caller-supplied template verbatim (e.g. "Regenerate" reusing a
    /// template a prior `AutoSelect` run already generated) — no LLM call.
    UseCustomJson,
    /// Ask the LLM to match an existing template or design a new one.
    AutoSelect,
    /// Load a specific template by id, as before this feature existed.
    UseTemplateId,
}

fn plan_template_resolution(
    custom_template_json: Option<&str>,
    template_id: &str,
) -> TemplateResolutionPlan {
    if custom_template_json.is_some() {
        TemplateResolutionPlan::UseCustomJson
    } else if template_id == AUTO_TEMPLATE_ID {
        TemplateResolutionPlan::AutoSelect
    } else {
        TemplateResolutionPlan::UseTemplateId
    }
}

fn normalise_summary_language_for_cache(summary_language: Option<&str>) -> Option<String> {
    language_name_from_code(summary_language?.trim()).map(str::to_string)
}

#[allow(clippy::too_many_arguments)]
fn build_summary_result_json(
    final_markdown: &str,
    english_markdown: &str,
    source: SummaryCacheSource,
    output_language: Option<&str>,
    resolved_template_id: Option<&str>,
    resolved_template_name: &str,
    is_generated_template: bool,
    generated_template_json: Option<&Template>,
) -> serde_json::Value {
    serde_json::json!({
        "markdown": strip_title_if_present(final_markdown),
        ENGLISH_CACHE_FIELD: EnglishSummaryCache {
            markdown: english_markdown.to_string(),
            source,
            output_language: normalise_summary_language_for_cache(output_language),
        },
        "resolved_template_id": resolved_template_id,
        "resolved_template_name": resolved_template_name,
        "is_generated_template": is_generated_template,
        "generated_template_json": generated_template_json,
    })
}

/// Parses a `summary_processes.result` JSON blob and extracts a cached English
/// summary only when it was produced from exactly the same source inputs and
/// the user is switching to a different non-English target language.
fn extract_cached_english_markdown(
    raw: &str,
    expected_source: &SummaryCacheSource,
    requested_language: Option<&str>,
) -> Result<Option<String>, serde_json::Error> {
    let requested_language = match normalise_summary_language_for_cache(requested_language) {
        Some(language) if language != "English" => language,
        _ => return Ok(None),
    };

    let value: serde_json::Value = serde_json::from_str(raw)?;
    let Some(cache_value) = value.get(ENGLISH_CACHE_FIELD) else {
        return Ok(None);
    };

    let cache: EnglishSummaryCache = match serde_json::from_value(cache_value.clone()) {
        Ok(cache) => cache,
        Err(_) => return Ok(None),
    };

    if cache.source != *expected_source {
        return Ok(None);
    }

    if cache.output_language.as_deref() == Some(requested_language.as_str()) {
        return Ok(None);
    }

    let markdown = cache.markdown.trim();
    if markdown.is_empty() {
        Ok(None)
    } else {
        Ok(Some(cache.markdown))
    }
}

/// Summary service - handles all summary generation logic
pub struct SummaryService;

impl SummaryService {
    /// Registers a new cancellation token for a meeting
    fn register_cancellation_token(meeting_id: &str) -> CancellationToken {
        let token = CancellationToken::new();
        if let Ok(mut registry) = CANCELLATION_REGISTRY.lock() {
            registry.insert(meeting_id.to_string(), token.clone());
            info!("Registered cancellation token for meeting: {}", meeting_id);
        }
        token
    }

    /// Cancels the summary generation for a meeting
    pub fn cancel_summary(meeting_id: &str) -> bool {
        if let Ok(registry) = CANCELLATION_REGISTRY.lock() {
            if let Some(token) = registry.get(meeting_id) {
                info!("Cancelling summary generation for meeting: {}", meeting_id);
                token.cancel();
                return true;
            }
        }
        warn!("No active summary generation found for meeting: {}", meeting_id);
        false
    }

    /// Cleans up the cancellation token after processing completes
    fn cleanup_cancellation_token(meeting_id: &str) {
        if let Ok(mut registry) = CANCELLATION_REGISTRY.lock() {
            if registry.remove(meeting_id).is_some() {
                info!("Cleaned up cancellation token for meeting: {}", meeting_id);
            }
        }
    }

    async fn read_detected_summary_language(
        pool: &SqlitePool,
        meeting_id: &str,
    ) -> Option<String> {
        let meeting = match MeetingsRepository::get_meeting_metadata(pool, meeting_id).await {
            Ok(Some(meeting)) => meeting,
            Ok(None) => {
                warn!("Meeting not found while reading detected summary language: {}", meeting_id);
                return None;
            }
            Err(e) => {
                warn!(
                    "Failed to read meeting metadata for detected summary language (meeting_id={}): {}",
                    meeting_id, e
                );
                return None;
            }
        };

        let Some(folder_path) = meeting.folder_path.filter(|p| !p.trim().is_empty()) else {
            return None;
        };

        match read_detected_summary_language_from_metadata(Path::new(&folder_path)) {
            Ok(language) => language,
            Err(e) => {
                warn!(
                    "Failed to read detected summary language metadata for meeting_id={}: {}",
                    meeting_id, e
                );
                None
            }
        }
    }

    fn detect_summary_language_from_text(text: &str) -> Option<String> {
        let transcript_texts = [text.to_string()];
        let detection = detect_summary_language(&transcript_texts);
        match &detection.language {
            Some(language) => {
                info!("Detected transcript summary language for normalization: {}", language);
            }
            None => {
                info!(
                    "Transcript summary language unknown for normalization: {:?}",
                    detection.reason
                );
            }
        }
        detection.language
    }

    /// Processes transcript in the background and generates summary
    ///
    /// This function is designed to be spawned as an async task and does not block
    /// the main thread. It updates the database with progress and results.
    ///
    /// # Arguments
    /// * `_app` - Tauri app handle (for future use)
    /// * `pool` - SQLx connection pool
    /// * `meeting_id` - Unique identifier for the meeting
    /// * `text` - Full transcript text
    /// * `model_provider` - LLM provider name (e.g., "ollama", "openai")
    /// * `model_name` - Specific model (e.g., "gpt-4", "llama3.2:latest")
    /// * `custom_prompt` - Optional user-provided context
    /// * `template_id` - Template identifier (e.g., "daily_standup", "standard_meeting"),
    ///   or `"auto"` to have the LLM pick/design one via `templates::select_template`
    /// * `custom_template_json` - When set, takes precedence over `template_id`: reuses
    ///   this caller-supplied template JSON verbatim instead of resolving one
    pub async fn process_transcript_background<R: tauri::Runtime>(
        _app: AppHandle<R>,
        pool: SqlitePool,
        meeting_id: String,
        text: String,
        model_provider: String,
        model_name: String,
        custom_prompt: String,
        template_id: String,
        custom_template_json: Option<String>,
        summary_language: Option<String>,
    ) {
        let start_time = Instant::now();
        info!(
            "Starting background processing for meeting_id: {}",
            meeting_id
        );

        // Register cancellation token for this meeting
        let cancellation_token = Self::register_cancellation_token(&meeting_id);

        // Parse provider
        let provider = match LLMProvider::from_str(&model_provider) {
            Ok(p) => p,
            Err(e) => {
                Self::update_process_failed(&pool, &meeting_id, &e).await;
                return;
            }
        };

        // Validate and setup api_key, Flexible for Ollama, BuiltInAI, and CustomOpenAI
        let api_key = if provider == LLMProvider::Ollama || provider == LLMProvider::BuiltInAI || provider == LLMProvider::CustomOpenAI {
            // These providers don't require API keys from the standard database column
            String::new()
        } else {
            match SettingsRepository::get_api_key(&pool, &model_provider).await {
                Ok(Some(key)) if !key.is_empty() => key,
                Ok(None) | Ok(Some(_)) => {
                    let err_msg = format!("API key not found for {}", &model_provider);
                    Self::update_process_failed(&pool, &meeting_id, &err_msg).await;
                    return;
                }
                Err(e) => {
                    let err_msg = format!("Failed to retrieve API key for {}: {}", &model_provider, e);
                    Self::update_process_failed(&pool, &meeting_id, &err_msg).await;
                    return;
                }
            }
        };

        // Get Ollama endpoint if provider is Ollama
        let ollama_endpoint = if provider == LLMProvider::Ollama {
            match SettingsRepository::get_model_config(&pool).await {
                Ok(Some(config)) => config.ollama_endpoint,
                Ok(None) => None,
                Err(e) => {
                    info!("Failed to retrieve Ollama endpoint: {}, using default", e);
                    None
                }
            }
        } else {
            None
        };

        // Get CustomOpenAI config if provider is CustomOpenAI
        let (custom_openai_endpoint, custom_openai_api_key, custom_openai_max_tokens, custom_openai_temperature, custom_openai_top_p) =
            if provider == LLMProvider::CustomOpenAI {
                match SettingsRepository::get_custom_openai_config(&pool).await {
                    Ok(Some(config)) => {
                        info!("✓ Using custom OpenAI endpoint: {}", config.endpoint);
                        (
                            Some(config.endpoint),
                            config.api_key,
                            config.max_tokens.map(|t| t as u32),
                            config.temperature,
                            config.top_p,
                        )
                    }
                    Ok(None) => {
                        let err_msg = "Custom OpenAI provider selected but no configuration found";
                        Self::update_process_failed(&pool, &meeting_id, err_msg).await;
                        return;
                    }
                    Err(e) => {
                        let err_msg = format!("Failed to retrieve custom OpenAI config: {}", e);
                        Self::update_process_failed(&pool, &meeting_id, &err_msg).await;
                        return;
                    }
                }
            } else {
                (None, None, None, None, None)
            };

        // For CustomOpenAI, use its API key (if any) instead of the empty string
        let final_api_key = if provider == LLMProvider::CustomOpenAI {
            custom_openai_api_key.unwrap_or_default()
        } else {
            api_key
        };

        // Dynamically size context based on provider and model - shared with
        // `summary::commands::ask_across_meetings`'s context-budget
        // resolution via `resolve_provider_context_budget`. Only
        // `budget_tokens` is needed here; the raw (unreserved) context
        // window `resolve_provider_context_budget` also resolves for Ollama
        // is only relevant to callers that forward it as `num_ctx`, which
        // this chunking path does not do.
        let token_threshold =
            resolve_provider_context_budget(&provider, &model_name, ollama_endpoint.as_deref())
                .await
                .budget_tokens;

        // Get app data directory for BuiltInAI provider
        let app_data_dir = _app.path().app_data_dir().ok();

        if let Some(code) = &summary_language {
            info!("📝 Summary language preference: {}", code);
        }

        let detected_summary_language =
            Self::read_detected_summary_language(&pool, &meeting_id)
                .await
                .or_else(|| Self::detect_summary_language_from_text(&text));

        if let Some(code) = &detected_summary_language {
            info!("📝 Detected transcript summary language: {}", code);
        }

        let client = reqwest::Client::new();

        let template_choice = match plan_template_resolution(custom_template_json.as_deref(), &template_id) {
            TemplateResolutionPlan::UseCustomJson => {
                // `custom_template_json` is `Some` per `plan_template_resolution`'s contract.
                let json = custom_template_json.as_deref().unwrap_or_default();
                match templates::validate_and_parse_template(json) {
                    Ok(template) => templates::TemplateChoice {
                        template,
                        template_id: None,
                        is_generated: true,
                    },
                    Err(e) => {
                        let err_msg = format!("Failed to parse custom template JSON: {}", e);
                        Self::update_process_failed(&pool, &meeting_id, &err_msg).await;
                        return;
                    }
                }
            }
            TemplateResolutionPlan::AutoSelect => {
                let candidates = template_commands::list_template_infos();
                let selection_ctx = templates::TemplateSelectionContext {
                    client: &client,
                    provider: &provider,
                    model_name: &model_name,
                    api_key: &final_api_key,
                    ollama_endpoint: ollama_endpoint.as_deref(),
                    custom_openai_endpoint: custom_openai_endpoint.as_deref(),
                    app_data_dir: app_data_dir.as_ref(),
                    cancellation_token: Some(&cancellation_token),
                };
                templates::select_template(selection_ctx, &text, candidates).await
            }
            TemplateResolutionPlan::UseTemplateId => match templates::get_template(&template_id) {
                Ok(template) => templates::TemplateChoice {
                    template,
                    template_id: Some(template_id.clone()),
                    is_generated: false,
                },
                Err(e) => {
                    let err_msg = format!("Failed to load template '{}': {}", template_id, e);
                    Self::update_process_failed(&pool, &meeting_id, &err_msg).await;
                    return;
                }
            },
        };

        let template = template_choice.template;
        let resolved_template_id = template_choice.template_id;
        let is_generated_template = template_choice.is_generated;
        let template_fingerprint = template_cache_fingerprint(&template);

        let cache_source = build_summary_cache_source(
            &text,
            &custom_prompt,
            &template_id,
            &template_fingerprint,
            token_threshold,
            &model_provider,
            &model_name,
            ollama_endpoint.as_deref(),
            custom_openai_endpoint.as_deref(),
            custom_openai_max_tokens,
            custom_openai_temperature,
            custom_openai_top_p,
        );

        let cached_english = match SummaryProcessesRepository::get_summary_data(&pool, &meeting_id).await {
            Err(e) => {
                warn!(
                    "Failed to load prior summary row for cache lookup (meeting_id={}): {}. Falling back to full pass-1 generation.",
                    meeting_id, e
                );
                None
            }
            Ok(None) => None,
            Ok(Some(process)) => process.result.and_then(|raw| {
                match extract_cached_english_markdown(
                    &raw,
                    &cache_source,
                    summary_language.as_deref(),
                ) {
                    Ok(opt) => opt,
                    Err(e) => {
                        warn!(
                            "Cached summary result for meeting_id={} is not valid JSON ({}); ignoring cache.",
                            meeting_id, e
                        );
                        None
                    }
                }
            }),
        };

        let result = generate_meeting_summary(
            &client,
            &provider,
            &model_name,
            &final_api_key,
            &text,
            &custom_prompt,
            &template_id,
            &template,
            token_threshold,
            ollama_endpoint.as_deref(),
            custom_openai_endpoint.as_deref(),
            custom_openai_max_tokens,
            custom_openai_temperature,
            custom_openai_top_p,
            app_data_dir.as_ref(),
            Some(&cancellation_token),
            summary_language.as_deref(),
            detected_summary_language.as_deref(),
            cached_english.as_deref(),
        )
        .await;

        let duration = start_time.elapsed().as_secs_f64();

        // Clean up cancellation token regardless of outcome
        Self::cleanup_cancellation_token(&meeting_id);

        match result {
            Ok((final_markdown, english_markdown, num_chunks)) => {
                info!(
                    "✓ Successfully processed {} chunks for meeting_id: {}. Duration: {:.2}s",
                    num_chunks, meeting_id, duration
                );
                info!("Final markdown generated ({} chars)", final_markdown.len());

                if let Some(name) = extract_meeting_name_from_markdown(&final_markdown)
                    .filter(|n| !n.is_empty())
                {
                    info!("Extracted meeting name from summary: '{}'", name);
                    if let Err(e) =
                        MeetingsRepository::update_meeting_name(&pool, &meeting_id, &name).await
                    {
                        error!("Failed to update meeting name for {}: {}", meeting_id, e);
                    } else {
                        info!("Successfully updated meeting name for {}", meeting_id);
                    }
                }

                let result_json = build_summary_result_json(
                    &final_markdown,
                    &english_markdown,
                    cache_source,
                    summary_language.as_deref(),
                    resolved_template_id.as_deref(),
                    &template.name,
                    is_generated_template,
                    is_generated_template.then_some(&template),
                );

                // Update database with completed status
                if let Err(e) = SummaryProcessesRepository::update_process_completed(
                    &pool,
                    &meeting_id,
                    result_json,
                    num_chunks,
                    duration,
                )
                .await
                {
                    error!(
                        "Failed to save completed process for {}: {}",
                        meeting_id, e
                    );
                } else {
                    info!(
                        "Summary saved successfully for meeting_id: {}",
                        meeting_id
                    );
                }
            }
            Err(e) => {
                // Check if error is due to cancellation
                if e.contains("cancelled") {
                    info!("Summary generation was cancelled for meeting_id: {}", meeting_id);
                    if let Err(db_err) = SummaryProcessesRepository::update_process_cancelled(&pool, &meeting_id).await {
                        error!("Failed to update DB status to cancelled for {}: {}", meeting_id, db_err);
                    }
                } else {
                    Self::update_process_failed(&pool, &meeting_id, &e).await;
                }
            }
        }
    }

    /// Updates the summary process status to failed with error message
    ///
    /// # Arguments
    /// * `pool` - SQLx connection pool
    /// * `meeting_id` - Meeting identifier
    /// * `error_msg` - Error message to store
    async fn update_process_failed(pool: &SqlitePool, meeting_id: &str, error_msg: &str) {
        error!(
            "Processing failed for meeting_id {}: {}",
            meeting_id, error_msg
        );
        if let Err(e) =
            SummaryProcessesRepository::update_process_failed(pool, meeting_id, error_msg).await
        {
            error!(
                "Failed to update DB status to failed for {}: {}",
                meeting_id, e
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_leading_title_with_body() {
        let input = "# Meeting Title\nThis is the body.\nMore content.";
        let result = strip_leading_title(input);
        assert_eq!(result, "This is the body.\nMore content.");
    }

    #[test]
    fn test_strip_leading_title_only() {
        let input = "# Meeting Title";
        let result = strip_leading_title(input);
        assert_eq!(result, "");
    }

    #[test]
    fn test_strip_leading_title_no_heading() {
        let input = "No heading here.\nJust body.";
        let result = strip_leading_title(input);
        assert_eq!(result, "");
    }

    #[test]
    fn test_strip_leading_title_multiline_body() {
        let input = "# Title\n## Subheading\nParagraph 1\n\nParagraph 2";
        let result = strip_leading_title(input);
        assert_eq!(result, "## Subheading\nParagraph 1\n\nParagraph 2");
    }

    #[test]
    fn test_strip_leading_title_empty_after_heading() {
        let input = "# Title\n";
        let result = strip_leading_title(input);
        assert_eq!(result, "");
    }

    #[test]
    fn test_strip_leading_title_whitespace_after_heading() {
        let input = "# Title\n   \n Body with leading spaces";
        let result = strip_leading_title(input);
        assert_eq!(result, "Body with leading spaces");
    }

    #[test]
    fn test_strip_title_if_present_preserves_already_stripped() {
        assert_eq!(strip_title_if_present("## Action Items\nfoo"), "## Action Items\nfoo");
    }

    #[test]
    fn test_strip_title_if_present_strips_leading_h1() {
        assert_eq!(strip_title_if_present("# Meeting Title\n## Action Items\nfoo"), "## Action Items\nfoo");
    }

    #[test]
    fn test_strip_title_if_present_no_heading_preserved() {
        // Distinct from strip_leading_title which returns "" — this preserves input.
        assert_eq!(strip_title_if_present("Just body text"), "Just body text");
    }

    #[test]
    fn test_strip_title_if_present_hash_no_space_preserved() {
        // `#NoSpace` is not a markdown H1 — preserve.
        assert_eq!(strip_title_if_present("#NoSpace\nbody"), "#NoSpace\nbody");
    }

    #[test]
    fn test_strip_title_if_present_mid_document_h1_preserved() {
        // H1 after body content must NOT be stripped — guards the asymmetry where
        // extract_meeting_name_from_markdown scans every line for "# ".
        let input = "Some paragraph\n\n# H1 on line 3\n## Section\nbody";
        assert_eq!(strip_title_if_present(input), input);
    }

    #[test]
    fn test_strip_title_if_present_leading_whitespace_h1_stripped() {
        assert_eq!(
            strip_title_if_present("  # Title\n## Section\nbody"),
            "## Section\nbody"
        );
    }

    // ---- plan_template_resolution ----

    #[test]
    fn plan_template_resolution_custom_json_takes_precedence_over_auto() {
        assert_eq!(
            plan_template_resolution(Some(r#"{"name":"x"}"#), AUTO_TEMPLATE_ID),
            TemplateResolutionPlan::UseCustomJson
        );
    }

    #[test]
    fn plan_template_resolution_custom_json_takes_precedence_over_concrete_id() {
        assert_eq!(
            plan_template_resolution(Some(r#"{"name":"x"}"#), "daily_standup"),
            TemplateResolutionPlan::UseCustomJson
        );
    }

    #[test]
    fn plan_template_resolution_empty_custom_json_string_still_takes_precedence() {
        // `Some("")` is still `Some` - the plan doesn't validate content, just
        // decides which branch owns validation.
        assert_eq!(
            plan_template_resolution(Some(""), "daily_standup"),
            TemplateResolutionPlan::UseCustomJson
        );
    }

    #[test]
    fn plan_template_resolution_no_custom_json_and_auto_id_selects_auto() {
        assert_eq!(
            plan_template_resolution(None, AUTO_TEMPLATE_ID),
            TemplateResolutionPlan::AutoSelect
        );
    }

    #[test]
    fn plan_template_resolution_no_custom_json_and_concrete_id_uses_id() {
        assert_eq!(
            plan_template_resolution(None, "daily_standup"),
            TemplateResolutionPlan::UseTemplateId
        );
    }

    #[test]
    fn plan_template_resolution_no_custom_json_and_empty_id_uses_id() {
        // An empty template_id isn't "auto", so it falls through to
        // UseTemplateId (and will fail downstream in `get_template`, which
        // already handles unknown ids as an error).
        assert_eq!(
            plan_template_resolution(None, ""),
            TemplateResolutionPlan::UseTemplateId
        );
    }

    #[test]
    fn plan_template_resolution_id_is_case_sensitive_for_auto() {
        // "Auto" (capitalized) is not the recognized sentinel - must fall
        // through to UseTemplateId rather than silently auto-selecting.
        assert_eq!(
            plan_template_resolution(None, "Auto"),
            TemplateResolutionPlan::UseTemplateId
        );
    }

    fn sample_cache_source() -> SummaryCacheSource {
        let template_fingerprint = stable_text_fingerprint("standard template prompt");
        build_summary_cache_source(
            "transcript body",
            "custom prompt",
            "standard_meeting",
            &template_fingerprint,
            3700,
            "ollama",
            "gemma3:1b",
            Some("http://localhost:11434"),
            None,
            None,
            None,
            None,
        )
    }

    /// `build_summary_result_json` with fixed, resolution-irrelevant template
    /// args, for cache-behavior tests that don't exercise template resolution.
    fn build_test_result_json(
        final_markdown: &str,
        english_markdown: &str,
        source: SummaryCacheSource,
        output_language: Option<&str>,
    ) -> serde_json::Value {
        build_summary_result_json(
            final_markdown,
            english_markdown,
            source,
            output_language,
            Some("standard_meeting"),
            "Standard Meeting Notes",
            false,
            None,
        )
    }

    fn test_template(section_title: &str) -> Template {
        Template {
            name: "Test".to_string(),
            description: "Test template".to_string(),
            sections: vec![crate::summary::templates::TemplateSection {
                title: section_title.to_string(),
                instruction: "Summarize this section".to_string(),
                format: "paragraph".to_string(),
                item_format: None,
                example_item_format: None,
            }],
        }
    }

    #[test]
    fn test_template_cache_fingerprint_changes_with_rendered_template() {
        assert_ne!(
            template_cache_fingerprint(&test_template("Summary")),
            template_cache_fingerprint(&test_template("Decisions"))
        );
    }

    #[test]
    fn test_legacy_english_markdown_field_is_cache_miss() {
        let raw = serde_json::json!({
            "markdown": "translated",
            "english_markdown": "# Old English\nBody"
        })
        .to_string();

        assert_eq!(
            extract_cached_english_markdown(&raw, &sample_cache_source(), Some("de")).unwrap(),
            None
        );
    }

    #[test]
    fn test_matching_source_changed_translation_target_reuses_cache() {
        let source = sample_cache_source();
        let raw = build_test_result_json(
            "# Reunion\n## Points\nBonjour",
            "# Meeting\n## Points\nHello",
            source.clone(),
            Some("fr"),
        )
        .to_string();

        assert_eq!(
            extract_cached_english_markdown(&raw, &source, Some("de")).unwrap(),
            Some("# Meeting\n## Points\nHello".to_string())
        );
    }

    #[test]
    fn test_same_language_regeneration_rejects_cache() {
        let source = sample_cache_source();
        let raw = build_test_result_json(
            "# Reunion\n## Points\nBonjour",
            "# Meeting\n## Points\nHello",
            source.clone(),
            Some("fr"),
        )
        .to_string();

        assert_eq!(
            extract_cached_english_markdown(&raw, &source, Some("fr")).unwrap(),
            None
        );
    }

    #[test]
    fn test_changed_summary_inputs_reject_cache() {
        let source = sample_cache_source();
        let template_fingerprint = source.template_fingerprint.clone();
        let raw = build_test_result_json(
            "# Reunion\n## Points\nBonjour",
            "# Meeting\n## Points\nHello",
            source,
            Some("fr"),
        )
        .to_string();

        let changed_sources = [
            build_summary_cache_source(
                "changed transcript",
                "custom prompt",
                "standard_meeting",
                &template_fingerprint,
                3700,
                "ollama",
                "gemma3:1b",
                Some("http://localhost:11434"),
                None,
                None,
                None,
                None,
            ),
            build_summary_cache_source(
                "transcript body",
                "changed prompt",
                "standard_meeting",
                &template_fingerprint,
                3700,
                "ollama",
                "gemma3:1b",
                Some("http://localhost:11434"),
                None,
                None,
                None,
                None,
            ),
            build_summary_cache_source(
                "transcript body",
                "custom prompt",
                "daily_standup",
                &template_fingerprint,
                3700,
                "ollama",
                "gemma3:1b",
                Some("http://localhost:11434"),
                None,
                None,
                None,
                None,
            ),
            build_summary_cache_source(
                "transcript body",
                "custom prompt",
                "standard_meeting",
                &template_fingerprint,
                3700,
                "openai",
                "gemma3:1b",
                Some("http://localhost:11434"),
                None,
                None,
                None,
                None,
            ),
            build_summary_cache_source(
                "transcript body",
                "custom prompt",
                "standard_meeting",
                &template_fingerprint,
                3700,
                "ollama",
                "qwen2.5:3b",
                Some("http://localhost:11434"),
                None,
                None,
                None,
                None,
            ),
            build_summary_cache_source(
                "transcript body",
                "custom prompt",
                "standard_meeting",
                &template_fingerprint,
                3700,
                "ollama",
                "gemma3:1b",
                Some("http://localhost:11500"),
                None,
                None,
                None,
                None,
            ),
            build_summary_cache_source(
                "transcript body",
                "custom prompt",
                "standard_meeting",
                &template_fingerprint,
                3700,
                "ollama",
                "gemma3:1b",
                Some("http://localhost:11434"),
                Some("https://custom.example/v1"),
                Some(2048),
                Some(0.2),
                Some(0.9),
            ),
        ];

        for changed_source in changed_sources {
            assert_eq!(
                extract_cached_english_markdown(&raw, &changed_source, Some("de")).unwrap(),
                None
            );
        }
    }

    #[test]
    fn test_changed_template_content_rejects_cache() {
        let source = sample_cache_source();
        let raw = build_test_result_json(
            "# Reunion\n## Points\nBonjour",
            "# Meeting\n## Points\nHello",
            source.clone(),
            Some("fr"),
        )
        .to_string();

        let changed_template = SummaryCacheSource {
            template_fingerprint: stable_text_fingerprint("changed template prompt"),
            ..source
        };

        assert_eq!(
            extract_cached_english_markdown(&raw, &changed_template, Some("de")).unwrap(),
            None
        );
    }

    #[test]
    fn test_changed_token_threshold_rejects_cache() {
        let source = sample_cache_source();
        let raw = build_test_result_json(
            "# Reunion\n## Points\nBonjour",
            "# Meeting\n## Points\nHello",
            source.clone(),
            Some("fr"),
        )
        .to_string();

        let changed_threshold = SummaryCacheSource {
            token_threshold: 8192,
            ..source
        };

        assert_eq!(
            extract_cached_english_markdown(&raw, &changed_threshold, Some("de")).unwrap(),
            None
        );
    }

    #[test]
    fn test_result_json_strips_display_markdown_but_keeps_cache_title() {
        let result = build_test_result_json(
            "# Translated Title\n## Decisions\nDone",
            "# English Title\n## Decisions\nDone",
            sample_cache_source(),
            Some("fr"),
        );

        assert_eq!(result["markdown"], "## Decisions\nDone");
        assert_eq!(
            result["english_cache"]["markdown"],
            "# English Title\n## Decisions\nDone"
        );
    }

    #[test]
    fn test_extract_cached_english_from_malformed_json_errors() {
        let raw = r#"{ not valid json"#;
        assert!(extract_cached_english_markdown(raw, &sample_cache_source(), Some("de")).is_err());
    }
}
