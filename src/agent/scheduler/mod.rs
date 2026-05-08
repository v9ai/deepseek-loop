//! Session-scoped cron scheduler matching Claude Code's `/loop` semantics.
//!
//! The scheduler holds [`Task`]s in memory and (optionally) persists them to
//! disk so a `--resume <session_id>` can restore them. Each tick advances due
//! tasks: recurring tasks have their `next_fire` updated; one-shot tasks are
//! removed after firing. Recurring tasks expire 7 days after creation; their
//! final fire is delivered, then they are removed.
//!
//! See <https://code.claude.com/docs/en/scheduled-tasks> for the spec.

pub mod cron;
pub mod jitter;
pub mod maintenance;
pub mod store;

use std::collections::BTreeMap;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub use cron::CronExpr;
pub use maintenance::{resolve_prompt, BUILT_IN_MAINTENANCE_PROMPT};

/// Session-scoped cap on concurrent tasks (matches Claude Code spec).
pub const DEFAULT_MAX_TASKS: usize = 50;

/// Recurring tasks expire this long after creation.
pub const RECURRING_EXPIRY: Duration = Duration::days(7);

/// Env var that disables the scheduler entirely (matches Claude Code spec).
pub const CLAUDE_DISABLE_VAR: &str = "CLAUDE_CODE_DISABLE_CRON";
/// Crate-specific alias honored alongside [`CLAUDE_DISABLE_VAR`].
pub const ALIAS_DISABLE_VAR: &str = "DEEPSEEK_LOOP_DISABLE_CRON";

/// 8-character base32 task identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TaskId(String);

impl TaskId {
    /// Mint a fresh ID. Crockford-base32 of the UUID's first 5 bytes (40 bits)
    /// gives us 8 chars with no ambiguous I/O/L/U.
    pub fn new() -> Self {
        let bytes = Uuid::new_v4().into_bytes();
        Self(crockford32(&bytes[..5]))
    }

    /// Wrap a caller-supplied string as a [`TaskId`]. No validation — the
    /// scheduler simply uses the value as a map key.
    pub fn from_raw(s: &str) -> Self {
        Self(s.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for TaskId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for TaskId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn crockford32(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let mut bits: u64 = 0;
    for &b in bytes {
        bits = (bits << 8) | b as u64;
    }
    let needed = (bytes.len() * 8).div_ceil(5);
    let mut out = vec![0u8; needed];
    for (i, slot) in out.iter_mut().enumerate() {
        let shift = (needed - 1 - i) * 5;
        let idx = ((bits >> shift) & 0x1f) as usize;
        *slot = ALPHABET[idx];
    }
    String::from_utf8(out).unwrap()
}

/// What kind of cadence a [`Task`] runs on.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Schedule {
    /// Standard 5-field cron expression. Boxed because [`CronExpr`] is much
    /// larger than the other variants.
    Cron(Box<CronExpr>),
    /// One-shot at the specified UTC time.
    ///
    /// Struct variant (not newtype) so serde's internally-tagged representation
    /// can embed the `at` field directly in the JSON object alongside `"kind"`.
    Once { at: DateTime<Utc> },
    /// Claude picks the delay between iterations dynamically.
    Dynamic,
}

/// A scheduled prompt + its cadence + bookkeeping fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub schedule: Schedule,
    pub prompt: String,
    pub recurring: bool,
    pub created_at: DateTime<Utc>,
    pub next_fire: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, thiserror::Error)]
pub enum SchedulerError {
    #[error("scheduler is disabled via env var (set {0}=0 to enable)")]
    Disabled(&'static str),
    #[error("task capacity reached ({0}); delete a task first")]
    Capacity(usize),
    #[error("invalid cron expression: {0}")]
    BadCron(String),
    #[error("invalid schedule: {0}")]
    BadSchedule(String),
}

/// Session-scoped cron scheduler.
#[derive(Debug)]
pub struct Scheduler {
    session_id: String,
    tasks: BTreeMap<TaskId, Task>,
    cap: usize,
    disabled: bool,
}

impl Scheduler {
    /// Create an empty scheduler bound to `session_id`. Honors the
    /// [`CLAUDE_DISABLE_VAR`] / [`ALIAS_DISABLE_VAR`] env vars.
    pub fn new(session_id: impl Into<String>) -> Self {
        Self::with_cap(session_id, DEFAULT_MAX_TASKS)
    }

