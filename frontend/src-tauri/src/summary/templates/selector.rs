//! Template selection/generation via a single LLM call.
//!
//! Given a transcript, this module asks the LLM to either pick the
//! best-matching template out of a set of candidates, or design a brand-new
//! one-use template when none of the candidates fit. It never propagates an
//! error to the caller: any failure (LLM call, JSON parsing, validation, or
//! an unknown template id) falls back to the bundled `standard_meeting`
//! template.

use super::loader::get_template;
use super::types::{Template, TemplateSection};
use crate::summary::llm_client::{generate_summary, LLMProvider};
use crate::summary::processor::chunk_text;
use crate::summary::template_commands::TemplateInfo;
use reqwest::Client;
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;
use tracing::{error, warn};

/// Template id used as the last-resort fallback whenever selection fails.
const FALLBACK_TEMPLATE_ID: &str = "standard_meeting";

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
    )
    .await
    {
        Ok(text) => text,
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

/// Resolves a `ParsedChoice` (already-parsed LLM intent) against the
/// template store, applying the fallback rule on any failure. Split out from
/// `select_template` so the parse -> resolve steps are each independently
/// testable without a live LLM.
fn resolve_parsed_choice(parsed: Result<ParsedChoice, String>) -> TemplateChoice {
    match parsed {
        Ok(ParsedChoice::Match(id)) => match get_template(&id) {
            Ok(template) => TemplateChoice {
                template,
                template_id: Some(id),
                is_generated: false,
            },
            Err(e) => {
                warn!(
                    "select_template: LLM matched unknown template id '{}' ({}), falling back to '{}'",
                    id, e, FALLBACK_TEMPLATE_ID
                );
                fallback_choice()
            }
        },
        Ok(ParsedChoice::Generate(template)) => TemplateChoice {
            template,
            template_id: None,
            is_generated: true,
        },
        Err(e) => {
            warn!(
                "select_template: failed to parse LLM response, falling back to '{}': {}",
                FALLBACK_TEMPLATE_ID, e
            );
            fallback_choice()
        }
    }
}

/// Last-resort fallback used whenever any step of selection fails.
fn fallback_choice() -> TemplateChoice {
    match get_template(FALLBACK_TEMPLATE_ID) {
        Ok(template) => TemplateChoice {
            template,
            template_id: Some(FALLBACK_TEMPLATE_ID.to_string()),
            is_generated: false,
        },
        Err(e) => {
            // Should never happen (standard_meeting is embedded in the
            // binary), but never panic on template selection - degrade to a
            // minimal hardcoded template instead.
            error!(
                "select_template: bundled fallback template '{}' failed to load ({}); using hardcoded minimal template",
                FALLBACK_TEMPLATE_ID, e
            );
            TemplateChoice {
                template: Template {
                    name: "Standard Meeting Notes".to_string(),
                    description: "General meeting summary.".to_string(),
                    sections: vec![TemplateSection {
                        title: "Summary".to_string(),
                        instruction: "Summarize the meeting.".to_string(),
                        format: "paragraph".to_string(),
                        item_format: None,
                        example_item_format: None,
                    }],
                },
                template_id: Some(FALLBACK_TEMPLATE_ID.to_string()),
                is_generated: false,
            }
        }
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

/// Strips a single leading/trailing markdown code fence (```` ``` ```` or
/// ```` ```json ````) from `raw`, if present. Returns the trimmed content
/// unchanged if it isn't fenced.
fn strip_code_fences(raw: &str) -> String {
    let trimmed = raw.trim();

    let Some(after_open) = trimmed.strip_prefix("```") else {
        return trimmed.to_string();
    };

    // Skip an optional language identifier (e.g. "json") up to the first
    // newline.
    let body = match after_open.find('\n') {
        Some(idx) => &after_open[idx + 1..],
        None => after_open,
    };

    body.strip_suffix("```")
        .unwrap_or(body)
        .trim()
        .to_string()
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

/// Builds the system prompt instructing the LLM to either match a candidate
/// template or generate a new one, as strict JSON.
fn build_system_prompt(candidates: &[TemplateInfo]) -> String {
    let candidate_list = if candidates.is_empty() {
        "(no existing templates available)".to_string()
    } else {
        candidates
            .iter()
            .map(|c| format!("- id: \"{}\", name: \"{}\", description: \"{}\"", c.id, c.name, c.description))
            .collect::<Vec<_>>()
            .join("\n")
    };

    format!(
        r#"You are selecting or designing a meeting summary template based on a transcript excerpt.

Below are the existing templates available. If one of them genuinely fits the meeting content based on its name and description, respond by matching it. Otherwise, design a brand-new one-use template tailored to this meeting.

EXISTING TEMPLATES:
{candidate_list}

Respond with strict JSON only (no markdown, no commentary, no code fences) in exactly one of these two shapes:

1. To match an existing template:
{{"match": "<template_id>"}}

2. To generate a new template:
{{"generate": {{"name": "...", "description": "...", "sections": [{{"title": "...", "instruction": "...", "format": "paragraph|list|string", "item_format": null, "example_item_format": null}}]}}}}

Example of a real template's shape (for reference only - do not copy verbatim unless it genuinely fits):
{{
  "name": "Standard Meeting Notes",
  "description": "A standard template for general meetings, focusing on key outcomes and actions.",
  "sections": [
    {{"title": "Summary", "instruction": "Provide a brief, one-paragraph executive summary of the entire meeting.", "format": "paragraph"}},
    {{"title": "Action Items", "instruction": "List all assigned tasks with their owners and due date.", "format": "list", "item_format": "| **Owner** | Task | Due |\n| --- | --- | --- |"}}
  ]
}}

Rules:
- "format" must be exactly one of "paragraph", "list", or "string".
- Only use "match" when a candidate's name/description genuinely fits this meeting's content.
- Output only the JSON object and nothing else."#
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
    fn resolve_parsed_choice_match_resolves_known_template() {
        let choice = resolve_parsed_choice(Ok(ParsedChoice::Match("daily_standup".to_string())));
        assert_eq!(choice.template_id.as_deref(), Some("daily_standup"));
        assert!(!choice.is_generated);
        assert_eq!(choice.template.name, "Daily Standup");
    }

    #[test]
    fn resolve_parsed_choice_match_falls_back_on_unknown_id() {
        let choice = resolve_parsed_choice(Ok(ParsedChoice::Match("does_not_exist".to_string())));
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
    fn fallback_choice_loads_standard_meeting() {
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
        let transcript = word.repeat(5000); // Well beyond the excerpt token budget.
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
    fn build_system_prompt_lists_candidates() {
        let candidates = vec![
            candidate("daily_standup", "Daily Standup", "Time-boxed daily updates"),
            candidate("standard_meeting", "Standard Meeting Notes", "General meeting notes"),
        ];
        let prompt = build_system_prompt(&candidates);
        assert!(prompt.contains("daily_standup"));
        assert!(prompt.contains("standard_meeting"));
        assert!(prompt.contains("\"match\""));
        assert!(prompt.contains("\"generate\""));
    }

    #[test]
    fn build_system_prompt_handles_no_candidates() {
        let prompt = build_system_prompt(&[]);
        assert!(prompt.contains("no existing templates available"));
    }
}
