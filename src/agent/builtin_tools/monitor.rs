//! `Monitor` tool — long-running watcher.
//!
//! Spawns a shell command and reads its stdout line-by-line until the process
//! exits or `timeout_ms` elapses. Returns the collected lines as a single
//! result string. Use this for tasks where you'd otherwise poll repeatedly
//! (`tail -f log | grep ERROR`, `inotifywait -m`, status pollers).
//!
//! Note: the `Tool` trait returns one result per invocation; we don't stream
//! events back to the loop in v0.3.0. The lines are concatenated and returned
//! when the watcher exits.

use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::time::timeout;

use crate::agent::tool::{Tool, ToolDefinition};

const DEFAULT_TIMEOUT_MS: u64 = 300_000; // 5 minutes
const MAX_TIMEOUT_MS: u64 = 3_600_000; // 1 hour
const MAX_LINES_RETURNED: usize = 1000;

pub struct MonitorTool;

#[async_trait]
impl Tool for MonitorTool {
    fn name(&self) -> &str {
        "Monitor"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: "Run a shell command and stream its stdout. Returns when the \
                          process exits or `timeout_ms` elapses (default 5 min, max 1 h). \
                          Lines beyond 1000 are truncated. Use this instead of polling \
                          when you want to react to events (CI status changes, log \
                          ERROR lines, file writes via inotifywait)."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string" },
                    "description": {
                        "type": "string",
                        "description": "Short human-readable label, surfaced in events."
                    },
                    "timeout_ms": {
                        "type": "integer",
                        "minimum": 1000,
                        "maximum": MAX_TIMEOUT_MS,
                    },
                },
                "required": ["command"]
            }),
        }
    }

    async fn call_json(&self, args: Value) -> Result<String, String> {
        let command = args
            .get("command")
            .and_then(Value::as_str)
            .ok_or_else(|| "Monitor: missing string `command`".to_string())?;
        let label = args
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("monitor");
        let timeout_ms = args
            .get("timeout_ms")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_TIMEOUT_MS)
            .min(MAX_TIMEOUT_MS);

        let mut child = Command::new("/bin/sh")
            .arg("-c")
            .arg(command)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Monitor: spawn failed: {e}"))?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Monitor: stdout pipe missing".to_string())?;
        let mut lines = BufReader::new(stdout).lines();

        let mut collected: Vec<String> = Vec::new();
        let mut truncated = false;

        let read_loop = async {
            while let Some(line) = lines.next_line().await.transpose() {
                match line {
                    Ok(s) => {
                        if collected.len() >= MAX_LINES_RETURNED {
                            truncated = true;
                            // Keep draining so the child doesn't block on a
                            // full pipe, but stop appending.
                            continue;
                        }
                        collected.push(s);
                    }
                    Err(e) => {
                        return Err(format!("Monitor: read error: {e}"));
                    }
                }
            }
            Ok::<(), String>(())
        };

        let result = timeout(Duration::from_millis(timeout_ms), read_loop).await;

        let timed_out = result.is_err();
        if timed_out {
            // Best-effort: try to kill the child so it doesn't outlive us.
            let _ = child.start_kill();
        } else if let Ok(Err(e)) = result {
            return Err(e);
        }

        // Reap exit status (non-blocking after kill).
        let exit = match child.wait().await {
            Ok(s) => s.code().unwrap_or(-1),
            Err(_) => -1,
        };

        Ok(serde_json::to_string(&json!({
            "label": label,
            "exit_code": exit,
            "lines": collected,
            "truncated": truncated,
            "timed_out": timed_out,
        }))
        .unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn echoes_a_few_lines() {
        let tool = MonitorTool;
        let args = json!({
            "command": "printf 'a\\nb\\nc\\n'",
            "description": "test echo",
            "timeout_ms": 5000
        });
        let raw = tool.call_json(args).await.unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        let lines: Vec<String> = v["lines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap().to_string())
            .collect();
        assert_eq!(lines, vec!["a", "b", "c"]);
        assert_eq!(v["exit_code"].as_i64().unwrap(), 0);
        assert_eq!(v["timed_out"].as_bool().unwrap(), false);
    }

    #[tokio::test]
    async fn timeout_terminates_runaway() {
        let tool = MonitorTool;
        let args = json!({
            "command": "while true; do echo tick; sleep 1; done",
            "timeout_ms": 1500
        });
        let raw = tool.call_json(args).await.unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["timed_out"].as_bool().unwrap(), true);
    }

    #[test]
    fn definition_marks_command_required() {
        let def = MonitorTool.definition();
        assert_eq!(def.name, "Monitor");
        let required = def.parameters.get("required").unwrap().as_array().unwrap();
        assert!(required.iter().any(|v| v == "command"));
    }

    #[tokio::test]
    async fn missing_command_returns_error() {
        let err = MonitorTool.call_json(json!({})).await.unwrap_err();
        assert!(err.contains("missing"), "got: {err}");
    }

    #[tokio::test]
    async fn nonzero_exit_code_is_captured() {
        let raw = MonitorTool
            .call_json(json!({
                "command": "exit 7",
                "timeout_ms": 5000
            }))
            .await
            .unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["exit_code"].as_i64().unwrap(), 7);
        assert_eq!(v["timed_out"].as_bool().unwrap(), false);
        assert_eq!(v["lines"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn stderr_is_not_collected() {
        let raw = MonitorTool
            .call_json(json!({
                "command": "echo on-stdout; echo on-stderr 1>&2",
                "timeout_ms": 5000
            }))
            .await
            .unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        let lines: Vec<String> = v["lines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap().to_string())
            .collect();
        assert_eq!(lines, vec!["on-stdout"]);
    }

    #[tokio::test]
    async fn default_label_is_monitor_when_omitted() {
        let raw = MonitorTool
            .call_json(json!({
                "command": "true",
                "timeout_ms": 5000
            }))
            .await
            .unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["label"].as_str().unwrap(), "monitor");
    }

    #[tokio::test]
    async fn truncates_after_max_lines() {
        // Emit 1005 lines; we should keep at most 1000 and flag truncated=true.
        let raw = MonitorTool
            .call_json(json!({
                "command": "for i in $(seq 1 1005); do echo $i; done",
                "timeout_ms": 10000
            }))
            .await
            .unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["truncated"].as_bool().unwrap(), true);
        assert_eq!(v["lines"].as_array().unwrap().len(), 1000);
    }
}
