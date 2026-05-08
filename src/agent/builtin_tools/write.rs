use async_trait::async_trait;
use serde_json::{json, Value};

use crate::agent::tool::{Tool, ToolDefinition};

pub struct WriteTool;

#[async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &str {
        "Write"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description:
                "Create or overwrite a file. Parent directories are created automatically.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path":    { "type": "string" },
                    "content": { "type": "string" }
                },
                "required": ["path", "content"]
            }),
        }
    }

    async fn call_json(&self, args: Value) -> Result<String, String> {
        let path = args
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| "Write: missing string `path`".to_string())?;
        let content = args
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| "Write: missing string `content`".to_string())?;

        if let Some(parent) = std::path::Path::new(path).parent() {
            if !parent.as_os_str().is_empty() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|e| format!("Write({path}): mkdir parent: {e}"))?;
            }
        }
        tokio::fs::write(path, content)
            .await
            .map_err(|e| format!("Write({path}): {e}"))?;
        Ok(format!("Wrote {} bytes to {path}", content.len()))
    }
}
