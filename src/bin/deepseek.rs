//! `deepseek` — a tiny Claude-Code-shaped CLI for the DeepSeek agent loop.
//!
//! Run a single agent turn with the six built-in tools and stream the loop's
//! `SdkMessage` events to stdout. Pretty mode by default; `--json` emits NDJSON
//! suitable for piping into other tools.

use std::io::Read;
use std::sync::Arc;

use clap::{Parser, ValueEnum};
use deepseek::agent::builtin_tools::default_tools;
use deepseek::{
    ContentBlock, PermissionMode, ResultSubtype, RunOptions, SdkMessage, SystemSubtype,
};
use deepseek::{ReqwestClient};
use deepseek::types::EffortLevel;
use futures::StreamExt;

#[derive(Parser, Debug)]
#[command(name = "deepseek", about = "DeepSeek agent loop — Claude-Code shape")]
struct Args {
    /// User prompt. If empty, read from stdin.
    #[arg(trailing_var_arg = true)]
    prompt: Vec<String>,

    /// Model id.
    #[arg(short, long, default_value = "deepseek-v4-pro")]
    model: String,

    /// Cap the number of tool-use turns.
    #[arg(long)]
    max_turns: Option<u32>,

    /// Cap total spend in USD.
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

    /// Emit NDJSON (one `SdkMessage` per line) instead of pretty output.
    #[arg(long)]
    json: bool,

    /// API key. Falls back to `$DEEPSEEK_API_KEY`.
    #[arg(long)]
    api_key: Option<String>,
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
    let _ = dotenvy::dotenv();
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
        .ok_or_else(|| {
            anyhow::anyhow!("DEEPSEEK_API_KEY not set and --api-key not provided")
        })?;

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

    let mut opts = RunOptions::new(&args.model)
        .effort(args.effort.into())
        .permission_mode(args.permission_mode.into());
    if let Some(n) = args.max_turns {
        opts = opts.max_turns(n);
    }
    if let Some(b) = args.max_budget_usd {
        opts = opts.max_budget_usd(b);
    }
    if let Some(allowed) = args.allowed_tools {
        opts = opts.allowed_tools(allowed);
    }
    if !args.disallowed_tools.is_empty() {
        opts = opts.disallowed_tools(args.disallowed_tools);
    }
    if let Some(sp) = args.system_prompt {
        opts = opts.system_prompt(sp);
    } else {
        opts = opts.system_prompt(default_system_prompt());
    }
    if let Some(b) = args.base_url {
        opts = opts.base_url(b);
    }

    let http = ReqwestClient::new();
    let tools = Arc::new(default_tools());

    let mut stream =
        Box::pin(deepseek::run(http, api_key, tools, prompt, opts));

    while let Some(msg) = stream.next().await {
        if args.json {
            println!("{}", serde_json::to_string(&msg)?);
        } else {
            print_pretty(&msg);
        }
        if msg.is_terminal() {
            break;
        }
    }
    Ok(())
}

fn default_system_prompt() -> String {
    "You are a coding agent running in a developer's terminal. Use the available tools \
     (Read, Write, Edit, Glob, Grep, Bash) to inspect and modify the working directory. \
     Be concise; explain only when asked."
        .into()
}

fn print_pretty(msg: &SdkMessage) {
    match msg {
        SdkMessage::System {
            subtype: SystemSubtype::Init,
            session_id,
            ..
        } => {
            eprintln!("● session {session_id}");
        }
        SdkMessage::Assistant { content, .. } => {
            for block in content {
                match block {
                    ContentBlock::Text { text } if !text.is_empty() => {
                        println!("{text}");
                    }
                    ContentBlock::ToolUse { name, input, .. } => {
                        let preview = serde_json::to_string(input).unwrap_or_default();
                        let preview = if preview.len() > 120 {
                            format!("{}…", &preview[..120])
                        } else {
                            preview
                        };
                        eprintln!("▸ {name} {preview}");
                    }
                    _ => {}
                }
            }
        }
        SdkMessage::User { content } => {
            for block in content {
                if let ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } = block
                {
                    let head = content.lines().next().unwrap_or("");
                    let marker = if *is_error { "✗" } else { "✓" };
                    eprintln!("  {marker} {tool_use_id}: {head}");
                }
            }
        }
        SdkMessage::Result {
            subtype,
            num_turns,
            total_cost_usd,
            ..
        } => {
            let cost = total_cost_usd
                .map(|c| format!("${c:.4}"))
                .unwrap_or_else(|| "n/a".into());
            let label = match subtype {
                ResultSubtype::Success => "✓ success",
                ResultSubtype::ErrorMaxTurns => "✗ max_turns",
                ResultSubtype::ErrorMaxBudgetUsd => "✗ max_budget_usd",
                ResultSubtype::ErrorDuringExecution => "✗ runtime_error",
            };
            eprintln!("{label} | turns={num_turns} | cost={cost}");
        }
    }
}
