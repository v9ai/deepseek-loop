//! Back-compatible `AgentBuilder` / `DeepSeekAgent` over the new streaming
//! loop. Existing callers (e.g. `crates/research`, `crates/genesis`) keep
//! working unchanged.

use std::sync::Arc;

use futures::StreamExt;

use crate::client::HttpClient;
use crate::error::{DeepSeekError, Result};

use super::loop_runner::run;
use super::messages::{ResultSubtype, SdkMessage};
use super::options::RunOptions;
use super::tool::Tool;

/// Fluent constructor for [`DeepSeekAgent`]. Mirrors the pre-port API.
pub struct AgentBuilder<H: HttpClient> {
    http: H,
    api_key: String,
    model: String,
    preamble: String,
    tools: Vec<Box<dyn Tool>>,
    base_url: String,
    worker_id: String,
    options: RunOptions,
}

impl<H: HttpClient> AgentBuilder<H> {
    pub fn new(http: H, api_key: impl Into<String>, model: impl Into<String>) -> Self {
        let model = model.into();
        let mut options = RunOptions::new(model.clone());
        options.base_url = "https://api.deepseek.com/v1".into();
        Self {
            http,
            api_key: api_key.into(),
            model,
            preamble: String::new(),
            tools: Vec::new(),
            base_url: "https://api.deepseek.com".into(),
            worker_id: String::new(),
            options,
        }
    }

    pub fn preamble(mut self, p: &str) -> Self {
        self.preamble = p.into();
        self
    }

    pub fn tool(mut self, t: impl Tool + 'static) -> Self {
        self.tools.push(Box::new(t));
        self
    }

    /// Back-compat: callers historically passed the `https://api.deepseek.com`
    /// host (no `/v1`). The loop appends `/v1/chat/completions` itself, so we
    /// store both shapes.
    pub fn base_url(mut self, url: &str) -> Self {
        let trimmed = url.trim_end_matches('/').to_string();
        self.base_url = trimmed.clone();
        // Preserve legacy behaviour: append `/v1` for the new loop's base URL.
        self.options.base_url = if trimmed.ends_with("/v1") {
            trimmed
        } else {
            format!("{trimmed}/v1")
        };
        self
    }

    pub fn worker_id(mut self, id: impl Into<String>) -> Self {
        self.worker_id = id.into();
        self
    }

    /// Override the full [`RunOptions`] for advanced control (max_turns,
    /// permission mode, hooks, etc.). Settings that overlap with the legacy
    /// builder methods (`base_url`, `model`) are kept in sync.
    pub fn options(mut self, opts: RunOptions) -> Self {
        self.options = opts;
        self
    }

    pub fn build(self) -> DeepSeekAgent<H> {
        let mut options = self.options;
        options.model = self.model.clone();
        options.system_prompt = self.preamble;
        DeepSeekAgent {
            http: self.http,
            api_key: self.api_key,
            tools: Arc::new(self.tools),
            options,
            worker_id: self.worker_id,
        }
    }
}

/// Bound DeepSeek agent. Call [`prompt`](Self::prompt) for the legacy
/// `Result<String>` API or [`run`](Self::run) for the streaming `SdkMessage`
/// API.
pub struct DeepSeekAgent<H: HttpClient> {
    http: H,
    api_key: String,
    tools: Arc<Vec<Box<dyn Tool>>>,
    options: RunOptions,
    #[allow(dead_code)]
    worker_id: String,
}

impl<H: HttpClient + Clone + Send + Sync + 'static> DeepSeekAgent<H> {
    /// Stream the loop's [`SdkMessage`] sequence.
    pub fn run(
        &self,
        user_prompt: String,
    ) -> impl futures::Stream<Item = SdkMessage> + use<H> {
        run(
            self.http.clone(),
            self.api_key.clone(),
            Arc::clone(&self.tools),
            user_prompt,
            self.options.clone_for_run(),
        )
    }

    /// Legacy entry point — runs the loop to completion and returns the final
    /// assistant text.
    pub async fn prompt(&self, user_prompt: String) -> Result<String> {
        let mut stream = Box::pin(self.run(user_prompt));
        let mut last_text: Option<String> = None;
        while let Some(msg) = stream.next().await {
            match msg {
                SdkMessage::Result {
                    subtype: ResultSubtype::Success,
                    result: Some(text),
                    ..
                } => return Ok(text),
                SdkMessage::Result { subtype, .. } => {
                    return Err(DeepSeekError::Other(format!(
                        "agent stopped with subtype {subtype:?}"
                    )));
                }
                SdkMessage::Assistant { content, .. } => {
                    if let Some(t) = content.iter().find_map(|b| match b {
                        super::messages::ContentBlock::Text { text } => Some(text.clone()),
                        _ => None,
                    }) {
                        last_text = Some(t);
                    }
                }
                _ => {}
            }
        }
        last_text.ok_or_else(|| {
            DeepSeekError::Other("agent stream ended without a Result message".into())
        })
    }
}

impl RunOptions {
    /// Cheap clone for spawning a single `run()` — `Arc<dyn PreToolHook>` is
    /// shared, all other fields are owned.
    fn clone_for_run(&self) -> RunOptions {
        RunOptions {
            model: self.model.clone(),
            system_prompt: self.system_prompt.clone(),
            allowed_tools: self.allowed_tools.clone(),
            disallowed_tools: self.disallowed_tools.clone(),
            max_turns: self.max_turns,
            max_budget_usd: self.max_budget_usd,
            effort: self.effort.clone(),
            permission_mode: self.permission_mode,
            pre_tool_hook: self.pre_tool_hook.clone(),
            session_id: self.session_id.clone(),
            base_url: self.base_url.clone(),
        }
    }
}
