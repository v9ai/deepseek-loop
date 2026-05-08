use thiserror::Error;

#[derive(Debug, Error)]
pub enum DeepSeekError {
    #[error("API error ({status}): {body}")]
    Api { status: u16, body: String },

    #[error("no choices in response")]
    EmptyResponse,

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("{0}")]
    Other(String),

    #[cfg(feature = "reqwest-client")]
    #[error("HTTP error: {0}")]
    Reqwest(#[from] reqwest::Error),
}

pub type Result<T> = std::result::Result<T, DeepSeekError>;
