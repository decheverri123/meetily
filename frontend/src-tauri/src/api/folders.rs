use log::{info as log_info, warn as log_warn};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Manager, Runtime};

use crate::{
    audio::recording_commands::{
        resolve_effective_model_name, resolve_provider_invocation, LiveLlmProviderInvocation,
    },
    database::{
        models::FolderModel,
        repositories::{
            folders::FoldersRepository, meeting::MeetingsRepository, setting::SettingsRepository,
        },
    },
    state::AppState,
    summary::llm_client::{generate_summary, LLMProvider},
};

#[derive(Debug, Serialize, Deserialize)]
pub struct Folder {
    pub id: String,
    pub name: String,
    pub created_at: String,
    pub updated_at: String,
}

impl From<FolderModel> for Folder {
    fn from(m: FolderModel) -> Self {
        Folder {
            id: m.id,
            name: m.name,
            created_at: m.created_at.0.to_rfc3339(),
            updated_at: m.updated_at.0.to_rfc3339(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MeetingWithFolder {
    pub id: String,
    pub title: String,
    pub folder_id: Option<String>,
    pub folder_name: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CategorizeResult {
    pub meeting_id: String,
    pub folder_id: Option<String>,
    pub folder_name: Option<String>,
    pub suggested_new_folder: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BatchCategorizeResult {
    pub total: usize,
    pub assigned: usize,
    pub suggested_new: usize,
    pub failed: usize,
    pub results: Vec<CategorizeResult>,
}

async fn resolve_llm_invocation(
    pool: &sqlx::SqlitePool,
) -> Result<LiveLlmProviderInvocation, String> {
    let config = SettingsRepository::get_model_config(pool)
        .await
        .map_err(|e| format!("Failed to read model config: {}", e))?
        .ok_or_else(|| "No LLM configured. Set a model in Settings first.".to_string())?;

    let provider = LLMProvider::from_str(&config.provider)
        .map_err(|e| format!("Unsupported LLM provider '{}': {}", config.provider, e))?;

    if provider == LLMProvider::BuiltInAI {
        return Err(
            "AI categorization requires a non-builtin LLM provider. Set one in Settings first."
                .to_string(),
        );
    }

    let effective_model_name = resolve_effective_model_name(None, Some(&config.model))
        .ok_or_else(|| "No model configured for the selected provider.".to_string())?;

    let api_key = SettingsRepository::get_api_key(pool, &config.provider)
        .await
        .map_err(|e| format!("Failed to read API key: {}", e))?
        .unwrap_or_default();

    let ollama_endpoint = config.ollama_endpoint.as_deref();

    let custom_openai_config = if provider == LLMProvider::CustomOpenAI {
        SettingsRepository::get_custom_openai_config(pool)
            .await
            .map_err(|e| format!("Failed to read custom OpenAI config: {}", e))?
    } else {
        None
    };
    let custom_openai_config_ref = custom_openai_config.as_ref();

    resolve_provider_invocation(
        &provider,
        &effective_model_name,
        Some(&api_key),
        ollama_endpoint,
        custom_openai_config_ref,
    )
    .map_err(|e| format!("Failed to resolve LLM provider invocation: {}", e))
}

fn build_categorize_prompt(
    meeting_title: &str,
    transcript_excerpt: &str,
    folder_names: &[String],
) -> (String, String) {
    let folder_list = if folder_names.is_empty() {
        "(no existing folders - propose a new folder name)".to_string()
    } else {
        folder_names
            .iter()
            .map(|n| format!("- {}", n))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let system = "You are a meeting categorization assistant. \
You assign a meeting to exactly one existing folder, or propose a new folder name. \
Reply with ONLY a JSON object of the form {\"folder\": \"<existing folder name exactly>\"} or {\"new_folder\": \"<short noun phrase>\"}. \
No prose, no markdown."

        .to_string();

    let user = format!(
        "Meeting title: {}\n\nTranscript excerpt:\n{}\n\nExisting folders:\n{}\n\nReturn JSON only.",
        meeting_title,
        if transcript_excerpt.is_empty() { "(no transcript)" } else { transcript_excerpt },
        folder_list
    );

    (system, user)
}

fn parse_categorize_response(raw: &str) -> Result<CategorizeDecision, String> {
    let trimmed = raw.trim();
    let start = trimmed.find('{');
    let end = trimmed.rfind('}');
    let json_str = match (start, end) {
        (Some(s), Some(e)) if e > s => &trimmed[s..=e],
        _ => return Err("LLM response did not contain a JSON object".to_string()),
    };

    #[derive(Deserialize)]
    struct Raw {
        #[serde(default)]
        folder: Option<String>,
        #[serde(default)]
        new_folder: Option<String>,
    }

    let parsed: Raw = serde_json::from_str(json_str)
        .map_err(|e| format!("LLM JSON parse failed: {} (raw: {})", e, json_str))?;

    if let Some(name) = parsed.new_folder {
        let name = name.trim();
        if !name.is_empty() {
            return Ok(CategorizeDecision::New(name.to_string()));
        }
    }
    if let Some(name) = parsed.folder {
        let name = name.trim();
        if !name.is_empty() {
            return Ok(CategorizeDecision::Existing(name.to_string()));
        }
    }
    Err("LLM response missing 'folder' or 'new_folder' field".to_string())
}

#[derive(Debug)]
enum CategorizeDecision {
    Existing(String),
    New(String),
}

async fn categorize_one(
    pool: &sqlx::SqlitePool,
    app_data_dir: Option<&PathBuf>,
    meeting_id: &str,
    folder_names: &[String],
) -> Result<CategorizeResult, String> {
    let meeting = MeetingsRepository::get_meeting_metadata(pool, meeting_id)
        .await
        .map_err(|e| format!("Failed to load meeting {}: {}", meeting_id, e))?
        .ok_or_else(|| format!("Meeting not found: {}", meeting_id))?;

    let transcript_excerpt =
        MeetingsRepository::get_recent_transcript_text(pool, meeting_id, 4000)
            .await
            .map_err(|e| format!("Failed to load transcript: {}", e))?;

    let invocation = resolve_llm_invocation(pool).await?;

    let (system_prompt, user_prompt) =
        build_categorize_prompt(&meeting.title, &transcript_excerpt, folder_names);

    let client = Client::new();
    let raw = generate_summary(
        &client,
        &invocation.provider,
        &invocation.model_name,
        &invocation.api_key,
        &system_prompt,
        &user_prompt,
        invocation.ollama_endpoint.as_deref(),
        invocation.custom_openai_endpoint.as_deref(),
        invocation.custom_openai_max_tokens,
        invocation.custom_openai_temperature,
        invocation.custom_openai_top_p,
        app_data_dir,
        None,
    )
    .await?;

    let decision = parse_categorize_response(&raw)?;

    let (folder_id, folder_name, suggested_new) = match decision {
        CategorizeDecision::Existing(name) => {
            let folder = sqlx::query_as::<_, FolderModel>(
                "SELECT * FROM folders WHERE LOWER(name) = LOWER(?)",
            )
            .bind(&name)
            .fetch_optional(pool)
            .await
            .map_err(|e| format!("DB error: {}", e))?;

            match folder {
                Some(f) => {
                    FoldersRepository::assign_meeting(pool, meeting_id, Some(&f.id))
                        .await
                        .map_err(|e| format!("Failed to assign meeting: {}", e))?;
                    (Some(f.id), Some(f.name), None)
                }
                None => {
                    let created = FoldersRepository::create_folder(pool, &name)
                        .await
                        .map_err(|e| format!("Failed to create folder: {}", e))?;
                    FoldersRepository::assign_meeting(pool, meeting_id, Some(&created.id))
                        .await
                        .map_err(|e| format!("Failed to assign meeting: {}", e))?;
                    (Some(created.id), Some(created.name), None)
                }
            }
        }
        CategorizeDecision::New(name) => {
            let created = FoldersRepository::create_folder(pool, &name)
                .await
                .map_err(|e| format!("Failed to create folder: {}", e))?;
            FoldersRepository::assign_meeting(pool, meeting_id, Some(&created.id))
                .await
                .map_err(|e| format!("Failed to assign meeting: {}", e))?;
            (Some(created.id), Some(created.name.clone()), Some(created.name))
        }
    };

    Ok(CategorizeResult {
        meeting_id: meeting_id.to_string(),
        folder_id,
        folder_name,
        suggested_new_folder: suggested_new,
    })
}

#[tauri::command]
pub async fn api_get_folders<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<Folder>, String> {
    log_info!("api_get_folders called");
    let pool = state.db_manager.pool();
    FoldersRepository::list_folders(pool)
        .await
        .map(|folders| folders.into_iter().map(Folder::from).collect())
        .map_err(|e| format!("Failed to list folders: {}", e))
}

#[tauri::command]
pub async fn api_create_folder<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    name: String,
) -> Result<Folder, String> {
    log_info!("api_create_folder called: '{}'", name);
    let pool = state.db_manager.pool();
    FoldersRepository::create_folder(pool, &name)
        .await
        .map(Folder::from)
        .map_err(|e| format!("Failed to create folder: {}", e))
}

#[tauri::command]
pub async fn api_rename_folder<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    id: String,
    name: String,
) -> Result<bool, String> {
    log_info!("api_rename_folder called: id='{}', new_name='{}'", id, name);
    let pool = state.db_manager.pool();
    FoldersRepository::rename_folder(pool, &id, &name)
        .await
        .map_err(|e| format!("Failed to rename folder: {}", e))
}

#[tauri::command]
pub async fn api_delete_folder<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<bool, String> {
    log_info!("api_delete_folder called: id='{}'", id);
    let pool = state.db_manager.pool();
    FoldersRepository::delete_folder(pool, &id)
        .await
        .map_err(|e| format!("Failed to delete folder: {}", e))
}

#[tauri::command]
pub async fn api_assign_meeting_to_folder<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    folder_id: Option<String>,
) -> Result<bool, String> {
    log_info!(
        "api_assign_meeting_to_folder called: meeting_id='{}', folder_id={:?}",
        meeting_id,
        folder_id
    );
    let pool = state.db_manager.pool();
    FoldersRepository::assign_meeting(pool, &meeting_id, folder_id.as_deref())
        .await
        .map_err(|e| format!("Failed to assign meeting: {}", e))
}

#[tauri::command]
pub async fn api_ai_categorize_meeting<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    meeting_id: String,
    folder_ids: Option<Vec<String>>,
) -> Result<CategorizeResult, String> {
    log_info!(
        "api_ai_categorize_meeting called: meeting_id='{}', folder_ids={:?}",
        meeting_id,
        folder_ids
    );
    let pool = state.db_manager.pool();

    let folder_names: Vec<String> = match folder_ids {
        Some(ids) if !ids.is_empty() => {
            let mut out = Vec::new();
            for id in ids {
                let f: Option<FolderModel> = sqlx::query_as("SELECT * FROM folders WHERE id = ?")
                    .bind(&id)
                    .fetch_optional(pool)
                    .await
                    .map_err(|e| format!("DB error: {}", e))?;
                if let Some(f) = f {
                    out.push(f.name);
                }
            }
            out
        }
        _ => FoldersRepository::list_folders(pool)
            .await
            .map_err(|e| format!("Failed to list folders: {}", e))?
            .into_iter()
            .map(|f| f.name)
            .collect(),
    };

    let app_data_dir = app.path().app_data_dir().ok();
    categorize_one(pool, app_data_dir.as_ref(), &meeting_id, &folder_names).await
}

#[tauri::command]
pub async fn api_ai_categorize_all_meetings<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
) -> Result<BatchCategorizeResult, String> {
    log_info!("api_ai_categorize_all_meetings called");
    let pool = state.db_manager.pool();

    let folder_names: Vec<String> = FoldersRepository::list_folders(pool)
        .await
        .map_err(|e| format!("Failed to list folders: {}", e))?
        .into_iter()
        .map(|f| f.name)
        .collect();

    let unfiled_ids = FoldersRepository::unfiled_meeting_ids(pool)
        .await
        .map_err(|e| format!("Failed to list unfiled meetings: {}", e))?;

    let total = unfiled_ids.len();
    let app_data_dir = app.path().app_data_dir().ok();

    let mut results = Vec::with_capacity(total);
    let mut assigned = 0;
    let mut suggested_new = 0;
    let mut failed = 0;

    for id in unfiled_ids {
        match categorize_one(pool, app_data_dir.as_ref(), &id, &folder_names).await {
            Ok(r) => {
                if r.suggested_new_folder.is_some() {
                    suggested_new += 1;
                } else {
                    assigned += 1;
                }
                results.push(r);
            }
            Err(e) => {
                log_warn!("Failed to categorize meeting {}: {}", id, e);
                failed += 1;
                results.push(CategorizeResult {
                    meeting_id: id,
                    folder_id: None,
                    folder_name: None,
                    suggested_new_folder: None,
                });
            }
        }
    }

    log_info!(
        "api_ai_categorize_all_meetings: total={}, assigned={}, suggested_new={}, failed={}",
        total,
        assigned,
        suggested_new,
        failed
    );

    Ok(BatchCategorizeResult {
        total,
        assigned,
        suggested_new,
        failed,
        results,
    })
}

#[tauri::command]
pub async fn api_get_meetings_with_folders<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<MeetingWithFolder>, String> {
    let pool = state.db_manager.pool();
    let meetings = MeetingsRepository::get_meetings(pool)
        .await
        .map_err(|e| format!("Failed to load meetings: {}", e))?;
    let folders = FoldersRepository::list_folders(pool)
        .await
        .map_err(|e| format!("Failed to load folders: {}", e))?;
    let folder_by_id: std::collections::HashMap<String, String> =
        folders.into_iter().map(|f| (f.id, f.name)).collect();

    Ok(meetings
        .into_iter()
        .map(|m| {
            let (folder_id, folder_name) = match m.meeting_folder_id.as_ref() {
                Some(id) => (Some(id.clone()), folder_by_id.get(id).cloned()),
                None => (None, None),
            };
            MeetingWithFolder {
                id: m.id,
                title: m.title,
                folder_id,
                folder_name,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_categorize_response_picks_folder() {
        let raw = r#"{"folder": "Q4 Planning"}"#;
        match parse_categorize_response(raw).unwrap() {
            CategorizeDecision::Existing(n) => assert_eq!(n, "Q4 Planning"),
            other => panic!("expected Existing, got {:?}", other),
        }
    }

    #[test]
    fn parse_categorize_response_picks_new_folder() {
        let raw = r#"{"new_folder": "Customer Calls"}"#;
        match parse_categorize_response(raw).unwrap() {
            CategorizeDecision::New(n) => assert_eq!(n, "Customer Calls"),
            other => panic!("expected New, got {:?}", other),
        }
    }

    #[test]
    fn parse_categorize_response_handles_wrapped_json() {
        let raw = "Sure! Here you go:\n{\"folder\": \"Engineering\"}\nThanks.";
        match parse_categorize_response(raw).unwrap() {
            CategorizeDecision::Existing(n) => assert_eq!(n, "Engineering"),
            other => panic!("expected Existing, got {:?}", other),
        }
    }

    #[test]
    fn parse_categorize_response_rejects_missing_fields() {
        let raw = r#"{"foo": "bar"}"#;
        assert!(parse_categorize_response(raw).is_err());
    }

    #[test]
    fn parse_categorize_response_rejects_garbage() {
        assert!(parse_categorize_response("not json").is_err());
    }

    #[test]
    fn build_categorize_prompt_mentions_existing_folders() {
        let (sys, user) =
            build_categorize_prompt("Standup", "Hello world", &vec!["a".into(), "b".into()]);
        assert!(sys.contains("JSON"));
        assert!(user.contains("Standup"));
        assert!(user.contains("- a"));
        assert!(user.contains("- b"));
    }

    #[test]
    fn build_categorize_prompt_handles_no_folders() {
        let (_sys, user) = build_categorize_prompt("Solo", "t", &[]);
        assert!(user.contains("propose a new folder"));
    }
}
