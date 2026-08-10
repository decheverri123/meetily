use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::time::{timeout, Duration};

#[derive(Debug)]
pub enum LmStudioError {
    Timeout,
    NetworkError(String),
    InvalidEndpoint(String),
    ServerError(String),
    NoModelsFound,
    ParseError(String),
}

impl std::fmt::Display for LmStudioError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            LmStudioError::Timeout => write!(f, "Request timed out after 5 seconds. Please check if LM Studio is running at the specified endpoint."),
            LmStudioError::NetworkError(msg) => write!(f, "Network error: {}. Please check your connection and endpoint URL.", msg),
            LmStudioError::InvalidEndpoint(msg) => write!(f, "Invalid endpoint: {}. Please check the URL format.", msg),
            LmStudioError::ServerError(msg) => write!(f, "LM Studio server error: {}", msg),
            LmStudioError::NoModelsFound => write!(f, "No models found on the LM Studio server."),
            LmStudioError::ParseError(msg) => write!(f, "Failed to parse server response: {}", msg),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct ModelResponse {
    data: Vec<ModelData>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ModelData {
    id: String,
}

fn validate_endpoint_url(url: &str) -> Result<(), LmStudioError> {
    if url.is_empty() {
        return Ok(());
    }

    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(LmStudioError::InvalidEndpoint(
            "URL must start with http:// or https://".to_string()
        ));
    }

    Ok(())
}

pub async fn get_lmstudio_models(endpoint: Option<String>) -> Result<Vec<String>, LmStudioError> {
    let endpoint = endpoint.unwrap_or_else(|| "http://localhost:1234/v1".to_string());

    if let Err(e) = validate_endpoint_url(&endpoint) {
        return Err(e);
    }

    let api_url = format!("{}/models", endpoint);

    let client = Client::new();

    // Retry logic with exponential backoff
    for attempt in 0..2 {
        match timeout(
            Duration::from_secs(5),
            client.get(&api_url).send(),
        ).await {
            Ok(Ok(response)) => {
                match response.status() {
                    reqwest::StatusCode::OK => {
                        match response.json::<ModelResponse>().await {
                            Ok(models_response) => {
                                let mut model_names: Vec<String> = models_response
                                    .data
                                    .iter()
                                    .map(|m| m.id.clone())
                                    .collect();
                                model_names.sort();

                                if model_names.is_empty() {
                                    return Err(LmStudioError::NoModelsFound);
                                }

                                return Ok(model_names);
                            }
                            Err(e) => {
                                return Err(LmStudioError::ParseError(e.to_string()));
                            }
                        }
                    }
                    status => {
                        return Err(LmStudioError::ServerError(format!(
                            "Server returned status: {}",
                            status
                        )));
                    }
                }
            }
            Ok(Err(e)) => {
                if attempt < 1 {
                    tokio::time::sleep(Duration::from_millis(100 * (2_u64.pow(attempt as u32)))).await;
                    continue;
                }
                return Err(LmStudioError::NetworkError(e.to_string()));
            }
            Err(_) => {
                if attempt < 1 {
                    tokio::time::sleep(Duration::from_millis(100 * (2_u64.pow(attempt as u32)))).await;
                    continue;
                }
                return Err(LmStudioError::Timeout);
            }
        }
    }

    Err(LmStudioError::Timeout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_endpoint_url() {
        assert!(validate_endpoint_url("").is_ok());
        assert!(validate_endpoint_url("http://localhost:1234").is_ok());
        assert!(validate_endpoint_url("https://example.com:1234/v1").is_ok());
        assert!(validate_endpoint_url("localhost:1234").is_err());
        assert!(validate_endpoint_url("not-a-url").is_err());
    }
}
