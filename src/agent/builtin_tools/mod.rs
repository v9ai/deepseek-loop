//! Built-in tools — same names and rough semantics as the Claude Agent SDK
//! built-ins: `Read`, `Write`, `Edit`, `Glob`, `Grep`, `Bash`.
//!
//! Each tool is gated behind the `builtin-tools` feature.

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

use super::tool::Tool;

/// Convenience: vector of all six built-ins, ready to pass to the loop.
pub fn default_tools() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(ReadTool),
        Box::new(WriteTool),
        Box::new(EditTool),
        Box::new(GlobTool),
        Box::new(GrepTool),
        Box::new(BashTool),
    ]
}
