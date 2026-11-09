//! AIOS prototype console (PR-015).
//!
//! A human-facing REPL around a model backend: workspaces (`home`, `work`),
//! session logs in NDJSON, and a `/status` view. This stage is intentionally
//! memory-free — segments, facts and retrieval arrive with card 016
//! (docs/PROTOTYPE.md §4).
//!
//! The library exposes the application state separately from the terminal
//! loop ([`app::App`] vs [`repl::run`]), so behaviour is tested without a TTY.

pub mod app;
pub mod config;
pub mod error;
pub mod paths;
pub mod repl;
pub mod session;
pub mod time;
pub mod workspace;

pub use app::{Action, App, BackendFactory, Reply};
pub use config::WorkspaceConfig;
pub use error::ConsoleError;
pub use paths::Paths;
pub use session::Session;
pub use workspace::{WORKSPACES, Workspace, Workspaces};
