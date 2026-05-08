//! Built-in maintenance prompt + `loop.md` discovery.
//!
//! When the user invokes `/loop` with no prompt, the scheduler runs this
//! prompt instead. A user-supplied `loop.md` (project-level or user-level)
//! takes precedence.

use std::fs;
use std::path::PathBuf;

/// Cap for `loop.md` content. Longer files are truncated.
pub const LOOP_MD_BYTE_CAP: usize = 25_000;

/// The default maintenance prompt used when no `loop.md` is found.
pub const BUILT_IN_MAINTENANCE_PROMPT: &str = "\
On each iteration, work through the following in order:

1. Continue any unfinished work from the conversation.
2. Tend to the current branch's pull request: review comments, failed CI \
   runs, merge conflicts.
3. If nothing else is pending, run a cleanup pass: bug hunts or \
   simplification.

Do not start new initiatives outside that scope. Irreversible actions \
(pushing, deleting, force-push) only proceed when they continue something \
the transcript already authorized.";

/// Resolve the prompt for a bare `/loop` invocation.
///
/// Search order, first hit wins:
/// 1. `<cwd>/.claude/loop.md` — project-level
/// 2. `~/.claude/loop.md` — user-level
/// 3. [`BUILT_IN_MAINTENANCE_PROMPT`] — fallback
pub fn resolve_prompt() -> String {
    if let Some(p) = read_capped(PathBuf::from(".claude/loop.md")) {
        return p;
    }
    if let Some(home) = dirs::home_dir() {
        if let Some(p) = read_capped(home.join(".claude/loop.md")) {
            return p;
        }
    }
    BUILT_IN_MAINTENANCE_PROMPT.to_string()
}

fn read_capped(path: PathBuf) -> Option<String> {
    let raw = fs::read_to_string(&path).ok()?;
    if raw.len() <= LOOP_MD_BYTE_CAP {
        Some(raw)
    } else {
        Some(raw[..LOOP_MD_BYTE_CAP].to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_prompt_is_non_empty() {
        assert!(!BUILT_IN_MAINTENANCE_PROMPT.is_empty());
        assert!(BUILT_IN_MAINTENANCE_PROMPT.contains("Continue"));
    }
}
