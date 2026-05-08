//! `deepseek-loop` — a Claude-Code-shaped CLI for the DeepSeek agent loop.
//!
//! Streams the loop's `SdkMessage` events as NDJSON (one JSON object per line)
//! to stdout. Pipe into `jq` or any line-oriented consumer.
//!
//! With the `scheduler` feature (on by default), the binary exposes the four
//! cron-style tools — `CronCreate`, `CronList`, `CronDelete`, `Monitor` — and
//! supports three `/loop` invocation shapes:
//!
//! * `--loop-every 5m --loop-prompt 'check the deploy'` — fixed cron interval
//! * `--loop --loop-prompt 'check whether CI passed'`   — dynamic interval
//! * `--loop`                                            — built-in maintenance
//!   prompt (resolved from `.claude/loop.md`, `~/.claude/loop.md`, or the
//!   built-in default)

use std::io::Read;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::Utc;
use clap::{Parser, ValueEnum};
use deepseek::agent::builtin_tools::{default_tools, default_tools_with_scheduler};
use deepseek::agent::scheduler::{maintenance, CronExpr, Schedule, Scheduler, DEFAULT_MAX_TASKS};
use deepseek::types::EffortLevel;
use deepseek::ReqwestClient;
use deepseek::{PermissionMode, RunOptions};
use futures::StreamExt;

#[derive(Parser, Debug)]
#[command(
    name = "deepseek-loop",
    about = "DeepSeek agent loop — Claude-Code shape"
)]
struct Args {
    /// User prompt. If empty, read from stdin (unless --loop is set).
    #[arg(trailing_var_arg = true)]
    prompt: Vec<String>,

    /// Model id.
    #[arg(short, long, default_value = "deepseek-v4-pro")]
    model: String,

    /// Cap the number of tool-use turns.
    #[arg(long)]
    max_turns: Option<u32>,

    /// Cap total spend in USD across all iterations.
    #[arg(long)]
    max_budget_usd: Option<f64>,

    /// Reasoning effort: low | medium | high | max.
    #[arg(long, value_enum, default_value_t = CliEffort::High)]
    effort: CliEffort,

    /// Permission mode.
    #[arg(long, value_enum, default_value_t = CliPermissionMode::AcceptEdits)]
    permission_mode: CliPermissionMode,

    /// Comma-separated allow-list (default: all built-ins).
    #[arg(long, value_delimiter = ',')]
    allowed_tools: Option<Vec<String>>,

    /// Comma-separated deny-list.
    #[arg(long, value_delimiter = ',', default_value = "")]
    disallowed_tools: Vec<String>,

    /// Override the system prompt.
    #[arg(long)]
    system_prompt: Option<String>,

    /// Override the API base (e.g. for self-hosted).
    #[arg(long)]
    base_url: Option<String>,

    /// API key. Falls back to `$DEEPSEEK_API_KEY`.
    #[arg(long)]
    api_key: Option<String>,

    /// Run the loop in `/loop` mode. Combine with `--loop-every` for a fixed
    /// cron interval, with `--loop-prompt` for the prompt body, or alone for
    /// the built-in maintenance prompt at a dynamic interval.
    #[arg(long)]
    r#loop: bool,

    /// Cron-style interval like `5m`, `30s`, `1h`. When set, the loop runs at
    /// that cadence regardless of `--loop`.
    #[arg(long)]
    loop_every: Option<String>,

    /// Prompt body for the loop. Defaults to the trailing positional args, or
    /// the resolved `loop.md` if both are empty.
    #[arg(long)]
    loop_prompt: Option<String>,

    /// Cap the number of loop iterations (defaults to unlimited).
    #[arg(long)]
    loop_max_iterations: Option<u32>,

    /// Resume scheduler state from a prior session id.
    #[arg(long)]
    resume: Option<String>,

