//! Segment record — ARCHITECTURE §5.2.
//!
//! Every record is self-contained and starts with magic; that is one of the
//! three invariants without which resync is impossible (§5.2).
//!
//! Layout (little-endian, `RECORD_HEADER_LEN = 32` bytes):
//!
//! ```text
//! magic    u32  0x4149_4F53 ("AIOS")
//! len      u32  payload length, ≤ MAX_RECORD = 1 MiB, multiple of 8
//! crc32c   u32  over the payload
//! kind     u8
//! flags    u8   bit0 = payload compressed with LZ4, bit1 = external blob
//! pad      [2]  zeros: align seq/ts_ns and the whole record to 8 bytes
//! seq      u64  monotonic (§5.2)
//! ts_ns    u64
//! payload  [len]
//! ```
//!
//! The document lists the fields in this order; two alignment bytes are
//! inserted after `flags` so that `seq` sits on an 8-byte boundary and the
//! whole record (`32 + len`, `len` a multiple of 8) is 8-byte aligned, as §5.2
//! requires. Alignment bytes are ignored on read (reserved).

use crate::crc32c::crc32c;
use crate::error::Error;

/// Record magic: `0x41_49_4F_53` ("AIOS" in big-endian byte order).
///
/// On disk the field is written little-endian, i.e. as bytes `53 4F 49 41`, as
/// required by the little-endian format of §5.2. For scanning the file for the
/// magic use [`RECORD_MAGIC_BYTES`].
pub const RECORD_MAGIC: u32 = 0x4149_4F53;

/// File bytes of the record magic (little-endian form of [`RECORD_MAGIC`]).
pub const RECORD_MAGIC_BYTES: [u8; 4] = RECORD_MAGIC.to_le_bytes();

/// Record header size including alignment, bytes.
pub const RECORD_HEADER_LEN: usize = 32;

/// Maximum payload length of a single record: 1 MiB (§5.2).
pub const MAX_RECORD: usize = 1024 * 1024;

/// `flags` bit0: payload is compressed with LZ4 frame.
pub const FLAG_LZ4: u8 = 0x01;

/// `flags` bit1: payload references an external blob.
pub const FLAG_EXTERNAL_BLOB: u8 = 0x02;

// Field offsets inside the record header.
const OFF_MAGIC: usize = 0;
const OFF_LEN: usize = 4;
const OFF_CRC: usize = 8;
const OFF_KIND: usize = 12;
const OFF_FLAGS: usize = 13;
const OFF_PAD: usize = 14;
const OFF_SEQ: usize = 16;
const OFF_TS_NS: usize = 24;

/// Rounds a length up to a multiple of 8 — the on-disk payload size.
pub const fn align8(len: usize) -> usize {
    (len + 7) & !7
}

/// Record kind — the `kind` table of §5.2.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RecordKind {
    /// User text: source, client id, taint=user.
    UserIn = 1,
    /// Model response text, usage, model.
    ModelOut = 2,
    /// Plan steps, confidence.
    Plan = 3,
    /// Capability name, redacted arguments, result hash.
    CapCall = 4,
    /// Policy engine verdict, rule, reason.
    Policy = 5,
    /// Semantic memory write: fact id, scope, provenance.
    MemWrite = 6,
    /// Reference to a KV snapshot and the prefix token count.
    KvSnapshot = 7,
    /// Checkpoint: seq, budgets, chain head hash (§5.3).
    Checkpoint = 8,
    /// Error code and context.
    Error = 9,
    /// Session state transition (§5.9).
    SessionState = 10,
    /// Context manifest: what exactly went into the model context (§5.6).
    ContextManifest = 11,
    /// Human confirmation request: id, action, TTL.
    Ask = 12,
    /// Audit chain head hash anchored outside the log (§7.4).
    AuditAnchor = 13,
}

impl RecordKind {
    /// Numeric value of the `kind` field.
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    /// Typed view of a known `kind`.
    ///
    /// `None` for unknown values: record framing must survive a record whose
    /// kind this code version does not know yet.
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            1 => Some(RecordKind::UserIn),
            2 => Some(RecordKind::ModelOut),
            3 => Some(RecordKind::Plan),
            4 => Some(RecordKind::CapCall),
            5 => Some(RecordKind::Policy),
            6 => Some(RecordKind::MemWrite),
            7 => Some(RecordKind::KvSnapshot),
            8 => Some(RecordKind::Checkpoint),
            9 => Some(RecordKind::Error),
            10 => Some(RecordKind::SessionState),
            11 => Some(RecordKind::ContextManifest),
            12 => Some(RecordKind::Ask),
            13 => Some(RecordKind::AuditAnchor),
            _ => None,
        }
    }
}

/// Record header (§5.2) without the payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordHeader {
    /// Payload length, bytes. Multiple of 8, at most [`MAX_RECORD`].
    pub len: u32,
    /// `crc32c` of the payload.
    pub crc32c: u32,
    /// Record kind, see [`RecordKind::as_u8`].
    pub kind: u8,
    /// Flags, [`FLAG_LZ4`] and [`FLAG_EXTERNAL_BLOB`].
    pub flags: u8,
    /// Monotonic record number.
    pub seq: u64,
    /// Record time, monotonic nanoseconds.
    pub ts_ns: u64,
}

impl RecordHeader {
    /// Encoded record header size, bytes.
    pub const ENCODED_LEN: usize = RECORD_HEADER_LEN;

