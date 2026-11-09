//! Memory format tests on the host (`cargo test -p memfmt`).
//!
//! The mandatory T5 set:
//! * record read/write round-trip;
//! * truncating the file at **every** byte: the prefix stays intact, the tail
//!   is marked torn;
//! * a corrupted byte in the middle → damaged;
//! * `len > MAX_RECORD` is rejected;
//! * checkpoint round-trip (`--features cbor`);
//! * `cargo check --target x86_64-unknown-none -p memfmt` — without that
//!   feature.

use std::vec;
use std::vec::Vec;

use crate::*;

/// In-memory segment store: one vector element is one segment file.
#[derive(Debug, Default)]
struct MemStore {
    segments: Vec<Vec<u8>>,
}

impl MemStore {
    fn new() -> Self {
        MemStore {
            segments: vec![Vec::new()],
        }
    }

    fn segment(&self, index: usize) -> &[u8] {
        &self.segments[index]
    }

    fn current(&self) -> &[u8] {
        self.segment(self.segments.len() - 1)
    }

    fn segment_count(&self) -> usize {
        self.segments.len()
    }
}

impl SegmentStore for MemStore {
    fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.segments
            .last_mut()
            .expect("a segment is open")
            .extend_from_slice(bytes);
        Ok(())
    }

    fn rotate(&mut self) -> Result<(), Error> {
        self.segments.push(Vec::new());
        Ok(())
    }
}

/// Payloads of different lengths (all multiples of 8 — the §5.2 invariant).
fn payloads() -> Vec<Vec<u8>> {
    vec![
        vec![0x11; 8],
        vec![0x22; 16],
        vec![0x33; 24],
        vec![0x44; 40],
    ]
}

/// Start offsets of the records in a segment holding the given payloads.
fn record_starts(payloads: &[Vec<u8>]) -> Vec<usize> {
    let mut starts = Vec::with_capacity(payloads.len());
    let mut offset = SEGMENT_HEADER_LEN;
    for payload in payloads {
        starts.push(offset);
        offset += RECORD_HEADER_LEN + payload.len();
    }
    starts
}

/// Builds one segment with records `seq = 1..` and returns its bytes.
fn build_segment(payloads: &[Vec<u8>]) -> Vec<u8> {
    let mut writer =
        Writer::create(MemStore::new(), SegmentKind::Session, 1, 1, 1_000, 0).expect("create");
    for (index, payload) in payloads.iter().enumerate() {
        writer
            .append(RecordKind::UserIn, 0, 10 + index as u64, payload)
            .expect("append");
    }
    writer.into_inner().current().to_vec()
}

#[test]
fn crc32c_known_vectors() {
    // CRC-32C reference value (RFC 3720 / iSCSI).
    assert_eq!(crc32c(b"123456789"), 0xE306_9283);
    assert_eq!(crc32c(b""), 0);
    // Streaming recomputation matches the one-shot value.
    let whole = crc32c(b"123456789");
    let split = crc32c_update(crc32c(b"1234"), b"56789");
    assert_eq!(whole, split);
    // A single flipped bit changes the crc.
    assert_ne!(crc32c(b"123456788"), whole);
}

#[test]
fn layout_constants_match_document() {
    assert_eq!(SEGMENT_MAGIC, *b"AIOSSEG1");
    assert_eq!(SEGMENT_HEADER_LEN, 64);
    assert_eq!(SEGMENT_RESERVED_LEN, 16);
    assert_eq!(
        SEGMENT_HEADER_LEN,
        8 + 2 + 2 + 8 + 8 + 4 + 4 + SEGMENT_RESERVED_LEN + SEGMENT_PADDING_LEN
    );
    assert_eq!(RECORD_MAGIC, 0x4149_4F53);
    assert_eq!(RECORD_MAGIC_BYTES, *b"SOIA"); // u32 0x41494F53 little-endian on disk
    assert_eq!(RECORD_HEADER_LEN, 32);
    assert_eq!(RECORD_HEADER_LEN % 8, 0);
    assert_eq!(MAX_RECORD, 1024 * 1024);
    assert_eq!(MAX_RECORD % 8, 0);
    assert_eq!(SEGMENT_MAX_BYTES, 16 * 1024 * 1024);
    assert_eq!(align8(0), 0);
    assert_eq!(align8(1), 8);
    assert_eq!(align8(8), 8);
    assert_eq!(align8(9), 16);
}

