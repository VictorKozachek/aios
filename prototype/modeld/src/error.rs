//! Model-layer errors with user-facing messages.
//!
//! A fresh prototype machine usually has no server and no model, so these two
//! cases are first-class errors with an explicit fix in the message, not an
//! `unwrap` deep inside the HTTP client.

use std::fmt;

/// Something went wrong between the console and the model.
#[derive(Debug)]
pub enum ModelError {
    /// TCP/TLS/HTTP transport failed — usually `ollama serve` is not running.
    ServerUnavailable {
        /// Base URL that was dialled.
        url: String,
        /// Transport detail from the client.
        detail: String,
    },
    /// The server is up but the model is not downloaded.
    ModelNotFound {
        /// Requested model tag.
        model: String,
        /// What the server said.
        detail: String,
    },
    /// The server answered with a non-2xx status.
    HttpStatus {
        /// HTTP status code.
        status: u16,
        /// Response body (trimmed).
        detail: String,
    },
    /// The response body is not the JSON we expect.
    BadResponse {
        /// Parse/schema detail.
        detail: String,
    },
    /// A stream line cannot be parsed as NDJSON.
    BadStream {
        /// Offending line (trimmed, length-limited by the caller).
        line: String,
    },
    /// The server asked us to stop via an in-stream `error` field.
    ServerError {
        /// Message from the server.
        message: String,
    },
    /// The stream ended without the final `done` chunk.
    IncompleteStream,
    /// Reading the stream failed.
    Io {
        /// I/O detail.
        detail: String,
    },
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModelError::ServerUnavailable { url, detail } => write!(
                f,
                "ollama недоступен по {url} ({detail}). Запустите `ollama serve` \
                 или укажите другой адрес в AIOS_OLLAMA_URL"
            ),
            ModelError::ModelNotFound { model, detail } => write!(
                f,
                "модель `{model}` не найдена на сервере ({detail}). \
                 Скачайте её: `ollama pull {model}`"
            ),
            ModelError::HttpStatus { status, detail } => {
                write!(f, "ollama ответил HTTP {status}: {detail}")
            }
            ModelError::BadResponse { detail } => {
                write!(f, "неожиданный ответ ollama: {detail}")
            }
            ModelError::BadStream { line } => {
                write!(f, "некорректная строка NDJSON в потоке ollama: `{line}`")
            }
            ModelError::ServerError { message } => {
                write!(f, "ollama сообщил об ошибке: {message}")
            }
            ModelError::IncompleteStream => {
                write!(f, "поток ollama оборвался до финального чанка done")
            }
            ModelError::Io { detail } => write!(f, "ошибка чтения потока ollama: {detail}"),
        }
    }
}

impl std::error::Error for ModelError {}
