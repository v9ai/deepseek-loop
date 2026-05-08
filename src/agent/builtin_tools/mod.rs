//! Built-in tools — same names and rough semantics as the Claude Agent SDK
//! built-ins. The `builtin-tools` feature gives you Read/Write/Edit/Glob/
//! Grep/Bash. The `scheduler` feature adds CronCreate/CronList/CronDelete/
//! Monitor.

pub mod bash;
pub mod edit;
pub mod glob;
pub mod grep;
pub mod read;
pub mod write;

pub use bash::BashTool;
pub use edit::EditTool;
pub use glob::GlobTool;
pub use grep::GrepTool;
pub use read::ReadTool;
pub use write::WriteTool;

#[cfg(feature = "scheduler")]
pub mod cron_create;
#[cfg(feature = "scheduler")]
pub mod cron_delete;
#[cfg(feature = "scheduler")]
pub mod cron_list;
#[cfg(feature = "scheduler")]
pub mod monitor;

#[cfg(feature = "scheduler")]
pub use cron_create::CronCreateTool;
#[cfg(feature = "scheduler")]
pub use cron_delete::CronDeleteTool;
#[cfg(feature = "scheduler")]
pub use cron_list::CronListTool;
#[cfg(feature = "scheduler")]
pub use monitor::MonitorTool;

use super::tool::Tool;

/// Convenience: vector of all built-ins that don't need shared state.
///
/// With just `builtin-tools`: Read/Write/Edit/Glob/Grep/Bash.
/// With `scheduler` also enabled: adds Monitor (the cron tools need a
/// scheduler — call [`default_tools_with_scheduler`] for the full set).
pub fn default_tools() -> Vec<Box<dyn Tool>> {
    let mut v: Vec<Box<dyn Tool>> = vec![
        Box::new(ReadTool),
        Box::new(WriteTool),
        Box::new(EditTool),
        Box::new(GlobTool),
        Box::new(GrepTool),
        Box::new(BashTool),
    ];
    #[cfg(feature = "scheduler")]
    v.push(Box::new(MonitorTool));
    v
}

/// All ten built-in tools wired against the supplied scheduler. Available
/// only with the `scheduler` feature.
#[cfg(feature = "scheduler")]
pub fn default_tools_with_scheduler(
    scheduler: std::sync::Arc<std::sync::Mutex<crate::agent::scheduler::Scheduler>>,
) -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(ReadTool),
        Box::new(WriteTool),
        Box::new(EditTool),
        Box::new(GlobTool),
        Box::new(GrepTool),
        Box::new(BashTool),
        Box::new(MonitorTool),
        Box::new(CronCreateTool::new(scheduler.clone())),
        Box::new(CronListTool::new(scheduler.clone())),
        Box::new(CronDeleteTool::new(scheduler)),
    ]
}
