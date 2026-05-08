use serde::{Deserialize, Serialize};

// ── DeepSeek Models ───────────────────────────────────────────────────────

/// DeepSeek model identifiers — parity with Anthropic's multi-model routing
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub enum DeepSeekModel {
    /// DeepSeek V4-Pro — flagship with thinking mode + reasoning_effort.
    #[serde(rename = "deepseek-v4-pro")]
    #[default]
    V4Pro,
    /// DeepSeek-R1 — kept for back-compat. New code should prefer V4Pro.
    #[serde(rename = "deepseek-reasoner")]
    Reasoner,
    /// DeepSeek-V3 — kept for back-compat.
    #[serde(rename = "deepseek-chat")]
    Chat,
}

impl DeepSeekModel {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::V4Pro => "deepseek-v4-pro",
            Self::Reasoner => "deepseek-reasoner",
            Self::Chat => "deepseek-chat",
        }
    }

    pub fn from_alias(alias: &str) -> Self {
        match alias {
            "v4" | "v4-pro" | "pro" | "latest" => Self::V4Pro,
            "reasoner" | "r1" | "deep" | "opus" => Self::Reasoner,
            "chat" | "v3" | "fast" | "sonnet" | "haiku" => Self::Chat,
            _ => Self::V4Pro,
        }
    }
}

// ── DeepSeek API Types (OpenAI-compatible) ────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: ChatContent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone)]
pub enum ChatContent {
    Text(String),
    Null,
}

impl ChatContent {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Text(s) => s,
            Self::Null => "",
        }
    }
}

impl Serialize for ChatContent {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Self::Text(s) => serializer.serialize_str(s),
            Self::Null => serializer.serialize_none(),
        }
    }
}

impl<'de> Deserialize<'de> for ChatContent {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        match value {
            serde_json::Value::String(s) => Ok(Self::Text(s)),
            serde_json::Value::Null => Ok(Self::Null),
            other => Ok(Self::Text(other.to_string())),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub r#type: String,
    pub function: FunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolSchema>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSchema {
    pub r#type: String,
    pub function: FunctionSchema,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionSchema {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponse {
    pub id: String,
    pub choices: Vec<Choice>,
    pub usage: Option<UsageInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Choice {
    pub index: u32,
    pub message: ChatMessage,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UsageInfo {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

// ── Agent Result (parity with Anthropic AgentResult) ──────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResult {
    pub success: bool,
    pub result: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub turns: u32,
    pub usage: UsageInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls_made: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

// ── Agent Definition (parity with Anthropic AgentDefinition) ──────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDefinition {
    pub name: String,
    pub description: String,
    pub prompt: String,
    #[serde(default)]
    pub model: DeepSeekModel,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub disallowed_tools: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<u32>,
}

// ── Effort Level (parity with Anthropic adaptive thinking) ────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub enum EffortLevel {
    Low,
    Medium,
    #[default]
    High,
    Max,
}

impl EffortLevel {
    /// Map effort to DeepSeek temperature (lower = more focused)
    pub fn temperature(&self) -> f64 {
        match self {
            Self::Low => 0.1,
            Self::Medium => 0.5,
            Self::High => 0.7,
            Self::Max => 1.0,
        }
    }

    /// Map effort to max tokens
    pub fn max_tokens(&self) -> u32 {
        match self {
            Self::Low => 2048,
            Self::Medium => 4096,
            Self::High => 8192,
            Self::Max => 16384,
        }
    }
}

// ── Message constructors ──────────────────────────────────────────────────

pub fn system_msg(content: &str) -> ChatMessage {
    ChatMessage {
        role: "system".into(),
        content: ChatContent::Text(content.into()),
        reasoning_content: None,
        tool_calls: None,
        tool_call_id: None,
        name: None,
    }
}

pub fn user_msg(content: &str) -> ChatMessage {
    ChatMessage {
        role: "user".into(),
        content: ChatContent::Text(content.into()),
        reasoning_content: None,
        tool_calls: None,
        tool_call_id: None,
        name: None,
    }
}

pub fn assistant_msg(content: &str) -> ChatMessage {
    ChatMessage {
        role: "assistant".into(),
        content: ChatContent::Text(content.into()),
        reasoning_content: None,
        tool_calls: None,
        tool_call_id: None,
        name: None,
    }
}

// ── Reasoner output ───────────────────────────────────────────────────────

pub struct ReasonerOutput {
    /// R1 chain-of-thought — logged at DEBUG, not passed to next agent.
    pub reasoning: String,
    /// Final answer — passed forward in the pipeline.
    pub content: String,
}

pub fn tool_result_msg(tool_call_id: &str, content: &str) -> ChatMessage {
    ChatMessage {
        role: "tool".into(),
        content: ChatContent::Text(content.into()),
        reasoning_content: None,
        tool_calls: None,
        tool_call_id: Some(tool_call_id.into()),
        name: None,
    }
}
