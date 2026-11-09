//! Segment reader with resync — ARCHITECTURE §5.2.
//!
//! Scan from the last valid offset: look for the magic (4 bytes, false-positive
//! chance 2⁻³² per position), check `len ≤ MAX_RECORD` and the bounds, compute
//! `crc32c`, expect the next `seq`. If it does not match, shift by 1 byte and
//! retry.
//!
//! A torn tail differs from mid-file damage:
//! * magic is at the expected place, the record is invalid, and there is no
//!   valid record anywhere after it → [`ReaderStatus::Torn`]: the tail is
//!   discarded;
//! * a later valid record was found (so some records are lost but the file
//!   continues) → [`ReaderStatus::Damaged`], and reading continues from there;
//! * magic is absent entirely (garbage instead of a record start) and no valid
//!   record follows → [`ReaderStatus::Damaged`]: the segment is read-only from
//!   then on.
//!
//! Clarification to §5.2, recorded in the T5 report: the document calls only
//! the "magic not found anywhere on the remainder" case mid-file damage. A
//! corrupted payload byte in the middle of a file looks like "magic present,
//! crc mismatch", but the tail after it is still readable — that is damage as
//! well, not a torn tail; otherwise the required test "corruption in the middle
//! → damaged" would be impossible.

use crate::error::Error;
use crate::record::{RECORD_HEADER_LEN, RECORD_MAGIC_BYTES, Record, parse_record_at};
use crate::segment::{SEGMENT_HEADER_LEN, SegmentHeader};

/// How reading a segment ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReaderStatus {
    /// All records were read, the file ended exactly on a record boundary.
    Clean,
    /// The tail is torn (the file was truncated during a write); the prefix
    /// that was read is intact.
    Torn {
        /// Offset where the incomplete record starts.
        at: usize,
    },
    /// The file is damaged in the middle: some records are lost, and reading
    /// continued after a resync.
    Damaged {
        /// Offset where the damage was detected.
        at: usize,
    },
}

impl ReaderStatus {
    /// `true` if the file was read completely without losses.
    pub const fn is_clean(self) -> bool {
        matches!(self, ReaderStatus::Clean)
    }

    /// `true` if the tail is torn.
    pub const fn is_torn(self) -> bool {
        matches!(self, ReaderStatus::Torn { .. })
    }

    /// `true` if the file is damaged in the middle.
    pub const fn is_damaged(self) -> bool {
        matches!(self, ReaderStatus::Damaged { .. })
    }
}

/// Iterator over the records of one segment.
///
/// The segment header is parsed lazily, on the first record access or
/// [`SegmentReader::header`] call. Errors do not abort iteration with an
/// exception: they are visible in [`SegmentReader::status`], and the iterator
/// simply ends.
#[derive(Debug)]
pub struct SegmentReader<'a> {
    buf: &'a [u8],
    pos: usize,
    last_seq: Option<u64>,
    status: ReaderStatus,
    header: Option<SegmentHeader>,
    stopped: bool,
    records: u64,
    last_valid_offset: usize,
}

impl<'a> SegmentReader<'a> {
    /// Creates a reader over the bytes of a segment file.
    pub fn new(buf: &'a [u8]) -> Self {
        SegmentReader {
            buf,
            pos: 0,
            last_seq: None,
            status: ReaderStatus::Clean,
            header: None,
            stopped: false,
            records: 0,
            last_valid_offset: 0,
        }
    }

    /// Parses and validates the segment header (64 bytes).
    ///
    /// A header torn short sets the status to [`ReaderStatus::Torn`]; an
    /// invalid header sets [`ReaderStatus::Damaged`] at offset 0.
    pub fn header(&mut self) -> Result<SegmentHeader, Error> {
        if let Some(header) = self.header {
            return Ok(header);
        }
        if self.buf.len() < SEGMENT_HEADER_LEN {
            self.status = ReaderStatus::Torn { at: self.buf.len() };
            self.stopped = true;
            return Err(Error::IncompleteSegmentHeader {
                available: self.buf.len(),
            });
        }
        match SegmentHeader::decode(&self.buf[..SEGMENT_HEADER_LEN]) {
            Ok(header) => {
                self.header = Some(header);
                self.pos = SEGMENT_HEADER_LEN;
                Ok(header)
            }
            Err(error) => {
                self.status = ReaderStatus::Damaged { at: 0 };
                self.stopped = true;
                Err(error)
            }
        }
    }

