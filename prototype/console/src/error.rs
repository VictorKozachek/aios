//! Console errors with user-facing messages.

use std::fmt;
use std::path::PathBuf;

use aios_modeld::ModelError;

/// Something went wrong in the console layer.
#[derive(Debug)]
pub enum ConsoleError {
    /// Filesystem problem while touching workspaces or logs.
    Io {
        /// What was being done.
        context: String,
        /// Underlying error.
        source: std::io::Error,
    },
    /// A `config.toml` of a workspace cannot be parsed.
    Config {
        /// Path of the broken file.
        path: PathBuf,
        /// Parser detail.
        detail: String,
    },
    /// The model layer failed.
    Model(ModelError),
    /// `HOME` is not set, so `~/.aios` cannot be located.
    NoHome,
}

impl ConsoleError {
    /// Wraps an I/O error with the action that failed.
    pub fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        ConsoleError::Io {
            context: context.into(),
            source,
        }
    }
}

impl fmt::Display for ConsoleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConsoleError::Io { context, source } => write!(f, "{context}: {source}"),
            ConsoleError::Config { path, detail } => write!(
                f,
                "не читается конфиг {}: {detail}. Исправьте файл или удалите его — \
                 он будет создан заново",
                path.display()
            ),
            ConsoleError::Model(error) => write!(f, "{error}"),
            ConsoleError::NoHome => write!(
                f,
                "переменная HOME не задана: не могу найти ~/.aios \
                 (можно задать AIOS_HOME)"
            ),
        }
    }
}

impl std::error::Error for ConsoleError {}

impl From<std::io::Error> for ConsoleError {
    fn from(source: std::io::Error) -> Self {
        ConsoleError::Io {
            context: "ошибка ввода-вывода".to_string(),
            source,
        }
    }
}

impl From<ModelError> for ConsoleError {
    fn from(error: ModelError) -> Self {
        ConsoleError::Model(error)
    }
}
