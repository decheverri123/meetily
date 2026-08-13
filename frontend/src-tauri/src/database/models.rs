use chrono::{DateTime, NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct MeetingModel {
    pub id: String,
    pub title: String,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
    pub folder_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meeting_folder_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::Type)]
#[sqlx(transparent)]
pub struct DateTimeUtc(pub DateTime<Utc>);

impl From<NaiveDateTime> for DateTimeUtc {
    fn from(naive: NaiveDateTime) -> Self {
        DateTimeUtc(DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc))
    }
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct FolderModel {
    pub id: String,
    pub name: String,
    pub created_at: DateTimeUtc,
    pub updated_at: DateTimeUtc,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

// Renamed from TranscriptSegment to Transcript to match the table name
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Transcript {
    pub id: String,
    pub meeting_id: String,
    pub transcript: String,
    pub timestamp: String,
    pub summary: Option<String>,
    pub action_items: Option<String>,
    pub key_points: Option<String>,
    // Recording-relative timestamps for audio-transcript synchronization
    pub audio_start_time: Option<f64>,
    pub audio_end_time: Option<f64>,
    pub duration: Option<f64>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct SummaryProcess {
    pub meeting_id: String,
    pub status: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub error: Option<String>,
    pub result: Option<String>, // JSON
    pub start_time: Option<chrono::DateTime<chrono::Utc>>,
    pub end_time: Option<chrono::DateTime<chrono::Utc>>,
    pub chunk_count: i64,
    pub processing_time: f64,
    pub metadata: Option<String>, // JSON
    pub result_backup: Option<String>, // Backup of result before regeneration
    pub result_backup_timestamp: Option<chrono::DateTime<chrono::Utc>>, // When backup was created
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct TranscriptChunk {
    pub meeting_id: String,
    pub meeting_name: Option<String>,
    pub transcript_text: String,
    pub model: String,
    pub model_name: String,
    pub chunk_size: Option<i64>,
    pub overlap: Option<i64>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Setting {
    pub id: String,
    pub provider: String,
    pub model: String,
    #[sqlx(rename = "whisperModel")]
    #[serde(rename = "whisperModel")]
    pub whisper_model: String,
    #[sqlx(rename = "groqApiKey")]
    #[serde(rename = "groqApiKey")]
    pub groq_api_key: Option<String>,
    #[sqlx(rename = "openaiApiKey")]
    #[serde(rename = "openaiApiKey")]
    pub openai_api_key: Option<String>,
    #[sqlx(rename = "anthropicApiKey")]
    #[serde(rename = "anthropicApiKey")]
    pub anthropic_api_key: Option<String>,
    #[sqlx(rename = "ollamaApiKey")]
    #[serde(rename = "ollamaApiKey")]
    pub ollama_api_key: Option<String>,
    #[sqlx(rename = "openRouterApiKey")]
    #[serde(rename = "openRouterApiKey")]
    pub open_router_api_key: Option<String>,
    #[sqlx(rename = "ollamaEndpoint")]
    #[serde(rename = "ollamaEndpoint")]
    pub ollama_endpoint: Option<String>,
    #[sqlx(rename = "lmstudioEndpoint")]
    #[serde(rename = "lmstudioEndpoint")]
    pub lmstudio_endpoint: Option<String>,
    /// Custom OpenAI-compatible endpoint configuration stored as JSON
    #[sqlx(rename = "customOpenAIConfig")]
    #[serde(rename = "customOpenAIConfig")]
    pub custom_openai_config: Option<String>,
}

impl Setting {
    /// Parse the custom OpenAI config from JSON string
    pub fn get_custom_openai_config(&self) -> Option<crate::summary::CustomOpenAIConfig> {
        self.custom_openai_config.as_ref().and_then(|json| {
            serde_json::from_str(json).ok()
        })
    }
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct TranscriptSetting {
    pub id: String,
    pub provider: String,
    pub model: String,
    #[sqlx(rename = "whisperApiKey")]
    #[serde(rename = "whisperApiKey")]
    pub whisper_api_key: Option<String>,
    #[sqlx(rename = "deepgramApiKey")]
    #[serde(rename = "deepgramApiKey")]
    pub deepgram_api_key: Option<String>,
    #[sqlx(rename = "elevenLabsApiKey")]
    #[serde(rename = "elevenLabsApiKey")]
    pub eleven_labs_api_key: Option<String>,
    #[sqlx(rename = "groqApiKey")]
    #[serde(rename = "groqApiKey")]
    pub groq_api_key: Option<String>,
    #[sqlx(rename = "openaiApiKey")]
    #[serde(rename = "openaiApiKey")]
    pub openai_api_key: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TokenUsagePurpose {
    SummaryChunk,
    SummaryCombine,
    SummaryFinal,
    TemplateSelect,
    QaMeeting,
    QaGlobal,
    QaLive,
    SuggestQuestions,
    LiveInsights,
    LiveActionChip,
    RecommendIcon,
    Normalize,
    Translate,
    Other,
}

impl TokenUsagePurpose {
    pub fn as_str(&self) -> &'static str {
        match self {
            TokenUsagePurpose::SummaryChunk => "summary_chunk",
            TokenUsagePurpose::SummaryCombine => "summary_combine",
            TokenUsagePurpose::SummaryFinal => "summary_final",
            TokenUsagePurpose::TemplateSelect => "template_select",
            TokenUsagePurpose::QaMeeting => "qa_meeting",
            TokenUsagePurpose::QaGlobal => "qa_global",
            TokenUsagePurpose::QaLive => "qa_live",
            TokenUsagePurpose::SuggestQuestions => "suggest_questions",
            TokenUsagePurpose::LiveInsights => "live_insights",
            TokenUsagePurpose::LiveActionChip => "live_action_chip",
            TokenUsagePurpose::RecommendIcon => "recommend_icon",
            TokenUsagePurpose::Normalize => "normalize",
            TokenUsagePurpose::Translate => "translate",
            TokenUsagePurpose::Other => "other",
        }
    }
}

impl From<TokenUsagePurpose> for String {
    fn from(purpose: TokenUsagePurpose) -> Self {
        purpose.as_str().to_string()
    }
}

impl TryFrom<&str> for TokenUsagePurpose {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "summary_chunk" => Ok(TokenUsagePurpose::SummaryChunk),
            "summary_combine" => Ok(TokenUsagePurpose::SummaryCombine),
            "summary_final" => Ok(TokenUsagePurpose::SummaryFinal),
            "template_select" => Ok(TokenUsagePurpose::TemplateSelect),
            "qa_meeting" => Ok(TokenUsagePurpose::QaMeeting),
            "qa_global" => Ok(TokenUsagePurpose::QaGlobal),
            "qa_live" => Ok(TokenUsagePurpose::QaLive),
            "suggest_questions" => Ok(TokenUsagePurpose::SuggestQuestions),
            "live_insights" => Ok(TokenUsagePurpose::LiveInsights),
            "live_action_chip" => Ok(TokenUsagePurpose::LiveActionChip),
            "normalize" => Ok(TokenUsagePurpose::Normalize),
            "translate" => Ok(TokenUsagePurpose::Translate),
            "other" => Ok(TokenUsagePurpose::Other),
            other => Err(format!("Unknown token usage purpose: {}", other)),
        }
    }
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsage {
    pub id: i64,
    pub meeting_id: Option<String>,
    pub provider: String,
    pub model: String,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub total_tokens: i64,
    pub estimated_cost_usd: Option<f64>,
    pub purpose: String,
    pub created_at: DateTime<Utc>,
    pub metadata: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageQueryOpts {
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub purpose: Option<String>,
    pub meeting_id: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelAggregate {
    pub provider: String,
    pub model: String,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub total_tokens: i64,
    pub call_count: i64,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeBucketAggregate {
    #[serde(rename = "bucketStart")]
    pub bucket_start: DateTime<Utc>,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub total_tokens: i64,
    pub call_count: i64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TimeBucket {
    Hour,
    Day,
    Month,
}

impl TimeBucket {
    pub fn as_str(&self) -> &'static str {
        match self {
            TimeBucket::Hour => "hour",
            TimeBucket::Day => "day",
            TimeBucket::Month => "month",
        }
    }
}

impl std::fmt::Display for TimeBucket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
