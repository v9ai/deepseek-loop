//! SDK message types — mirrors the Claude Agent SDK message stream.
//!
//! See <https://code.claude.com/docs/en/agent-sdk/agent-loop>. The loop yields
//! a sequence of these in order: `System{Init}` → (`Assistant` →
//! `User(tool_results)`)* → final `Assistant` → `Result`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::types::UsageInfo;

/// Per-block content inside an `Assistant` or `User` message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Text emitted by the assistant.
    Text { text: String },

    /// Assistant requested a tool call.
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },

    /// Result of a tool call, fed back to the model on the next turn.
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        is_error: bool,
    },
}

/// Subtype of a `System` message.
///
/// - `Init` is emitted once at session start, carrying the session id and
///   the run configuration.
/// - `Compact` is emitted after a successful history-compaction step (see
///   [`crate::agent::CompactionConfig`]). Its `data` field carries
///   `{"message_count_after": <usize>}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemSubtype {
    Init,
    Compact,
}

/// Final disposition of an agent run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultSubtype {
    Success,
    ErrorMaxTurns,
    ErrorMaxBudgetUsd,
    ErrorDuringExecution,
}

impl ResultSubtype {
    pub fn is_success(self) -> bool {
        matches!(self, Self::Success)
    }
}

/// Streamed message types in turn order.
///
/// Matches `SystemMessage` / `AssistantMessage` / `UserMessage` /
/// `ResultMessage` from the Claude Agent SDK. We do not emit `StreamEvent`
/// (token deltas) — the loop runs in non-streaming mode.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SdkMessage {
    /// Lifecycle event. `Init` carries session metadata.
    System {
        subtype: SystemSubtype,
        session_id: String,
        #[serde(default, skip_serializing_if = "Value::is_null")]
        data: Value,
    },

    /// One assistant turn — text and/or tool_use blocks. The final assistant
    /// message in a successful run has no tool_use blocks.
    Assistant {
        content: Vec<ContentBlock>,
        #[serde(skip_serializing_if = "Option::is_none")]
        stop_reason: Option<String>,
    },

    /// One synthetic user turn carrying tool_result blocks back to the model.
    User { content: Vec<ContentBlock> },

    /// Terminal message. `result` is `Some` only when `subtype = Success`.
    Result {
        subtype: ResultSubtype,
        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        total_cost_usd: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<UsageInfo>,
        num_turns: u32,
        session_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        stop_reason: Option<String>,
    },
}

impl SdkMessage {
    /// True for the terminal `Result` message.
    pub fn is_terminal(&self) -> bool {
        matches!(self, SdkMessage::Result { .. })
    }
}
