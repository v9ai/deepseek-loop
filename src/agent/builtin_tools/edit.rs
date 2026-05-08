use async_trait::async_trait;
use serde_json::{json, Value};

use crate::agent::tool::{Tool, ToolDefinition};


pub struct EditTool;

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &str {
        "Edit"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: "Replace one occurrence (or all, with `replace_all`) of an exact \
                          string in a file. Errors if `old_string` is missing or appears \
                          multiple times unless `replace_all` is set."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path":        { "type": "string" },
                    "old_string":  { "type": "string" },
                    "new_string":  { "type": "string" },
                    "replace_all": { "type": "boolean", "default": false }
                },
                "required": ["path", "old_string", "new_string"]
            }),
        }
    }

    async fn call_json(&self, args: Value) -> Result<String, String> {
        let path = args
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| "Edit: missing string `path`".to_string())?;
        let old = args
            .get("old_string")
            .and_then(Value::as_str)
            .ok_or_else(|| "Edit: missing string `old_string`".to_string())?;
        let new = args
            .get("new_string")
            .and_then(Value::as_str)
            .ok_or_else(|| "Edit: missing string `new_string`".to_string())?;
        let replace_all = args
            .get("replace_all")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let body = tokio::fs::read_to_string(path)
            .await
            .map_err(|e| format!("Edit({path}): {e}"))?;

        let count = body.matches(old).count();
        if count == 0 {
            return Err(format!("Edit({path}): `old_string` not found"));
        }
        if count > 1 && !replace_all {
            return Err(format!(
                "Edit({path}): `old_string` matches {count} times; \
                 set `replace_all=true` or supply more context"
            ));
        }
        let updated = if replace_all {
            body.replace(old, new)
        } else {
            body.replacen(old, new, 1)
        };
        tokio::fs::write(path, updated)
            .await
            .map_err(|e| format!("Edit({path}): write: {e}"))?;
        Ok(format!(
            "Edited {path}: replaced {} occurrence(s)",
            if replace_all { count } else { 1 }
        ))
    }
}
