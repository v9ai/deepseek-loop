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
}