#[test]
fn segment_header_is_64_bytes_and_roundtrips() {
    let mut header = SegmentHeader::new(SegmentKind::Audit, 41, 123_456_789, 0xDEAD_BEEF);
    header.reserved[3] = 0x7F;
    let bytes = header.encode();

    assert_eq!(bytes.len(), 64);
    assert_eq!(&bytes[0..8], b"AIOSSEG1");
    assert_eq!(u16::from_le_bytes([bytes[8], bytes[9]]), SEGMENT_VERSION);
    assert_eq!(u16::from_le_bytes([bytes[10], bytes[11]]), 2); // audit
    assert_eq!(u64::from_le_bytes(bytes[12..20].try_into().unwrap()), 41);
    assert_eq!(
        u64::from_le_bytes(bytes[20..28].try_into().unwrap()),
        123_456_789
    );
    assert_eq!(
        u32::from_le_bytes(bytes[28..32].try_into().unwrap()),
        0xDEAD_BEEF
    );
    assert_eq!(&bytes[52..64], &[0u8; 12]); // alignment up to 64 bytes

    let decoded = SegmentHeader::decode(&bytes).expect("decode");
    assert_eq!(decoded, header);
    assert_eq!(decoded.reserved[3], 0x7F);
    // The stored hdr_crc is exactly what the header fields compute to.
    assert_eq!(
        u32::from_le_bytes(bytes[32..36].try_into().unwrap()),
        decoded.hdr_crc()
    );

    // A short header is a torn header, not a magic error.
    assert_eq!(
        SegmentHeader::decode(&bytes[..63]).unwrap_err(),
        Error::IncompleteSegmentHeader { available: 63 }
    );

    // Corrupting any field breaks the crc.
    let mut broken = bytes;
    broken[20] ^= 0x01;
    assert!(matches!(
        SegmentHeader::decode(&broken),
        Err(Error::BadHeaderCrc { .. })
    ));

    let mut bad_magic = bytes;
    bad_magic[0] = b'X';
    assert_eq!(
        SegmentHeader::decode(&bad_magic).unwrap_err(),
        Error::BadSegmentMagic
    );

    // Version and kind are checked separately from the crc: recompute hdr_crc.
    let rewrite_with_valid_crc = |bytes: &mut [u8; 64]| {
        bytes[32..36].fill(0);
        let crc = crc32c(&bytes[..]);
        bytes[32..36].copy_from_slice(&crc.to_le_bytes());
    };

    let mut future_version = bytes;
    future_version[8..10].copy_from_slice(&2u16.to_le_bytes());
    rewrite_with_valid_crc(&mut future_version);
    assert_eq!(
        SegmentHeader::decode(&future_version).unwrap_err(),
        Error::UnsupportedSegmentVersion {
            stored: 2,
            supported: SEGMENT_VERSION
        }
    );

    let mut unknown_kind = bytes;
    unknown_kind[10..12].copy_from_slice(&9u16.to_le_bytes());
    rewrite_with_valid_crc(&mut unknown_kind);
    assert_eq!(
        SegmentHeader::decode(&unknown_kind).unwrap_err(),
        Error::UnknownSegmentKind { stored: 9 }
    );
}

