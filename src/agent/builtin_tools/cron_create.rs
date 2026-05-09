//! `CronCreate` tool — schedule a new task.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use crate::agent::scheduler::{CronExpr, Schedule, Scheduler};
use crate::agent::tool::{Tool, ToolDefinition};

pub struct CronCreateTool {
    pub scheduler: Arc<Mutex<Scheduler>>,
}

impl CronCreateTool {
    pub fn new(scheduler: Arc<Mutex<Scheduler>>) -> Self {
        Self { scheduler }
    }
}

#[async_trait]
impl Tool for CronCreateTool {
    fn name(&self) -> &str {
        "CronCreate"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name().to_string(),
            description: "Schedule a prompt to run on a cron schedule, dynamic interval, \
                          or at a one-shot time. Returns an 8-character `task_id` you can \
                          pass to `CronDelete`. Set `recurring=false` for one-shot \
                          (auto-deletes after firing). All times are UTC. Up to 50 tasks \
                          per session."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "cron": {
                        "type": "string",
                        "description": "5-field cron expression: \
                                        `minute hour day-of-month month day-of-week`. \
                                        Use this OR `at` OR `dynamic`."
                    },
                    "at": {
                        "type": "string",
                        "description": "ISO-8601 UTC timestamp for one-shot. \
                                        Mutually exclusive with `cron`/`dynamic`."
                    },
                    "dynamic": {
                        "type": "boolean",
                        "description": "If true, the host picks the delay between iterations \
                                        (60s..3600s). Mutually exclusive with `cron`/`at`."
                    },
                    "prompt": { "type": "string" },
                    "recurring": {
                        "type": "boolean",
                        "description": "Default: true for cron/dynamic, false for `at`."
                    }
                },
                "required": ["prompt"]
            }),
        }
    }

    async fn call_json(&self, args: Value) -> Result<String, String> {
        let prompt = args
            .get("prompt")
            .and_then(Value::as_str)
            .ok_or_else(|| "CronCreate: missing string `prompt`".to_string())?;

        let has_cron = args.get("cron").is_some();
        let has_at = args.get("at").is_some();
        let has_dynamic = args.get("dynamic").and_then(Value::as_bool) == Some(true);
        let kinds = [has_cron, has_at, has_dynamic]
            .iter()
            .filter(|b| **b)
            .count();
        if kinds == 0 {
            return Err("CronCreate: provide one of `cron`, `at`, or `dynamic=true`".into());
        }
        if kinds > 1 {
            return Err("CronCreate: `cron`, `at`, `dynamic` are mutually exclusive".into());
        }

        let (schedule, default_recurring) = if has_cron {
            let expr = args.get("cron").and_then(Value::as_str).unwrap();
            let cron = CronExpr::parse(expr).map_err(|e| format!("CronCreate: {e}"))?;
            (Schedule::Cron(Box::new(cron)), true)
        } else if has_at {
            let raw = args.get("at").and_then(Value::as_str).unwrap();
            let parsed: DateTime<Utc> = raw
                .parse::<DateTime<Utc>>()
                .map_err(|e| format!("CronCreate: invalid `at` timestamp: {e}"))?;
            (Schedule::Once { at: parsed }, false)
        } else {
            (Schedule::Dynamic, true)
        };

        let recurring = args
            .get("recurring")
            .and_then(Value::as_bool)
            .unwrap_or(default_recurring);

        let mut sched = self
            .scheduler
            .lock()
            .map_err(|_| "CronCreate: scheduler lock poisoned".to_string())?;

        match sched.create(schedule, prompt, recurring) {
            Ok(id) => {
                let task = sched
                    .list()
                    .into_iter()
                    .find(|t| t.id == id)
                    .cloned()
                    .ok_or_else(|| "CronCreate: just-created task vanished".to_string())?;
                Ok(serde_json::to_string(&json!({
                    "task_id": id.as_str(),
                    "next_fire": task.next_fire,
                    "expires_at": task.expires_at,
                    "recurring": task.recurring,
                }))
                .unwrap())
            }
            Err(e) => Err(format!("CronCreate: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::scheduler::Scheduler;
    use serde_json::Value;

    fn fresh_sched() -> Arc<Mutex<Scheduler>> {
        // Ensure no host env var disables us during the test.
        std::env::remove_var("CLAUDE_CODE_DISABLE_CRON");
        std::env::remove_var("DEEPSEEK_LOOP_DISABLE_CRON");
        Arc::new(Mutex::new(Scheduler::new("cron-create-test")))
    }

    #[tokio::test]
    async fn create_cron_then_list_then_delete() {
        let sched = fresh_sched();
        let create = CronCreateTool::new(sched.clone());
        let list = crate::agent::builtin_tools::CronListTool::new(sched.clone());
        let delete = crate::agent::builtin_tools::CronDeleteTool::new(sched.clone());

        let raw = create
            .call_json(json!({
                "cron": "*/5 * * * *",
                "prompt": "check the deploy",
                "recurring": true
            }))
            .await
            .unwrap();
        let resp: Value = serde_json::from_str(&raw).unwrap();
        let task_id = resp["task_id"].as_str().unwrap().to_string();
        assert_eq!(task_id.len(), 8);
        assert_eq!(resp["recurring"].as_bool().unwrap(), true);

        let listed: Value =
            serde_json::from_str(&list.call_json(json!({})).await.unwrap()).unwrap();
        assert_eq!(listed["count"].as_u64().unwrap(), 1);

        let deleted: Value =
            serde_json::from_str(&delete.call_json(json!({"task_id": task_id})).await.unwrap())
                .unwrap();
        assert_eq!(deleted["deleted"].as_bool().unwrap(), true);

        let listed2: Value =
            serde_json::from_str(&list.call_json(json!({})).await.unwrap()).unwrap();
        assert_eq!(listed2["count"].as_u64().unwrap(), 0);
    }

    #[tokio::test]
    async fn rejects_two_kinds_at_once() {
        let sched = fresh_sched();
        let create = CronCreateTool::new(sched);
        let err = create
            .call_json(json!({
                "cron": "*/5 * * * *",
                "dynamic": true,
                "prompt": "x"
            }))
            .await
            .unwrap_err();
        assert!(err.contains("mutually exclusive"), "got: {err}");
    }

    #[tokio::test]
    async fn requires_one_schedule_kind() {
        let sched = fresh_sched();
        let create = CronCreateTool::new(sched);
        let err = create.call_json(json!({"prompt": "x"})).await.unwrap_err();
        assert!(err.contains("one of"), "got: {err}");
    }

    #[tokio::test]
    async fn at_default_recurring_is_false() {
        let sched = fresh_sched();
        let create = CronCreateTool::new(sched);
        let future = (Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
        let raw = create
            .call_json(json!({
                "at": future,
                "prompt": "ping me later",
            }))
            .await
            .unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["recurring"].as_bool().unwrap(), false);
    }

    #[tokio::test]
    async fn dynamic_default_recurring_is_true() {
        let sched = fresh_sched();
        let create = CronCreateTool::new(sched);
        let raw = create
            .call_json(json!({
                "dynamic": true,
                "prompt": "loop me",
            }))
            .await
            .unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["recurring"].as_bool().unwrap(), true);
    }

    #[tokio::test]
    async fn recurring_override_wins() {
        let sched = fresh_sched();
        let create = CronCreateTool::new(sched);
        // cron defaults to recurring=true; explicit false should win.
        let raw = create
            .call_json(json!({
                "cron": "*/5 * * * *",
                "prompt": "once-only via cron",
                "recurring": false,
            }))
            .await
            .unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["recurring"].as_bool().unwrap(), false);
    }

    #[tokio::test]
    async fn invalid_cron_surfaces_error() {
        let sched = fresh_sched();
        let create = CronCreateTool::new(sched);
        let err = create
            .call_json(json!({
                "cron": "0 9 * * MON",
                "prompt": "x"
            }))
            .await
            .unwrap_err();
        assert!(
            err.contains("CronCreate") && err.contains("unsupported"),
            "got: {err}"
        );
    }

    #[tokio::test]
    async fn invalid_at_timestamp_surfaces_error() {
        let sched = fresh_sched();
        let create = CronCreateTool::new(sched);
        let err = create
            .call_json(json!({
                "at": "not-a-timestamp",
                "prompt": "x"
            }))
            .await
            .unwrap_err();
        assert!(err.contains("invalid `at`"), "got: {err}");
    }

    #[tokio::test]
    async fn missing_prompt_returns_error() {
        let sched = fresh_sched();
        let create = CronCreateTool::new(sched);
        let err = create
            .call_json(json!({"cron": "*/5 * * * *"}))
            .await
            .unwrap_err();
        assert!(err.contains("missing string `prompt`"), "got: {err}");
    }

    #[tokio::test]
    async fn capacity_exceeded_surfaces_error() {
        std::env::remove_var("CLAUDE_CODE_DISABLE_CRON");
        std::env::remove_var("DEEPSEEK_LOOP_DISABLE_CRON");
        let sched = Arc::new(Mutex::new(crate::agent::scheduler::Scheduler::with_cap(
            "cap-test", 2,
        )));
        let create = CronCreateTool::new(sched);

        // Two creates ok.
        for i in 0..2 {
            create
                .call_json(json!({
                    "cron": "*/5 * * * *",
                    "prompt": format!("p{i}"),
                }))
                .await
                .unwrap();
        }

        // Third should fail with capacity message.
        let err = create
            .call_json(json!({
                "cron": "*/5 * * * *",
                "prompt": "overflow"
            }))
            .await
            .unwrap_err();
        assert!(
            err.to_lowercase().contains("cap")
                || err.to_lowercase().contains("max")
                || err.to_lowercase().contains("limit"),
            "got: {err}"
        );
    }
}
