//! Segment header — ARCHITECTURE §5.2.
//!
//! Layout (little-endian, exactly 64 bytes):
//!
//! ```text
//! magic      [8]  "AIOSSEG1"
//! version    u16
//! kind       u16  1=session 2=audit 3=index
//! seq_first  u64
//! created_ns u64
//! prev_crc   u32  crc32c of the previous segment (file-level chain)
//! hdr_crc    u32
//! reserved   [16]
//! ```
//!
//! The document mandates exactly 64 bytes, but the listed fields occupy 52.
//! To satisfy both requirements, 12 zero bytes of alignment are appended after
//! `reserved`; the `reserved` field itself stays exactly `[u8; 16]` as
//! documented. This is the only divergence from the §5.2 text; it is recorded
//! in the T5 task report.

use crate::crc32c::crc32c;
use crate::error::Error;

/// Segment header magic.
pub const SEGMENT_MAGIC: [u8; 8] = *b"AIOSSEG1";

/// Full segment header size, bytes.
pub const SEGMENT_HEADER_LEN: usize = 64;

/// Segment format version understood by this implementation.
pub const SEGMENT_VERSION: u16 = 1;

/// Size of the `reserved` field, bytes (document: `reserved [16]`).
pub const SEGMENT_RESERVED_LEN: usize = 16;

// Field offsets inside the header.
const OFF_MAGIC: usize = 0;
const OFF_VERSION: usize = 8;
const OFF_KIND: usize = 10;
const OFF_SEQ_FIRST: usize = 12;
const OFF_CREATED_NS: usize = 20;
const OFF_PREV_CRC: usize = 28;
const OFF_HDR_CRC: usize = 32;
const OFF_RESERVED: usize = 36;
const OFF_PADDING: usize = OFF_RESERVED + SEGMENT_RESERVED_LEN; // 52

/// Number of zero alignment bytes added to make the header 64 bytes long.
pub const SEGMENT_PADDING_LEN: usize = SEGMENT_HEADER_LEN - OFF_PADDING; // 12

/// Segment kind (§5.1: session transcript, audit log, index).
#[repr(u16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SegmentKind {
    /// `log/NNNNNN.seg` — append-only session transcript.
    Session = 1,
    /// `audit/audit.seg` — global hash-chained log.
    Audit = 2,
    /// Rebuildable memory index cache.
    Index = 3,
}

impl SegmentKind {
    /// Numeric value of the `kind` field.
    pub const fn as_u16(self) -> u16 {
        self as u16
    }

    /// Parses `kind` from the header.
    pub fn from_u16(value: u16) -> Result<Self, Error> {
        match value {
            1 => Ok(SegmentKind::Session),
            2 => Ok(SegmentKind::Audit),
            3 => Ok(SegmentKind::Index),
            stored => Err(Error::UnknownSegmentKind { stored }),
        }
    }
}

/// Segment header (§5.2).
///
/// `hdr_crc` is not stored in the struct: it is fully determined by the other
/// fields (`crc32c` over the 64 bytes with the `hdr_crc` field zeroed), so a
/// change to `reserved` is automatically covered when encoding. The computed
/// value is available via [`SegmentHeader::hdr_crc`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentHeader {
    /// Format version.
    pub version: u16,
    /// Segment kind.
    pub kind: SegmentKind,
    /// `seq` of the first record in the segment.
    pub seq_first: u64,
    /// Segment creation time, monotonic nanoseconds.
    pub created_ns: u64,
    /// `crc32c` of the previous segment file (0 for the first one).
    pub prev_crc: u32,
    /// Reserved for format extensions; zero on write.
    pub reserved: [u8; SEGMENT_RESERVED_LEN],
}

impl SegmentHeader {
    /// Creates a header of the current version with zeroed `reserved`.
    ///
    /// `prev_crc` is the `crc32c` of the previous segment file, `0` for the
    /// very first segment.
    pub fn new(kind: SegmentKind, seq_first: u64, created_ns: u64, prev_crc: u32) -> Self {
        SegmentHeader {
            version: SEGMENT_VERSION,
            kind,
            seq_first,
            created_ns,
            prev_crc,
            reserved: [0u8; SEGMENT_RESERVED_LEN],
        }
    }

