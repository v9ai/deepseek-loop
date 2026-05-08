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

Matches Claude Code's [`/loop` semantics](https://code.claude.com/docs/en/scheduled-tasks). Four built-in tools (`CronCreate`, `CronList`, `CronDelete`, `Monitor`) plus three CLI invocation shapes:

```bash
# Fixed cron interval (5 minutes between fires)
deepseek-loop --loop-every 5m --loop-prompt "check the deploy and report" \
    --loop-max-iterations 10

# Dynamic interval — agent picks delay each iteration (60s floor)
deepseek-loop --loop --loop-prompt "check whether CI passed and address review comments"

# Bare /loop — runs the built-in maintenance prompt (or your loop.md)
deepseek-loop --loop

# Resume a prior session's tasks
deepseek-loop --resume <session-id> --loop
```

`loop.md` discovery: `.claude/loop.md` (project) → `~/.claude/loop.md` (user) → built-in default. Capped at 25 KB.

Cron grammar: 5-field `minute hour dom month dow` with `*`, `5`, `*/15`, `1-5`, `1,15,30`. Extended syntax (`L`, `W`, `?`, `MON`, `JAN`) is rejected.

Limits: 50 tasks per session, recurring tasks expire 7 days after creation, deterministic jitter is applied to fire times. Set `CLAUDE_CODE_DISABLE_CRON=1` (or the `DEEPSEEK_LOOP_DISABLE_CRON=1` alias) to disable the scheduler entirely.

Persistence: tasks are written to `${cache_dir}/deepseek-loop/sessions/<session_id>/tasks.json` after each mutation, restored via `--resume <session_id>`.

### Cache (feature `cache`)

```rust
pub struct Cache { .. }          // entry-level TTL, evicts expired on insert
pub struct InFlightDedup<T> { .. } // broadcast-based — first caller inserts, others subscribe
```

## Retry Logic

`reason_with_retry()` retries only on 5xx / network errors with exponential backoff (1s → 2s → 4s). 4xx errors fail immediately.

## Dependencies

Standalone library — no sibling crate dependencies. Used by `sdd`, `research`, and `genesis`.
