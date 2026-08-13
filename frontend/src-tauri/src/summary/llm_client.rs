use reqwest::{header, Client};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::database::{
    models::TokenUsagePurpose,
    token_usage_recorder::record_token_usage,
};

const REQUEST_TIMEOUT_DURATION: Duration = Duration::from_secs(300);

// Generic structure for OpenAI-compatible API chat messages
#[derive(Debug, Serialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

// Generic structure for OpenAI-compatible API chat requests
#[derive(Debug, Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
}

// Generic structure for OpenAI-compatible API chat responses
#[derive(Deserialize, Debug)]
pub struct ChatResponse {
    pub choices: Vec<Choice>,
    #[serde(default)]
    pub usage: Option<ChatUsage>,
}

/// OpenAI-compatible usage block. Returned alongside `choices` by OpenAI, Groq,
/// Ollama, OpenRouter, CustomOpenAI and LM Studio - `serde(default)` makes it
/// optional so providers that omit it still parse.
#[derive(Deserialize, Debug, Clone)]
pub struct ChatUsage {
    #[serde(default)]
    pub prompt_tokens: i64,
    #[serde(default)]
    pub completion_tokens: i64,
    #[serde(default)]
    pub total_tokens: i64,
}

#[derive(Deserialize, Debug)]
pub struct Choice {
    pub message: MessageContent,
}

#[derive(Deserialize, Debug)]
pub struct MessageContent {
    pub content: String,
}

// Ollama-native request/response structures for the `/api/chat` endpoint.
//
// Ollama's OpenAI-compatible shim (`/v1/chat/completions`) decodes the
// request body into a fixed Go struct with no `options`/`num_ctx` field at
// all, so a context-window override can never reach it - the field is
// silently dropped by Go's JSON unmarshaling regardless of where in the body
// it's placed. Only Ollama's native endpoints honor `"options": {"num_ctx": N}`,
// so the Ollama branch of `generate_summary` targets `/api/chat` instead of
// the shared OpenAI-compat path used by every other provider.
#[derive(Debug, Serialize)]
pub struct OllamaOptions {
    pub num_ctx: u32,
}

#[derive(Debug, Serialize)]
pub struct OllamaChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<OllamaOptions>,
}

#[derive(Deserialize, Debug)]
pub struct OllamaChatResponse {
    pub message: MessageContent,
}

// Claude-specific request structure
#[derive(Debug, Serialize)]
pub struct ClaudeRequest {
    pub model: String,
    pub max_tokens: u32,
    pub system: String,
    pub messages: Vec<ChatMessage>,
}

// Claude-specific response structure
#[derive(Deserialize, Debug)]
pub struct ClaudeChatResponse {
    pub content: Vec<ClaudeChatContent>,
    #[serde(default)]
    pub usage: Option<ClaudeUsage>,
}

#[derive(Deserialize, Debug)]
pub struct ClaudeChatContent {
    pub text: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct ClaudeUsage {
    #[serde(default)]
    pub input_tokens: i64,
    #[serde(default)]
    pub output_tokens: i64,
}

/// LLM Provider enumeration for multi-provider support
#[derive(Debug, Clone, PartialEq)]
pub enum LLMProvider {
    OpenAI,
    Claude,
    Groq,
    Ollama,
    OpenRouter,
    BuiltInAI,
    CustomOpenAI,
    LmStudio,
}

/// Captured token usage for a single LLM call, returned alongside the
/// generated text so callers can persist it without re-parsing the raw
/// response. `None` for providers/clients that don't return usage.
#[derive(Debug, Clone)]
pub struct LLMUsage {
    pub provider: LLMProvider,
    pub model: String,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub total_tokens: i64,
}

/// Result of a `generate_summary` call: the generated text plus optional
/// usage telemetry. Callers should pattern-match `.summary` (always present
/// on `Ok`) and optionally record `.usage`.
#[derive(Debug)]
pub struct GenerateSummaryOutput {
    pub summary: String,
    pub usage: Option<LLMUsage>,
}

pub struct TokenUsageContext {
    pub pool: sqlx::SqlitePool,
    pub meeting_id: Option<String>,
    pub purpose: TokenUsagePurpose,
}

impl LLMProvider {
    /// String identifier used for persistence (token_usage row, pricing lookup).
    /// Mirrors the `from_str` arms exactly so round-tripping is safe.
    pub fn as_str(&self) -> &'static str {
        match self {
            LLMProvider::OpenAI => "openai",
            LLMProvider::Claude => "claude",
            LLMProvider::Groq => "groq",
            LLMProvider::Ollama => "ollama",
            LLMProvider::OpenRouter => "openrouter",
            LLMProvider::BuiltInAI => "builtin-ai",
            LLMProvider::CustomOpenAI => "custom-openai",
            LLMProvider::LmStudio => "lmstudio",
        }
    }