    /// Encodes the header into exactly 64 bytes (`hdr_crc` recomputed).
    pub fn encode(&self) -> [u8; SEGMENT_HEADER_LEN] {
        let mut bytes = self.encode_with_zero_crc();
        let crc = crc32c(&bytes);
        bytes[OFF_HDR_CRC..OFF_HDR_CRC + 4].copy_from_slice(&crc.to_le_bytes());
        bytes
    }

    /// `crc32c` of the header as it is stored in the file.
    pub fn hdr_crc(&self) -> u32 {
        crc32c(&self.encode_with_zero_crc())
    }

    fn encode_with_zero_crc(&self) -> [u8; SEGMENT_HEADER_LEN] {
        let mut bytes = [0u8; SEGMENT_HEADER_LEN];
        bytes[OFF_MAGIC..OFF_MAGIC + 8].copy_from_slice(&SEGMENT_MAGIC);
        bytes[OFF_VERSION..OFF_VERSION + 2].copy_from_slice(&self.version.to_le_bytes());
        bytes[OFF_KIND..OFF_KIND + 2].copy_from_slice(&self.kind.as_u16().to_le_bytes());
        bytes[OFF_SEQ_FIRST..OFF_SEQ_FIRST + 8].copy_from_slice(&self.seq_first.to_le_bytes());
        bytes[OFF_CREATED_NS..OFF_CREATED_NS + 8].copy_from_slice(&self.created_ns.to_le_bytes());
        bytes[OFF_PREV_CRC..OFF_PREV_CRC + 4].copy_from_slice(&self.prev_crc.to_le_bytes());
        bytes[OFF_RESERVED..OFF_RESERVED + SEGMENT_RESERVED_LEN].copy_from_slice(&self.reserved);
        bytes
    }

    /// Parses a header from the first bytes of a segment.
    ///
    /// Checks: magic, `hdr_crc`, version, `kind`. Alignment bytes (52..64) are
    /// not checked — they are reserved, not meaningful.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < SEGMENT_HEADER_LEN {
            return Err(Error::IncompleteSegmentHeader {
                available: bytes.len(),
            });
        }
        let bytes = &bytes[..SEGMENT_HEADER_LEN];
        if bytes[OFF_MAGIC..OFF_MAGIC + 8] != SEGMENT_MAGIC {
            return Err(Error::BadSegmentMagic);
        }
        let stored_crc = u32::from_le_bytes(
            bytes[OFF_HDR_CRC..OFF_HDR_CRC + 4]
                .try_into()
                .expect("4 bytes"),
        );
        let mut zeroed = [0u8; SEGMENT_HEADER_LEN];
        zeroed.copy_from_slice(bytes);
        zeroed[OFF_HDR_CRC..OFF_HDR_CRC + 4].fill(0);
        let computed = crc32c(&zeroed);
        if stored_crc != computed {
            return Err(Error::BadHeaderCrc {
                stored: stored_crc,
                computed,
            });
        }

        let version = u16::from_le_bytes(
            bytes[OFF_VERSION..OFF_VERSION + 2]
                .try_into()
                .expect("2 bytes"),
        );
        if version != SEGMENT_VERSION {
            return Err(Error::UnsupportedSegmentVersion {
                stored: version,
                supported: SEGMENT_VERSION,
            });
        }
        let kind_raw =
            u16::from_le_bytes(bytes[OFF_KIND..OFF_KIND + 2].try_into().expect("2 bytes"));
        let kind = SegmentKind::from_u16(kind_raw)?;
        let seq_first = u64::from_le_bytes(
            bytes[OFF_SEQ_FIRST..OFF_SEQ_FIRST + 8]
                .try_into()
                .expect("8 bytes"),
        );
        let created_ns = u64::from_le_bytes(
            bytes[OFF_CREATED_NS..OFF_CREATED_NS + 8]
                .try_into()
                .expect("8 bytes"),
        );
        let prev_crc = u32::from_le_bytes(
            bytes[OFF_PREV_CRC..OFF_PREV_CRC + 4]
                .try_into()
                .expect("4 bytes"),
        );
        let mut reserved = [0u8; SEGMENT_RESERVED_LEN];
        reserved.copy_from_slice(&bytes[OFF_RESERVED..OFF_RESERVED + SEGMENT_RESERVED_LEN]);

        Ok(SegmentHeader {
            version,
            kind,
            seq_first,
            created_ns,
            prev_crc,
            reserved,
        })
    }
}