    /// Cap on concurrent scheduled tasks (default 50).
    #[arg(long, default_value_t = DEFAULT_MAX_TASKS)]
    max_tasks: usize,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum CliEffort {
    Low,
    Medium,
    High,
    Max,
}

impl From<CliEffort> for EffortLevel {
    fn from(v: CliEffort) -> Self {
        match v {
            CliEffort::Low => EffortLevel::Low,
            CliEffort::Medium => EffortLevel::Medium,
            CliEffort::High => EffortLevel::High,
            CliEffort::Max => EffortLevel::Max,
        }
    }
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum CliPermissionMode {
    Default,
    AcceptEdits,
    Plan,
    DontAsk,
    BypassPermissions,
}

impl From<CliPermissionMode> for PermissionMode {
    fn from(v: CliPermissionMode) -> Self {
        match v {
            CliPermissionMode::Default => PermissionMode::Default,
            CliPermissionMode::AcceptEdits => PermissionMode::AcceptEdits,
            CliPermissionMode::Plan => PermissionMode::Plan,
            CliPermissionMode::DontAsk => PermissionMode::DontAsk,
            CliPermissionMode::BypassPermissions => PermissionMode::BypassPermissions,
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load `.env` first, then `.env.local` (Next.js / Vite convention — local
    // overrides shared). `dotenvy::from_filename` skips missing files
    // silently. Without this, a `LANGGRAPH_AUTH_TOKEN` (or any secret) that
    // lives only in `.env.local` would never reach a Bash subprocess the agent
    // spawns, even though `.env` got picked up — a confusing partial-load
    // failure mode we hit in practice.
    let _ = dotenvy::from_filename(".env");
    let _ = dotenvy::from_filename(".env.local").or_else(|_| dotenvy::dotenv());
    // `from_filename` does not override existing vars, so `.env.local` only
    // adds keys that `.env` didn't already set. To honor the convention that
    // `.env.local` overrides `.env`, we re-read `.env.local` with `override_*`.
    let _ = dotenvy::from_filename_override(".env.local");
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,deepseek=info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let api_key = args
        .api_key
        .clone()
        .or_else(|| std::env::var("DEEPSEEK_API_KEY").ok())
        .ok_or_else(|| anyhow::anyhow!("DEEPSEEK_API_KEY not set and --api-key not provided"))?;

    // Build the scheduler; restore prior tasks if --resume was passed.
    let session_id = args
        .resume
        .clone()
        .unwrap_or_else(|| format!("cli-{}", uuid_short()));
    let mut scheduler = if args.resume.is_some() {
        Scheduler::restore(&session_id)
    } else {
        Scheduler::with_cap(&session_id, args.max_tasks)
    };
    // If the env-var is set or the user requested a smaller cap, honor it.
    if args.max_tasks != DEFAULT_MAX_TASKS && !scheduler.is_disabled() {
        let mut s = Scheduler::with_cap(&session_id, args.max_tasks);
        // Move tasks across.
        for t in scheduler.list().into_iter().cloned().collect::<Vec<_>>() {
            // Direct insert: respects cap implicitly via re-create-on-restore.
            let _ = s.create(t.schedule.clone(), t.prompt.clone(), t.recurring);
        }
        scheduler = s;
    }
    let scheduler = Arc::new(Mutex::new(scheduler));

    let opts = build_opts(&args);
    let http = ReqwestClient::new();

    let tools_arc: Arc<Vec<Box<dyn deepseek::Tool>>> = if scheduler.lock().unwrap().is_disabled() {
        Arc::new(default_tools())
    } else {
        Arc::new(default_tools_with_scheduler(scheduler.clone()))
    };

    // Fixed-interval loop: --loop-every implies --loop.
    if let Some(interval) = args.loop_every.as_deref() {
        let cron_expr = interval_to_cron(interval)?;
        eprintln!(
            "/loop interval='{interval}' resolved to cron='{cron}' (local timezone)",
            cron = cron_expr.as_str()
        );
        let prompt = resolve_loop_prompt(&args)?;
        let cap = args.loop_max_iterations;
        return run_fixed_interval_loop(
            scheduler, cron_expr, prompt, tools_arc, http, api_key, opts, cap,
        )
        .await;
    }

    // Dynamic / maintenance loop.
    if args.r#loop {
        let prompt = resolve_loop_prompt(&args)?;
        let cap = args.loop_max_iterations;
        return run_dynamic_loop(prompt, tools_arc, http, api_key, opts, cap).await;
    }

    // One-shot: stream a single agent run end-to-end.
    let prompt = if args.prompt.is_empty() {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        buf.trim().to_string()
    } else {
        args.prompt.join(" ")
    };
    if prompt.is_empty() {
        anyhow::bail!("empty prompt; pass it as positional args or via stdin");
    }
    stream_one_run(prompt, tools_arc, http, api_key, opts).await
}

fn build_opts(args: &Args) -> RunOptions {
    let mut opts = RunOptions::new(&args.model)
        .effort(args.effort.into())
        .permission_mode(args.permission_mode.into());
    if let Some(n) = args.max_turns {
        opts = opts.max_turns(n);
    }
    if let Some(b) = args.max_budget_usd {
        opts = opts.max_budget_usd(b);
    }
    if let Some(allowed) = args.allowed_tools.clone() {
        opts = opts.allowed_tools(allowed);
    }
    if !args.disallowed_tools.is_empty() {
        opts = opts.disallowed_tools(args.disallowed_tools.clone());
    }
    if let Some(sp) = args.system_prompt.clone() {
        opts = opts.system_prompt(sp);
    } else {
        opts = opts.system_prompt(default_system_prompt());
    }
    if let Some(b) = args.base_url.clone() {
        opts = opts.base_url(b);
    }
    opts
}

fn default_system_prompt() -> String {
    "You are a coding agent running in a developer's terminal. Use the available tools \
     (Read, Write, Edit, Glob, Grep, Bash, CronCreate, CronList, CronDelete, Monitor) \
     to inspect, modify, and schedule work in the working directory. Be concise; \
     explain only when asked."
        .into()
}

fn resolve_loop_prompt(args: &Args) -> anyhow::Result<String> {
    if let Some(p) = args.loop_prompt.clone() {
        return Ok(p);
    }
    if !args.prompt.is_empty() {
        return Ok(args.prompt.join(" "));
    }
    Ok(maintenance::resolve_prompt())
}

/// Convert `5m`, `30s`, `1h`, `1d` into the closest 5-field cron expression.
fn interval_to_cron(input: &str) -> anyhow::Result<CronExpr> {
    let s = input.trim();
    if s.is_empty() {
        anyhow::bail!("--loop-every: empty interval");
    }
    let (num_part, unit) = s.split_at(s.len().saturating_sub(1));
    let n: u64 = num_part
        .parse()
        .map_err(|e| anyhow::anyhow!("--loop-every: invalid number '{num_part}': {e}"))?;
    if n == 0 {
        anyhow::bail!("--loop-every: interval must be > 0");
    }
    let cron = match unit {
        "s" => {
            // Cron has 1-minute granularity; round up to 1m for sub-minute.
            tracing::warn!(
                "--loop-every: sub-minute interval {n}s rounded up to 1 minute (cron granularity)"
            );
            "*/1 * * * *".to_string()
        }
        "m" => {
            if n > 60 {
                anyhow::bail!("--loop-every: minute intervals must be ≤ 60");
            }
            if n == 60 {
                // 60m == 1h; `*/60` is rejected by cron (minutes cap at 59).
                "0 */1 * * *".to_string()
            } else {
                // Round to a clean cron step where possible.
                let step = pick_cron_step(n, 60);
                format!("*/{step} * * * *")
            }
        }
        "h" => {
            if n > 24 {
                anyhow::bail!("--loop-every: hour intervals must be ≤ 24");
            }
            let step = pick_cron_step(n, 24);
            format!("0 */{step} * * *")
        }
        "d" => {
            format!("0 0 */{n} * *")
        }
        other => anyhow::bail!("--loop-every: unsupported unit '{other}'; use s|m|h|d"),
    };
    CronExpr::parse(&cron).map_err(|e| anyhow::anyhow!("--loop-every: {e}"))
}

/// Round `n` to the nearest divisor of `base` so the cron step is clean.
fn pick_cron_step(n: u64, base: u64) -> u64 {
    let mut best = n;
    let mut best_diff = u64::MAX;
    for d in 1..=base {
        if base.is_multiple_of(d) {
            let diff = d.abs_diff(n);
            if diff < best_diff {
                best_diff = diff;
                best = d;
            }
        }
    }
    best
}

#[allow(clippy::too_many_arguments)] // private bin helper, fine
async fn run_fixed_interval_loop(
    scheduler: Arc<Mutex<Scheduler>>,
    cron: CronExpr,
    prompt: String,
    tools: Arc<Vec<Box<dyn deepseek::Tool>>>,
    http: ReqwestClient,
    api_key: String,
    opts: RunOptions,
    max_iterations: Option<u32>,
) -> anyhow::Result<()> {
    // Register the loop's recurring task so it shows up in CronList.
    let task_id = {
        let mut s = scheduler.lock().unwrap();
        s.create(Schedule::Cron(Box::new(cron)), prompt.clone(), true)
            .map_err(|e| anyhow::anyhow!("scheduler: {e}"))?
    };
    eprintln!(
        "/loop registered task_id={} cron='{cron}' prompt={prompt:?}",
        task_id.as_str(),
        cron = scheduler
            .lock()
            .unwrap()
            .list()
            .iter()
            .find(|t| t.id == task_id)
            .and_then(|t| match &t.schedule {
                deepseek::agent::scheduler::Schedule::Cron(c) => Some(c.as_str().to_string()),
                _ => None,
            })
            .unwrap_or_else(|| "?".to_string()),
    );

    let mut iter_count: u32 = 0;
    loop {
        let due = {
            let mut s = scheduler.lock().unwrap();
            s.tick(Utc::now())
        };
        for fire in due {
            stream_one_run(
                fire.prompt.clone(),
                tools.clone(),
                http.clone(),
                api_key.clone(),
                opts.clone(),
            )
            .await?;
            iter_count += 1;
            if let Some(cap) = max_iterations {
                if iter_count >= cap {
                    return Ok(());
                }
            }
        }
        // Sleep 1s between ticks; matches Claude Code's scheduler granularity.
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn run_dynamic_loop(
    prompt: String,
    tools: Arc<Vec<Box<dyn deepseek::Tool>>>,
    http: ReqwestClient,
    api_key: String,
    opts: RunOptions,
    max_iterations: Option<u32>,
) -> anyhow::Result<()> {
    eprintln!(
        "/loop dynamic mode — prompt={prompt:?} (current build: fixed 60s floor between \
         iterations; the spec's 60-3600s adaptive delay lands in v0.4)"
    );
    let mut iter_count: u32 = 0;
    loop {
        stream_one_run(
            prompt.clone(),
            tools.clone(),
            http.clone(),
            api_key.clone(),
            opts.clone(),
        )
        .await?;
        iter_count += 1;
        if let Some(cap) = max_iterations {
            if iter_count >= cap {
                return Ok(());
            }
        }
        // 60-second floor between iterations (matches Claude Code's dynamic
        // delay floor). Future work: have the agent emit a chosen-delay
        // signal between iterations to actually vary the wait.
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}

async fn stream_one_run(
    prompt: String,
    tools: Arc<Vec<Box<dyn deepseek::Tool>>>,
    http: ReqwestClient,
    api_key: String,
    opts: RunOptions,
) -> anyhow::Result<()> {
    let mut stream = Box::pin(deepseek::run(http, api_key, tools, prompt, opts));
    while let Some(msg) = stream.next().await {
        println!("{}", serde_json::to_string(&msg)?);
        if msg.is_terminal() {
            break;
        }
    }
    Ok(())
}

fn uuid_short() -> String {
    // Avoid pulling uuid in two places; the lib already re-exports it via
    // the agent module. Keep this as a tiny helper.
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos:x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- pick_cron_step ----

    #[test]
    fn pick_cron_step_exact_divisor_returned_unchanged() {
        // 5 already divides 60 → no rounding.
        assert_eq!(pick_cron_step(5, 60), 5);
    }

    #[test]
    fn pick_cron_step_rounds_to_nearest_divisor() {
        // 7 isn't a divisor of 60; the closest divisors are 6 and 10. The
        // implementation walks 1..=base and keeps the *first* minimum, so
        // it returns 6.
        assert_eq!(pick_cron_step(7, 60), 6);
    }

    #[test]
    fn pick_cron_step_handles_one() {
        assert_eq!(pick_cron_step(1, 60), 1);
        assert_eq!(pick_cron_step(1, 24), 1);
    }

    #[test]
    fn pick_cron_step_handles_value_equal_to_base() {
        assert_eq!(pick_cron_step(60, 60), 60);
        assert_eq!(pick_cron_step(24, 24), 24);
    }

    // ---- interval_to_cron ----

    #[test]
    fn interval_to_cron_5m() {
        let c = interval_to_cron("5m").unwrap();
        assert_eq!(c.as_str(), "*/5 * * * *");
    }

    #[test]
    fn interval_to_cron_30m() {
        let c = interval_to_cron("30m").unwrap();
        assert_eq!(c.as_str(), "*/30 * * * *");
    }

    #[test]
    fn interval_to_cron_60m_maps_to_hourly() {
        // 60m == 1h; `*/60` is invalid cron (minutes cap at 59), so the
        // function maps it to the hourly expression instead.
        let c = interval_to_cron("60m").unwrap();
        assert_eq!(c.as_str(), "0 */1 * * *");
    }

    #[test]
    fn interval_to_cron_7m_rounds_to_nearest_clean_step() {
        // 7m has no clean cron step; pick_cron_step rounds to the closest
        // divisor of 60. That's 6 (6 vs 10, 6 is the first minimum).
        let c = interval_to_cron("7m").unwrap();
        assert_eq!(c.as_str(), "*/6 * * * *");
    }

    #[test]
    fn interval_to_cron_1h() {
        let c = interval_to_cron("1h").unwrap();
        assert_eq!(c.as_str(), "0 */1 * * *");
    }

    #[test]
    fn interval_to_cron_12h() {
        let c = interval_to_cron("12h").unwrap();
        assert_eq!(c.as_str(), "0 */12 * * *");
    }

    #[test]
    fn interval_to_cron_1d() {
        let c = interval_to_cron("1d").unwrap();
        assert_eq!(c.as_str(), "0 0 */1 * *");
    }

    #[test]
    fn interval_to_cron_30s_rounds_up_to_one_minute() {
        // Cron has 1-minute granularity, so any sub-minute interval rounds up
        // to `*/1 * * * *`.
        let c = interval_to_cron("30s").unwrap();
        assert_eq!(c.as_str(), "*/1 * * * *");
    }

    #[test]
    fn interval_to_cron_with_whitespace() {
        // Leading/trailing whitespace is trimmed.
        let c = interval_to_cron("  5m  ").unwrap();
        assert_eq!(c.as_str(), "*/5 * * * *");
    }

    #[test]
    fn interval_to_cron_rejects_empty() {
        let err = interval_to_cron("").unwrap_err().to_string();
        assert!(err.contains("empty"), "got: {err}");
    }

    #[test]
    fn interval_to_cron_rejects_zero() {
        let err = interval_to_cron("0m").unwrap_err().to_string();
        assert!(err.contains("> 0"), "got: {err}");
    }

    #[test]
    fn interval_to_cron_rejects_too_many_minutes() {
        let err = interval_to_cron("61m").unwrap_err().to_string();
        assert!(err.contains("≤ 60"), "got: {err}");
    }

    #[test]
    fn interval_to_cron_rejects_too_many_hours() {
        let err = interval_to_cron("25h").unwrap_err().to_string();
        assert!(err.contains("≤ 24"), "got: {err}");
    }

    #[test]
    fn interval_to_cron_rejects_unknown_unit() {
        let err = interval_to_cron("5x").unwrap_err().to_string();
        assert!(err.contains("unsupported unit"), "got: {err}");
    }

    #[test]
    fn interval_to_cron_rejects_non_numeric_prefix() {
        let err = interval_to_cron("abm").unwrap_err().to_string();
        assert!(err.contains("invalid number"), "got: {err}");
    }

    #[test]
    fn interval_to_cron_rejects_unit_only() {
        // No leading number — `s` alone parses the empty-prefix as a number,
        // which fails with `invalid number`.
        let err = interval_to_cron("m").unwrap_err().to_string();
        assert!(err.contains("invalid number"), "got: {err}");
    }
}
