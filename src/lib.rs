pub mod error;
pub mod types;
pub mod client;

#[cfg(feature = "reqwest-client")]
pub mod reqwest_client;

#[cfg(feature = "wasm")]
pub mod wasm;

#[cfg(feature = "agent")]
pub mod agent;

#[cfg(feature = "cache")]
pub mod cache;

// Re-export key types
pub use error::{DeepSeekError, Result};
pub use types::*;
pub use client::{HttpClient, DeepSeekClient, DEFAULT_BASE_URL, build_request};

#[cfg(feature = "reqwest-client")]
pub use reqwest_client::{ReqwestClient, client_from_env, reason, reason_with_retry};

#[cfg(feature = "wasm")]
pub use wasm::WasmClient;

#[cfg(feature = "agent")]
pub use agent::{
    AgentBuilder, ContentBlock, DeepSeekAgent, PermissionDecision, PermissionMode, PreToolHook,
    ResultSubtype, RunOptions, SdkMessage, SystemSubtype, Tool, ToolDefinition, run,
};

#[cfg(feature = "cache")]
pub use cache::{Cache, InFlightDedup};
