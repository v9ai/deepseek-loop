//! JSON persistence for [`super::Scheduler`] state.
//!
//! Tasks are stored at `${cache_dir}/deepseek-loop/sessions/<session_id>/tasks.json`.
//! Writes are atomic (tempfile + rename).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::Task;

/// File-level wrapper so we can extend later without breaking older readers.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoreFile {
    /// Schema version; bumped on incompatible changes.
    version: u32,
    tasks: Vec<Task>,
}

const SCHEMA_VERSION: u32 = 1;

/// Resolve the on-disk directory for `session_id`.
pub fn session_dir(session_id: &str) -> Option<PathBuf> {
    dirs::cache_dir().map(|d| d.join("deepseek-loop").join("sessions").join(session_id))
}

/// Load tasks for `session_id`. Returns an empty vec when no file exists.
pub fn load(session_id: &str) -> io::Result<Vec<Task>> {
    let Some(dir) = session_dir(session_id) else {
        return Ok(Vec::new());
    };
    let path = dir.join("tasks.json");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = fs::read_to_string(&path)?;
    let store: StoreFile = serde_json::from_str(&raw).map_err(io::Error::other)?;
    if store.version != SCHEMA_VERSION {
        // Unknown future schema → treat as empty rather than crashing.
        return Ok(Vec::new());
    }
    Ok(store.tasks)
}

/// Atomically persist `tasks` for `session_id`.
pub fn save(session_id: &str, tasks: &[Task]) -> io::Result<()> {
    let Some(dir) = session_dir(session_id) else {
        return Ok(()); // no cache dir on this platform → silently skip
    };
    save_to(&dir, tasks)
}

/// Save into an explicit directory (testable path).
pub fn save_to(dir: &Path, tasks: &[Task]) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let store = StoreFile {
        version: SCHEMA_VERSION,
        tasks: tasks.to_vec(),
    };
    let json = serde_json::to_vec_pretty(&store).map_err(io::Error::other)?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    use std::io::Write;
    tmp.write_all(&json)?;
    tmp.flush()?;
    let target = dir.join("tasks.json");
    tmp.persist(&target).map_err(|e| e.error)?;
    Ok(())
}

/// Load from an explicit directory (testable path).
pub fn load_from(dir: &Path) -> io::Result<Vec<Task>> {
    let path = dir.join("tasks.json");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = fs::read_to_string(&path)?;
    let store: StoreFile = serde_json::from_str(&raw).map_err(io::Error::other)?;
    if store.version != SCHEMA_VERSION {
        return Ok(Vec::new());
    }
    Ok(store.tasks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::scheduler::{Schedule, Task, TaskId};
    use chrono::Utc;

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let task = Task {
            id: TaskId::new(),
            schedule: Schedule::Dynamic,
            prompt: "test".into(),
            recurring: false,
            created_at: Utc::now(),
            next_fire: Utc::now(),
            expires_at: None,
        };
        save_to(dir.path(), &[task.clone()]).unwrap();
        let loaded = load_from(dir.path()).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id.as_str(), task.id.as_str());
        assert_eq!(loaded[0].prompt, task.prompt);
    }

    #[test]
    fn missing_file_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_from(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn unknown_schema_version_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tasks.json");
        // Forge a future-version file; load_from should treat it as empty
        // rather than panicking or surfacing a deserialization error.
        std::fs::write(&path, r#"{"version":99,"tasks":[]}"#).unwrap();
        assert!(load_from(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn corrupt_json_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tasks.json");
        std::fs::write(&path, "not json at all").unwrap();
        assert!(load_from(dir.path()).is_err());
    }

    #[test]
    fn save_creates_directory_if_missing() {
        let parent = tempfile::tempdir().unwrap();
        let nested = parent.path().join("does/not/yet/exist");
        let task = Task {
            id: TaskId::new(),
            schedule: Schedule::Dynamic,
            prompt: "p".into(),
            recurring: false,
            created_at: Utc::now(),
            next_fire: Utc::now(),
            expires_at: None,
        };
        save_to(&nested, &[task.clone()]).unwrap();
        let loaded = load_from(&nested).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, task.id);
    }

    #[test]
    fn save_is_atomic_no_tmp_leftover() {
        // After a successful save, only `tasks.json` should be present —
        // tempfile::persist atomically renames, so no `.tmp*` leftover.
        let dir = tempfile::tempdir().unwrap();
        let task = Task {
            id: TaskId::new(),
            schedule: Schedule::Dynamic,
            prompt: "p".into(),
            recurring: false,
            created_at: Utc::now(),
            next_fire: Utc::now(),
            expires_at: None,
        };
        save_to(dir.path(), &[task]).unwrap();

        let entries: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(entries, vec!["tasks.json"], "got entries: {entries:?}");
    }

    #[test]
    fn round_trip_preserves_schedule_kind() {
        // Each Schedule variant must survive serde round-trip.
        let dir = tempfile::tempdir().unwrap();
        let now = Utc::now();
        let cron = crate::agent::scheduler::CronExpr::parse("*/15 * * * *").unwrap();
        let tasks = vec![
            Task {
                id: TaskId::new(),
                schedule: Schedule::Cron(Box::new(cron)),
                prompt: "cron".into(),
                recurring: true,
                created_at: now,
                next_fire: now,
                expires_at: Some(now),
            },
            Task {
                id: TaskId::new(),
                schedule: Schedule::Once { at: now },
                prompt: "once".into(),
                recurring: false,
                created_at: now,
                next_fire: now,
                expires_at: None,
            },
            Task {
                id: TaskId::new(),
                schedule: Schedule::Dynamic,
                prompt: "dynamic".into(),
                recurring: true,
                created_at: now,
                next_fire: now,
                expires_at: Some(now),
            },
        ];
        save_to(dir.path(), &tasks).unwrap();
        let loaded = load_from(dir.path()).unwrap();
        assert_eq!(loaded.len(), 3);
        let kinds: Vec<&'static str> = loaded
            .iter()
            .map(|t| match &t.schedule {
                Schedule::Cron(_) => "cron",
                Schedule::Once { .. } => "once",
                Schedule::Dynamic => "dynamic",
            })
            .collect();
        assert_eq!(kinds, vec!["cron", "once", "dynamic"]);
    }

    #[test]
    fn save_overwrites_previous_file() {
        let dir = tempfile::tempdir().unwrap();
        let task1 = Task {
            id: TaskId::from_raw("FIRST"),
            schedule: Schedule::Dynamic,
            prompt: "v1".into(),
            recurring: false,
            created_at: Utc::now(),
            next_fire: Utc::now(),
            expires_at: None,
        };
        save_to(dir.path(), &[task1]).unwrap();
        let task2 = Task {
            id: TaskId::from_raw("SECOND"),
            schedule: Schedule::Dynamic,
            prompt: "v2".into(),
            recurring: false,
            created_at: Utc::now(),
            next_fire: Utc::now(),
            expires_at: None,
        };
        save_to(dir.path(), &[task2]).unwrap();
        let loaded = load_from(dir.path()).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id.as_str(), "SECOND");
    }
}
