//! Template selection/generation via a single LLM call.
//!
//! Given a transcript, this module asks the LLM to either pick the
//! best-matching template out of a set of candidates, or design a brand-new
//! one-use template when none of the candidates fit. It never propagates an
//! error to the caller: any failure (LLM call, JSON parsing, validation, or
//! an unknown template id) falls back to the bundled `standard_meeting`
//! template.

use super::types::{Template, TemplateSection};
use crate::database::models::TokenUsagePurpose;
use crate::summary::llm_client::{generate_summary, LLMProvider};
use crate::summary::processor::chunk_text;
use crate::summary::template_commands::TemplateInfo;
use reqwest::Client;
use sqlx::SqlitePool;
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;
use tracing::warn;

/// Template id used as the last-resort fallback whenever generation fails.
/// With templates temporarily disabled we cannot fall back to a preset —
/// `select_template` always generates one on the fly, but if generation
/// itself fails (LLM error, parse error, validation error) we degrade to a
/// minimal hardcoded template so summary generation still produces
/// *something* rather than failing the whole run.
const FALLBACK_TEMPLATE_ID: &str = "__generated_fallback__";

/// Token budget for the transcript excerpt sent to the classification call.
/// This is a lightweight selection/classification prompt, not a summary, so
/// we intentionally send far less than the full transcript (see
/// `processor::chunk_text` for the same token-bounding approach used
/// elsewhere for LLM calls).
const EXCERPT_CHUNK_TOKENS: usize = 600;

/// Result of a successful template selection/generation call.
#[derive(Debug)]
pub struct TemplateChoice {
    pub template: Template,
    /// `Some(id)` when an existing template was matched (including the
    /// fallback), `None` when a brand-new template was generated.
    pub template_id: Option<String>,
    pub is_generated: bool,
}

/// Everything needed to call through to `llm_client::generate_summary`,
/// bundled up so `select_template`'s signature doesn't sprawl across a dozen
/// positional arguments.
pub struct TemplateSelectionContext<'a> {
    pub client: &'a Client,
    pub provider: &'a LLMProvider,
    pub model_name: &'a str,
    pub api_key: &'a str,
    pub ollama_endpoint: Option<&'a str>,
    pub custom_openai_endpoint: Option<&'a str>,
    pub app_data_dir: Option<&'a PathBuf>,
    pub cancellation_token: Option<&'a CancellationToken>,
    /// Optional meeting id used to attach the recorded token usage row to
    /// the right meeting. `None` for callers that don't have a meeting
    /// (e.g. ad-hoc / test paths).
    pub meeting_id: Option<&'a str>,
}

/// Outcome of parsing the LLM's raw text response, before it has been
/// resolved against the template store (a `Match` id might still not exist,
/// and a `Generate` template has already passed `Template::validate()`).
#[derive(Debug)]
enum ParsedChoice {
    Match(String),
    Generate(Template),
}

/// Selects an existing template or generates a new one for `transcript`, via
/// a single LLM call. Never fails: any error along the way falls back to the
/// bundled `standard_meeting` template.
pub async fn select_template(
    pool: &SqlitePool,
    ctx: TemplateSelectionContext<'_>,
    transcript: &str,
    candidates: Vec<TemplateInfo>,
) -> TemplateChoice {
    let system_prompt = build_system_prompt(&candidates);
    let user_prompt = build_transcript_excerpt(transcript);

    let raw = match generate_summary(
        ctx.client,
        ctx.provider,
        ctx.model_name,
        ctx.api_key,
        &system_prompt,
        &user_prompt,
        ctx.ollama_endpoint,
        ctx.custom_openai_endpoint,
        None,
        None,
        None,
        ctx.app_data_dir,
        ctx.cancellation_token,
        None,
        Some(crate::summary::llm_client::TokenUsageContext {
            pool: pool.clone(),
            meeting_id: ctx.meeting_id.map(str::to_string),
            purpose: TokenUsagePurpose::TemplateSelect,
        }),
    )
    .await
    {
        Ok(output) => output.summary,
        Err(e) => {
            warn!(
                "select_template: LLM call failed, falling back to '{}': {}",
                FALLBACK_TEMPLATE_ID, e
            );
            return fallback_choice();
        }
    };

    resolve_parsed_choice(parse_llm_response(&raw))
}

