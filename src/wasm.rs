use async_trait::async_trait;
use worker::*;
use crate::client::HttpClient;
use crate::error::{DeepSeekError, Result};
use crate::types::*;

/// WASM-compatible transport using Cloudflare Workers native fetch.
pub struct WasmClient;

impl WasmClient {
    pub fn new() -> Self { Self }
}

impl Default for WasmClient {
    fn default() -> Self { Self::new() }
}

#[async_trait(?Send)]
impl HttpClient for WasmClient {
    async fn post_json(&self, url: &str, bearer_token: &str, body: &ChatRequest) -> Result<ChatResponse> {
        let body_str = serde_json::to_string(body)?;

        let headers = Headers::new();
        headers.set("Content-Type", "application/json")
            .map_err(|e| DeepSeekError::Other(format!("header error: {e}")))?;
        headers.set("Authorization", &format!("Bearer {bearer_token}"))
            .map_err(|e| DeepSeekError::Other(format!("header error: {e}")))?;

        let mut init = RequestInit::new();
        init.with_method(Method::Post)
            .with_headers(headers)
            .with_body(Some(worker::wasm_bindgen::JsValue::from_str(&body_str)));

        let request = Request::new_with_init(url, &init)
            .map_err(|e| DeepSeekError::Other(format!("request error: {e}")))?;
        let mut response = Fetch::Request(request).send().await
            .map_err(|e| DeepSeekError::Other(format!("fetch error: {e}")))?;

        if response.status_code() != 200 {
            let error_text = response.text().await.unwrap_or_default();
            return Err(DeepSeekError::Api {
                status: response.status_code(),
                body: error_text,
            });
        }

        let response_text = response.text().await
            .map_err(|e| DeepSeekError::Other(format!("read body error: {e}")))?;
        let chat_response: ChatResponse = serde_json::from_str(&response_text)?;
        Ok(chat_response)
    }
}
