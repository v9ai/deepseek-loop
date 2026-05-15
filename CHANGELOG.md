# Changelog

## 0.5.0 — 2026-05-15

### Features

- New opt-in **recall-aware compaction** behind the `memory` cargo feature.
  When history compaction would discard the middle of a conversation, the
  dropped turns are embedded locally and archived into a LanceDB vector
  store instead of being lost. Enable with
  `RunOptions::memory(Arc<dyn ConversationMemory>)` alongside
  `RunOptions::compaction(..)`. Archival is best-effort and non-fatal: a
  store error is logged and the compaction proceeds regardless.
- New built-in **`MemorySearch`** tool (read-only) for explicit semantic
  recall of archived turns. Wired via
  `builtin_tools::default_tools_with_memory(store, session_id)` or
  `MemorySearchTool::new(..)`. Available whenever `builtin-tools` is on —
  it depends only on the `ConversationMemory` trait, not on LanceDB.
- `LanceMemory` + `FastEmbedEmbedder` (gated by `memory`): a LanceDB-backed
  `ConversationMemory` and a local `fastembed` (bge-small-en-v1.5, 384-dim)
  `Embedder`. Native-only — never enabled by `default` or `wasm`.
- New public API (always exported under the `agent` feature):
  `ConversationMemory`, `Embedder`, `MemoryRecord`, `MemoryHit`,
  `MemoryError`. `RunOptions` gains a `memory` field and `.memory(..)`
  builder. **Adding a public field to `RunOptions`** — construct via
  `RunOptions::new(..)` / `Default` + builders rather than a struct
  literal.
- CLI: new `--memory` flag (requires building with `--features memory`).
  Pair with `--resume <session>` to recall across runs.

### Fixes

- CLI: the agent loop's `session_id` is now bound to the resolved CLI
  session id. Previously `build_opts` never set it, so the loop
  auto-generated a UUID that diverged from the scheduler's (and any
  memory) session namespace.

## 0.4.0 — 2026-05-13

### Cost efficiency

- `UsageInfo` now carries optional `prompt_cache_hit_tokens` /
  `prompt_cache_miss_tokens` so callers see DeepSeek's prompt-cache split.
  `serde(default, skip_serializing_if = Option::is_none)` keeps the wire
  format backwards-compatible.
- `ModelPricing` adds a `cached_input_per_mtok` rate and the per-model
  table is updated to reflect DeepSeek's published cache-hit / cache-miss
  pricing. `turn_cost_usd` now takes `&UsageInfo` and charges the
  cache-hit portion at the discounted rate (typically ~25% of miss).
  Reported `total_cost_usd` now matches the bill DeepSeek actually
  charges for long-running tool loops instead of overstating it by the
  cache-hit discount.

### Features

- New opt-in **natural-language history compaction** for the agent loop.
  Set `RunOptions::compaction(CompactionConfig { ... })` to have the loop
  periodically summarize old turns into a short text summary, dropping
  the originals and bounding prompt-token growth on long tool loops. The
  default config triggers at `prompt_tokens >= 32_000`, keeps the most
  recent 3 turns verbatim, and uses `deepseek-chat` as the (cheap)
  compactor. Compaction preserves tool-call/tool-result pair atomicity
  and is non-fatal on compactor errors.
- New `SystemSubtype::Compact` variant emitted by the stream on a
  successful compaction step. **This is an additive but non-exhaustive
  change** — downstream consumers that exhaustively match on
  `SystemSubtype` need to handle the new variant.

## 0.3.6 — 2026-05-09

### Tests

- Added 27 new tests (total: 130) closing coverage gaps in the cron tool
  wrappers and maintenance helpers. Untested or thinly-tested modules now have
  meaningful suites:
  - `cron_list` (0 → 4): empty/multi-kind listings, definition shape, payload
    fields surface task id / prompt / recurring.
  - `cron_delete` (0 → 5): definition required-fields check, unknown-id
    returns `deleted=false`, repeat-delete idempotency, missing/non-string
    arg errors.
  - `cron_create` (3 → 10): default-recurring matrix for `at`/`dynamic`/cron,
    explicit-recurring override, invalid cron / invalid `at` / missing
    prompt errors, capacity-exceeded path.
  - `monitor` (2 → 8): definition required-fields, missing-arg error,
    non-zero exit-code captured, stderr ignored, default `label`,
    truncation past `MAX_LINES_RETURNED`.
  - `maintenance` (1 → 6): built-in prompt structure, `LOOP_MD_BYTE_CAP`
    pinned at 25k, `read_capped` returns `None` for missing files, full
    contents under cap, truncates oversized files. `read_capped` lifted
    to `pub(crate)` for direct testability.

### Notes

- No behavior changes. This release is tests-only.

## 0.3.5 — 2026-05-09

### Fixed

- **`Schedule::Once` serialization bug** — the variant was a newtype which
  serde's internally-tagged enum representation cannot serialize. Changed to
  `Once { at: DateTime<Utc> }`. Persisted `tasks.json` from v0.3.x with `Once`
  tasks will not round-trip; the scheduler already prunes expired tasks on
  restore, so only future one-shots are affected in practice.
- **`--loop-every 60m`** now correctly maps to `0 */1 * * *` instead of the
  invalid `*/60 * * * *` that the cron library rejects.

### Tests

- Added 81 new tests (total: 103) covering cron grammar, scheduler lifecycle,
  jitter envelopes, store persistence, `interval_to_cron`, and `pick_cron_step`.
  Notable additions: serde round-trip for all `Schedule` variants, atomic save,
  schema-version guard, monotonic `next_after`, full `interval_to_cron` input
  matrix including all rejection paths.

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
