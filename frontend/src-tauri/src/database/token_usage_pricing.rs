use std::sync::Arc;
use std::time::{Duration, Instant};

use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Runtime};
use tokio::sync::RwLock;

use crate::ollama::metadata::{ModelMetadata, ModelMetadataCache};
use crate::openrouter::OpenRouterModel;
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPricingRequest {
    pub model: String,
    pub provider: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPricing {
    pub model: String,
    pub provider: String,
    pub prompt_price_per_million: Option<f64>,
    pub completion_price_per_million: Option<f64>,
    pub matched_openrouter_id: Option<String>,
    pub source: String,
}

const OPENROUTER_CACHE_TTL: Duration = Duration::from_secs(600);

struct OpenRouterCache {
    models: Arc<Vec<OpenRouterModel>>,
    fetched_at: Instant,
}

static OPENROUTER_CACHE: Lazy<RwLock<Option<OpenRouterCache>>> = Lazy::new(|| RwLock::new(None));

static METADATA_CACHE: Lazy<ModelMetadataCache> = Lazy::new(|| {
    ModelMetadataCache::new(Duration::from_secs(300))
});

async fn get_openrouter_models_cached() -> Option<Arc<Vec<OpenRouterModel>>> {
    {
        let cache = OPENROUTER_CACHE.read().await;
        if let Some(entry) = cache.as_ref() {
            if entry.fetched_at.elapsed() < OPENROUTER_CACHE_TTL {
                return Some(entry.models.clone());
            }
        }
    }
    let fetched = tokio::task::spawn_blocking(crate::openrouter::get_openrouter_models)
        .await
        .unwrap_or_else(|_| Err("blocking task panicked".to_string()));
    match fetched {
        Ok(models) => {
            log::info!("token_usage_pricing: fetched {} OpenRouter models", models.len());
            let models = Arc::new(models);
            let mut cache = OPENROUTER_CACHE.write().await;
            *cache = Some(OpenRouterCache {
                models: models.clone(),
                fetched_at: Instant::now(),
            });
            Some(models)
        }
        Err(e) => {
            log::warn!("token_usage_pricing: OpenRouter fetch failed: {e}");
            None
        }
    }
}

#[tauri::command]
pub async fn api_resolve_model_pricing<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, AppState>,
    models: Vec<ModelPricingRequest>,
    ollama_endpoint: Option<String>,
) -> Result<Vec<ModelPricing>, String> {
    let _ = state;
    let openrouter_models = get_openrouter_models_cached().await;

    let mut results = Vec::with_capacity(models.len());
    for request in models {
        let metadata = if request.provider == "ollama" {
            METADATA_CACHE
                .get_or_fetch(&request.model, ollama_endpoint.as_deref())
                .await
                .ok()
        } else {
            None
        };

        let pricing = match &openrouter_models {
            Some(models) => resolve_pricing(&request, models, metadata.as_ref()),
            None => ModelPricing {
                model: request.model.clone(),
                provider: request.provider.clone(),
                prompt_price_per_million: None,
                completion_price_per_million: None,
                matched_openrouter_id: None,
                source: if is_local_provider(&request.provider) {
                    "local".to_string()
                } else {
                    "unknown".to_string()
                },
            },
        };
        log::info!(
            "token_usage_pricing: {} -> source={} matched={:?} prompt={:?}",
            request.model,
            pricing.source,
            pricing.matched_openrouter_id,
            pricing.prompt_price_per_million
        );
        results.push(pricing);
    }
    Ok(results)
}

fn is_local_provider(provider: &str) -> bool {
    matches!(provider, "ollama" | "builtin-ai" | "lm_studio")
}

fn resolve_pricing(
    request: &ModelPricingRequest,
    openrouter_models: &[OpenRouterModel],
    metadata: Option<&ModelMetadata>,
) -> ModelPricing {
    let base = ModelPricing {
        model: request.model.clone(),
        provider: request.provider.clone(),
        prompt_price_per_million: None,
        completion_price_per_million: None,
        matched_openrouter_id: None,
        source: "unknown".to_string(),
    };

    if is_local_provider(&request.provider) {
        if request.provider == "ollama" {
            return resolve_ollama(request, openrouter_models, metadata, base);
        }
        return ModelPricing {
            source: "local".to_string(),
            ..base
        };
    }

    resolve_other(request, openrouter_models, base)
}

