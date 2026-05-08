# deepseek

Shared Rust client for the DeepSeek API with pluggable HTTP transport, agentic tool-use loop, and in-memory caching.

## Modules

| Module | Feature | Description |
|--------|---------|-------------|
| `types` | — | Core API types: messages, requests, responses, models |
| `error` | — | `DeepSeekError` enum and `Result<T>` alias |
| `client` | — | `HttpClient` trait + generic `DeepSeekClient<H>` |
| `reqwest_client` | `reqwest-client` | Native reqwest transport, `client_from_env()`, `reason()`, `reason_with_retry()` |
| `wasm` | `wasm` | Cloudflare Workers fetch-based transport |
| `agent` | `agent` | `Tool` trait, `AgentBuilder`, `DeepSeekAgent`, **streaming `run()` loop with `SdkMessage` events** |
| `agent::builtin_tools` | `builtin-tools` | Read / Write / Edit / Glob / Grep / Bash, Claude-Code-style |
| `cache` | `cache` | TTL cache + broadcast-based in-flight deduplication |

## Feature Flags

| Flag | Default | Description |
|------|---------|-------------|
| `reqwest-client` | yes | Native HTTP via reqwest + tokio |
| `wasm` | no | CF Workers WASM fetch transport (`?Send`) |
| `agent` | no | Streaming agent loop (`run`, `SdkMessage`, `RunOptions`, `PermissionMode`) |
| `builtin-tools` | no | Read / Write / Edit / Glob / Grep / Bash (implies `agent`) |
| `scheduler` | yes | `Scheduler` + CronCreate / CronList / CronDelete / Monitor tools and `/loop` semantics (implies `agent` + `builtin-tools`) |
| `cli` | no | `deepseek-loop` binary (implies `scheduler` + `builtin-tools` + `reqwest-client`) |
| `cache` | no | TTL cache + in-flight dedup (dashmap, sha2, tokio) |

## Key Types

### Models & Effort

```rust
pub enum DeepSeekModel { Reasoner, Chat }  // aliases: "r1"/"deep" → Reasoner, "v3"/"fast" → Chat
pub enum EffortLevel { Low, Medium, High, Max }
// Low: 0.1 temp / 2048 tokens → Max: 1.0 temp / 16384 tokens
```

### Messages

```rust
pub struct ChatMessage { role, content, reasoning_content?, tool_calls?, tool_call_id?, name? }
pub struct ChatRequest  { model, messages, tools?, tool_choice?, temperature?, max_tokens?, stream? }
pub struct ChatResponse { id, choices, usage? }

// Constructors
pub fn system_msg(s: &str) -> ChatMessage
pub fn user_msg(s: &str) -> ChatMessage
pub fn assistant_msg(s: &str) -> ChatMessage
pub fn tool_result_msg(tool_call_id: &str, content: &str) -> ChatMessage
```

### Request Builder (free function)

```rust
pub fn build_request(model, messages, tools?, effort) -> ChatRequest
```

Usable without a client instance — builds a typed `ChatRequest` with effort-mapped temperature/max_tokens.

### Client

```rust
pub trait HttpClient: Send + Sync {
    async fn post_json(&self, url: &str, token: &str, body: ChatRequest) -> Result<ChatResponse>;
}

pub struct DeepSeekClient<H: HttpClient> { .. }
impl<H: HttpClient> DeepSeekClient<H> {
    pub fn new(api_key, http) -> Self
    pub fn with_base_url(self, url) -> Self
    pub async fn chat(&self, req: &ChatRequest) -> Result<ChatResponse>
}

// reqwest-client feature
pub fn client_from_env() -> Result<DeepSeekClient<ReqwestClient>>  // reads DEEPSEEK_API_KEY
pub async fn reason(client, system, user) -> Result<ReasonerOutput>
pub async fn reason_with_retry(client, system, user) -> Result<ReasonerOutput>  // 3 attempts, exp backoff
```

### Agent (feature `agent`)

```rust
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn definition(&self) -> ToolDefinition;
    async fn call_json(&self, args: Value) -> Result<String, String>;
}

pub struct AgentBuilder<H: HttpClient> { .. }
impl<H: HttpClient> AgentBuilder<H> {
    pub fn new() -> Self
    pub fn preamble(self, s: &str) -> Self
    pub fn tool(self, t: impl Tool + 'static) -> Self
    pub fn build(self) -> DeepSeekAgent<H>
}

impl<H: HttpClient> DeepSeekAgent<H> {
    pub async fn prompt(&self, input: String) -> Result<String>  // legacy: collects to text
    pub fn run(&self, input: String) -> impl Stream<Item = SdkMessage>  // Claude-Code shape
}
```

