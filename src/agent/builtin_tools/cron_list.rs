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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::scheduler::Scheduler;
    use chrono::{Duration, Utc};
    use serde_json::Value;

    fn fresh_sched(session: &str) -> Arc<Mutex<Scheduler>> {
        std::env::remove_var("CLAUDE_CODE_DISABLE_CRON");
        std::env::remove_var("DEEPSEEK_LOOP_DISABLE_CRON");
        Arc::new(Mutex::new(Scheduler::new(session)))
    }

    #[test]
    fn definition_advertises_read_only_and_no_args() {
        let sched = fresh_sched("def");
        let tool = CronListTool::new(sched);
        let def = tool.definition();
        assert_eq!(def.name, "CronList");
        assert!(tool.read_only_hint());
        let props = def.parameters.get("properties").unwrap();
        assert!(props.as_object().unwrap().is_empty());
    }

    #[tokio::test]
    async fn empty_scheduler_lists_zero_tasks() {
        let sched = fresh_sched("empty");
        let tool = CronListTool::new(sched);
        let raw = tool.call_json(json!({})).await.unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["count"].as_u64().unwrap(), 0);
        assert_eq!(v["session_id"].as_str().unwrap(), "empty");
        assert_eq!(v["disabled"].as_bool().unwrap(), false);
        assert!(v["cap"].as_u64().unwrap() >= 50);
        assert_eq!(v["tasks"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn lists_all_three_schedule_kinds() {
        let sched = fresh_sched("kinds");
        {
            let mut s = sched.lock().unwrap();
            let cron = CronExpr_for_test("*/10 * * * *");
            s.create(Schedule::Cron(Box::new(cron)), "ping cron", true)
                .unwrap();
            let when = Utc::now() + Duration::hours(1);
            s.create(Schedule::Once { at: when }, "one shot", false)
                .unwrap();
            s.create(Schedule::Dynamic, "dyn", true).unwrap();
        }
        let tool = CronListTool::new(sched);
        let raw = tool.call_json(json!({})).await.unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["count"].as_u64().unwrap(), 3);
        let kinds: Vec<&str> = v["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["schedule"]["kind"].as_str().unwrap())
            .collect();
        assert!(kinds.contains(&"cron"));
        assert!(kinds.contains(&"once"));
        assert!(kinds.contains(&"dynamic"));
    }

    // (`disabled` flag JSON surface is trivial — env-var driven Scheduler
    // construction races with parallel tests that also touch the env var,
    // and `disabled` is module-private. The env-var detection itself is
    // covered by `mod.rs::schedule_is_disabled_when_either_var_set`.)

    #[tokio::test]
    async fn task_payload_includes_id_prompt_recurring() {
        let sched = fresh_sched("payload");
        {
            let mut s = sched.lock().unwrap();
            let cron = CronExpr_for_test("0 9 * * *");
            s.create(Schedule::Cron(Box::new(cron)), "morning check", true)
                .unwrap();
        }
        let tool = CronListTool::new(sched);
        let raw = tool.call_json(json!({})).await.unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        let task = &v["tasks"][0];
        assert_eq!(task["task_id"].as_str().unwrap().len(), 8);
        assert_eq!(task["prompt"].as_str().unwrap(), "morning check");
        assert_eq!(task["recurring"].as_bool().unwrap(), true);
        assert!(task["next_fire"].is_string());
    }

    // Helper kept inside tests so it doesn't widen the public surface.
    #[allow(non_snake_case)]
    fn CronExpr_for_test(expr: &str) -> crate::agent::scheduler::CronExpr {
        crate::agent::scheduler::CronExpr::parse(expr).unwrap()
    }
}
