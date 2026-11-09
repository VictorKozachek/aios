//! Prototype memory layer: episodes, facts, BM25 index, context builder, RAG.
//!
//! Implements docs/PROTOTYPE.md §4 on top of the on-disk formats of
//! ARCHITECTURE §5:
//!
//! | Level | This crate |
//! |---|---|
//! | episodic | [`session`] — session logs as `libs/memfmt` segments |
//! | semantic | [`fact`] — facts with provenance in `memory/facts.jsonl` |
//! | index | [`bm25`] — deterministic BM25, rebuilt by scanning at startup |
//! | context | [`context`] — budgeted context: system, facts, tail, retrieved |
//! | files | [`files`] — chunked `files/`, retrieved by the same BM25 |
//!
//! The memory never asks the model what to read: retrieval is a deterministic
//! policy over the index (docs/PROTOTYPE.md §4). Everything a retrieved block
//! carries includes provenance, so the console can show `[files/report.md §2]`
//! or `[fact:…]` instead of "the model just knows".

pub mod audit;
pub mod bm25;
pub mod context;
pub mod error;
pub mod fact;
pub mod files;
pub mod memory;
pub mod session;
pub(crate) mod store;
pub mod time;

pub use bm25::{Bm25Index, Document, RefKind, SearchHit, tokenize};
pub use context::{Breakdown, ContextPlan, build_context};
pub use error::MemoryError;
pub use fact::{Author, Fact, FactStore, Source, State};
pub use files::FileChunk;
pub use memory::WorkspaceMemory;
pub use session::{Event, LoadedSession, SessionMeta, SessionWriter};