    /// Builds a header and computes `crc32c` over the payload.
    pub fn new(kind: u8, flags: u8, seq: u64, ts_ns: u64, payload: &[u8]) -> Self {
        RecordHeader {
            len: payload.len() as u32,
            crc32c: crc32c(payload),
            kind,
            flags,
            seq,
            ts_ns,
        }
    }

    /// Encodes the header into 32 bytes; alignment bytes are zero.
    pub fn encode(&self) -> [u8; RECORD_HEADER_LEN] {
        let mut bytes = [0u8; RECORD_HEADER_LEN];
        bytes[OFF_MAGIC..OFF_MAGIC + 4].copy_from_slice(&RECORD_MAGIC_BYTES);
        bytes[OFF_LEN..OFF_LEN + 4].copy_from_slice(&self.len.to_le_bytes());
        bytes[OFF_CRC..OFF_CRC + 4].copy_from_slice(&self.crc32c.to_le_bytes());
        bytes[OFF_KIND] = self.kind;
        bytes[OFF_FLAGS] = self.flags;
        bytes[OFF_PAD] = 0;
        bytes[OFF_PAD + 1] = 0;
        bytes[OFF_SEQ..OFF_SEQ + 8].copy_from_slice(&self.seq.to_le_bytes());
        bytes[OFF_TS_NS..OFF_TS_NS + 8].copy_from_slice(&self.ts_ns.to_le_bytes());
        bytes
    }

    /// Parses a record header from 32 bytes.
    ///
    /// Checks the magic and that `len` is at most [`MAX_RECORD`] and a multiple
    /// of 8. Alignment bytes are ignored.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < RECORD_HEADER_LEN {
            return Err(Error::RecordTooShort {
                available: bytes.len(),
            });
        }
        if bytes[OFF_MAGIC..OFF_MAGIC + 4] != RECORD_MAGIC_BYTES {
            return Err(Error::BadRecordMagic { offset: 0 });
        }
        let len = u32::from_le_bytes(bytes[OFF_LEN..OFF_LEN + 4].try_into().expect("4 bytes"));
        if len as usize > MAX_RECORD {
            return Err(Error::RecordLenTooLarge {
                len,
                max: MAX_RECORD as u32,
            });
        }
        if !len.is_multiple_of(8) {
            return Err(Error::RecordLenUnaligned { len });
        }
        Ok(RecordHeader {
            len,
            crc32c: u32::from_le_bytes(bytes[OFF_CRC..OFF_CRC + 4].try_into().expect("4 bytes")),
            kind: bytes[OFF_KIND],
            flags: bytes[OFF_FLAGS],
            seq: u64::from_le_bytes(bytes[OFF_SEQ..OFF_SEQ + 8].try_into().expect("8 bytes")),
            ts_ns: u64::from_le_bytes(bytes[OFF_TS_NS..OFF_TS_NS + 8].try_into().expect("8 bytes")),
        })
    }

    /// Full on-disk record size: header + payload (a multiple of 8).
    pub const fn record_len(&self) -> usize {
        RECORD_HEADER_LEN + self.len as usize
    }

    /// `true` if the payload is LZ4-compressed (`flags` bit0).
    pub const fn is_lz4(&self) -> bool {
        self.flags & FLAG_LZ4 != 0
    }

    /// `true` if the payload references an external blob (`flags` bit1).
    pub const fn has_external_blob(&self) -> bool {
        self.flags & FLAG_EXTERNAL_BLOB != 0
    }
}

/// A segment record: header plus payload borrowed from the segment buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record<'a> {
    /// Record header.
    pub header: RecordHeader,
    /// Payload of exactly `header.len` bytes (may include zero alignment up to
    /// a multiple of 8 — see `Writer::append`).
    pub payload: &'a [u8],
}

impl Record<'_> {
    /// Record `seq`.
    pub const fn seq(&self) -> u64 {
        self.header.seq
    }

    /// Record `kind`.
    pub const fn kind(&self) -> u8 {
        self.header.kind
    }

    /// Record `flags`.
    pub const fn flags(&self) -> u8 {
        self.header.flags
    }

    /// Full on-disk record size in bytes.
    pub const fn record_len(&self) -> usize {
        RECORD_HEADER_LEN + self.header.len as usize
    }
}

/// Parses a record at `offset` inside a segment buffer.
///
/// Checks the magic, `len` (limit and multiple of 8), buffer bounds and the
/// payload `crc32c`. `seq` is not checked here — that is done by
/// [`crate::SegmentReader`], which knows which record is expected next.
pub fn parse_record_at(bytes: &[u8], offset: usize) -> Result<Record<'_>, Error> {
    let available = bytes.len().saturating_sub(offset);
    if available < RECORD_HEADER_LEN {
        return Err(Error::RecordTooShort { available });
    }
    if bytes[offset..offset + 4] != RECORD_MAGIC_BYTES {
        return Err(Error::BadRecordMagic { offset });
    }
    let header = RecordHeader::decode(&bytes[offset..offset + RECORD_HEADER_LEN])?;
    let len = header.len as usize;
    let end = offset + RECORD_HEADER_LEN + len;
    if end > bytes.len() {
        return Err(Error::RecordOutOfBounds {
            offset,
            len: header.len,
            available: bytes.len(),
        });
    }
    let payload = &bytes[offset + RECORD_HEADER_LEN..end];
    let computed = crc32c(payload);
    if computed != header.crc32c {
        return Err(Error::BadRecordCrc {
            stored: header.crc32c,
            computed,
        });
    }
    Ok(Record { header, payload })
}