#### Streaming loop — Claude Code shape

The `run()` function (and the `DeepSeekAgent::run()` method) yields a typed
sequence of [`SdkMessage`] events that mirror the Claude Agent SDK's loop:

```text
System{Init}                   ← session metadata, fresh uuid session id
Assistant{ ToolUse… }          ← per turn the model requests tools
User{ ToolResult… }            ← combined results fed back to the model
…repeat until no tool_use…
Assistant{ Text }              ← final answer
Result{ subtype, result, total_cost_usd, usage, num_turns, session_id, stop_reason }
```

`RunOptions` lets you configure: `model`, `system_prompt`, `allowed_tools`,
`disallowed_tools`, `max_turns`, `max_budget_usd`, `effort`,
`permission_mode`, `pre_tool_hook`, `session_id`, `base_url`.

Read-only tools (those whose `read_only_hint() == true`) are dispatched in
parallel within a turn; mutating tools run sequentially. `PermissionMode`
mirrors the SDK: `Default`, `AcceptEdits`, `Plan`, `DontAsk`,
`BypassPermissions`. Hitting `max_turns` / `max_budget_usd` yields a
`Result` with the matching error subtype.

#### CLI (feature `cli`)

The binary streams `SdkMessage` events as **NDJSON** (one JSON object per line) on stdout. Pipe it into `jq` or any line-oriented consumer.

```bash
# One-shot run
cargo run -p deepseek-loop --features cli --bin deepseek-loop -- \
    "list rust files under crates/deepseek-loop"

# Read-only review (Plan mode blocks Bash/Write/Edit)
cargo run -p deepseek-loop --features cli --bin deepseek-loop -- \
    --permission-mode plan --max-turns 4 --max-budget-usd 0.05 \
    "summarize the agent loop"
```

### Scheduler / `/loop` (feature `scheduler`, on by default)

Models Claude Code's `/loop` scheduler — see the upstream spec at <https://code.claude.com/docs/en/scheduled-tasks>. The crate ships four built-in tools (`CronCreate`, `CronList`, `CronDelete`, `Monitor`) and three CLI invocation shapes.

#### Three invocation shapes

