# Changelog

## 0.3.4 — 2026-05-09

### Fixed

- **Cron expressions now evaluate in the host's local timezone**, matching the
  Claude Code `/loop` spec. Earlier releases used UTC, which surprised users
  whose `0 9 * * *` jobs fired at 9 AM UTC instead of 9 AM local. Stored fire
  times are still serialized as `DateTime<Utc>` so persisted task state is
  timezone-independent.
- **`Schedule::Once` serialization bug fixed** — the variant was previously
  declared as a newtype, which serde's internally-tagged enum representation
  cannot serialize. Changed to a struct variant (`Once { at: DateTime<Utc> }`).
  Any persisted `tasks.json` from v0.3.x with `Once` tasks will not deserialize
  correctly and should be discarded (the scheduler already prunes expired tasks
  on restore, so in practice only future one-shots are affected).
- **`--loop-every 60m` now correctly maps to `0 */1 * * *`** instead of the
  invalid `*/60 * * * *` that cron rejects (cron minute values cap at 59).
- **`--loop-every` registration now prints the chosen cron expression** so the
  user can see what step the interval mapped to (e.g. a `7m` request rounds
  to `*/6 * * * *` and you see it in the banner).

### Tests

- Added 81 new tests covering the scheduler, cron grammar, jitter, store
  persistence, `interval_to_cron`, and `pick_cron_step`. Previous count: 22.
  Notable additions: `next_after_uses_local_timezone`, serde round-trip for
  all `Schedule` variants, atomic save, schema-version guard, monotonic
  `next_after`, approx-interval values, distinct-jitter-offsets, and full
  `interval_to_cron` input matrix.

### Documentation

- README now explicitly notes that cron expressions are interpreted in the
  host's local timezone (since v0.3.4).
- Dynamic-mode banner now says explicitly that v0.3.x uses a fixed 60 s floor
  between iterations (vs. the spec's 60–3600 s adaptive delay, which lands in
  v0.4).

## 0.3.3 — 2026-05-08

### Changed

- `repository` metadata now points at `https://github.com/v9ai/deepseek-loop`
  (the dedicated source repo). v0.3.2 mistakenly kept the umbrella URL.

## 0.3.2 — 2026-05-08

### Changed

- Source now lives at https://github.com/v9ai/deepseek-loop and is mirrored
  into the umbrella monorepo at https://github.com/nicolad/ai-apps as a git
  submodule. The crates.io `repository` field intentionally still points at
  the umbrella so issue/discussion traffic centralizes there.

## 0.3.1 — 2026-05-08

### Fixed

- Bin now loads both `.env` and `.env.local`, with `.env.local` overriding —
  matches Next.js / Vite convention. Previously only `.env` loaded, so a
  secret living only in `.env.local` was silently invisible to Bash
  subprocesses the agent spawned, even though the bin's own
  `DEEPSEEK_API_KEY` (often duplicated in both files) loaded fine. Hit in
  practice when curling an authenticated downstream service from inside an
  agent run.

## 0.3.0 — 2026-05-08

### Breaking

- **`--json` flag removed.** The bin always streams `SdkMessage` events as
  NDJSON (one JSON object per line). The previous "pretty" mode (Unicode
  bullets) is gone. Pipe into `jq` if you want pretty output.

### Added

- **Cron-style scheduler** matching Claude Code's `/loop` semantics. New
  module `agent::scheduler` exposes:
  - `Scheduler` — session-scoped, capped at 50 tasks by default
  - `Schedule` enum: `Cron(CronExpr) | Once(DateTime<Utc>) | Dynamic`
  - `Task`, `TaskId`, `Fire` — bookkeeping primitives
  - 7-day expiry on recurring tasks (final fire delivered, then removed)
  - Deterministic jitter (30 min cap for hourly+, half-interval for sub-hourly,
    90 s early for one-shot top-of-hour)
  - JSON persistence at `${cache_dir}/deepseek-loop/sessions/<sid>/tasks.json`
  - `loop.md` discovery: `.claude/loop.md` → `~/.claude/loop.md` → built-in
    maintenance prompt
  - `CLAUDE_CODE_DISABLE_CRON=1` env disable flag (plus
    `DEEPSEEK_LOOP_DISABLE_CRON=1` alias)
- **Four new built-in tools** (gated behind the `scheduler` feature):
  `CronCreate`, `CronList`, `CronDelete`, `Monitor`
- `default_tools_with_scheduler(sched)` factory — full 10-tool set wired
  against a shared `Arc<Mutex<Scheduler>>`
- New CLI flags: `--loop`, `--loop-every <interval>`, `--loop-prompt <text>`,
  `--loop-max-iterations <N>`, `--resume <session-id>`, `--max-tasks <N>`
- `RunOptions` is now `Clone` (needed by the loop modes that re-use options
  across iterations)

### Notes

- Cron grammar is strict 5-field; extended syntax (`L`, `W`, `?`, `MON`,
  `JAN`) is rejected.
- The bin's dynamic-interval mode currently uses a 60 s floor between
  iterations; a future release will let the agent emit a chosen-delay
  signal so the wait actually varies (matching Claude Code's spec exactly).

## 0.2.0 — earlier

Initial publish under the `deepseek-loop` name (rename from `deepseek`).
Per-turn agent loop, six built-in tools (Read / Write / Edit / Glob / Grep
/ Bash), permission modes, streaming `SdkMessage` events.