fn resolve_ollama(
    request: &ModelPricingRequest,
    openrouter_models: &[OpenRouterModel],
    metadata: Option<&ModelMetadata>,
    base: ModelPricing,
) -> ModelPricing {
    let Some(meta) = metadata else {
        return ModelPricing {
            source: "local".to_string(),
            ..base
        };
    };

    let family_norm = normalize(&meta.family);
    let target_billions = parse_param_billions(&meta.parameter_count);

    let mut candidates: Vec<&OpenRouterModel> = if family_norm.is_empty() {
        Vec::new()
    } else {
        openrouter_models
            .iter()
            .filter(|m| normalize(&m.id).contains(&family_norm))
            .collect()
    };

    if candidates.is_empty() {
        let base_name = request
            .model
            .split(':')
            .next()
            .unwrap_or(&request.model);
        let base_norm = normalize(base_name);
        candidates = openrouter_models
            .iter()
            .filter(|m| normalize(&m.id).contains(&base_norm))
            .collect();
    }

    match pick_closest(candidates, target_billions) {
        Some(m) => ModelPricing {
            prompt_price_per_million: parse_price_per_million(&m.prompt_price),
            completion_price_per_million: parse_price_per_million(&m.completion_price),
            matched_openrouter_id: Some(m.id.clone()),
            source: "openrouter".to_string(),
            ..base
        },
        None => ModelPricing {
            source: "local".to_string(),
            ..base
        },
    }
}

fn resolve_other(
    request: &ModelPricingRequest,
    openrouter_models: &[OpenRouterModel],
    base: ModelPricing,
) -> ModelPricing {
    let model_lower = request.model.to_lowercase();

    let exact = openrouter_models.iter().find(|m| {
        let seg = last_path_segment(&m.id).to_lowercase();
        seg == model_lower || seg.ends_with(&model_lower)
    });

    let matched = exact.or_else(|| {
        distinctive_token(&request.model)
            .and_then(|t| openrouter_models.iter().find(|m| m.id.to_lowercase().contains(&t)))
    });

    match matched {
        Some(m) => ModelPricing {
            prompt_price_per_million: parse_price_per_million(&m.prompt_price),
            completion_price_per_million: parse_price_per_million(&m.completion_price),
            matched_openrouter_id: Some(m.id.clone()),
            source: "openrouter".to_string(),
            ..base
        },
        None => base,
    }
}

fn pick_closest<'a>(
    candidates: Vec<&'a OpenRouterModel>,
    target_billions: Option<f64>,
) -> Option<&'a OpenRouterModel> {
    match target_billions {
        Some(target) => candidates.into_iter().min_by(|a, b| {
            let da = (estimate_openrouter_billions(&a.id).unwrap_or(0.0) - target).abs();
            let db = (estimate_openrouter_billions(&b.id).unwrap_or(0.0) - target).abs();
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        }),
        None => candidates.into_iter().next(),
    }
}

fn parse_price_per_million(price: &Option<String>) -> Option<f64> {
    price
        .as_deref()?
        .parse::<f64>()
        .ok()
        .map(|v| v * 1_000_000.0)
}

fn last_path_segment(id: &str) -> &str {
    id.rsplit('/').next().unwrap_or(id)
}

fn normalize(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| !matches!(c, '-' | '.' | '_' | ':'))
        .collect()
}

fn parse_param_billions(s: &str) -> Option<f64> {
    let digits: String = s
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    if digits.is_empty() {
        return None;
    }
    let value: f64 = digits.parse().ok()?;
    if value >= 1_000_000_000.0 {
        Some(value / 1_000_000_000.0)
    } else {
        Some(value)
    }
}