/// Resolves a `ParsedChoice` (already-parsed LLM intent) into a
/// `TemplateChoice`. Templates are temporarily disabled, so `Match` is
/// always rejected and the LLM is forced to `Generate` on the fly. Split
/// out from `select_template` so the parse -> resolve steps are each
/// independently testable without a live LLM.
fn resolve_parsed_choice(parsed: Result<ParsedChoice, String>) -> TemplateChoice {
    match parsed {
        Ok(ParsedChoice::Match(id)) => {
            warn!(
                "select_template: ignoring LLM 'match' response ('{}') — templates are temporarily disabled, on-the-fly generation only",
                id
            );
            fallback_choice()
        }
        Ok(ParsedChoice::Generate(template)) => TemplateChoice {
            template,
            template_id: None,
            is_generated: true,
        },
        Err(e) => {
            warn!(
                "select_template: failed to parse LLM response, falling back: {}",
                e
            );
            fallback_choice()
        }
    }
}

/// Last-resort fallback used whenever any step of selection/generation
/// fails. With templates disabled we never load from the bundled store —
/// we degrade to a minimal hardcoded template so summary generation still
/// produces *something* rather than failing the whole run.
fn fallback_choice() -> TemplateChoice {
    warn!(
        "select_template: falling back to a hardcoded minimal template (templates are temporarily disabled)"
    );
    TemplateChoice {
        template: Template {
            name: "Auto-generated Summary".to_string(),
            description: "Hardcoded fallback used when on-the-fly template generation fails.".to_string(),
            sections: vec![TemplateSection {
                title: "Summary".to_string(),
                instruction: "Summarize the transcript.".to_string(),
                format: "paragraph".to_string(),
                item_format: None,
                example_item_format: None,
            }],
        },
        template_id: Some(FALLBACK_TEMPLATE_ID.to_string()),
        is_generated: false,
    }
}

/// Parses the LLM's raw text response into a `ParsedChoice`, stripping any
/// markdown code fences and validating a generated template's shape.
fn parse_llm_response(raw: &str) -> Result<ParsedChoice, String> {
    let stripped = strip_code_fences(raw);

    let value: serde_json::Value = serde_json::from_str(&stripped)
        .map_err(|e| format!("Failed to parse LLM response as JSON: {}", e))?;

    if let Some(id) = value.get("match").and_then(|v| v.as_str()) {
        let id = id.trim();
        if id.is_empty() {
            return Err("LLM 'match' response had an empty template id".to_string());
        }
        return Ok(ParsedChoice::Match(id.to_string()));
    }

    if let Some(generate) = value.get("generate") {
        let template: Template = serde_json::from_value(generate.clone())
            .map_err(|e| format!("Failed to parse generated template JSON: {}", e))?;
        template.validate()?;
        return Ok(ParsedChoice::Generate(template));
    }

    Err("LLM response JSON had neither a 'match' nor a 'generate' key".to_string())
}

fn strip_code_fences(raw: &str) -> String {
    crate::summary::processor::strip_code_fence(raw)
}

/// Builds a bounded transcript excerpt (prefix + suffix) for the
/// classification call, reusing `processor::chunk_text`'s existing
/// token-bounding logic instead of sending the whole transcript.
fn build_transcript_excerpt(transcript: &str) -> String {
    let chunks = chunk_text(transcript, EXCERPT_CHUNK_TOKENS, 0);

    match chunks.as_slice() {
        [] => String::new(),
        [only] => only.clone(),
        chunks => format!(
            "{}\n\n[...transcript truncated...]\n\n{}",
            chunks.first().unwrap(),
            chunks.last().unwrap()
        ),
    }
}