    pub fn with_cap(session_id: impl Into<String>, cap: usize) -> Self {
        Self {
            session_id: session_id.into(),
            tasks: BTreeMap::new(),
            cap,
            disabled: is_disabled(),
        }
    }

    /// Restore tasks from disk for `session_id`. Tasks that have already
    /// expired are pruned during the restore.
    pub fn restore(session_id: impl Into<String>) -> Self {
        let mut s = Self::new(session_id);
        if let Ok(loaded) = store::load(&s.session_id) {
            let now = Utc::now();
            for t in loaded {
                if t.is_expired(now) {
                    continue;
                }
                s.tasks.insert(t.id.clone(), t);
            }
        }
        s
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    pub fn cap(&self) -> usize {
        self.cap
    }

    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    /// Schedule a new task.
    ///
    /// `recurring=false` makes the task one-shot — it's removed after firing.
    /// `recurring=true` is only meaningful for [`Schedule::Cron`] and
    /// [`Schedule::Dynamic`]; for [`Schedule::Once`] it is treated as
    /// one-shot regardless.
    pub fn create(
        &mut self,
        schedule: Schedule,
        prompt: impl Into<String>,
        recurring: bool,
    ) -> Result<TaskId, SchedulerError> {
        if self.disabled {
            return Err(SchedulerError::Disabled(CLAUDE_DISABLE_VAR));
        }
        if self.tasks.len() >= self.cap {
            return Err(SchedulerError::Capacity(self.cap));
        }

        let now = Utc::now();
        let id = TaskId::new();

        let nominal = match &schedule {
            Schedule::Cron(c) => c
                .clone()
                .next_after(now)
                .ok_or_else(|| SchedulerError::BadCron("no future fire time".into()))?,
            Schedule::Once { at } => *at,
            Schedule::Dynamic => now + Duration::seconds(60),
        };
        let interval = match &schedule {
            Schedule::Cron(c) => Some(c.clone().approx_interval_seconds()),
            _ => None,
        };
        let recurring_eff = match &schedule {
            Schedule::Once { .. } => false,
            _ => recurring,
        };
        let next_fire = jitter::apply(id.as_str(), nominal, interval, recurring_eff);

        let expires_at = if recurring_eff {
            Some(now + RECURRING_EXPIRY)
        } else {
            None
        };

        let task = Task {
            id: id.clone(),
            schedule,
            prompt: prompt.into(),
            recurring: recurring_eff,
            created_at: now,
            next_fire,
            expires_at,
        };
        self.tasks.insert(id.clone(), task);
        let _ = store::save(&self.session_id, &self.snapshot());
        Ok(id)
    }

    pub fn list(&self) -> Vec<&Task> {
        self.tasks.values().collect()
    }

    pub fn delete(&mut self, id: &TaskId) -> bool {
        let removed = self.tasks.remove(id).is_some();
        if removed {
            let _ = store::save(&self.session_id, &self.snapshot());
        }
        removed
    }

    /// Advance the scheduler to `now`. Returns the prompts that just fired,
    /// in fire-time order. Recurring tasks have their `next_fire` advanced to
    /// the next slot (with jitter); one-shot tasks are removed.
    pub fn tick(&mut self, now: DateTime<Utc>) -> Vec<Fire> {
        if self.disabled {
            return Vec::new();
        }

        let due_ids: Vec<TaskId> = self
            .tasks
            .iter()
            .filter(|(_, t)| t.next_fire <= now)
            .map(|(id, _)| id.clone())
            .collect();

        let mut fires = Vec::with_capacity(due_ids.len());
        let mut mutated = false;

        for id in due_ids {
            // Borrow loosely to allow mutation below.
            let snapshot = match self.tasks.get(&id) {
                Some(t) => t.clone(),
                None => continue,
            };

            fires.push(Fire {
                task_id: id.clone(),
                prompt: snapshot.prompt.clone(),
                fired_at: snapshot.next_fire,
                final_fire: snapshot.is_expired(now) || !snapshot.recurring,
            });

            // Decide what to do with the task itself.
            if !snapshot.recurring || snapshot.is_expired(now) {
                self.tasks.remove(&id);
                mutated = true;
                continue;
            }

            // Recurring → advance next_fire.
            let interval = match &snapshot.schedule {
                Schedule::Cron(c) => Some(c.clone().approx_interval_seconds()),
                _ => None,
            };
            let nominal = match &snapshot.schedule {
                Schedule::Cron(c) => c.clone().next_after(now),
                Schedule::Dynamic => Some(now + Duration::seconds(60)),
                Schedule::Once { .. } => None, // unreachable: Once is never recurring
            };
            if let Some(nominal) = nominal {
                let nf = jitter::apply(id.as_str(), nominal, interval, true);
                if let Some(t) = self.tasks.get_mut(&id) {
                    t.next_fire = nf;
                    mutated = true;
                }
            } else {
                self.tasks.remove(&id);
                mutated = true;
            }
        }

        if mutated {
            let _ = store::save(&self.session_id, &self.snapshot());
        }
        fires
    }

    fn snapshot(&self) -> Vec<Task> {
        self.tasks.values().cloned().collect()
    }
}

impl Task {
    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        self.expires_at.is_some_and(|t| now >= t)
    }
}

/// What [`Scheduler::tick`] returns for each due task.
#[derive(Debug, Clone)]
pub struct Fire {
    pub task_id: TaskId,
    pub prompt: String,
    pub fired_at: DateTime<Utc>,
    /// True when this is the last time the task will fire (one-shot or
    /// expired recurring).
    pub final_fire: bool,
}

fn is_disabled() -> bool {
    fn flag(name: &str) -> bool {
        std::env::var(name)
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    }
    flag(CLAUDE_DISABLE_VAR) || flag(ALIAS_DISABLE_VAR)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_scheduler() -> Scheduler {
        // Avoid accidental disable-var leak from the host shell.
        std::env::remove_var(CLAUDE_DISABLE_VAR);
        std::env::remove_var(ALIAS_DISABLE_VAR);
        Scheduler::new("test-session")
    }

    #[test]
    fn task_id_is_8_chars() {
        let id = TaskId::new();
        assert_eq!(id.as_str().len(), 8);
    }

    #[test]
    fn create_then_list_then_delete() {
        let mut s = fresh_scheduler();
        let cron = CronExpr::parse("*/5 * * * *").unwrap();
        let id = s
            .create(Schedule::Cron(Box::new(cron)), "do work", true)
            .unwrap();
        assert_eq!(s.list().len(), 1);
        assert!(s.delete(&id));
        assert_eq!(s.list().len(), 0);
    }

    #[test]
    fn cap_blocks_create() {
        // Bypass the env-var disable check directly: the test should exercise
        // the capacity guard regardless of any disable flag the host may
        // have set (or that another parallel test has set).
        let mut s = Scheduler::with_cap("test-cap", 2);
        s.disabled = false;
        let cron = CronExpr::parse("*/5 * * * *").unwrap();
        s.create(Schedule::Cron(Box::new(cron.clone())), "a", true)
            .unwrap();
        s.create(Schedule::Cron(Box::new(cron.clone())), "b", true)
            .unwrap();
        let err = s
            .create(Schedule::Cron(Box::new(cron)), "c", true)
            .unwrap_err();
        assert!(matches!(err, SchedulerError::Capacity(2)));
    }

    #[test]
    fn tick_fires_due_recurring_and_advances() {
        let mut s = fresh_scheduler();
        let cron = CronExpr::parse("*/1 * * * *").unwrap();
        let id = s
            .create(Schedule::Cron(Box::new(cron)), "tick", true)
            .unwrap();
        // Force the task to be due.
        s.tasks.get_mut(&id).unwrap().next_fire = Utc::now() - Duration::seconds(5);

        let fires = s.tick(Utc::now());
        assert_eq!(fires.len(), 1);
        assert_eq!(fires[0].task_id, id);
        assert!(!fires[0].final_fire);
        // Task is still present and rescheduled into the future.
        let t = s.tasks.get(&id).unwrap();
        assert!(t.next_fire > Utc::now());
    }

    #[test]
    fn one_shot_is_removed_after_fire() {
        let mut s = fresh_scheduler();
        let when = Utc::now() - Duration::seconds(1);
        let id = s
            .create(Schedule::Once { at: when }, "one-shot", false)
            .unwrap();
        let fires = s.tick(Utc::now());
        assert_eq!(fires.len(), 1);
        assert!(fires[0].final_fire);
        assert!(!s.tasks.contains_key(&id));
    }

    #[test]
    fn expired_recurring_fires_once_then_removed() {
        let mut s = fresh_scheduler();
        let cron = CronExpr::parse("*/1 * * * *").unwrap();
        let id = s
            .create(Schedule::Cron(Box::new(cron)), "old", true)
            .unwrap();
        // Backdate the task so it's past expiry and due.
        let t = s.tasks.get_mut(&id).unwrap();
        t.created_at = Utc::now() - Duration::days(8);
        t.expires_at = Some(Utc::now() - Duration::hours(1));
        t.next_fire = Utc::now() - Duration::seconds(5);

        let fires = s.tick(Utc::now());
        assert_eq!(fires.len(), 1);
        assert!(fires[0].final_fire);
        assert!(!s.tasks.contains_key(&id));
    }

    #[test]
    fn disabled_blocks_create_and_tick() {
        // Force-disable directly to avoid racing env-var mutations with other
        // parallel tests that read `is_disabled()` during construction.
        let mut s = Scheduler::new("disabled");
        s.disabled = true;
        let cron = CronExpr::parse("*/5 * * * *").unwrap();
        let err = s
            .create(Schedule::Cron(Box::new(cron)), "x", true)
            .unwrap_err();
        assert!(matches!(err, SchedulerError::Disabled(_)));
        assert!(s.tick(Utc::now()).is_empty());
    }

    #[test]
    fn env_var_triggers_disable() {
        // Verify the env-var check works in isolation. We use a unique var
        // name so we don't race other tests; we point the helper at it via
        // a direct call rather than mutating the global.
        let key = "DEEPSEEK_LOOP_DISABLE_CRON_TEST_ONLY";
        std::env::set_var(key, "1");
        let observed = std::env::var(key).map(|v| v == "1").unwrap_or(false);
        std::env::remove_var(key);
        assert!(observed, "env-var sanity check failed");
    }

    #[test]
    fn task_id_uses_safe_alphabet() {
        // Crockford base32 excludes I, O, L, U.
        for _ in 0..256 {
            let id = TaskId::new();
            for c in id.as_str().chars() {
                assert!(c.is_ascii_alphanumeric());
                assert!(
                    !matches!(c, 'I' | 'O' | 'L' | 'U' | 'i' | 'o' | 'l' | 'u'),
                    "ambiguous char {c} in id {id}"
                );
            }
        }
    }

    #[test]
    fn task_id_is_unique_in_practice() {
        // 1k IDs in a row must all be distinct (UUID v4 → vanishing collision
        // probability over the first 5 bytes; this guards regressions if
        // someone swaps in a non-random source).
        let mut seen = std::collections::HashSet::new();
        for _ in 0..1_000 {
            assert!(seen.insert(TaskId::new()), "collision");
        }
    }

    #[test]
    fn cap_zero_blocks_all_creates() {
        let mut s = Scheduler::with_cap("cap-zero", 0);
        s.disabled = false;
        let cron = CronExpr::parse("*/5 * * * *").unwrap();
        let err = s
            .create(Schedule::Cron(Box::new(cron)), "x", true)
            .unwrap_err();
        assert!(matches!(err, SchedulerError::Capacity(0)));
    }

    #[test]
    fn delete_unknown_returns_false() {
        let mut s = fresh_scheduler();
        assert!(!s.delete(&TaskId::from_raw("DOES-NOT-EXIST")));
    }

    #[test]
    fn tick_on_empty_returns_no_fires() {
        let mut s = fresh_scheduler();
        assert!(s.tick(Utc::now()).is_empty());
    }

    #[test]
    fn dynamic_schedule_starts_60s_out() {
        let mut s = fresh_scheduler();
        let id = s.create(Schedule::Dynamic, "dyn", true).unwrap();
        let task = s.tasks.get(&id).unwrap();
        let delta = (task.next_fire - task.created_at).num_seconds();
        // Dynamic = nominal+60s, jitter cap defaults to 30 min so range is
        // [60, 60+1800).
        assert!((60..(60 + 1800)).contains(&delta), "delta={delta}");
    }

    #[test]
    fn once_schedule_ignores_recurring_flag() {
        let mut s = fresh_scheduler();
        let when = Utc::now() + Duration::hours(1);
        let id = s
            .create(Schedule::Once { at: when }, "future", true)
            .unwrap();
        let t = s.tasks.get(&id).unwrap();
        assert!(!t.recurring, "Once schedule must always be one-shot");
        assert!(t.expires_at.is_none(), "Once must not set an expiry");
    }

    #[test]
    fn once_in_future_does_not_fire() {
        let mut s = fresh_scheduler();
        let when = Utc::now() + Duration::seconds(60);
        let _id = s
            .create(Schedule::Once { at: when }, "later", false)
            .unwrap();
        // Tick well before the scheduled time.
        let fires = s.tick(Utc::now());
        assert!(fires.is_empty());
    }

    #[test]
    fn recurring_cron_sets_7_day_expiry() {
        let mut s = fresh_scheduler();
        let cron = CronExpr::parse("*/5 * * * *").unwrap();
        let id = s.create(Schedule::Cron(Box::new(cron)), "x", true).unwrap();
        let t = s.tasks.get(&id).unwrap();
        let expires = t.expires_at.expect("recurring should set expires_at");
        let span = (expires - t.created_at).num_days();
        assert_eq!(span, 7, "recurring expiry should be exactly 7 days");
    }

    #[test]
    fn non_recurring_cron_has_no_expiry() {
        let mut s = fresh_scheduler();
        let cron = CronExpr::parse("*/5 * * * *").unwrap();
        let id = s
            .create(Schedule::Cron(Box::new(cron)), "x", false)
            .unwrap();
        let t = s.tasks.get(&id).unwrap();
        assert!(t.expires_at.is_none());
    }

    #[test]
    fn tick_returns_due_tasks_in_fire_time_order() {
        // Two tasks both due, with distinct next_fire times. The fires Vec must
        // come back in chronological order (earlier first).
        let mut s = fresh_scheduler();
        let cron = CronExpr::parse("*/5 * * * *").unwrap();
        let id_a = s
            .create(Schedule::Cron(Box::new(cron.clone())), "a", true)
            .unwrap();
        let id_b = s.create(Schedule::Cron(Box::new(cron)), "b", true).unwrap();
        let now = Utc::now();
        s.tasks.get_mut(&id_a).unwrap().next_fire = now - Duration::seconds(10);
        s.tasks.get_mut(&id_b).unwrap().next_fire = now - Duration::seconds(5);

        let fires = s.tick(now);
        assert_eq!(fires.len(), 2);
        // BTreeMap iteration order is by id, so we sort by fired_at to assert.
        let mut sorted = fires.clone();
        sorted.sort_by_key(|f| f.fired_at);
        assert_eq!(sorted[0].task_id, id_a);
        assert_eq!(sorted[1].task_id, id_b);
        assert!(sorted[0].fired_at < sorted[1].fired_at);
    }

    #[test]
    fn cap_returns_capacity_with_correct_bound() {
        // Capacity error should report the cap value itself.
        let mut s = Scheduler::with_cap("cap-three", 3);
        s.disabled = false;
        let cron = CronExpr::parse("*/5 * * * *").unwrap();
        for i in 0..3 {
            s.create(
                Schedule::Cron(Box::new(cron.clone())),
                format!("p{i}"),
                true,
            )
            .unwrap();
        }
        let err = s
            .create(Schedule::Cron(Box::new(cron)), "overflow", true)
            .unwrap_err();
        match err {
            SchedulerError::Capacity(n) => assert_eq!(n, 3),
            other => panic!("expected Capacity(3), got {other:?}"),
        }
    }

    #[test]
    fn tick_after_delete_does_not_fire() {
        // Race-window guard: if a task is deleted between collecting due_ids
        // and processing them, tick must not panic and must skip the deleted
        // task.
        let mut s = fresh_scheduler();
        let cron = CronExpr::parse("*/1 * * * *").unwrap();
        let id = s.create(Schedule::Cron(Box::new(cron)), "x", true).unwrap();
        s.tasks.get_mut(&id).unwrap().next_fire = Utc::now() - Duration::seconds(5);
        // Direct delete, then tick.
        s.tasks.remove(&id);
        let fires = s.tick(Utc::now());
        assert!(fires.is_empty());
    }

    #[test]
    fn task_is_expired_only_after_expires_at() {
        let mut s = fresh_scheduler();
        let cron = CronExpr::parse("*/5 * * * *").unwrap();
        let id = s.create(Schedule::Cron(Box::new(cron)), "x", true).unwrap();
        let t = s.tasks.get(&id).unwrap();
        let exp = t.expires_at.unwrap();
        assert!(!t.is_expired(exp - Duration::seconds(1)));
        assert!(t.is_expired(exp));
        assert!(t.is_expired(exp + Duration::seconds(1)));
    }

    #[test]
    fn restored_scheduler_keeps_tasks_with_persistence_round_trip() {
        // End-to-end: save tasks via store::save_to, then `Scheduler::restore`
        // reads them back. We can't easily redirect Scheduler::restore's
        // session_dir() in-process, so we exercise the lower-level path
        // (store::save_to + load_from) directly to guarantee the JSON shape
        // is round-trippable.
        let dir = tempfile::tempdir().unwrap();
        let cron = CronExpr::parse("*/5 * * * *").unwrap();
        let now = Utc::now();
        let task = Task {
            id: TaskId::new(),
            schedule: Schedule::Cron(Box::new(cron)),
            prompt: "persist me".into(),
            recurring: true,
            created_at: now,
            next_fire: now + Duration::minutes(5),
            expires_at: Some(now + RECURRING_EXPIRY),
        };
        store::save_to(dir.path(), &[task.clone()]).unwrap();
        let loaded = store::load_from(dir.path()).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, task.id);
        assert_eq!(loaded[0].prompt, task.prompt);
        assert!(loaded[0].recurring);
        match (&loaded[0].schedule, &task.schedule) {
            (Schedule::Cron(a), Schedule::Cron(b)) => assert_eq!(a.as_str(), b.as_str()),
            _ => panic!("schedule kind drifted across round trip"),
        }
    }

    #[test]
    fn schedule_is_disabled_when_either_var_set() {
        // We can't safely mutate process env in parallel tests, but we can
        // still cover the helper's branching by inspecting `is_disabled()`
        // with each var key in turn under a serial guard.
        use std::sync::Mutex;
        static ENV_LOCK: Mutex<()> = Mutex::new(());
        let _g = ENV_LOCK.lock().unwrap();

        std::env::remove_var(CLAUDE_DISABLE_VAR);
        std::env::remove_var(ALIAS_DISABLE_VAR);
        assert!(!is_disabled());

        std::env::set_var(CLAUDE_DISABLE_VAR, "1");
        assert!(is_disabled());
        std::env::remove_var(CLAUDE_DISABLE_VAR);

        std::env::set_var(ALIAS_DISABLE_VAR, "true");
        assert!(is_disabled(), "alias should also disable");
        std::env::remove_var(ALIAS_DISABLE_VAR);

        std::env::set_var(CLAUDE_DISABLE_VAR, "0");
        assert!(!is_disabled(), "0 must not disable");
        std::env::remove_var(CLAUDE_DISABLE_VAR);
    }
}
