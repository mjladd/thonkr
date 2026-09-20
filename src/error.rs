//! One error type for the whole crate.

use std::path::Path;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    /// A file that is not the format its name or header claims.
    #[error("{path}: {reason}")]
    Format { path: String, reason: String },

    #[error("unknown score: {name}\navailable scores: {available}")]
    UnknownScore { name: String, available: String },

    #[error("bad --set {arg}\n  {reason}")]
    BadSet { arg: String, reason: String },

    /// Every validation failure for one score, reported together.
    #[error("score \"{name}\" is not usable:\n{}", .problems.iter()
        .map(|p| format!("  {p}")).collect::<Vec<_>>().join("\n"))]
    Invalid { name: String, problems: Vec<String> },

    #[error("{0}")]
    Other(String),
}

impl Error {
    pub fn io(path: impl AsRef<Path>, source: std::io::Error) -> Self {
        Error::Io {
            path: path.as_ref().display().to_string(),
            source,
        }
    }

    pub fn format(path: impl AsRef<Path>, reason: impl Into<String>) -> Self {
        Error::Format {
            path: path.as_ref().display().to_string(),
            reason: reason.into(),
        }
    }

    pub fn other(reason: impl Into<String>) -> Self {
        Error::Other(reason.into())
    }
}