fn estimate_openrouter_billions(id: &str) -> Option<f64> {
    static RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(\d+)b").expect("invalid regex"));
    RE.captures(id)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse::<f64>().ok())
}

fn distinctive_token(model: &str) -> Option<String> {
    static RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"-\d{6,}$").expect("invalid regex"));
    let stripped = RE.replace(model, "").to_string();
    let token = stripped.to_lowercase();
    if token.is_empty() {
        None
    } else {
        Some(token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn or_model(id: &str, prompt: &str, completion: &str) -> OpenRouterModel {
        OpenRouterModel {
            id: id.to_string(),
            name: id.to_string(),
            context_length: None,
            prompt_price: Some(prompt.to_string()),
            completion_price: Some(completion.to_string()),
        }
    }

    fn request(model: &str, provider: &str) -> ModelPricingRequest {
        ModelPricingRequest {
            model: model.to_string(),
            provider: provider.to_string(),
        }
    }

    fn metadata(family: &str, param_count: &str) -> ModelMetadata {
        ModelMetadata {
            name: String::new(),
            context_size: 0,
            parameter_count: param_count.to_string(),
            family: family.to_string(),
        }
    }

    #[test]
    fn gemma4_cloud_matches_closest_gemma_4() {
        let models = vec![
            or_model("google/gemma-4-31b-it", "0.0000001", "0.00000034"),
            or_model("google/gemma-4-120b-it", "0.0000002", "0.00000068"),
        ];
        let req = request("gemma4:cloud", "ollama");
        let meta = metadata("gemma4", "32682372656");
        let result = resolve_pricing(&req, &models, Some(&meta));
        assert_eq!(result.source, "openrouter");
        assert_eq!(result.matched_openrouter_id.as_deref(), Some("google/gemma-4-31b-it"));
        assert!((result.prompt_price_per_million.unwrap() - 0.1).abs() < 1e-9);
        assert!((result.completion_price_per_million.unwrap() - 0.34).abs() < 1e-9);
    }

    #[test]
    fn gpt4o_mini_exact_matches() {
        let models = vec![or_model("openai/gpt-4o-mini", "0.00000015", "0.0000006")];
        let req = request("gpt-4o-mini", "openai");
        let result = resolve_pricing(&req, &models, None);
        assert_eq!(result.source, "openrouter");
        assert_eq!(result.matched_openrouter_id.as_deref(), Some("openai/gpt-4o-mini"));
    }

    #[test]
    fn claude_sonnet_exact_matches() {
        let models = vec![or_model(
            "anthropic/claude-sonnet-4-5-20250929",
            "0.000003",
            "0.000015",
        )];
        let req = request("claude-sonnet-4-5-20250929", "claude");
        let result = resolve_pricing(&req, &models, None);
        assert_eq!(result.source, "openrouter");
        assert_eq!(
            result.matched_openrouter_id.as_deref(),
            Some("anthropic/claude-sonnet-4-5-20250929")
        );
    }

    #[test]
    fn unknown_model_returns_unknown() {
        let models = vec![or_model("openai/gpt-4o-mini", "0.00000015", "0.0000006")];
        let req = request("some-unknown-model", "openai");
        let result = resolve_pricing(&req, &models, None);
        assert_eq!(result.source, "unknown");
        assert_eq!(result.prompt_price_per_million, None);
        assert_eq!(result.matched_openrouter_id, None);
    }

    #[test]
    fn local_model_returns_local() {
        let req = request("builtin-ai", "builtin-ai");
        let result = resolve_pricing(&req, &[], None);
        assert_eq!(result.source, "local");
        assert_eq!(result.prompt_price_per_million, None);
    }

    #[test]
    fn price_string_parses_to_per_million() {
        assert!((parse_price_per_million(&Some("0.0000001".to_string())).unwrap() - 0.1).abs() < 1e-9);
        assert_eq!(parse_price_per_million(&Some("not-a-number".to_string())), None);
        assert_eq!(parse_price_per_million(&None), None);
    }
}
