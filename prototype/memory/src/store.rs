//! Shared `std` plumbing for the memory layer.
//!
//! `libs/memfmt` deliberately knows nothing about files: it calls a
//! [`memfmt::SegmentStore`]. This module provides the file-backed store used by
//! session logs and by the audit segment, plus the JSON payload framing
//! (record `len` must be a multiple of 8, so JSON is padded with zero bytes).

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;

use memfmt::SegmentStore;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::MemoryError;

/// `memfmt` store that appends to a single file and flushes every write.
#[derive(Debug)]
pub(crate) struct FileStore {
    file: File,
}

impl FileStore {
    /// Creates a new file (fails if it already exists).
    pub(crate) fn create_new(path: &Path) -> Result<Self, MemoryError> {
        let file = OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(path)
            .map_err(|error| MemoryError::io(format!("создать {}", path.display()), error))?;
        Ok(FileStore { file })
    }
}

impl SegmentStore for FileStore {
    fn write(&mut self, bytes: &[u8]) -> Result<(), memfmt::Error> {
        self.file
            .write_all(bytes)
            .map_err(|_| memfmt::Error::Store)?;
        self.file.flush().map_err(|_| memfmt::Error::Store)?;
        Ok(())
    }

    fn rotate(&mut self) -> Result<(), memfmt::Error> {
        // Rotation is driven by the caller, which opens the next file.
        Ok(())
    }
}

/// Serializes a value into a record payload: JSON padded with zero bytes up to
/// a multiple of 8 (§5.2 requires `len` to be a multiple of 8).
pub(crate) fn encode_json_padded<T: Serialize>(value: &T) -> Result<Vec<u8>, MemoryError> {
    let json = serde_json::to_vec(value)
        .map_err(|error| MemoryError::json("сериализовать запись памяти", error))?;
    let mut payload = json;
    payload.resize(memfmt::align8(payload.len()), 0);
    Ok(payload)
}

/// Reads a record payload written by [`encode_json_padded`].
///
/// JSON text never ends with a zero byte, so trimming trailing zeros is
/// unambiguous.
pub(crate) fn decode_json_padded<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, MemoryError> {
    let end = bytes
        .iter()
        .rposition(|byte| *byte != 0)
        .map(|index| index + 1)
        .unwrap_or(0);
    serde_json::from_slice(&bytes[..end])
        .map_err(|error| MemoryError::json("разобрать запись памяти", error))
}
