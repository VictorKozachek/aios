//! Segment writer — ARCHITECTURE §5.1, §5.2.
//!
//! The writer knows nothing about files: it calls a [`SegmentStore`], and the
//! store (`logd`/`storaged` service) decides where the bytes go. That keeps the
//! format testable on the host and lets the same code run in the kernel.
//!
//! Rotation: a segment is never longer than 16 MiB (§5.1). When the next record
//! does not fit, the current segment is closed and a new one is opened with
//! `prev_crc = crc32c of the previous file` — a file-level chain that detects a
//! whole segment being deleted (§5.2).

use crate::crc32c::crc32c_update;
use crate::error::Error;
use crate::record::{MAX_RECORD, RECORD_HEADER_LEN, RecordHeader, RecordKind};
use crate::segment::{SEGMENT_HEADER_LEN, SegmentHeader, SegmentKind};

/// Segment size limit: 16 MiB (§5.1 "≤ 16 MiB per segment").
///
/// Records are never split: as soon as the next record does not fit into the
/// remaining space, a new segment starts. A file may therefore be smaller than
/// the limit by the size of one record tail, but never larger.
pub const SEGMENT_MAX_BYTES: u64 = 16 * 1024 * 1024;

/// Where the writer puts segment bytes.
///
/// Implementations: a file in `storaged`, an in-memory buffer (tests,
/// snapshots).
pub trait SegmentStore {
    /// Appends bytes to the current segment.
    fn write(&mut self, bytes: &[u8]) -> Result<(), Error>;

    /// The current segment is finished: subsequent `write` calls belong to a
    /// new file.
    ///
    /// Called on rotation only, before the header of the new segment is
    /// written.
    fn rotate(&mut self) -> Result<(), Error>;
}

/// Where the log currently ends — exactly what goes into a checkpoint
/// (`segment_id`, `byte_offset`, §5.3) so that resume is a seek, not a scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentPos {
    /// Segment file number (`log/000042.seg` → 42).
    pub segment_id: u64,
    /// Offset just past the last whole record in this file.
    pub byte_offset: u64,
    /// `seq` that the next record will get.
    pub next_seq: u64,
}

/// What `append` returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Appended {
    /// `seq` of the record that was written.
    pub seq: u64,
    /// Segment file number it went into.
    pub segment_id: u64,
    /// Offset of the record start inside the segment file.
    pub byte_offset: u64,
    /// Full on-disk record size: header + payload (a multiple of 8).
    pub record_len: usize,
}

/// Append-only segment writer with rotation.
#[derive(Debug)]
pub struct Writer<S: SegmentStore> {
    store: S,
    kind: SegmentKind,
    segment_id: u64,
    header: SegmentHeader,
    /// `crc32c` of all bytes of the current segment (header + records).
    segment_crc: u32,
    /// How many bytes are already written to the current segment.
    segment_len: u64,
    next_seq: u64,
}

impl<S: SegmentStore> Writer<S> {
    /// Opens the first segment and writes its header.
    ///
    /// `segment_id` is the file number (`log/000042.seg` → 42), `seq_first` is
    /// the `seq` of the first record, `prev_crc` is the `crc32c` of the
    /// previous file (0 for the first segment of a session/audit log).
    pub fn create(
        store: S,
        kind: SegmentKind,
        segment_id: u64,
        seq_first: u64,
        created_ns: u64,
        prev_crc: u32,
    ) -> Result<Self, Error> {
        let header = SegmentHeader::new(kind, seq_first, created_ns, prev_crc);
        let mut writer = Writer {
            store,
            kind,
            segment_id,
            header,
            segment_crc: 0,
            segment_len: 0,
            next_seq: seq_first,
        };
        writer.write_header()?;
        Ok(writer)
    }

    fn write_header(&mut self) -> Result<(), Error> {
        let bytes = self.header.encode();
        self.store.write(&bytes)?;
        self.segment_crc = crc32c_update(self.segment_crc, &bytes);
        self.segment_len += SEGMENT_HEADER_LEN as u64;
        Ok(())
    }

    fn rotate(&mut self, created_ns: u64) -> Result<(), Error> {
        self.store.rotate()?;
        self.segment_id += 1;
        // File chain: the next segment remembers the crc of the previous one.
        self.header = SegmentHeader::new(self.kind, self.next_seq, created_ns, self.segment_crc);
        self.segment_crc = 0;
        self.segment_len = 0;
        self.write_header()
    }

    /// Appends a record and returns its `seq`.
    ///
    /// `payload` is already serialized data (CBOR for session records, §5.12);
    /// its length must be a multiple of 8 (§5.2: resync invariant).
    ///
    /// `ts_ns` is the monotonic record time; it also becomes `created_ns` of the
    /// new segment if this call triggers rotation.
    pub fn append(
        &mut self,
        kind: RecordKind,
        flags: u8,
        ts_ns: u64,
        payload: &[u8],
    ) -> Result<Appended, Error> {
        if payload.len() > MAX_RECORD {
            return Err(Error::PayloadTooLarge {
                len: payload.len(),
                max: MAX_RECORD,
            });
        }
        if !payload.len().is_multiple_of(8) {
            return Err(Error::PayloadUnaligned { len: payload.len() });
        }

        let record_len = RECORD_HEADER_LEN + payload.len();
        if self.segment_len + record_len as u64 > SEGMENT_MAX_BYTES {
            self.rotate(ts_ns)?;
        }

        let header = RecordHeader::new(kind.as_u8(), flags, self.next_seq, ts_ns, payload);
        let start = self.segment_len;
        let encoded = header.encode();
        self.store.write(&encoded)?;
        self.store.write(payload)?;
        self.segment_crc = crc32c_update(self.segment_crc, &encoded);
        self.segment_crc = crc32c_update(self.segment_crc, payload);
        self.segment_len += record_len as u64;
        self.next_seq += 1;

        Ok(Appended {
            seq: header.seq,
            segment_id: self.segment_id,
            byte_offset: start,
            record_len,
        })
    }

    /// Where the log ends right now — for checkpoints (§5.3).
    pub fn position(&self) -> SegmentPos {
        SegmentPos {
            segment_id: self.segment_id,
            byte_offset: self.segment_len,
            next_seq: self.next_seq,
        }
    }

    /// Header of the current segment.
    pub fn header(&self) -> SegmentHeader {
        self.header
    }

    /// `crc32c` of the current segment so far (if the segment were closed now,
    /// this would be its `prev_crc` for the next one).
    pub fn segment_crc(&self) -> u32 {
        self.segment_crc
    }

    /// Current segment file number.
    pub fn segment_id(&self) -> u64 {
        self.segment_id
    }

    /// Gives the store back (for example, to close the file).
    pub fn into_inner(self) -> S {
        self.store
    }
}
