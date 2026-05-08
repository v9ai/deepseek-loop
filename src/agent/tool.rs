//! `Tool` trait and `ToolDefinition` schema — verbatim from the original
//! `agent.rs`, extended with [`Tool::read_only_hint`] used by the loop's
//! parallel-dispatch and permission logic.

use async_trait::async_trait;
use serde_json::Value;

/// JSON-schema description of a tool, sent to the model in the request.
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// Tool callable from the agent loop.
///
/// `read_only_hint()` defaults to `false`. Tools that don't mutate the host
/// system (Read, Glob, Grep, Web*) should override it to `true` so the loop
/// runs them in parallel and so `PermissionMode::Plan` allows them.
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn definition(&self) -> ToolDefinition;
    async fn call_json(&self, args: Value) -> std::result::Result<String, String>;

    /// Hint to the loop that this tool is safe to execute concurrently with
    /// other read-only tools and is allowed under `PermissionMode::Plan`.
    fn read_only_hint(&self) -> bool {
        false
    }
}
