pub mod client;
pub mod error;
pub mod types;

#[cfg(feature = "reqwest-client")]
pub mod reqwest_client;

#[cfg(feature = "wasm")]
pub mod wasm;

#[cfg(feature = "agent")]
pub mod agent;

#[cfg(feature = "cache")]
pub mod cache;

// Re-export key types
pub use client::{build_request, DeepSeekClient, HttpClient, DEFAULT_BASE_URL};
pub use error::{DeepSeekError, Result};
pub use types::*;

#[cfg(feature = "reqwest-client")]
pub use reqwest_client::{
    client_from_env, reason, reason_with_retry, reason_with_tokens, reason_with_tokens_at,
    ReqwestClient,
};

#[cfg(feature = "wasm")]
pub use wasm::WasmClient;

#[cfg(feature = "agent")]
pub use agent::{
    run, AgentBuilder, CompactionConfig, ContentBlock, ConversationMemory, DeepSeekAgent, Embedder,
    MemoryError, MemoryHit, MemoryRecord, PermissionDecision, PermissionMode, PreToolHook,
    ResultSubtype, RunOptions, SdkMessage, SystemSubtype, Tool, ToolDefinition,
};

#[cfg(feature = "memory")]
pub use agent::{FastEmbedEmbedder, LanceMemory};

#[cfg(feature = "cache")]
pub use cache::{Cache, InFlightDedup};
