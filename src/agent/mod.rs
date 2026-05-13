//! Claude-Code-shaped agent loop for DeepSeek.
//!
//! See <https://code.claude.com/docs/en/agent-sdk/agent-loop>. The crate-level
//! re-exports give you:
//!
//! - [`run`] — the core function that returns a stream of [`SdkMessage`].
//! - [`AgentBuilder`] / [`DeepSeekAgent`] — back-compat builder API.
//! - [`Tool`] / [`ToolDefinition`] — extend with custom tools.
//! - [`RunOptions`], [`PermissionMode`], [`PreToolHook`] — control the loop.
//!
//! With the `builtin-tools` feature enabled, [`builtin_tools::default_tools`]
//! returns Read / Write / Edit / Glob / Grep / Bash ready to plug in.

pub mod builder;
pub mod loop_runner;
pub mod messages;
pub mod options;
pub mod permissions;
pub mod pricing;
pub mod tool;

#[cfg(feature = "builtin-tools")]
pub mod builtin_tools;

#[cfg(feature = "scheduler")]
pub mod scheduler;

pub use builder::{AgentBuilder, DeepSeekAgent};
pub use loop_runner::run;
pub use messages::{ContentBlock, ResultSubtype, SdkMessage, SystemSubtype};
pub use options::{CompactionConfig, RunOptions};
pub use permissions::{PermissionDecision, PermissionMode, PreToolHook};
pub use tool::{Tool, ToolDefinition};
