//! Model adapter for the AIOS prototype (`modeld`).
//!
//! The console and the future memory layer must not know which engine serves
//! the tokens. They talk to [`ModelBackend`]; today it is [`OllamaBackend`]
//! (ollama on `127.0.0.1:11434`), later it will be `LlamaCppBackend` and the
//! in-kernel engine without changes in callers (docs/PROTOTYPE.md §3).
//!
//! Design rules kept from ARCHITECTURE §5:
//! * streaming is the only way to get tokens — a reply is an iterator of
//!   deltas, not a `String`;
//! * the token counter is part of the interface, because the context builder
//!   needs a budget estimate before sending anything;
//! * errors are values with user-facing messages, not panics: a missing server
//!   or an un-downloaded model is the normal state of a fresh machine.

pub mod backend;
pub mod error;
pub mod ollama;
pub mod stream;
pub mod tokens;

pub use backend::{GenChunk, GenRequest, GenStats, Message, ModelBackend, ModelInfo, Role};
pub use error::ModelError;
pub use ollama::{DEFAULT_BASE_URL, DEFAULT_MODEL, OllamaBackend, map_http_status, parse_show};
pub use stream::{GenStream, parse_line};
pub use tokens::estimate_tokens;
