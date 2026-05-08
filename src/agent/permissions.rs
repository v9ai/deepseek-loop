//! Permission gating for tool calls.
//!
//! Mirrors Claude Code's permission modes. Every tool call passes through
//! [`PermissionMode::evaluate`]; if a [`PreToolHook`] is supplied it can
//! override the mode-default behaviour.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What to do with a single tool call before it executes.
#[derive(Debug, Clone)]
pub enum PermissionDecision {
    /// Run the tool.
    Allow,
    /// Synthesize a tool_result with `is_error=true` carrying `reason`.
    Deny(String),
    /// Defer to the mode default. Returned only by `PreToolHook`.
    Ask,
}

/// Permission modes — same set as the Claude Agent SDK doc.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionMode {
    /// Calls the hook; deny if no hook is set.
    #[default]
    Default,
    /// Auto-approves Read/Write/Edit/Glob/Grep and falls back to the hook
    /// for everything else.
    AcceptEdits,
    /// Read-only — denies any tool whose `read_only_hint()` is false.
    Plan,
    /// Never asks; everything not pre-allowed is denied.
    DontAsk,
    /// Allow everything. Use only in sandboxes.
    BypassPermissions,
}

/// Hook invoked before every tool call. Implementations may inspect the tool
/// name and arguments and return a [`PermissionDecision`].
#[async_trait]
pub trait PreToolHook: Send + Sync {
    async fn check(&self, tool_name: &str, args: &Value) -> PermissionDecision;
}

/// Names of the built-in edit-class tools that `AcceptEdits` auto-approves.
pub(crate) const EDIT_TOOLS: &[&str] = &["Read", "Write", "Edit", "Glob", "Grep"];

impl PermissionMode {
    /// Apply the mode to a single tool call. `read_only_hint` reflects the
    /// tool's annotation. Returns `Allow`, `Deny(reason)`, or `Ask` (only the
    /// `Default` mode and `AcceptEdits` fallback ever return `Ask`).
    pub fn evaluate(self, tool_name: &str, read_only_hint: bool) -> PermissionDecision {
        match self {
            Self::BypassPermissions => PermissionDecision::Allow,
            Self::Plan => {
                if read_only_hint {
                    PermissionDecision::Allow
                } else {
                    PermissionDecision::Deny(format!(
                        "Plan mode: tool `{tool_name}` is not read-only"
                    ))
                }
            }
            Self::AcceptEdits => {
                if EDIT_TOOLS.contains(&tool_name) || read_only_hint {
                    PermissionDecision::Allow
                } else {
                    PermissionDecision::Ask
                }
            }
            Self::DontAsk => PermissionDecision::Deny(format!(
                "Permission mode dontAsk: `{tool_name}` is not pre-approved"
            )),
            Self::Default => PermissionDecision::Ask,
        }
    }
}