#[test]
fn record_layout_matches_document() {
    let payload = [0xABu8; 16];
    let header = RecordHeader::new(RecordKind::Policy.as_u8(), FLAG_LZ4, 5, 99, &payload);
    let encoded = header.encode();

    assert_eq!(encoded.len(), RECORD_HEADER_LEN);
    assert_eq!(&encoded[0..4], &RECORD_MAGIC_BYTES);
    assert_eq!(u32::from_le_bytes(encoded[4..8].try_into().unwrap()), 16);
    assert_eq!(
        u32::from_le_bytes(encoded[8..12].try_into().unwrap()),
        crc32c(&payload)
    );
    assert_eq!(encoded[12], RecordKind::Policy.as_u8());
    assert_eq!(encoded[13], FLAG_LZ4);
    assert_eq!(&encoded[14..16], &[0, 0]); // seq/ts_ns alignment
    assert_eq!(u64::from_le_bytes(encoded[16..24].try_into().unwrap()), 5);
    assert_eq!(u64::from_le_bytes(encoded[24..32].try_into().unwrap()), 99);
    assert_eq!((RECORD_HEADER_LEN + payload.len()) % 8, 0);

    let decoded = RecordHeader::decode(&encoded).expect("decode");
    assert_eq!(decoded, header);
    assert!(decoded.is_lz4());
    assert!(!decoded.has_external_blob());
    assert_eq!(decoded.record_len(), RECORD_HEADER_LEN + 16);
}

#[test]
fn writer_reader_roundtrip() {
    let payloads = payloads();
    let mut writer = Writer::create(
        MemStore::new(),
        SegmentKind::Session,
        3,
        100,
        42,
        0x1234_5678,
    )
    .expect("create");
    let mut appended = Vec::new();
    for (index, payload) in payloads.iter().enumerate() {
        appended.push(
            writer
                .append(
                    RecordKind::ModelOut,
                    FLAG_EXTERNAL_BLOB,
                    1_000 + index as u64,
                    payload,
                )
                .expect("append"),
        );
    }
    let position = writer.position();
    let store = writer.into_inner();
    let bytes = store.current();

    assert_eq!(position.segment_id, 3);
    assert_eq!(position.next_seq, 104);
    assert_eq!(position.byte_offset as usize, bytes.len());
    assert_eq!(appended[0].byte_offset, SEGMENT_HEADER_LEN as u64);
    assert_eq!(appended[0].seq, 100);
    assert_eq!(appended[3].record_len, RECORD_HEADER_LEN + 40);

    let header = SegmentHeader::decode(bytes).expect("header");
    assert_eq!(header.kind, SegmentKind::Session);
    assert_eq!(header.seq_first, 100);
    assert_eq!(header.created_ns, 42);
    assert_eq!(header.prev_crc, 0x1234_5678);

    let mut reader = SegmentReader::new(bytes);
    assert_eq!(reader.header().expect("header"), header);
    let records: Vec<Record<'_>> = reader.by_ref().collect();
    assert_eq!(records.len(), payloads.len());
    for (index, record) in records.iter().enumerate() {
        assert_eq!(record.header.seq, 100 + index as u64);
        assert_eq!(record.header.kind, RecordKind::ModelOut.as_u8());
        assert_eq!(record.header.flags, FLAG_EXTERNAL_BLOB);
        assert_eq!(record.header.ts_ns, 1_000 + index as u64);
        assert_eq!(record.payload, &payloads[index][..]);
        assert!(record.header.has_external_blob());
        assert!(!record.header.is_lz4());
    }
    assert_eq!(reader.status(), ReaderStatus::Clean);
    assert_eq!(reader.records_read(), payloads.len() as u64);
    assert_eq!(reader.last_valid_offset(), bytes.len());
    assert_eq!(reader.last_seq(), Some(103));
}

