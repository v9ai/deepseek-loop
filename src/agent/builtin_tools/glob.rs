use async_trait::async_trait;
use serde_json::{json, Value};

use crate::agent::tool::{Tool, ToolDefinition};

const MAX_RESULTS: usize = 1000;


pub struct GlobTool;

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &str {
        "Glob"
    }

    fn read_only_hint(&self) -> bool {
        true
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: "Find files by glob pattern under a directory. Returns paths \
                          relative to that directory, one per line, sorted, capped at \
                          1000 results."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "e.g. `**/*.rs`" },
                    "path":    { "type": "string", "description": "Root directory (default: cwd)." }
                },
                "required": ["pattern"]
            }),
        }
    }

    async fn call_json(&self, args: Value) -> Result<String, String> {
        let pattern = args
            .get("pattern")
            .and_then(Value::as_str)
            .ok_or_else(|| "Glob: missing string `pattern`".to_string())?
            .to_string();
        let root = args
            .get("path")
            .and_then(Value::as_str)
            .map(String::from)
            .unwrap_or_else(|| ".".into());

        let result = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let glob = globset::Glob::new(&pattern)
                .map_err(|e| format!("Glob: bad pattern `{pattern}`: {e}"))?
                .compile_matcher();
            let root_path = std::path::PathBuf::from(&root);
            let mut hits: Vec<String> = Vec::new();
            for entry in walkdir::WalkDir::new(&root_path)
                .into_iter()
                .filter_map(Result::ok)
            {
                if !entry.file_type().is_file() {
                    continue;
                }
                let rel = entry
                    .path()
                    .strip_prefix(&root_path)
                    .unwrap_or(entry.path());
                if glob.is_match(rel) {
                    hits.push(rel.display().to_string());
                    if hits.len() >= MAX_RESULTS {
                        break;
                    }
                }
            }
            hits.sort();
            Ok(hits.join("\n"))
        })
        .await
        .map_err(|e| format!("Glob: task join error: {e}"))??;

        Ok(result)
    }
}
