//! `MemorySearch` tool — semantic recall of conversation turns the
//! history-compactor archived before discarding them.
//!
//! Unconditional (no LanceDB/`fastembed` dependency): it talks only to the
//! [`ConversationMemory`] trait, so it is available whenever `builtin-tools`
//! is enabled and is unit-testable with the in-memory fake.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::agent::memory::ConversationMemory;
use crate::agent::tool::{Tool, ToolDefinition};

/// Recall archived, compacted-out turns for the run's session.
pub struct MemorySearchTool {
    mem: Arc<dyn ConversationMemory>,
    session_id: String,
}

impl MemorySearchTool {
    /// `session_id` MUST match the one the loop archives under (i.e. the
    /// value passed to [`crate::agent::RunOptions::session_id`]) — see the
    /// invariant on [`crate::agent::RunOptions::memory`].
    pub fn new(mem: Arc<dyn ConversationMemory>, session_id: impl Into<String>) -> Self {
        Self {
            mem,
            session_id: session_id.into(),
        }
    }
}

#[async_trait]
impl Tool for MemorySearchTool {
    fn name(&self) -> &str {
        "MemorySearch"
    }

    fn read_only_hint(&self) -> bool {
        true
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: "Semantically search earlier turns of THIS conversation that were \
                          compacted out of the live context (tool outputs, files read, \
                          decisions). Use it to recover specifics — a path, a command's \
                          output, a prior finding — instead of redoing the work. Returns \
                          the best-matching archived turns with a relevance score."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "What to recall, in natural language."
                    },
                    "k": {
                        "type": "integer",
                        "description": "Max results to return (1..=50, default 5)."
                    }
                },
                "required": ["query"]
            }),
        }
    }

    async fn call_json(&self, args: Value) -> Result<String, String> {
        let query = args
            .get("query")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| "MemorySearch: missing non-empty string `query`".to_string())?;

        let k = args
            .get("k")
            .and_then(Value::as_u64)
            .map(|v| v.clamp(1, 50) as usize)
            .unwrap_or(5);

        let hits = self
            .mem
            .search(&self.session_id, query, k)
            .await
            .map_err(|e| format!("MemorySearch: {e}"))?;

        Ok(serde_json::to_string(&json!({
            "count": hits.len(),
            "hits": hits.iter().map(|h| json!({
                "role": h.role,
                "tool_name": h.tool_name,
                "text": h.text,
                "created_at": h.created_at,
                "score": h.score,
            })).collect::<Vec<_>>(),
        }))
        .unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::memory::{ConversationMemory, InMemoryMemory, MemoryRecord};

    #[tokio::test]
    async fn searches_under_bound_session_and_serializes_hits() {
        let mem = Arc::new(InMemoryMemory::new());
        mem.archive(
            "sess-1",
            vec![
                MemoryRecord {
                    role: "tool".into(),
                    tool_name: "Grep".into(),
                    text: "found the bug in parser.rs at line 88".into(),
                    created_at: "t".into(),
                },
                MemoryRecord {
                    role: "assistant".into(),
                    tool_name: String::new(),
                    text: "weather is irrelevant".into(),
                    created_at: "t".into(),
                },
            ],
        )
        .await
        .unwrap();

        let tool = MemorySearchTool::new(mem, "sess-1");
        let raw = tool
            .call_json(json!({"query": "found the bug in parser.rs at line 88", "k": 2}))
            .await
            .unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["count"].as_u64().unwrap(), 2);
        assert_eq!(
            v["hits"][0]["text"].as_str().unwrap(),
            "found the bug in parser.rs at line 88"
        );
        assert!(tool.read_only_hint());
    }

    #[tokio::test]
    async fn missing_query_errors() {
        let mem = Arc::new(InMemoryMemory::new());
        let tool = MemorySearchTool::new(mem, "s");
        let err = tool.call_json(json!({"k": 3})).await.unwrap_err();
        assert!(err.contains("missing non-empty string `query`"), "got: {err}");
    }

    #[tokio::test]
    async fn wrong_session_returns_no_hits() {
        let mem = Arc::new(InMemoryMemory::new());
        mem.archive(
            "real",
            vec![MemoryRecord {
                role: "tool".into(),
                tool_name: String::new(),
                text: "secret".into(),
                created_at: "t".into(),
            }],
        )
        .await
        .unwrap();
        let tool = MemorySearchTool::new(mem, "other");
        let raw = tool.call_json(json!({"query": "secret"})).await.unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["count"].as_u64().unwrap(), 0);
    }
}
