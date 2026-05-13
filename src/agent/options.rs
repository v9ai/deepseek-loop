//! Options passed into [`crate::agent::run`].

use std::sync::Arc;

use crate::types::EffortLevel;

use super::permissions::{PermissionMode, PreToolHook};

/// Configuration for natural-language history compaction.
///
/// When set on [`RunOptions::compaction`], the agent loop monitors the
/// previous turn's `prompt_tokens` and — once it crosses
/// `threshold_prompt_tokens` — replaces the middle of the conversation with
/// a single synthetic system message containing a compacted summary
/// produced by a separate (typically cheaper) DeepSeek call.
///
/// # Cost trade-off
///
/// Compaction costs one extra API call (input = serialized middle slice,
/// output ≤ `max_summary_tokens`) and **invalidates the prompt cache prefix
/// for subsequent turns** because the front of the message vector changes.
/// DeepSeek's cache-hit input price is ~25% of the cache-miss price, so:
///
/// - For short loops (a few turns): leave `RunOptions::compaction = None` —
///   cache hits already make resending cheap.
/// - For long loops where `prompt_tokens` crosses ~30K: one compaction call
///   on `deepseek-chat` costs ~$0.008, but saves resending ~30K tokens ×
///   each remaining turn at the cache-hit rate. Break-even is roughly four
///   future turns.
///
/// Defaults are tuned for the long-loop case; tune
/// `threshold_prompt_tokens` deliberately if your workload is short.
#[derive(Clone, Debug)]
pub struct CompactionConfig {
    /// Trigger compaction when the previous turn's `prompt_tokens >= this`.
    pub threshold_prompt_tokens: u32,
    /// Number of complete recent turns to keep verbatim after the summary.
    /// A "turn" is one assistant message plus its associated tool-result
    /// group (if the assistant requested tool calls). Tool-call/result
    /// pairs are never split — compaction may keep one extra turn to
    /// preserve atomicity.
    pub keep_recent_turns: u32,
    /// Model used for the compaction call. Default: `deepseek-chat`
    /// (cheapest input rate).
    pub compactor_model: String,
    /// Soft cap on summary completion tokens.
    pub max_summary_tokens: u32,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            threshold_prompt_tokens: 32_000,
            keep_recent_turns: 3,
            compactor_model: "deepseek-chat".into(),
            max_summary_tokens: 512,
        }
    }
}

/// Configuration for an agent run.
///
/// Mirrors `ClaudeAgentOptions` / `Options` from the Claude Agent SDK, scoped
/// to the fields meaningful for a non-streaming OpenAI-compatible loop.
#[derive(Clone)]
pub struct RunOptions {
    pub model: String,
    pub system_prompt: String,
    /// If `Some`, only listed tools may be invoked. Tools not on the list are
    /// hidden from the model entirely.
    pub allowed_tools: Option<Vec<String>>,
    /// Tools listed here are hidden from the model and any call is denied.
    pub disallowed_tools: Vec<String>,
    pub max_turns: Option<u32>,
    pub max_budget_usd: Option<f64>,
    pub effort: EffortLevel,
    pub permission_mode: PermissionMode,
    pub pre_tool_hook: Option<Arc<dyn PreToolHook>>,
    pub session_id: Option<String>,
    pub base_url: String,
    /// Opt-in history compaction. `None` (default) disables compaction —
    /// the full conversation is resent every turn. See [`CompactionConfig`].
    pub compaction: Option<CompactionConfig>,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            model: "deepseek-v4-pro".into(),
            system_prompt: String::new(),
            allowed_tools: None,
            disallowed_tools: Vec::new(),
            max_turns: None,
            max_budget_usd: None,
            effort: EffortLevel::default(),
            permission_mode: PermissionMode::default(),
            pre_tool_hook: None,
            session_id: None,
            base_url: "https://api.deepseek.com/v1".into(),
            compaction: None,
        }
    }
}

impl RunOptions {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            ..Self::default()
        }
    }

    pub fn system_prompt(mut self, p: impl Into<String>) -> Self {
        self.system_prompt = p.into();
        self
    }

    pub fn max_turns(mut self, n: u32) -> Self {
        self.max_turns = Some(n);
        self
    }

    pub fn max_budget_usd(mut self, b: f64) -> Self {
        self.max_budget_usd = Some(b);
        self
    }

    pub fn effort(mut self, e: EffortLevel) -> Self {
        self.effort = e;
        self
    }

    pub fn permission_mode(mut self, m: PermissionMode) -> Self {
        self.permission_mode = m;
        self
    }

    pub fn allowed_tools<I, S>(mut self, tools: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.allowed_tools = Some(tools.into_iter().map(Into::into).collect());
        self
    }

    pub fn disallowed_tools<I, S>(mut self, tools: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.disallowed_tools = tools.into_iter().map(Into::into).collect();
        self
    }

    pub fn pre_tool_hook(mut self, hook: Arc<dyn PreToolHook>) -> Self {
        self.pre_tool_hook = Some(hook);
        self
    }

    pub fn session_id(mut self, id: impl Into<String>) -> Self {
        self.session_id = Some(id.into());
        self
    }

    pub fn base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into().trim_end_matches('/').to_string();
        self
    }

    /// Enable natural-language history compaction with the given config.
    pub fn compaction(mut self, cfg: CompactionConfig) -> Self {
        self.compaction = Some(cfg);
        self
    }
}
