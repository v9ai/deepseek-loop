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

pub(crate) fn read_capped(path: PathBuf) -> Option<String> {
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
    use std::io::Write;

    #[test]
    fn built_in_prompt_is_non_empty() {
        assert!(!BUILT_IN_MAINTENANCE_PROMPT.is_empty());
        assert!(BUILT_IN_MAINTENANCE_PROMPT.contains("Continue"));
    }

    #[test]
    fn built_in_prompt_mentions_pull_request_and_cleanup() {
        // Sanity-check the structure of the spec: PRs and cleanup are
        // explicitly part of the maintenance loop.
        assert!(BUILT_IN_MAINTENANCE_PROMPT.contains("pull request"));
        assert!(BUILT_IN_MAINTENANCE_PROMPT.contains("cleanup"));
    }

    #[test]
    fn loop_md_byte_cap_is_25kb() {
        assert_eq!(LOOP_MD_BYTE_CAP, 25_000);
    }

    #[test]
    fn read_capped_returns_none_for_missing_file() {
        let bogus = PathBuf::from("/nonexistent/path/loop.md");
        assert!(read_capped(bogus).is_none());
    }

    #[test]
    fn read_capped_returns_full_content_under_cap() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("small.md");
        let mut f = fs::File::create(&p).unwrap();
        f.write_all(b"hello world").unwrap();
        let got = read_capped(p).unwrap();
        assert_eq!(got, "hello world");
    }

    #[test]
    fn read_capped_truncates_oversized_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("big.md");
        let mut f = fs::File::create(&p).unwrap();
        // 30k bytes of `a`. Cap is 25k.
        let payload = "a".repeat(30_000);
        f.write_all(payload.as_bytes()).unwrap();
        let got = read_capped(p).unwrap();
        assert_eq!(got.len(), LOOP_MD_BYTE_CAP);
        assert!(got.chars().all(|c| c == 'a'));
    }
}