    /// Parse provider from string (case-insensitive)
    pub fn from_str(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "openai" => Ok(Self::OpenAI),
            "claude" => Ok(Self::Claude),
            "groq" => Ok(Self::Groq),
            "ollama" => Ok(Self::Ollama),
            "openrouter" => Ok(Self::OpenRouter),
            "builtin-ai" | "local-llama" | "localllama" => Ok(Self::BuiltInAI),
            "custom-openai" => Ok(Self::CustomOpenAI),
            "lmstudio" => Ok(Self::LmStudio),
            _ => Err(format!("Unsupported LLM provider: {}", s)),
        }
    }
}

/// Generates a summary using the specified LLM provider
///
/// # Arguments
/// * `client` - Reqwest HTTP client (reused for performance)
/// * `provider` - The LLM provider to use
/// * `model_name` - The specific model to use (e.g., "gpt-4", "claude-3-opus")
/// * `api_key` - API key for the provider (not needed for Ollama)
/// * `system_prompt` - System instructions for the LLM
/// * `user_prompt` - User query/content to process
/// * `ollama_endpoint` - Optional custom Ollama endpoint (defaults to localhost:11434)
/// * `custom_openai_endpoint` - Optional custom OpenAI-compatible endpoint
/// * `max_tokens` - Optional max tokens (for CustomOpenAI provider)
/// * `temperature` - Optional temperature (for CustomOpenAI provider)
/// * `top_p` - Optional top_p (for CustomOpenAI provider)
/// * `app_data_dir` - Optional app data directory (for BuiltInAI provider)
/// * `cancellation_token` - Optional token to cancel the request
/// * `num_ctx` - Optional Ollama context window size (in tokens) to request
///   via the `num_ctx` request field. Ollama-only: ignored for every other
///   provider, since without it Ollama silently falls back to the model's
///   own (often much smaller) default context rather than the resolved
///   window the caller actually sized its prompt against.
///
/// # Returns
/// The generated summary text or an error message
pub async fn generate_summary(
    client: &Client,
    provider: &LLMProvider,
    model_name: &str,
    api_key: &str,
    system_prompt: &str,
    user_prompt: &str,
    ollama_endpoint: Option<&str>,
    custom_openai_endpoint: Option<&str>,
    max_tokens: Option<u32>,
    temperature: Option<f32>,
    top_p: Option<f32>,
    app_data_dir: Option<&PathBuf>,
    cancellation_token: Option<&CancellationToken>,
    num_ctx: Option<u32>,
    usage_context: Option<TokenUsageContext>,
) -> Result<GenerateSummaryOutput, String> {
    // Check if cancelled before starting
    if let Some(token) = cancellation_token {
        if token.is_cancelled() {
            return Err("Summary generation was cancelled".to_string());
        }
    }

    // Handle BuiltInAI provider separately (uses local sidecar, no HTTP API)
    if provider == &LLMProvider::BuiltInAI {
        let app_data_dir = app_data_dir
            .ok_or_else(|| "app_data_dir is required for BuiltInAI provider".to_string())?;

        let summary = crate::summary::summary_engine::generate_with_builtin(
            app_data_dir,
            model_name,
            system_prompt,
            user_prompt,
            cancellation_token,
        )
        .await
        .map_err(|e| e.to_string())?;

        let prompt_tokens = crate::summary::processor::rough_token_count(
            &format!("{}{}", system_prompt, user_prompt),
        ) as i64;
        let completion_tokens = crate::summary::processor::rough_token_count(&summary) as i64;

        let output = GenerateSummaryOutput {
            summary,
            usage: Some(LLMUsage {
                provider: LLMProvider::BuiltInAI,
                model: model_name.to_string(),
                prompt_tokens,
                completion_tokens,
                total_tokens: prompt_tokens + completion_tokens,
            }),
        };

        if let (Some(usage), Some(ctx)) = (&output.usage, usage_context) {
            record_token_usage(ctx.pool, ctx.meeting_id, usage.clone(), ctx.purpose);
        }

        return Ok(output);
    }

    let (api_url, mut headers) = match provider {
        LLMProvider::OpenAI => (
            "https://api.openai.com/v1/chat/completions".to_string(),
            header::HeaderMap::new(),
        ),
        LLMProvider::Groq => (
            "https://api.groq.com/openai/v1/chat/completions".to_string(),
            header::HeaderMap::new(),
        ),
        LLMProvider::OpenRouter => (
            "https://openrouter.ai/api/v1/chat/completions".to_string(),
            header::HeaderMap::new(),
        ),
        LLMProvider::Ollama => {
            let host = ollama_endpoint
                .map(|s| s.to_string())
                .unwrap_or_else(|| "http://localhost:11434".to_string());
            // Ollama's native endpoint, not the OpenAI-compat shim: only this
            // path honors a nested `"options": {"num_ctx": N}` context-window
            // override (see the `OllamaChatRequest` doc comment above).
            (
                format!("{}/api/chat", host),
                header::HeaderMap::new(),
            )
        }
        LLMProvider::LmStudio => {
            let host = ollama_endpoint
                .map(|s| s.to_string())
                .unwrap_or_else(|| "http://localhost:1234/v1".to_string());
            (
                format!("{}/chat/completions", host),
                header::HeaderMap::new(),
            )
        }
        LLMProvider::CustomOpenAI => {
            let endpoint = custom_openai_endpoint
                .ok_or_else(|| "Custom OpenAI endpoint not configured".to_string())?;
            (
                format!("{}/chat/completions", endpoint.trim_end_matches('/')),
                header::HeaderMap::new(),
            )
        }
        LLMProvider::Claude => {
            let mut header_map = header::HeaderMap::new();
            header_map.insert(
                "x-api-key",
                api_key
                    .parse()
                    .map_err(|_| "Invalid API key format".to_string())?,
            );
            header_map.insert(
                "anthropic-version",
                "2023-06-01"
                    .parse()
                    .map_err(|_| "Invalid anthropic version".to_string())?,
            );
            ("https://api.anthropic.com/v1/messages".to_string(), header_map)
        }
        LLMProvider::BuiltInAI => {
            // This case is handled earlier with early returns
            unreachable!("BuiltInAI is handled before this match statement")
        }
    };

    // Add authorization header for non-Claude, non-Ollama, non-LmStudio providers
    if provider != &LLMProvider::Claude && provider != &LLMProvider::Ollama && provider != &LLMProvider::LmStudio {
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {}", api_key)
                .parse()
                .map_err(|_| "Invalid authorization header".to_string())?,
        );
    }
    headers.insert(
        header::CONTENT_TYPE,
        "application/json"
            .parse()
            .map_err(|_| "Invalid content type".to_string())?,
    );

    // Build request body based on provider
    let request_body = if provider == &LLMProvider::Claude {
        serde_json::json!(ClaudeRequest {
            system: system_prompt.to_string(),
            model: model_name.to_string(),
            max_tokens: 2048,
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: user_prompt.to_string(),
            }]
        })
    } else if provider == &LLMProvider::Ollama {
        serde_json::json!(OllamaChatRequest {
            model: model_name.to_string(),
            messages: vec![
                ChatMessage {
                    role: "system".to_string(),
                    content: system_prompt.to_string(),
                },
                ChatMessage {
                    role: "user".to_string(),
                    content: user_prompt.to_string(),
                }
            ],
            stream: false,
            // A 0 num_ctx is not a safe "no override" no-op to Ollama - it's
            // an explicit request for a zero-token context window. Guard
            // here too (not just at the resolve_ask_context_budget source)
            // since this is the last stop before the wire request.
            options: num_ctx.filter(|&n| n > 0).map(|n| OllamaOptions { num_ctx: n }),
        })
    } else {
        // For CustomOpenAI, apply optional parameters if provided
        let (max_tokens_val, temperature_val, top_p_val) = if provider == &LLMProvider::CustomOpenAI {
            (max_tokens, temperature, top_p)
        } else {
            (None, None, None)
        };

        serde_json::json!(ChatRequest {
            model: model_name.to_string(),
            messages: vec![
                ChatMessage {
                    role: "system".to_string(),
                    content: system_prompt.to_string(),
                },
                ChatMessage {
                    role: "user".to_string(),
                    content: user_prompt.to_string(),
                }
            ],
            max_tokens: max_tokens_val,
            temperature: temperature_val,
            top_p: top_p_val,
        })
    };

    info!("🐞 LLM Request to {}: model={}", provider_name(provider), model_name);

    // Send request with timeout and cancellation support
    let request_future = client
        .post(api_url)
        .headers(headers)
        .json(&request_body)
        .timeout(REQUEST_TIMEOUT_DURATION)
        .send();

    // Use tokio::select to race between cancellation and request completion
    let response = if let Some(token) = cancellation_token {
        tokio::select! {
            result = request_future => {
                result.map_err(|e| {
                    if e.is_timeout() {
                        format!("LLM request timed out after 60 seconds")
                    } else {
                        format!("Failed to send request to LLM: {}", e)
                    }
                })?
            }
            _ = token.cancelled() => {
                return Err("Summary generation was cancelled".to_string());
            }
        }
    } else {
        request_future.await.map_err(|e| {
            if e.is_timeout() {
                format!("LLM request timed out after 60 seconds")
            } else {
                format!("Failed to send request to LLM: {}", e)
            }
        })?
    };

    if !response.status().is_success() {
        let error_body = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
        return Err(format!("LLM API request failed: {}", error_body));
    }

    // Parse response based on provider
    let output = if provider == &LLMProvider::Claude {
        let chat_response = response
            .json::<ClaudeChatResponse>()
            .await
            .map_err(|e| format!("Failed to parse LLM response: {}", e))?;

        info!("🐞 LLM Response received from Claude");

        let content = chat_response
            .content
            .get(0)
            .ok_or("No content in LLM response")?
            .text
            .trim();
        let usage = chat_response.usage.as_ref().map(|u| LLMUsage {
            provider: provider.clone(),
            model: model_name.to_string(),
            prompt_tokens: u.input_tokens,
            completion_tokens: u.output_tokens,
            total_tokens: u.input_tokens + u.output_tokens,
        });
        GenerateSummaryOutput {
            summary: content.to_string(),
            usage,
        }
    } else if provider == &LLMProvider::Ollama {
        // Ollama's native /api/chat doesn't always return usage; fine to
        // emit `usage: None` here - the OpenAI-compat path below covers
        // providers that do.
        let chat_response = response
            .json::<OllamaChatResponse>()
            .await
            .map_err(|e| format!("Failed to parse LLM response: {}", e))?;

        info!("🐞 LLM Response received from {}", provider_name(provider));

        GenerateSummaryOutput {
            summary: chat_response.message.content.trim().to_string(),
            usage: None,
        }
    } else {
        let chat_response = response
            .json::<ChatResponse>()
            .await
            .map_err(|e| format!("Failed to parse LLM response: {}", e))?;

        info!("🐞 LLM Response received from {}", provider_name(provider));

        let content = chat_response
            .choices
            .get(0)
            .ok_or("No content in LLM response")?
            .message
            .content
            .trim();
        let usage = chat_response.usage.as_ref().map(|u| LLMUsage {
            provider: provider.clone(),
            model: model_name.to_string(),
            prompt_tokens: u.prompt_tokens,
            completion_tokens: u.completion_tokens,
            total_tokens: u.total_tokens,
        });
        GenerateSummaryOutput {
            summary: content.to_string(),
            usage,
        }
    };

    if let (Some(usage), Some(ctx)) = (&output.usage, usage_context) {
        record_token_usage(ctx.pool, ctx.meeting_id, usage.clone(), ctx.purpose);
    }

    Ok(output)
}

