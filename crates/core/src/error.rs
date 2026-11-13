//! Errors returned by conversion engines and the planner.

use crate::format::{Format, FormatParseError};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConvertError {
    #[error("no conversion route from {from} to {to}: {detail}")]
    NoRoute {
        from: Format,
        to: Format,
        detail: String,
    },
    #[error("conversion from {from} to {to} is not supported by the installed engines")]
    Unsupported { from: Format, to: Format },
    #[error("invalid conversion input: {0}")]
    InvalidInput(String),
    #[error("the input is not a valid {format} file: {detail}")]
    InvalidFormat { format: Format, detail: String },
    #[error("conversion failed in engine '{engine}': {message}")]
    Engine { engine: String, message: String },
    #[error("external tool '{tool}' is not installed or not on PATH")]
    ToolMissing { tool: String },
    #[error("conversion timed out after {seconds} seconds")]
    Timeout { seconds: u64 },
    #[error("conversion was cancelled")]
    Cancelled,
    #[error("file is too large: {actual} bytes (limit {limit} bytes)")]
    TooLarge { actual: u64, limit: u64 },
    #[error("unsafe or invalid file name: {0}")]
    InvalidFileName(String),
    #[error("the conversion produced no output")]
    MissingOutput,
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("format error: {0}")]
    Format(#[from] FormatParseError),
    #[error("corrupt or unreadable archive: {0}")]
    Archive(String),
    #[error("serialization error: {0}")]
    Serialization(String),
    #[error("internal error: {0}")]
    Internal(String),
}

impl ConvertError {
    /// Stable machine-readable code used by the REST error envelope.
    pub fn code(&self) -> &'static str {
        match self {
            ConvertError::NoRoute { .. } => "no_route",
            ConvertError::Unsupported { .. } => "unsupported_conversion",
            ConvertError::InvalidInput(_) => "invalid_input",
            ConvertError::InvalidFormat { .. } => "invalid_format",
            ConvertError::Engine { .. } => "engine_failed",
            ConvertError::ToolMissing { .. } => "tool_missing",
            ConvertError::Timeout { .. } => "timeout",
            ConvertError::Cancelled => "cancelled",
            ConvertError::TooLarge { .. } => "too_large",
            ConvertError::InvalidFileName(_) => "invalid_filename",
            ConvertError::MissingOutput => "missing_output",
            ConvertError::Io(_) => "io_error",
            ConvertError::Format(_) => "invalid_format",
            ConvertError::Archive(_) => "archive_error",
            ConvertError::Serialization(_) => "serialization_error",
            ConvertError::Internal(_) => "internal_error",
        }
    }

    /// True when the caller can fix the problem by changing the request.
    pub fn is_client_error(&self) -> bool {
        matches!(
            self,
            ConvertError::NoRoute { .. }
                | ConvertError::Unsupported { .. }
                | ConvertError::InvalidInput(_)
                | ConvertError::InvalidFormat { .. }
                | ConvertError::TooLarge { .. }
                | ConvertError::InvalidFileName(_)
                | ConvertError::Format(_)
                | ConvertError::Archive(_)
                | ConvertError::Serialization(_)
        )
    }

    pub fn engine(engine: impl Into<String>, error: impl std::fmt::Display) -> Self {
        ConvertError::Engine {
            engine: engine.into(),
            message: error.to_string(),
        }
    }
}

pub type Result<T> = std::result::Result<T, ConvertError>;
