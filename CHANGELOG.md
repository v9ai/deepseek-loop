# Changelog

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