/// Builds the system prompt instructing the LLM to generate a fresh
/// template tailored to the transcript. Templates are temporarily
/// disabled — there are no presets to match against, so the LLM is
/// forced to always generate on the fly.
fn build_system_prompt(_candidates: &[TemplateInfo]) -> String {
    format!(
        r#"You are designing a brand-new, one-use meeting summary template tailored to the transcript excerpt below. There are no preset templates to choose from — you must generate one from scratch.

Read the transcript excerpt carefully and design a template whose sections match what this specific content actually needs. Do not reuse a generic "meeting notes" structure unless the content really is a generic meeting. If it's a video essay, design sections for thesis, argument structure, evidence, counterpoints, and verdict. If it's a standup, design sections for blockers, progress, and next steps. Match the medium and the subject.

Respond with strict JSON only (no markdown, no commentary, no code fences) in exactly this shape:

{{
  "name": "<short human-readable template name>",
  "description": "<one-sentence description of what this template captures>",
  "sections": [
    {{
      "title": "<section heading>",
      "instruction": "<specific, detailed instruction for what to write in this section — be concrete, not generic>",
      "format": "paragraph|list|string",
      "item_format": null,
      "example_item_format": null
    }}
  ]
}}

Rules:
- "format" must be exactly one of "paragraph", "list", or "string".
- "item_format" and "example_item_format" must be null unless you have a strong reason to set them (table-shaped output); leaving them null is the safe default.
- Output only the JSON object and nothing else.
- The first section should be a high-level overview; subsequent sections should drill into specifics the content actually contains.
- Do not invent generic sections like "Action Items" or "Key Takeaways" unless the transcript genuinely has action items or takeaways.
- Do NOT emit sections for metadata that is not in the transcript. Sections like "Video Info", "Channel/Creator", "URL", "Title", "Publication Date", "Source" are almost never useful — the transcript rarely contains them and any placeholder value ("Not stated", "Unknown", "Not provided") wastes the user's time. Only include such a section if the transcript explicitly and unambiguously states the relevant fact."#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(id: &str, name: &str, description: &str) -> TemplateInfo {
        TemplateInfo {
            id: id.to_string(),
            name: name.to_string(),
            description: description.to_string(),
        }
    }

    // ---- strip_code_fences ----

    #[test]
    fn strip_code_fences_leaves_plain_json_untouched() {
        let raw = r#"{"match": "daily_standup"}"#;
        assert_eq!(strip_code_fences(raw), raw);
    }

    #[test]
    fn strip_code_fences_strips_plain_fence() {
        let raw = "```\n{\"match\": \"daily_standup\"}\n```";
        assert_eq!(strip_code_fences(raw), r#"{"match": "daily_standup"}"#);
    }

    #[test]
    fn strip_code_fences_strips_json_language_fence() {
        let raw = "```json\n{\"match\": \"daily_standup\"}\n```";
        assert_eq!(strip_code_fences(raw), r#"{"match": "daily_standup"}"#);
    }

    // ---- parse_llm_response ----

    #[test]
    fn parse_llm_response_valid_match() {
        let raw = r#"{"match": "daily_standup"}"#;
        match parse_llm_response(raw) {
            Ok(ParsedChoice::Match(id)) => assert_eq!(id, "daily_standup"),
            other => panic!("expected Match, got {:?}", other),
        }
    }

    #[test]
    fn parse_llm_response_valid_generate() {
        let raw = r#"{"generate": {"name": "Custom", "description": "A custom template", "sections": [{"title": "Summary", "instruction": "Summarize", "format": "paragraph"}]}}"#;
        match parse_llm_response(raw) {
            Ok(ParsedChoice::Generate(template)) => {
                assert_eq!(template.name, "Custom");
                assert_eq!(template.sections.len(), 1);
            }
            other => panic!("expected Generate, got {:?}", other),
        }
    }

    #[test]
    fn parse_llm_response_strips_markdown_fences_before_parsing() {
        let raw = "```json\n{\"match\": \"daily_standup\"}\n```";
        match parse_llm_response(raw) {
            Ok(ParsedChoice::Match(id)) => assert_eq!(id, "daily_standup"),
            other => panic!("expected Match, got {:?}", other),
        }
    }

    #[test]
    fn parse_llm_response_rejects_malformed_json() {
        let raw = "this is not json at all";
        assert!(parse_llm_response(raw).is_err());
    }

    #[test]
    fn parse_llm_response_rejects_empty_match_id() {
        let raw = r#"{"match": "  "}"#;
        assert!(parse_llm_response(raw).is_err());
    }

    #[test]
    fn parse_llm_response_rejects_generate_failing_validation() {
        // Invalid "format" value should fail Template::validate().
        let raw = r#"{"generate": {"name": "Custom", "description": "desc", "sections": [{"title": "Summary", "instruction": "Summarize", "format": "not-a-real-format"}]}}"#;
        assert!(parse_llm_response(raw).is_err());
    }

    #[test]
    fn parse_llm_response_rejects_generate_with_empty_name() {
        let raw = r#"{"generate": {"name": "", "description": "desc", "sections": [{"title": "Summary", "instruction": "Summarize", "format": "list"}]}}"#;
        assert!(parse_llm_response(raw).is_err());
    }

    #[test]
    fn parse_llm_response_rejects_unknown_shape() {
        let raw = r#"{"something_else": true}"#;
        assert!(parse_llm_response(raw).is_err());
    }

    // ---- resolve_parsed_choice ----

    #[test]
    fn resolve_parsed_choice_match_always_falls_back_when_templates_disabled() {
        // Templates are temporarily disabled, so a `match` response is always
        // rejected regardless of whether the id corresponds to a known preset.
        let choice = resolve_parsed_choice(Ok(ParsedChoice::Match("daily_standup".to_string())));
        assert_eq!(choice.template_id.as_deref(), Some(FALLBACK_TEMPLATE_ID));
        assert!(!choice.is_generated);
    }

    #[test]
    fn resolve_parsed_choice_generate_passes_through_as_generated() {
        let template = Template {
            name: "Custom".to_string(),
            description: "desc".to_string(),
            sections: vec![TemplateSection {
                title: "Summary".to_string(),
                instruction: "Summarize".to_string(),
                format: "paragraph".to_string(),
                item_format: None,
                example_item_format: None,
            }],
        };
        let choice = resolve_parsed_choice(Ok(ParsedChoice::Generate(template)));
        assert!(choice.is_generated);
        assert_eq!(choice.template_id, None);
        assert_eq!(choice.template.name, "Custom");
    }

    #[test]
    fn resolve_parsed_choice_parse_error_falls_back() {
        let choice = resolve_parsed_choice(Err("boom".to_string()));
        assert_eq!(choice.template_id.as_deref(), Some(FALLBACK_TEMPLATE_ID));
        assert!(!choice.is_generated);
    }

    // ---- fallback_choice ----

    #[test]
    fn fallback_choice_returns_hardcoded_minimal_template() {
        let choice = fallback_choice();
        assert_eq!(choice.template_id.as_deref(), Some(FALLBACK_TEMPLATE_ID));
        assert!(!choice.is_generated);
        assert!(!choice.template.sections.is_empty());
    }

    // ---- build_transcript_excerpt ----

    #[test]
    fn build_transcript_excerpt_returns_short_transcript_unchanged() {
        let transcript = "Short meeting transcript.";
        assert_eq!(build_transcript_excerpt(transcript), transcript);
    }

    #[test]
    fn build_transcript_excerpt_bounds_long_transcript() {
        let word = "word ";
        // Well beyond the excerpt token budget.
        let transcript = word.repeat(5000);
        let excerpt = build_transcript_excerpt(&transcript);
        assert!(excerpt.len() < transcript.len());
        assert!(excerpt.contains("[...transcript truncated...]"));
    }

    #[test]
    fn build_transcript_excerpt_handles_empty_transcript() {
        assert_eq!(build_transcript_excerpt(""), "");
    }

    // ---- build_system_prompt ----

    #[test]
    fn build_system_prompt_instructs_on_the_fly_generation_only() {
        // Templates are temporarily disabled - the prompt must only describe
        // the on-the-fly generation shape and never advertise a `match`
        // option pointing at preset templates.
        let prompt = build_system_prompt(&[]);
        assert!(
            prompt.contains("\"name\""),
            "prompt should describe the top-level JSON shape (name/description/sections)"
        );
        assert!(
            prompt.contains("\"sections\""),
            "prompt should describe the sections array"
        );
        assert!(
            !prompt.contains("\"match\""),
            "system prompt should not advertise a 'match' option when templates are disabled"
        );
    }
}
