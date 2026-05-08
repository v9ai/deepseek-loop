use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::agent::tool::{Tool, ToolDefinition};

const DEFAULT_TIMEOUT_SECS: u64 = 120;


pub struct BashTool;

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "Bash"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: "Run a shell command via `/bin/sh -c`. Captures stdout+stderr. \
                          Default timeout 120s."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command":      { "type": "string" },
                    "timeout_secs": { "type": "integer", "minimum": 1, "maximum": 600 }
                },
                "required": ["command"]
            }),
        }
    }

    async fn call_json(&self, args: Value) -> Result<String, String> {
        let command = args
            .get("command")
            .and_then(Value::as_str)
            .ok_or_else(|| "Bash: missing string `command`".to_string())?;
        let timeout = args
            .get("timeout_secs")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_TIMEOUT_SECS);

        let mut cmd = tokio::process::Command::new("/bin/sh");
        cmd.arg("-c").arg(command);
        cmd.stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let child_fut = cmd.output();
        let out = match tokio::time::timeout(Duration::from_secs(timeout), child_fut).await {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => return Err(format!("Bash: spawn error: {e}")),
            Err(_) => return Err(format!("Bash: timed out after {timeout}s")),
        };
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        let status = out.status.code().unwrap_or(-1);
        Ok(format!(
            "exit={status}\n--- stdout ---\n{stdout}--- stderr ---\n{stderr}"
        ))
    }
}
