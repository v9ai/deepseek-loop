use async_trait::async_trait;
use serde_json::{json, Value};

use crate::agent::tool::{Tool, ToolDefinition};

const MAX_MATCHES: usize = 200;


pub struct GrepTool;

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "Grep"
    }

    fn read_only_hint(&self) -> bool {
        true
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: "Search files for a regex pattern. Output is `path:line:match`, \
                          one per line, capped at 200 matches."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Rust regex." },
                    "path":    { "type": "string", "description": "Root directory (default: cwd)." },
                    "include": { "type": "string", "description": "Optional glob filter, e.g. `*.rs`." }
                },
                "required": ["pattern"]
            }),
        }
    }

    async fn call_json(&self, args: Value) -> Result<String, String> {
        let pattern = args
            .get("pattern")
            .and_then(Value::as_str)
            .ok_or_else(|| "Grep: missing string `pattern`".to_string())?
            .to_string();
        let root = args
            .get("path")
            .and_then(Value::as_str)
            .map(String::from)
            .unwrap_or_else(|| ".".into());
        let include = args
            .get("include")
            .and_then(Value::as_str)
            .map(String::from);

        let result = tokio::task::spawn_blocking(move || -> Result<String, String> {
            let re = regex::Regex::new(&pattern)
                .map_err(|e| format!("Grep: bad regex `{pattern}`: {e}"))?;
            let include_matcher = include
                .map(|p| {
                    globset::Glob::new(&p)
                        .map(|g| g.compile_matcher())
                        .map_err(|e| format!("Grep: bad include `{p}`: {e}"))
                })
                .transpose()?;

            let mut out = String::new();
            let mut matches = 0usize;
            for entry in walkdir::WalkDir::new(&root)
                .into_iter()
                .filter_map(Result::ok)
            {
                if !entry.file_type().is_file() {
                    continue;
                }
                let path = entry.path();
                if let Some(m) = &include_matcher {
                    let name = path.file_name().unwrap_or_default();
                    if !m.is_match(name) {
                        continue;
                    }
                }
                let body = match std::fs::read_to_string(path) {
                    Ok(s) => s,
                    Err(_) => continue, // skip binary/unreadable
                };
                for (lineno, line) in body.lines().enumerate() {
                    if re.is_match(line) {
                        out.push_str(&format!(
                            "{}:{}:{}\n",
                            path.display(),
                            lineno + 1,
                            line
                        ));
                        matches += 1;
                        if matches >= MAX_MATCHES {
                            return Ok(out);
                        }
                    }
                }
            }
            Ok(out)
        })
        .await
        .map_err(|e| format!("Grep: task join error: {e}"))??;

        Ok(result)
    }
}