    /// The next valid record, or `None` if the file is over (cleanly, torn or
    /// damaged — see [`SegmentReader::status`]).
    pub fn next_record(&mut self) -> Option<Record<'a>> {
        if self.stopped {
            return None;
        }
        let header = match self.header() {
            Ok(header) => header,
            Err(_) => return None,
        };
        let remaining = self.buf.len() - self.pos;
        if remaining == 0 {
            self.stopped = true;
            return None;
        }
        if remaining < RECORD_HEADER_LEN {
            // Less than a record header: no valid record can start here, this
            // can only be a torn tail.
            self.status = ReaderStatus::Torn { at: self.pos };
            self.stopped = true;
            return None;
        }

        let expected = Some(match self.last_seq {
            Some(seq) => seq + 1,
            None => header.seq_first,
        });
        let record = match self.parse_at(self.pos, expected) {
            Ok(record) => Some(record),
            Err(_) => self.resync(),
        };
        match record {
            Some(record) => {
                self.accept(record);
                Some(record)
            }
            None => {
                self.stopped = true;
                None
            }
        }
    }

    fn parse_at(&self, offset: usize, expected: Option<u64>) -> Result<Record<'a>, Error> {
        let record = parse_record_at(self.buf, offset)?;
        if let Some(expected) = expected
            && record.header.seq != expected
        {
            return Err(Error::UnexpectedSeq {
                expected,
                actual: record.header.seq,
            });
        }
        Ok(record)
    }

    fn accept(&mut self, record: Record<'a>) {
        self.last_seq = Some(record.header.seq);
        self.pos += record.record_len();
        self.last_valid_offset = self.pos;
        self.records += 1;
    }

    /// Scans byte by byte from the current position for the next valid record.
    /// Returns it and records the damage in the status.
    fn resync(&mut self) -> Option<Record<'a>> {
        let start = self.pos;
        let min_seq = match self.last_seq {
            Some(seq) => seq + 1,
            None => self.header.map_or(0, |header| header.seq_first),
        };
        let mut candidate = start + 1;
        while candidate + RECORD_HEADER_LEN <= self.buf.len() {
            if self.buf[candidate..candidate + 4] == RECORD_MAGIC_BYTES
                && let Ok(record) = parse_record_at(self.buf, candidate)
                // After lost records seq jumps forward; it may never go back.
                && record.header.seq >= min_seq
            {
                if self.status.is_clean() {
                    self.status = ReaderStatus::Damaged { at: start };
                }
                self.pos = candidate;
                return Some(record);
            }
            candidate += 1;
        }

        let magic_present =
            start + 4 <= self.buf.len() && self.buf[start..start + 4] == RECORD_MAGIC_BYTES;
        self.status = if magic_present {
            ReaderStatus::Torn { at: start }
        } else {
            ReaderStatus::Damaged { at: start }
        };
        None
    }

    /// How reading ended (or is ending so far).
    pub fn status(&self) -> ReaderStatus {
        self.status
    }

    /// How many records have been read.
    pub fn records_read(&self) -> u64 {
        self.records
    }

    /// Offset just past the last whole record that was read.
    ///
    /// This is the checkpoint anchor: `seek(byte_offset)` instead of a scan.
    pub fn last_valid_offset(&self) -> usize {
        self.last_valid_offset
    }

    /// `seq` of the last record that was read.
    pub fn last_seq(&self) -> Option<u64> {
        self.last_seq
    }

    /// `true` if the reader will not yield any more records.
    pub fn is_stopped(&self) -> bool {
        self.stopped
    }
}

impl<'a> Iterator for SegmentReader<'a> {
    type Item = Record<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        self.next_record()
    }
}
