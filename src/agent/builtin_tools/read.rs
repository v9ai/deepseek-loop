use async_trait::async_trait;
use serde_json::{json, Value};

use crate::agent::tool::{Tool, ToolDefinition};

pub struct ReadTool;

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &str {
        "Read"
    }

    fn read_only_hint(&self) -> bool {
        true
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: "Read a UTF-8 text file from disk. Optionally slice by 1-based line offset and limit.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path":   { "type": "string", "description": "Absolute or cwd-relative path." },
                    "offset": { "type": "integer", "description": "1-based line to start at.", "minimum": 1 },
                    "limit":  { "type": "integer", "description": "Maximum number of lines to return.", "minimum": 1 }
                },
                "required": ["path"]
            }),
        }
    }

    async fn call_json(&self, args: Value) -> Result<String, String> {
        let path = args
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| "Read: missing string `path`".to_string())?;
        let offset = args
            .get("offset")
            .and_then(Value::as_u64)
            .map(|n| n as usize);
        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .map(|n| n as usize);

        let body = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| format!("Read({path}): {e}"))?;

        if offset.is_none() && limit.is_none() {
            return Ok(body);
        }

        let mut out = String::new();
        let start = offset.unwrap_or(1).saturating_sub(1);
        let count = limit.unwrap_or(usize::MAX);
        for line in body.lines().skip(start).take(count) {
            out.push_str(line);
            out.push('\n');
        }
        Ok(out)
    }
}
