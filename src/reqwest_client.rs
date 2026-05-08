use async_trait::async_trait;
use anyhow::{Context, Result};
use tracing::{debug, warn};
use crate::client::{HttpClient, DeepSeekClient, DEFAULT_BASE_URL};
use crate::error::DeepSeekError;
use crate::types::{ChatRequest, ChatResponse, ReasonerOutput};

/// Native reqwest-based transport.
#[derive(Clone)]
pub struct ReqwestClient {
    client: reqwest::Client,
}

impl ReqwestClient {
    pub fn new() -> Self {
        Self { client: reqwest::Client::new() }
    }

    pub fn with_client(client: reqwest::Client) -> Self {
        Self { client }
    }
}

impl Default for ReqwestClient {
    fn default() -> Self { Self::new() }
}

#[async_trait]
impl HttpClient for ReqwestClient {
    async fn post_json(&self, url: &str, bearer_token: &str, body: &ChatRequest) -> crate::error::Result<ChatResponse> {
        let resp = self
            .client
            .post(url)
            .bearer_auth(bearer_token)
            .json(body)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(DeepSeekError::Api { status: status.as_u16(), body: text });
        }

        let chat_resp: ChatResponse = resp.json().await?;
        Ok(chat_resp)
    }
}

const REASONER_MODEL: &str = "deepseek-reasoner";

/// Production constructor — reads `DEEPSEEK_API_KEY` (and optionally
/// `DEEPSEEK_BASE_URL`) from the environment.
pub fn client_from_env() -> Result<DeepSeekClient<ReqwestClient>> {
    let api_key = std::env::var("DEEPSEEK_API_KEY")
        .context("DEEPSEEK_API_KEY environment variable not set")?;
    let base_url = std::env::var("DEEPSEEK_BASE_URL")
        .unwrap_or_else(|_| DEFAULT_BASE_URL.to_string());
    Ok(DeepSeekClient::new(ReqwestClient::new(), api_key).with_base_url(base_url))
}

/// Thin wrapper that builds a ChatRequest for deepseek-reasoner and calls `client.chat()`.
pub async fn reason(
    client: &DeepSeekClient<ReqwestClient>,
    system: &str,
    user: &str,
) -> Result<ReasonerOutput> {
    debug!("deepseek-reasoner call: system={:.80}…", system);

    let request = ChatRequest {
        model: REASONER_MODEL.to_string(),
        messages: vec![
            crate::types::system_msg(system),
            crate::types::user_msg(user),
        ],
        tools: None,
        tool_choice: None,
        temperature: Some(0.6),
        max_tokens: Some(8192),
        stream: Some(false),
        reasoning_effort: Some("high".to_string()),
        thinking: Some(serde_json::json!({"type": "enabled"})),
    };

    let resp = client
        .chat(&request)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    let choice = resp
        .choices
        .into_iter()
        .next()
        .context("No choices in DeepSeek response")?;

    Ok(ReasonerOutput {
        reasoning: choice.message.reasoning_content.unwrap_or_default(),
        content: choice.message.content.as_str().to_string(),
    })
}

/// Returns `true` for errors worth retrying (5xx, network). 4xx errors are not retried.
fn is_retryable(err: &anyhow::Error) -> bool {
    let msg = err.to_string();
    // DeepSeekError::Api includes "API error (STATUS): …"
    if let Some(rest) = msg.strip_prefix("API error (") {
        if let Some(code_str) = rest.split(')').next() {
            if let Ok(code) = code_str.parse::<u16>() {
                return code >= 500;
            }
        }
    }
    // Network / reqwest errors are retryable
    msg.contains("HTTP error:") || msg.contains("connection") || msg.contains("timed out")
}

/// Wrapper around [`reason`] with exponential backoff: 3 attempts, 1s → 2s → 4s.
/// Only retries on 5xx / network errors, not 4xx.
pub async fn reason_with_retry(
    client: &DeepSeekClient<ReqwestClient>,
    system: &str,
    user: &str,
) -> Result<ReasonerOutput> {
    let delays = [1, 2, 4]; // seconds
    let mut last_err = None;
    for (attempt, &delay_secs) in std::iter::once(&0).chain(delays.iter()).enumerate() {
        if attempt > 0 {
            warn!("Retry attempt {attempt}/3 after {delay_secs}s backoff");
            tokio::time::sleep(std::time::Duration::from_secs(delay_secs)).await;
        }
        match reason(client, system, user).await {
            Ok(output) => return Ok(output),
            Err(e) => {
                if !is_retryable(&e) || attempt == 3 {
                    return Err(e);
                }
                warn!("Retryable error: {e}");
                last_err = Some(e);
            }
        }
    }
    Err(last_err.unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_from_env_missing_key() {
        let original = std::env::var("DEEPSEEK_API_KEY").ok();
        std::env::remove_var("DEEPSEEK_API_KEY");
        let result = client_from_env();
        let err = result.err().expect("should error when DEEPSEEK_API_KEY is unset");
        assert!(
            err.to_string().contains("DEEPSEEK_API_KEY"),
            "error should mention the missing env var"
        );
        if let Some(val) = original {
            std::env::set_var("DEEPSEEK_API_KEY", val);
        }
    }

    #[test]
    fn test_is_retryable_5xx() {
        let err = anyhow::anyhow!("API error (500): Internal Server Error");
        assert!(is_retryable(&err));
    }

    #[test]
    fn test_is_not_retryable_4xx() {
        let err = anyhow::anyhow!("API error (400): Bad Request");
        assert!(!is_retryable(&err));
    }

    #[test]
    fn test_is_retryable_network() {
        let err = anyhow::anyhow!("HTTP error: connection refused");
        assert!(is_retryable(&err));
    }
}