#[test]
fn truncation_at_every_byte_keeps_prefix_and_marks_torn() {
    let payloads = payloads();
    let full = build_segment(&payloads);
    let starts = record_starts(&payloads);
    let mut ends = Vec::new();
    for (start, payload) in starts.iter().zip(&payloads) {
        ends.push(start + RECORD_HEADER_LEN + payload.len());
    }
    assert_eq!(*ends.last().unwrap(), full.len());

    // How many records fit entirely into a prefix of length `len`.
    let complete = |len: usize| ends.iter().filter(|&&end| end <= len).count();

    for len in 0..=full.len() {
        let prefix = &full[..len];
        let mut reader = SegmentReader::new(prefix);
        let records: Vec<Record<'_>> = reader.by_ref().collect();

        let expected = complete(len);
        assert_eq!(
            records.len(),
            expected,
            "len={len}: read {} records, expected {expected}",
            records.len()
        );
        // The prefix is intact: everything read matches the original.
        for (index, record) in records.iter().enumerate() {
            assert_eq!(record.header.seq, 1 + index as u64, "len={len}");
            assert_eq!(record.payload, &payloads[index][..], "len={len}");
        }

        // Truncation never counts as damage.
        assert!(
            !reader.status().is_damaged(),
            "len={len}: truncation must not produce damaged, got {:?}",
            reader.status()
        );

        let at_boundary = len == SEGMENT_HEADER_LEN || ends.contains(&len);
        if at_boundary {
            assert_eq!(reader.status(), ReaderStatus::Clean, "len={len}");
        } else if len < SEGMENT_HEADER_LEN {
            assert_eq!(reader.status(), ReaderStatus::Torn { at: len }, "len={len}");
        } else {
            assert_eq!(
                reader.status(),
                ReaderStatus::Torn {
                    at: starts[expected]
                },
                "len={len}"
            );
        }
    }
}

#[test]
fn corrupted_payload_in_middle_is_damaged_and_reading_continues() {
    let payloads = payloads();
    let mut bytes = build_segment(&payloads);
    let starts = record_starts(&payloads);
    let damaged_at = starts[1];
    // Corrupt one payload byte of the second record: magic is in place, crc
    // will not match.
    bytes[damaged_at + RECORD_HEADER_LEN + 3] ^= 0x01;

    let mut reader = SegmentReader::new(&bytes);
    let seqs: Vec<u64> = reader.by_ref().map(|record| record.header.seq).collect();

    assert_eq!(seqs, vec![1, 3, 4]);
    assert_eq!(reader.status(), ReaderStatus::Damaged { at: damaged_at });
    assert_eq!(reader.records_read(), 3);
    // Recovery reached the end of the file: only one record was lost.
    assert_eq!(reader.last_valid_offset(), bytes.len());
}

#[test]
fn corrupted_record_magic_in_middle_is_damaged() {
    let payloads = payloads();
    let mut bytes = build_segment(&payloads);
    let starts = record_starts(&payloads);
    let damaged_at = starts[1];
    bytes[damaged_at] ^= 0xFF; // magic destroyed

    let mut reader = SegmentReader::new(&bytes);
    let seqs: Vec<u64> = reader.by_ref().map(|record| record.header.seq).collect();
    assert_eq!(seqs, vec![1, 3, 4]);
    assert_eq!(reader.status(), ReaderStatus::Damaged { at: damaged_at });
}

#[test]
fn garbage_after_last_record_is_damaged_not_torn() {
    let payloads = payloads();
    let mut bytes = build_segment(&payloads);
    let end = bytes.len();
    bytes.extend_from_slice(&[0xAA; 40]); // garbage without magic on the remainder

    let mut reader = SegmentReader::new(&bytes);
    let seqs: Vec<u64> = reader.by_ref().map(|record| record.header.seq).collect();
    assert_eq!(seqs, vec![1, 2, 3, 4]);
    assert_eq!(reader.status(), ReaderStatus::Damaged { at: end });
}

#[test]
fn truncated_last_record_is_torn() {
    let payloads = payloads();
    let bytes = build_segment(&payloads);
    let starts = record_starts(&payloads);
    let cut = bytes.len() - 5;
    let prefix = &bytes[..cut];

    let mut reader = SegmentReader::new(prefix);
    let seqs: Vec<u64> = reader.by_ref().map(|record| record.header.seq).collect();
    assert_eq!(seqs, vec![1, 2, 3]);
    assert_eq!(reader.status(), ReaderStatus::Torn { at: starts[3] });
}

