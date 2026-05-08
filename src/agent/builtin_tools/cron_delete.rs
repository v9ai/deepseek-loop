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
