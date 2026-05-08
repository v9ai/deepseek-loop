use async_trait::async_trait;
use crate::error::Result;
use crate::types::*;

pub const DEFAULT_BASE_URL: &str = "https://api.deepseek.com/v1";

/// Transport abstraction — reqwest for native, worker::Fetch for WASM.
///
/// Native builds require `Send`; WASM builds relax it via `?Send`.
#[cfg_attr(not(feature = "wasm"), async_trait)]
#[cfg_attr(feature = "wasm", async_trait(?Send))]
pub trait HttpClient {
    async fn post_json(&self, url: &str, bearer_token: &str, body: &ChatRequest) -> Result<ChatResponse>;
}

/// Generic DeepSeek client over any transport.
pub struct DeepSeekClient<H: HttpClient> {
    pub http: H,
    pub api_key: String,
    pub base_url: String,
}

impl<H: HttpClient + Clone> Clone for DeepSeekClient<H> {
    fn clone(&self) -> Self {
        Self {
            http: self.http.clone(),
            api_key: self.api_key.clone(),
            base_url: self.base_url.clone(),
        }
    }
}

impl<H: HttpClient> DeepSeekClient<H> {
    pub fn new(http: H, api_key: impl Into<String>) -> Self {
        Self {
            http,
            api_key: api_key.into(),
            base_url: DEFAULT_BASE_URL.to_string(),
        }
    }

    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    /// Send a chat completion request.
    pub async fn chat(&self, request: &ChatRequest) -> Result<ChatResponse> {
        let url = format!("{}/chat/completions", self.base_url);
        self.http.post_json(&url, &self.api_key, request).await
    }

}

/// Build a ChatRequest with optional tool schemas.
/// Free function — usable without a client instance.
pub fn build_request(
    model: &DeepSeekModel,
    messages: Vec<ChatMessage>,
    tools: Option<Vec<ToolSchema>>,
    effort: &EffortLevel,
) -> ChatRequest {
    let has_tools = tools.is_some();
    let reasoning_effort = match effort {
        EffortLevel::Max => Some("max".to_string()),
        _ => Some("high".to_string()),
    };
    ChatRequest {
        model: model.as_str().to_string(),
        messages,
        tools,
        tool_choice: if has_tools { Some(serde_json::json!("auto")) } else { None },
        temperature: Some(effort.temperature()),
        max_tokens: Some(effort.max_tokens()),
        stream: Some(false),
        reasoning_effort,
        thinking: Some(serde_json::json!({"type": "enabled"})),
    }
}
