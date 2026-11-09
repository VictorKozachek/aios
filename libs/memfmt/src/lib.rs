#![no_std]
#![forbid(unsafe_code)]

//! AIOS memory subsystem formats: segments and records (ARCHITECTURE §5.2),
//! resync after a torn write (§5.2), checkpoints (§5.3) and canonical
//! serialization (§5.12).
//!
//! The format core is `no_std`, allocation-free and dependency-free:
//!
//! * [`segment`] — segment header, exactly 64 bytes;
//! * [`record`] — record framing (`magic/len/crc32c/kind/flags/seq/ts_ns`),
//!   8-byte aligned;
//! * [`crc32c`] — CRC32C (Castagnoli) implemented here;
//! * [`writer`] — append with 16 MiB rotation and a `prev_crc` file chain;
//! * [`reader`] — record iteration with a resync scan that distinguishes a
//!   torn tail from mid-file damage.
//!
//! The checkpoint codec (canonical CBOR via `ciborium` + `serde`) lives behind
//! the `cbor` feature (`cargo test -p memfmt --features cbor`): per §5.12 "the
//! kernel does not parse CBOR at all", so the dependency is not a default one
//! and never enters the kernel build.
//!
//! # Example
//!
//! ```ignore
//! let mut writer = Writer::create(store, SegmentKind::Session, 1, 1, now_ns, 0)?;
//! writer.append(RecordKind::UserIn, 0, now_ns, payload)?;
//! let pos = writer.position(); // (segment_id, byte_offset) — goes to a checkpoint
//!
//! let mut reader = SegmentReader::new(bytes);
//! for record in reader.by_ref() {
//!     // record.header.seq, record.payload
//! }
//! assert!(!reader.status().is_damaged());
//! ```

#[cfg(feature = "cbor")]
extern crate alloc;
#[cfg(test)]
extern crate std;

pub mod checkpoint;
pub mod crc32c;
pub mod error;
pub mod reader;
pub mod record;
pub mod segment;
pub mod writer;

pub use checkpoint::CHECKPOINT_SCHEMA_VERSION;
pub use crc32c::{crc32c, crc32c_update};
pub use error::Error;
pub use reader::{ReaderStatus, SegmentReader};
pub use record::{
    FLAG_EXTERNAL_BLOB, FLAG_LZ4, MAX_RECORD, RECORD_HEADER_LEN, RECORD_MAGIC, RECORD_MAGIC_BYTES,
    Record, RecordHeader, RecordKind, align8, parse_record_at,
};
pub use segment::{
    SEGMENT_HEADER_LEN, SEGMENT_MAGIC, SEGMENT_PADDING_LEN, SEGMENT_RESERVED_LEN, SEGMENT_VERSION,
    SegmentHeader, SegmentKind,
};
pub use writer::{Appended, SEGMENT_MAX_BYTES, SegmentPos, SegmentStore, Writer};

#[cfg(feature = "cbor")]
pub use checkpoint::{Budgets, Checkpoint, KvSnapshotRef, canonical};

#[cfg(test)]
mod tests;
