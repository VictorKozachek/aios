//! Errors of the memory formats.
//!
//! One enum for the whole crate: the format is a single protocol
//! (ARCHITECTURE §5.2) and callers benefit from seeing all of it at once.
//! No allocations, `Copy`.

use core::fmt;

/// Error of segment parsing or writing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Segment header is shorter than 64 bytes: the file is torn on the header.
    IncompleteSegmentHeader {
        /// How many bytes are actually present.
        available: usize,
    },
    /// The first 8 bytes of the header are not `AIOSSEG1`.
    BadSegmentMagic,
    /// Segment header `crc32c` does not match.
    BadHeaderCrc {
        /// Value stored in the `hdr_crc` field.
        stored: u32,
        /// Value recomputed from the actual bytes.
        computed: u32,
    },
    /// Segment format version is not supported.
    UnsupportedSegmentVersion {
        /// Version stored in the header.
        stored: u16,
        /// Version this implementation understands.
        supported: u16,
    },
    /// Unknown segment `kind` (only 1=session, 2=audit, 3=index are valid).
    UnknownSegmentKind {
        /// Value stored in the header.
        stored: u16,
    },
    /// The rest of the record is shorter than the 32-byte record header.
    RecordTooShort {
        /// How many bytes remain before the end of the buffer.
        available: usize,
    },
    /// The record does not start with magic `0x4149_4F53`.
    BadRecordMagic {
        /// Offset where a record was expected.
        offset: usize,
    },
    /// Record `len` is greater than [`crate::MAX_RECORD`].
    RecordLenTooLarge {
        /// Value of the `len` field.
        len: u32,
        /// Limit, [`crate::MAX_RECORD`].
        max: u32,
    },
    /// Record `len` is not a multiple of 8, breaking the resync invariant (§5.2).
    RecordLenUnaligned {
        /// Value of the `len` field.
        len: u32,
    },
    /// The record runs past the buffer: it is torn in the file.
    RecordOutOfBounds {
        /// Offset of the record start.
        offset: usize,
        /// Value of the `len` field.
        len: u32,
        /// Buffer length that was searched.
        available: usize,
    },
    /// Payload `crc32c` does not match.
    BadRecordCrc {
        /// Value stored in the `crc32c` field.
        stored: u32,
        /// Value computed from the payload.
        computed: u32,
    },
    /// The strict `seq` ordering invariant (§5.2) is broken.
    UnexpectedSeq {
        /// Expected value.
        expected: u64,
        /// Actual value from the record header.
        actual: u64,
    },
    /// Writer rejected the payload: it is longer than [`crate::MAX_RECORD`].
    PayloadTooLarge {
        /// Payload length.
        len: usize,
        /// Limit, [`crate::MAX_RECORD`].
        max: usize,
    },
    /// Writer rejected the payload: its length is not a multiple of 8 (§5.2).
    PayloadUnaligned {
        /// Payload length.
        len: usize,
    },
    /// The underlying segment store failed (disk, filesystem, buffer).
    Store,
    /// The record has an unexpected kind.
    UnexpectedRecordKind {
        /// Expected `kind`.
        expected: u8,
        /// Actual `kind`.
        actual: u8,
    },
    /// Payload is empty: it has no schema version byte (§5.12).
    #[cfg(feature = "cbor")]
    SchemaVersionMissing,
    /// Payload schema version is not supported (§5.12: mismatch is a refusal).
    #[cfg(feature = "cbor")]
    UnsupportedSchemaVersion {
        /// Version from the first payload byte.
        stored: u8,
        /// Version this implementation understands.
        supported: u8,
    },
    /// CBOR is not in canonical form (§5.12); the string is the reason.
    #[cfg(feature = "cbor")]
    NonCanonicalCbor(&'static str),
    /// CBOR serialization failed.
    #[cfg(feature = "cbor")]
    CborEncode,
    /// CBOR deserialization failed (wrong type, truncated data).
    #[cfg(feature = "cbor")]
    CborDecode,
    /// Non-zero bytes remain after the CBOR value in the payload.
    #[cfg(feature = "cbor")]
    TrailingBytes {
        /// Offset in the payload where the garbage starts.
        at: usize,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::IncompleteSegmentHeader { available } => write!(
                f,
                "segment header truncated: {available} of {} bytes",
                crate::SEGMENT_HEADER_LEN
            ),
            Error::BadSegmentMagic => write!(f, "segment magic AIOSSEG1 mismatch"),
            Error::BadHeaderCrc { stored, computed } => write!(
                f,
                "segment header crc32c mismatch: stored {stored:#010x}, computed {computed:#010x}"
            ),
            Error::UnsupportedSegmentVersion { stored, supported } => write!(
                f,
                "unsupported segment version {stored} (supported: {supported})"
            ),
            Error::UnknownSegmentKind { stored } => {
                write!(f, "unknown segment kind {stored}")
            }
            Error::RecordTooShort { available } => write!(
                f,
                "record framing shorter than header: {available} of {} bytes",
                crate::RECORD_HEADER_LEN
            ),
            Error::BadRecordMagic { offset } => {
                write!(f, "no record magic at offset {offset}")
            }
            Error::RecordLenTooLarge { len, max } => {
                write!(f, "record len {len} exceeds limit {max}")
            }
            Error::RecordLenUnaligned { len } => {
                write!(f, "record len {len} is not a multiple of 8")
            }
            Error::RecordOutOfBounds {
                offset,
                len,
                available,
            } => write!(
                f,
                "record at offset {offset} with len {len} runs past buffer {available}"
            ),
            Error::BadRecordCrc { stored, computed } => write!(
                f,
                "record payload crc32c mismatch: stored {stored:#010x}, computed {computed:#010x}"
            ),
            Error::UnexpectedSeq { expected, actual } => {
                write!(f, "seq {actual} instead of expected {expected}")
            }
            Error::PayloadTooLarge { len, max } => {
                write!(f, "payload of {len} bytes exceeds limit {max}")
            }
            Error::PayloadUnaligned { len } => {
                write!(f, "payload of {len} bytes is not a multiple of 8")
            }
            Error::Store => write!(f, "segment store failure"),
            Error::UnexpectedRecordKind { expected, actual } => {
                write!(f, "record kind {actual} instead of expected {expected}")
            }
            #[cfg(feature = "cbor")]
            Error::SchemaVersionMissing => write!(f, "empty payload: schema version byte missing"),
            #[cfg(feature = "cbor")]
            Error::UnsupportedSchemaVersion { stored, supported } => write!(
                f,
                "unsupported schema version {stored} (supported: {supported})"
            ),
            #[cfg(feature = "cbor")]
            Error::NonCanonicalCbor(reason) => {
                write!(f, "CBOR is not canonical: {reason}")
            }
            #[cfg(feature = "cbor")]
            Error::CborEncode => write!(f, "CBOR encoding failed"),
            #[cfg(feature = "cbor")]
            Error::CborDecode => write!(f, "CBOR decoding failed"),
            #[cfg(feature = "cbor")]
            Error::TrailingBytes { at } => {
                write!(f, "trailing bytes after the CBOR value at offset {at}")
            }
        }
    }
}

impl core::error::Error for Error {}