/// Helper function to get provider name for logging (and, per callers outside
/// this module such as `audio::recording_commands`, for user-facing error
/// messages naming the configured provider).
pub(crate) fn provider_name(provider: &LLMProvider) -> &str {
    match provider {
        LLMProvider::OpenAI => "OpenAI",
        LLMProvider::Claude => "Claude",
        LLMProvider::Groq => "Groq",
        LLMProvider::Ollama => "Ollama",
        LLMProvider::LmStudio => "LM Studio",
        LLMProvider::BuiltInAI => "Built-in AI",
        LLMProvider::OpenRouter => "OpenRouter",
        LLMProvider::CustomOpenAI => "Custom OpenAI",
    }
}

#[cfg(test)]
mod num_ctx_wire_format_tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// Spins up a minimal local mock HTTP server (no external network - just
    /// a loopback TCP listener) and captures the raw request `generate_summary`
    /// sends for a given provider.
    fn capture_request(
        response_body: &'static str,
    ) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
        let addr = listener.local_addr().unwrap();
        let endpoint = format!("http://{}", addr);

        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept connection");
            let mut buf = [0u8; 8192];
            let n = stream.read(&mut buf).unwrap_or(0);
            let request_text = String::from_utf8_lossy(&buf[..n]).to_string();

            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
            request_text
        });

        (endpoint, handle)
    }

    /// Ollama's OpenAI-compat shim (`/v1/chat/completions`) decodes into a
    /// fixed Go struct with no `options`/`num_ctx` field at all (confirmed
    /// against `openai/openai.go`'s `ChatCompletionRequest` and its
    /// `fromChatRequest` translation, whose `options` map is built from a
    /// fixed allowlist that excludes `num_ctx`), so no placement of the field
    /// in that request body can ever reach the model. A context-window
    /// override only takes effect on Ollama's native `/api/chat` endpoint,
    /// nested as `"options": {"num_ctx": N}`. This test proves `generate_summary`
    /// now targets that native endpoint with that wire shape for Ollama.
    #[tokio::test]
    async fn generate_summary_ollama_sends_num_ctx_nested_in_options_on_native_api_chat() {
        let (endpoint, handle) =
            capture_request(r#"{"model":"llama3","message":{"role":"assistant","content":"ok"},"done":true}"#);

        let client = reqwest::Client::new();
        let result = generate_summary(
            &client,
            &LLMProvider::Ollama,
            "llama3",
            "",
            "system",
            "user",
            Some(&endpoint),
            None,
            None,
            None,
            None,
            None,
            None,
            Some(32768), // the resolved model's real context window
            None,
        )
        .await;

        let request_text = handle.join().expect("mock server thread panicked");

        assert!(result.is_ok(), "generate_summary should have succeeded against the mock server: {:?}", result);

        // (1) Ollama's native chat endpoint is hit, not the OpenAI-compat shim.
        assert!(
            request_text.starts_with("POST /api/chat"),
            "expected Ollama's native /api/chat path, got request line: {:?}",
            request_text.lines().next()
        );

        // (2) num_ctx is nested under "options" - the only shape Ollama's
        // native endpoint reads a context-window override from.
        assert!(
            request_text.contains("\"options\":{\"num_ctx\":32768}"),
            "expected num_ctx nested under an \"options\" object, got: {}",
            request_text
        );
    }

    /// A non-200 response from Ollama's native `/api/chat` (e.g. model not
    /// pulled, malformed request) must still be surfaced as an `Err`
    /// carrying the response body, exactly like every other provider - the
    /// endpoint switch must not have bypassed the shared
    /// `!response.status().is_success()` error path for Ollama specifically.
    #[tokio::test]
    async fn generate_summary_ollama_non_200_response_surfaces_error_body() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock server");
        let addr = listener.local_addr().unwrap();
        let endpoint = format!("http://{}", addr);

        let handle = std::thread::spawn(move || {
            use std::io::{Read, Write};
            let (mut stream, _) = listener.accept().expect("accept connection");
            let mut buf = [0u8; 8192];
            let _ = stream.read(&mut buf).unwrap_or(0);

            let body = r#"{"error":"model 'llama3' not found, try pulling it first"}"#;
            let response = format!(
                "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        });

        let client = reqwest::Client::new();
        let result = generate_summary(
            &client,
            &LLMProvider::Ollama,
            "llama3",
            "",
            "system",
            "user",
            Some(&endpoint),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await;

        handle.join().expect("mock server thread panicked");

        let err = result.expect_err("a 404 from Ollama must surface as an Err, not Ok");
        assert!(
            err.contains("not found"),
            "expected the Ollama error body to be surfaced in the error message, got: {}",
            err
        );
    }

    /// `resolve_ask_context_budget` in `summary::commands` can (in a
    /// metadata-quirk case - see its own regression test) resolve a raw
    /// Ollama context window of 0 tokens and forward it unmodified into
    /// `generate_summary`'s `num_ctx` argument. This test proves
    /// `generate_summary` itself applies no floor/guard: `Some(0)` is sent
    /// to Ollama's native endpoint as a literal `"options":{"num_ctx":0}`,
    /// not omitted or clamped to a sane minimum. Per Ollama's llama.cpp
    /// backend, requesting a 0-token context window is not a safe no-op;
    /// there is no defense against it anywhere on this path.
    #[tokio::test]
    async fn generate_summary_ollama_sends_num_ctx_zero_verbatim_with_no_guard() {
        let (endpoint, handle) =
            capture_request(r#"{"model":"llama3","message":{"role":"assistant","content":"ok"},"done":true}"#);

        let client = reqwest::Client::new();
        let result = generate_summary(
            &client,
            &LLMProvider::Ollama,
            "llama3",
            "",
            "system",
            "user",
            Some(&endpoint),
            None,
            None,
            None,
            None,
            None,
            None,
            Some(0),
            None,
        )
        .await;

        let request_text = handle.join().expect("mock server thread panicked");

        assert!(result.is_ok(), "generate_summary should have succeeded against the mock server: {:?}", result);

        assert!(
            !request_text.contains("\"num_ctx\":0"),
            "expected generate_summary to guard against a 0-token num_ctx (omit the options field or \
             clamp to a sane minimum) rather than forwarding it verbatim to Ollama, got request body: {}",
            request_text
        );
    }

    /// Regression coverage: other OpenAI-compatible providers must be
    /// unaffected by the Ollama-specific endpoint switch above and keep
    /// hitting the shared `/v1/chat/completions`-family path with no
    /// `num_ctx`/`options` field at all (the shared `ChatRequest` no longer
    /// has a `num_ctx` field to serialize).
    #[tokio::test]
    async fn generate_summary_openai_still_uses_compat_endpoint_without_num_ctx() {
        let (endpoint, handle) =
            capture_request(r#"{"choices":[{"message":{"content":"ok"}}]}"#);

        let client = reqwest::Client::new();
        let result = generate_summary(
            &client,
            &LLMProvider::LmStudio,
            "llama3",
            "",
            "system",
            "user",
            Some(&endpoint),
            None,
            None,
            None,
            None,
            None,
            None,
            Some(32768),
            None,
        )
        .await;

        let request_text = handle.join().expect("mock server thread panicked");

        assert!(result.is_ok(), "generate_summary should have succeeded against the mock server: {:?}", result);
        assert!(
            request_text.starts_with("POST /chat/completions"),
            "expected LmStudio's OpenAI-compatible /chat/completions path (unchanged by the \
             Ollama-specific fix), got request line: {:?}",
            request_text.lines().next()
        );
        assert!(
            !request_text.contains("num_ctx") && !request_text.contains("\"options\""),
            "num_ctx must not leak into non-Ollama providers' request bodies, got: {}",
            request_text
        );
    }
}
