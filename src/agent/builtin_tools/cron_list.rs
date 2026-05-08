//! `CronList` tool — return all scheduled tasks.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::agent::scheduler::{Schedule, Scheduler};
use crate::agent::tool::{Tool, ToolDefinition};

pub struct CronListTool {
    pub scheduler: Arc<Mutex<Scheduler>>,
}

impl CronListTool {
    pub fn new(scheduler: Arc<Mutex<Scheduler>>) -> Self {
        Self { scheduler }
    }
}

#[async_trait]
impl Tool for CronListTool {
    fn name(&self) -> &str {
        "CronList"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: "List every scheduled task in the current session, with \
                          schedule, prompt, next fire time (UTC), and expiry."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {},
            }),
        }
    }

    fn read_only_hint(&self) -> bool {
        true
    }

    async fn call_json(&self, _args: Value) -> Result<String, String> {
        let sched = self
            .scheduler
            .lock()
            .map_err(|_| "CronList: scheduler lock poisoned".to_string())?;

        let tasks: Vec<_> = sched
            .list()
            .into_iter()
            .map(|t| {
                let schedule = match &t.schedule {
                    Schedule::Cron(c) => json!({"kind": "cron", "expr": c.as_str()}),
                    Schedule::Once { at } => json!({"kind": "once", "at": at}),
                    Schedule::Dynamic => json!({"kind": "dynamic"}),
                };
                json!({
                    "task_id": t.id.as_str(),
                    "schedule": schedule,
                    "prompt": t.prompt,
                    "recurring": t.recurring,
                    "next_fire": t.next_fire,
                    "expires_at": t.expires_at,
                    "created_at": t.created_at,
                })
            })
            .collect();

        Ok(serde_json::to_string(&json!({
            "session_id": sched.session_id(),
            "count": tasks.len(),
            "cap": sched.cap(),
            "disabled": sched.is_disabled(),
            "tasks": tasks,
        }))
        .unwrap())
    }
}