| What you provide | Example | What happens |
| --- | --- | --- |
| Interval + prompt | `deepseek-loop --loop-every 5m --loop-prompt 'check the deploy'` | Fixed cron schedule. Interval is converted to a clean cron step and the resolved expression is printed at registration. |
| Prompt only | `deepseek-loop --loop --loop-prompt 'check whether CI passed'` | Dynamic interval. Currently a fixed 60 s floor between iterations (the upstream spec's adaptive 60–3600 s delay lands in v0.4). |
| Bare or interval-only | `deepseek-loop --loop` or `--loop-every 15m` | Runs the [built-in maintenance prompt](#built-in-maintenance-prompt) — or your `loop.md` if one exists. |

```bash
# Fixed cron interval, capped at 10 fires
deepseek-loop --loop-every 5m --loop-prompt 'check the deploy and report' \
    --loop-max-iterations 10

# Dynamic interval
deepseek-loop --loop --loop-prompt 'check whether CI passed and address review comments'

# Bare /loop with the built-in maintenance prompt (or your loop.md)
deepseek-loop --loop

# Resume a prior session's scheduled tasks
deepseek-loop --resume <session-id> --loop
```

> **Times are interpreted in the host's local timezone** as of v0.3.4 (was UTC in 0.3.3 and earlier). A cron expression like `0 9 * * *` fires at 9 AM local. Stored fire times are still serialized as UTC so persisted state is timezone-independent.

#### `loop.md` discovery

When you run `/loop` (bare or interval-only) and don't supply a prompt, the bin resolves the prompt from these locations in order:

| Path | Scope |
| --- | --- |
| `.claude/loop.md` | Project-level. Takes precedence when both files exist. |
| `~/.claude/loop.md` | User-level. Applies in any project that does not define its own. |

Files larger than 25 KB are truncated. When neither file exists, the bin falls back to the built-in maintenance prompt.

#### Built-in maintenance prompt

A bare `--loop` (no prompt, no `loop.md`) runs a maintenance prompt that, on each iteration, in order:

1. Continues any unfinished work from the conversation.
2. Tends to the current branch's pull request: review comments, failed CI runs, merge conflicts.
3. If nothing else is pending, runs a cleanup pass — bug hunts or simplification.

It does not start new initiatives outside that scope, and irreversible actions only proceed when they continue something the transcript already authorized.

#### Manage tasks

Ask the agent in natural language during a run:

```text
what scheduled tasks do I have?
cancel the deploy check job
remind me at 3pm to push the release branch
in 45 minutes, check whether the integration tests passed
```

Under the hood, the agent calls these tools:

| Tool | Purpose |
| --- | --- |
| `CronCreate` | Schedule a new task. Accepts a 5-field cron expression, an ISO-8601 timestamp for one-shot, or `dynamic=true`; the prompt to run; `recurring` true/false. |
| `CronList` | List all scheduled tasks with their 8-character IDs, schedules, prompts, next fire time. |
| `CronDelete` | Cancel a task by `task_id`. |

Each task has an 8-character base32 ID. A session can hold up to **50** scheduled tasks at once.

#### Cron expression reference

5-field standard cron: `minute hour day-of-month month day-of-week`. All fields support wildcards (`*`), single values (`5`), steps (`*/15`), ranges (`1-5`), and comma-separated lists (`1,15,30`).

| Example | Meaning |
| --- | --- |
| `*/5 * * * *` | Every 5 minutes |
| `0 * * * *` | Every hour on the hour |
| `7 * * * *` | Every hour at 7 minutes past |
| `0 9 * * *` | Every day at 9 AM local |
| `0 9 * * 1-5` | Weekdays at 9 AM local |
| `30 14 15 3 *` | March 15 at 2:30 PM local |

Day-of-week uses `0` or `7` for Sunday through `6` for Saturday. Extended syntax (`L`, `W`, `?`, `MON`, `JAN`) is **not** supported. When both day-of-month and day-of-week are constrained, a date matches if either field matches (vixie-cron semantics).

> **Timezone note (since v0.3.4):** cron expressions are evaluated in the host's local timezone. `0 9 * * *` means 9 AM local, not 9 AM UTC. Fire times are stored as UTC so persisted task state is portable.

#### Jitter & expiry

The scheduler adds a deterministic offset to fire times so independent sessions don't hit downstream APIs at the same wall-clock moment:

- Recurring tasks fire up to **30 minutes** after the scheduled time, or up to **half the interval** for sub-hourly tasks. An hourly job at `:00` may fire anywhere up to `:30`.
- One-shot tasks scheduled at the top or bottom of the hour fire up to **90 seconds early**.

The offset is derived from the task ID, so the same task always gets the same offset. If exact timing matters, pick a minute that isn't `:00` or `:30` (e.g. `3 9 * * *` instead of `0 9 * * *`).

Recurring tasks **expire 7 days** after creation: they fire one final time, then delete themselves. This bounds how long a forgotten loop can run.

#### Persistence

Tasks are written to `${cache_dir}/deepseek-loop/sessions/<session_id>/tasks.json` after each mutation. Restore them with `--resume <session_id>`. Background `Bash` and `Monitor` tasks are **not** restored.

#### Disable

Set either env var to `1` to disable the scheduler entirely. Cron tools and the `--loop` flags become unavailable, and any already-scheduled tasks stop firing:

```bash
CLAUDE_CODE_DISABLE_CRON=1   # canonical; matches Claude Code
DEEPSEEK_LOOP_DISABLE_CRON=1 # crate-specific alias
```

#### Limitations

- Tasks only fire while `deepseek-loop` is running and idle. Closing the terminal or letting the bin exit stops them firing.
- No catch-up for missed fires — if a task's scheduled time passes while the bin is busy on a long request, it fires once when the bin becomes idle, not once per missed interval.
- Dynamic mode currently uses a fixed **60 s floor** between iterations. The upstream spec's adaptive 60–3600 s delay lands in v0.4 (requires a host↔agent protocol for the agent to signal a chosen delay).
- For unattended cron-driven automation that must survive without the bin running, drive `deepseek-loop` from system `cron` / launchd / GitHub Actions instead.

### Cache (feature `cache`)

```rust
pub struct Cache { .. }          // entry-level TTL, evicts expired on insert
pub struct InFlightDedup<T> { .. } // broadcast-based — first caller inserts, others subscribe
```

## Retry Logic

`reason_with_retry()` retries only on 5xx / network errors with exponential backoff (1s → 2s → 4s). 4xx errors fail immediately.

## Dependencies

Standalone library — no sibling crate dependencies. Used by `sdd`, `research`, and `genesis`.