#[test]
fn broken_seq_is_detected_and_resync_continues() {
    let payloads = payloads();
    let mut bytes = build_segment(&payloads);
    let starts = record_starts(&payloads);
    let damaged_at = starts[1];
    // The crc covers only the payload, so seq can be corrupted silently.
    bytes[damaged_at + 16..damaged_at + 24].copy_from_slice(&99u64.to_le_bytes());

    let mut reader = SegmentReader::new(&bytes);
    let seqs: Vec<u64> = reader.by_ref().map(|record| record.header.seq).collect();
    assert_eq!(seqs, vec![1, 3, 4]);
    assert_eq!(reader.status(), ReaderStatus::Damaged { at: damaged_at });
}

#[test]
fn damaged_segment_header_is_reported() {
    let payloads = payloads();
    let mut bytes = build_segment(&payloads);
    bytes[24] ^= 0x01; // created_ns inside the header

    let mut reader = SegmentReader::new(&bytes);
    assert!(matches!(reader.header(), Err(Error::BadHeaderCrc { .. })));
    assert_eq!(reader.status(), ReaderStatus::Damaged { at: 0 });
    assert_eq!(reader.next_record(), None);
}

#[test]
fn record_len_over_max_is_rejected() {
    // Parser: len above MAX_RECORD is refused before reading the payload.
    let mut bytes = [0u8; RECORD_HEADER_LEN];
    bytes[0..4].copy_from_slice(&RECORD_MAGIC_BYTES);
    bytes[4..8].copy_from_slice(&(MAX_RECORD as u32 + 8).to_le_bytes());
    let mut buffer = bytes.to_vec();
    buffer.resize(RECORD_HEADER_LEN + MAX_RECORD + 8, 0);
    assert_eq!(
        parse_record_at(&buffer, 0).unwrap_err(),
        Error::RecordLenTooLarge {
            len: MAX_RECORD as u32 + 8,
            max: MAX_RECORD as u32,
        }
    );

    // Parser: len not a multiple of 8 is also refused (resync invariant).
    let mut unaligned = [0u8; RECORD_HEADER_LEN];
    unaligned[0..4].copy_from_slice(&RECORD_MAGIC_BYTES);
    unaligned[4..8].copy_from_slice(&12u32.to_le_bytes());
    let mut buffer = unaligned.to_vec();
    buffer.resize(RECORD_HEADER_LEN + 12, 0);
    assert_eq!(
        parse_record_at(&buffer, 0).unwrap_err(),
        Error::RecordLenUnaligned { len: 12 }
    );

    // Writer: the same limit and alignment.
    let mut writer =
        Writer::create(MemStore::new(), SegmentKind::Session, 1, 1, 0, 0).expect("create");
    let oversized = vec![0u8; MAX_RECORD + 8];
    assert_eq!(
        writer
            .append(RecordKind::UserIn, 0, 0, &oversized)
            .unwrap_err(),
        Error::PayloadTooLarge {
            len: MAX_RECORD + 8,
            max: MAX_RECORD,
        }
    );
    let unaligned = vec![0u8; 12];
    assert_eq!(
        writer
            .append(RecordKind::UserIn, 0, 0, &unaligned)
            .unwrap_err(),
        Error::PayloadUnaligned { len: 12 }
    );
    // After the refusals the writer is intact and writes a normal record.
    writer
        .append(RecordKind::UserIn, 0, 0, &[0u8; 8])
        .expect("append after refusals");
}

#[test]
fn max_record_size_record_roundtrips() {
    let payload = vec![0x5Au8; MAX_RECORD];
    let mut writer =
        Writer::create(MemStore::new(), SegmentKind::Session, 1, 1, 0, 0).expect("create");
    let appended = writer
        .append(RecordKind::ModelOut, 0, 7, &payload)
        .expect("append");
    assert_eq!(appended.record_len, RECORD_HEADER_LEN + MAX_RECORD);
    let store = writer.into_inner();

    let mut reader = SegmentReader::new(store.current());
    let records: Vec<Record<'_>> = reader.by_ref().collect();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].payload.len(), MAX_RECORD);
    assert_eq!(records[0].payload, &payload[..]);
    assert!(reader.status().is_clean());
}

