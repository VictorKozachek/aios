//! Memory-layer errors with user-facing messages.

use std::fmt;

/// Something went wrong in the memory layer.
#[derive(Debug)]
pub enum MemoryError {
    /// Filesystem problem while reading or writing memory files.
    Io {
        /// What was being done.
        context: String,
        /// Underlying error.
        source: std::io::Error,
    },
    /// A JSON payload cannot be (de)serialized.
    Json {
        /// What was being parsed.
        context: String,
        /// Parser detail.
        detail: String,
    },
    /// The segment format layer refused an operation.
    Segment {
        /// What was being written or read.
        context: String,
        /// Format error.
        detail: String,
    },
    /// Memory state is inconsistent (bad fact id, broken index, ...).
    Corrupt {
        /// What was being checked.
        context: String,
        /// Detail.
        detail: String,
    },
}

impl MemoryError {
    /// Wraps an I/O error with the action that failed.
    pub fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        MemoryError::Io {
            context: context.into(),
            source,
        }
    }

    /// Wraps a JSON error with the action that failed.
    pub fn json(context: impl Into<String>, source: serde_json::Error) -> Self {
        MemoryError::Json {
            context: context.into(),
            detail: source.to_string(),
        }
    }

    /// Wraps a segment-format error with the action that failed.
    pub fn segment(context: impl Into<String>, error: memfmt::Error) -> Self {
        MemoryError::Segment {
            context: context.into(),
            detail: error.to_string(),
        }
    }

    /// Reports inconsistent memory state.
    pub fn corrupt(context: impl Into<String>, detail: impl Into<String>) -> Self {
        MemoryError::Corrupt {
            context: context.into(),
            detail: detail.into(),
        }
    }
}

impl fmt::Display for MemoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MemoryError::Io { context, source } => write!(f, "{context}: {source}"),
            MemoryError::Json { context, detail } => {
                write!(f, "{context}: некорректный JSON: {detail}")
            }
            MemoryError::Segment { context, detail } => {
                write!(f, "{context}: ошибка формата сегмента: {detail}")
            }
            MemoryError::Corrupt { context, detail } => {
                write!(f, "{context}: повреждённое состояние памяти: {detail}")
            }
        }
    }
}

impl std::error::Error for MemoryError {}

impl From<std::io::Error> for MemoryError {
    fn from(source: std::io::Error) -> Self {
        MemoryError::io("операция с файлом памяти", source)
    }
}
