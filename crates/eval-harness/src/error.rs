use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum EvalError {
    #[error("invalid evaluation configuration: {0}")]
    InvalidConfig(String),

    #[error("evaluation input is invalid: {0}")]
    InvalidInput(String),

    #[error("evaluation I/O failed for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("evaluation artifact serialization failed: {0}")]
    Artifact(#[from] serde_json::Error),

    #[error("evaluation suite failed: {0}")]
    Suite(String),
}