#[test]
fn rotation_at_16_mib_links_segments_with_prev_crc() {
    let payload = vec![0x5A; MAX_RECORD];
    let mut writer =
        Writer::create(MemStore::new(), SegmentKind::Session, 1, 7, 42, 0).expect("create");
    for index in 0..16u64 {
        writer
            .append(RecordKind::UserIn, 0, 100 + index, &payload)
            .expect("append");
    }
    let position = writer.position();
    let store = writer.into_inner();

    // At 16 MiB: 15 records in the first segment, the 16th does not fit.
    assert_eq!(store.segment_count(), 2);
    let first = store.segment(0);
    let second = store.segment(1);
    assert_eq!(
        first.len(),
        SEGMENT_HEADER_LEN + 15 * (RECORD_HEADER_LEN + MAX_RECORD)
    );
    assert!(first.len() as u64 <= SEGMENT_MAX_BYTES);
    assert_eq!(
        second.len(),
        SEGMENT_HEADER_LEN + RECORD_HEADER_LEN + MAX_RECORD
    );

    let second_header = SegmentHeader::decode(second).expect("header");
    // File chain: prev_crc = crc32c of the whole previous file.
    assert_eq!(second_header.prev_crc, crc32c(first));
    assert_eq!(second_header.seq_first, 7 + 15);
    assert_eq!(second_header.created_ns, 100 + 15);
    assert_eq!(second_header.kind, SegmentKind::Session);

    assert_eq!(position.segment_id, 2);
    assert_eq!(position.next_seq, 7 + 16);
    assert_eq!(
        position.byte_offset as usize,
        SEGMENT_HEADER_LEN + RECORD_HEADER_LEN + MAX_RECORD
    );

    // Both segments read back completely, seq is continuous.
    let mut seqs = Vec::new();
    for bytes in [first, second] {
        let mut reader = SegmentReader::new(bytes);
        for record in reader.by_ref() {
            seqs.push(record.header.seq);
        }
        assert_eq!(reader.status(), ReaderStatus::Clean);
    }
    assert_eq!(seqs, (7..23).collect::<Vec<u64>>());
}

#[cfg(feature = "cbor")]
mod cbor_codec {
    use std::vec;
    use std::vec::Vec;

    use super::{MemStore, build_segment};
    use crate::*;

    fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack
            .windows(needle.len())
            .position(|window| window == needle)
    }

    fn sample_checkpoint() -> Checkpoint {
        Checkpoint {
            kv: Some(KvSnapshotRef {
                crc: 0xDEAD_BEEF,
                bytes: 123_456,
                tokens: 4_096,
                key_hash: [7u8; 32],
            }),
            seq: 42,
            facts: vec![[1u8; 16], [2u8; 16], [3u8; 16]],
            budgets: Budgets {
                cost_us: 1_500_000,
                wall_ms: 250,
                tokens_in: 8_192,
                tokens_out: 512,
            },
            model_fp: [9u8; 32],
            segment_id: 7,
            byte_offset: 123_456,
            transcript_head_hash: [0xABu8; 32],
        }
    }

    #[test]
    fn checkpoint_roundtrip_through_log() {
        let checkpoint = sample_checkpoint();
        let mut writer =
            Writer::create(MemStore::new(), SegmentKind::Session, 7, 43, 555, 0).expect("create");
        let appended = checkpoint.append(&mut writer, 777).expect("append");
        assert_eq!(appended.seq, 43);
        let store = writer.into_inner();

        let mut reader = SegmentReader::new(store.current());
        let records: Vec<Record<'_>> = reader.by_ref().collect();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].header.kind, RecordKind::Checkpoint.as_u8());
        // Payload is 8-byte aligned and starts with the schema version (§5.12).
        assert_eq!(records[0].payload.len() % 8, 0);
        assert_eq!(records[0].payload[0], CHECKPOINT_SCHEMA_VERSION);

        let decoded = Checkpoint::from_record(&records[0]).expect("decode");
        assert_eq!(decoded, checkpoint);
        assert!(reader.status().is_clean());

        // Without kv: the field is absent from the map, everything else stays.
        let without_kv = Checkpoint {
            kv: None,
            ..checkpoint.clone()
        };
        let payload = without_kv.encode_payload().expect("encode");
        assert_eq!(
            Checkpoint::decode_payload(&payload).expect("decode"),
            without_kv
        );
        assert_eq!(payload[0], CHECKPOINT_SCHEMA_VERSION);
        assert_eq!(payload.len() % 8, 0);
        // kv=None is not written to the map at all; kv=Some is.
        assert!(find(&payload[1..], b"kv").is_none());
        let with_kv = sample_checkpoint().encode_payload().expect("encode");
        assert!(find(&with_kv[1..], b"kv").is_some());
    }

    #[test]
    fn checkpoint_rejects_wrong_version_and_garbage() {
        let payload = sample_checkpoint().encode_payload().expect("encode");

        let mut wrong_version = payload.clone();
        wrong_version[0] = 2;
        assert_eq!(
            Checkpoint::decode_payload(&wrong_version).unwrap_err(),
            Error::UnsupportedSchemaVersion {
                stored: 2,
                supported: CHECKPOINT_SCHEMA_VERSION,
            }
        );

        assert_eq!(
            Checkpoint::decode_payload(&[]).unwrap_err(),
            Error::SchemaVersionMissing
        );

        let mut trailing = payload.clone();
        trailing.push(0xFF);
        trailing.resize(align8(trailing.len()), 0xFF);
        assert!(matches!(
            Checkpoint::decode_payload(&trailing),
            Err(Error::TrailingBytes { .. })
        ));

        // A record of the wrong kind is refused instead of parsed as a foreign
        // payload.
        let other = build_segment(&[vec![0u8; 8]]);
        let record = parse_record_at(&other, SEGMENT_HEADER_LEN).expect("record");
        assert_eq!(
            Checkpoint::from_record(&record).unwrap_err(),
            Error::UnexpectedRecordKind {
                expected: RecordKind::Checkpoint.as_u8(),
                actual: RecordKind::UserIn.as_u8(),
            }
        );
    }

    #[test]
    fn canonical_check_accepts_encoder_output_and_rejects_violations() {
        // ciborium output is canonical and the keys are sorted by (length,
        // bytes).
        let payload = sample_checkpoint().encode_payload().expect("encode");
        let body = &payload[1..];
        let consumed = canonical::check(body).expect("canonical CBOR");
        assert!(body[consumed..].iter().all(|byte| *byte == 0));
        let keys: [&[u8]; 8] = [
            b"kv",
            b"seq",
            b"facts",
            b"budgets",
            b"model_fp",
            b"segment_id",
            b"byte_offset",
            b"transcript_head_hash",
        ];
        let mut previous = 0;
        for key in keys {
            let position = find(&body[..consumed], key).expect("key present in the payload");
            assert!(position >= previous, "keys are not in canonical order");
            previous = position;
        }

        // Indefinite-length array.
        assert!(canonical::check(&[0x9F, 0x01, 0xFF]).is_err());
        // Floats are forbidden in hashed structures.
        assert!(canonical::check(&[0xA1, 0x61, b'x', 0xFA, 0x3F, 0x80, 0x00, 0x00]).is_err());
        // Non-minimal integer.
        assert!(canonical::check(&[0x18, 0x05]).is_err());
        // Map keys out of order ({"bb": 0, "a": 1}).
        assert!(canonical::check(&[0xA2, 0x62, b'b', b'b', 0x00, 0x61, b'a', 0x01]).is_err());
        // Duplicate key.
        assert!(canonical::check(&[0xA2, 0x61, b'a', 0x00, 0x61, b'a', 0x01]).is_err());
        // Text that is not UTF-8.
        assert!(canonical::check(&[0x61, 0xFF]).is_err());
        // Truncated input.
        assert!(canonical::check(&[0x82, 0x01]).is_err());

        // A correct map passes and reports the value length.
        let good = [0xA2, 0x61, b'a', 0x01, 0x62, b'b', b'b', 0x02];
        assert_eq!(canonical::check(&good), Ok(good.len()));
    }
}
