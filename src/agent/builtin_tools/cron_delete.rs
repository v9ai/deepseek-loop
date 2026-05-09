//! `CronDelete` tool — cancel a scheduled task by id.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::agent::scheduler::{Scheduler, TaskId};
use crate::agent::tool::{Tool, ToolDefinition};

pub struct CronDeleteTool {
    pub scheduler: Arc<Mutex<Scheduler>>,
}

impl CronDeleteTool {
    pub fn new(scheduler: Arc<Mutex<Scheduler>>) -> Self {
        Self { scheduler }
    }
}

#[async_trait]
impl Tool for CronDeleteTool {
    fn name(&self) -> &str {
        "CronDelete"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: "Cancel a scheduled task by its 8-character `task_id` \
                          (as returned by CronCreate or CronList)."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "task_id": { "type": "string" }
                },
                "required": ["task_id"]
            }),
        }
    }

    async fn call_json(&self, args: Value) -> Result<String, String> {
        let raw = args
            .get("task_id")
            .and_then(Value::as_str)
            .ok_or_else(|| "CronDelete: missing string `task_id`".to_string())?;
        let id = TaskId::from_raw(raw);

        let mut sched = self
            .scheduler
            .lock()
            .map_err(|_| "CronDelete: scheduler lock poisoned".to_string())?;

        let deleted = sched.delete(&id);
        Ok(serde_json::to_string(&json!({
            "task_id": id.as_str(),
            "deleted": deleted,
        }))
        .unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::scheduler::{CronExpr, Schedule, Scheduler};
    use serde_json::Value;

    fn fresh_sched() -> Arc<Mutex<Scheduler>> {
        std::env::remove_var("CLAUDE_CODE_DISABLE_CRON");
        std::env::remove_var("DEEPSEEK_LOOP_DISABLE_CRON");
        Arc::new(Mutex::new(Scheduler::new("cron-delete-test")))
    }

    #[test]
    fn definition_marks_task_id_required() {
        let tool = CronDeleteTool::new(fresh_sched());
        let def = tool.definition();
        assert_eq!(def.name, "CronDelete");
        let required = def.parameters.get("required").unwrap().as_array().unwrap();
        assert!(required.iter().any(|v| v == "task_id"));
    }

    #[tokio::test]
    async fn delete_returns_false_for_unknown_id() {
        let tool = CronDeleteTool::new(fresh_sched());
        let raw = tool
            .call_json(json!({"task_id": "ZZZZZZZZ"}))
            .await
            .unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["deleted"].as_bool().unwrap(), false);
        assert_eq!(v["task_id"].as_str().unwrap(), "ZZZZZZZZ");
    }

    #[tokio::test]
    async fn delete_returns_true_then_false_on_repeat() {
        let sched = fresh_sched();
        let id = {
            let mut s = sched.lock().unwrap();
            let cron = CronExpr::parse("*/5 * * * *").unwrap();
            s.create(Schedule::Cron(Box::new(cron)), "x", true).unwrap()
        };
        let tool = CronDeleteTool::new(sched);
        let first: Value = serde_json::from_str(
            &tool
                .call_json(json!({"task_id": id.as_str()}))
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(first["deleted"].as_bool().unwrap(), true);

        let second: Value = serde_json::from_str(
            &tool
                .call_json(json!({"task_id": id.as_str()}))
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(second["deleted"].as_bool().unwrap(), false);
    }

    #[tokio::test]
    async fn missing_task_id_returns_error() {
        let tool = CronDeleteTool::new(fresh_sched());
        let err = tool.call_json(json!({})).await.unwrap_err();
        assert!(err.contains("missing"), "got: {err}");
    }

    #[tokio::test]
    async fn non_string_task_id_returns_error() {
        let tool = CronDeleteTool::new(fresh_sched());
        let err = tool.call_json(json!({"task_id": 12345})).await.unwrap_err();
        assert!(err.contains("missing"), "got: {err}");
    }
}
