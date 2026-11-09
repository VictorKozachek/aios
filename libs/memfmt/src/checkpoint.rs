//! Checkpoint (§5.3) and canonical CBOR (§5.12).
//!
//! A checkpoint is the envelope from §5.3:
//!
//! ```text
//! { seq, segment_id, byte_offset,          // where the log ends: resume = seek
//!   transcript_head_hash,
//!   kv: Option<{key_hash, tokens, bytes, crc}>,
//!   facts: [fact_id…],
//!   budgets: { tokens_in, tokens_out, cost_us, wall_ms },
//!   model_fp }
//! ```
//!
//! The record payload (`kind = 8`, §5.2) is encoded per §5.12: the first byte is
//! the schema version, then CBOR in deterministic form (definite-length, map
//! keys sorted by length and then lexicographically, floats forbidden, numbers
//! in the minimal representation). The codec lives behind the `cbor` feature
//! and requires an allocator: "the kernel does not parse CBOR at all" (§5.12).
//!
//! The field order in the structs below is not a matter of taste: `serde`
//! writes map keys in declaration order, while §5.12 requires sorting by
//! (key length, bytes). Fields are therefore declared in canonical order, not
//! in the order of §5.3.

#[cfg(feature = "cbor")]
use alloc::vec::Vec;
#[cfg(feature = "cbor")]
use serde::{Deserialize, Serialize};

#[cfg(feature = "cbor")]
use crate::error::Error;
#[cfg(feature = "cbor")]
use crate::record::{Record, RecordKind, align8};
#[cfg(feature = "cbor")]
use crate::writer::{Appended, SegmentStore, Writer};

/// Checkpoint payload schema version — the first payload byte (§5.12).
///
/// A version mismatch is a parsing refusal, not a "best effort".
pub const CHECKPOINT_SCHEMA_VERSION: u8 = 1;

/// Session budgets at checkpoint time (§5.3).
///
/// Fields are declared in canonical key order (§5.12):
/// `cost_us` (7), `wall_ms` (7), `tokens_in` (9), `tokens_out` (10).
#[cfg(feature = "cbor")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Budgets {
    /// Spent microseconds of CPU/inference.
    pub cost_us: u64,
    /// Spent wall-clock time, ms.
    pub wall_ms: u64,
    /// Input tokens.
    pub tokens_in: u64,
    /// Output tokens.
    pub tokens_out: u64,
}

/// Reference to a KV-cache snapshot (§5.3; key format — §5.4).
///
/// Fields are declared in canonical key order (§5.12):
/// `crc` (3), `bytes` (5), `tokens` (6), `key_hash` (8).
#[cfg(feature = "cbor")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvSnapshotRef {
    /// `crc32c` of the snapshot file.
    pub crc: u32,
    /// Snapshot size, bytes.
    pub bytes: u64,
    /// Number of prefix tokens covered by the snapshot.
    pub tokens: u64,
    /// Hash of the `SnapshotKey` (§5.4) — 32 bytes.
    #[serde(with = "serde_hash32")]
    pub key_hash: [u8; 32],
}

/// Session checkpoint (§5.3).
///
/// Fields are declared in canonical key order (§5.12):
/// `kv` (2), `seq` (3), `facts` (5), `budgets` (7), `model_fp` (8),
/// `segment_id` (10), `byte_offset` (11), `transcript_head_hash` (20).
/// When `kv` is `None` it is not written to the map at all, which is
/// deterministic.
#[cfg(feature = "cbor")]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkpoint {
    /// KV-cache snapshot, if there was one at checkpoint time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kv: Option<KvSnapshotRef>,
    /// `seq` of the last record covered by this checkpoint.
    pub seq: u64,
    /// Identifiers of the facts that went into the context (`Fact.id`, 16
    /// bytes, §5.7).
    #[serde(with = "serde_facts")]
    pub facts: Vec<[u8; 16]>,
    /// Session budgets.
    pub budgets: Budgets,
    /// Model fingerprint: blake3 of the weights manifest (`weights_hash`,
    /// §5.4), 32 bytes.
    #[serde(with = "serde_hash32")]
    pub model_fp: [u8; 32],
    /// Segment file number where the log ends (`log/000042.seg` → 42).
    pub segment_id: u64,
    /// Offset in that file: resume = seek, not a scan.
    pub byte_offset: u64,
    /// Transcript head hash at checkpoint time, 32 bytes.
    #[serde(with = "serde_hash32")]
    pub transcript_head_hash: [u8; 32],
}

#[cfg(feature = "cbor")]
impl Checkpoint {
    /// Encodes the payload of a `kind = 8` record.
    ///
    /// Format: `[schema version][canonical CBOR][zeros up to a multiple of 8]`.
    /// Zero alignment is needed because record `len` must be a multiple of 8
    /// (§5.2) while the CBOR length is arbitrary; the decoder tells alignment
    /// from data by the length of the CBOR value.
    pub fn encode_payload(&self) -> Result<Vec<u8>, Error> {
        let mut cbor = Vec::new();
        ciborium::ser::into_writer(self, &mut cbor).map_err(|_| Error::CborEncode)?;
        let consumed = canonical::check(&cbor).map_err(Error::NonCanonicalCbor)?;
        if consumed != cbor.len() {
            return Err(Error::NonCanonicalCbor(
                "trailing bytes after the CBOR value",
            ));
        }

        let mut payload = Vec::with_capacity(align8(1 + cbor.len()));
        payload.push(CHECKPOINT_SCHEMA_VERSION);
        payload.extend_from_slice(&cbor);
        payload.resize(align8(payload.len()), 0);
        Ok(payload)
    }

    /// Decodes the payload of a `kind = 8` record.
    ///
    /// Checks per §5.12: the schema version byte and the canonical CBOR form.
    /// Zero alignment in the tail is allowed; any other garbage is an error.
    pub fn decode_payload(payload: &[u8]) -> Result<Self, Error> {
        let version = *payload.first().ok_or(Error::SchemaVersionMissing)?;
        if version != CHECKPOINT_SCHEMA_VERSION {
            return Err(Error::UnsupportedSchemaVersion {
                stored: version,
                supported: CHECKPOINT_SCHEMA_VERSION,
            });
        }
        let body = &payload[1..];
        let consumed = canonical::check(body).map_err(Error::NonCanonicalCbor)?;
        if body[consumed..].iter().any(|byte| *byte != 0) {
            return Err(Error::TrailingBytes { at: 1 + consumed });
        }
        ciborium::de::from_reader(&body[..consumed]).map_err(|_| Error::CborDecode)
    }

    /// Extracts a checkpoint from a segment record (the record must be
    /// `kind = 8`).
    pub fn from_record(record: &Record<'_>) -> Result<Self, Error> {
        if record.header.kind != RecordKind::Checkpoint.as_u8() {
            return Err(Error::UnexpectedRecordKind {
                expected: RecordKind::Checkpoint.as_u8(),
                actual: record.header.kind,
            });
        }
        Self::decode_payload(record.payload)
    }

    /// Writes the checkpoint to the log as a `kind = 8` record and returns its
    /// position.
    pub fn append<S: SegmentStore>(
        &self,
        writer: &mut Writer<S>,
        ts_ns: u64,
    ) -> Result<Appended, Error> {
        let payload = self.encode_payload()?;
        writer.append(RecordKind::Checkpoint, 0, ts_ns, &payload)
    }
}

/// Canonical CBOR (§5.12): form control, not schema control.
///
/// The validator knows nothing about checkpoint fields — it checks exactly what
/// makes canonicalization deterministic: definite lengths, minimal numbers, map
/// key ordering, no floats, UTF-8 in text strings.
#[cfg(feature = "cbor")]
pub mod canonical {
    /// Maximum nesting depth accepted by the validator.
    pub const MAX_DEPTH: u8 = 16;

    /// Checks that `bytes` is a single CBOR value in canonical form.
    ///
    /// Returns the length of the value in bytes: only zero alignment may follow
    /// in the payload. The refusal reason is a short string for reports.
    pub fn check(bytes: &[u8]) -> Result<usize, &'static str> {
        item(bytes, 0, 0)
    }

    /// Reads a CBOR argument (length/value) and checks that it is minimal.
    fn read_argument(bytes: &[u8], pos: usize, info: u8) -> Result<(u64, usize), &'static str> {
        match info {
            0..=23 => Ok((u64::from(info), pos)),
            24 => {
                let byte = *bytes.get(pos).ok_or("CBOR truncated")?;
                if byte < 24 {
                    return Err("number is not minimally encoded");
                }
                Ok((u64::from(byte), pos + 1))
            }
            25 => {
                let raw = bytes.get(pos..pos + 2).ok_or("CBOR truncated")?;
                let value = u64::from(u16::from_be_bytes([raw[0], raw[1]]));
                if value <= 0xFF {
                    return Err("number is not minimally encoded");
                }
                Ok((value, pos + 2))
            }
            26 => {
                let raw = bytes.get(pos..pos + 4).ok_or("CBOR truncated")?;
                let value = u64::from(u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]));
                if value <= 0xFFFF {
                    return Err("number is not minimally encoded");
                }
                Ok((value, pos + 4))
            }
            27 => {
                let raw = bytes.get(pos..pos + 8).ok_or("CBOR truncated")?;
                let value = u64::from_be_bytes([
                    raw[0], raw[1], raw[2], raw[3], raw[4], raw[5], raw[6], raw[7],
                ]);
                if value <= 0xFFFF_FFFF {
                    return Err("number is not minimally encoded");
                }
                Ok((value, pos + 8))
            }
            28..=30 => Err("reserved additional info"),
            31 => Err("indefinite-length is not allowed in canonical form"),
            _ => Err("invalid additional info"),
        }
    }

    fn bounds(bytes: &[u8], start: usize, len: u64) -> Result<usize, &'static str> {
        let len = usize::try_from(len).map_err(|_| "length does not fit into usize")?;
        let end = start.checked_add(len).ok_or("CBOR length overflow")?;
        if end > bytes.len() {
            return Err("CBOR truncated");
        }
        Ok(end)
    }

    fn item(bytes: &[u8], pos: usize, depth: u8) -> Result<usize, &'static str> {
        if depth > MAX_DEPTH {
            return Err("CBOR nesting too deep");
        }
        let initial = *bytes.get(pos).ok_or("CBOR truncated")?;
        let major = initial >> 5;
        let info = initial & 0x1F;
        if info == 31 {
            return Err("indefinite-length is not allowed in canonical form");
        }
        match major {
            // Unsigned/negative integers.
            0 | 1 => {
                let (_value, next) = read_argument(bytes, pos + 1, info)?;
                Ok(next)
            }
            // Byte strings.
            2 => {
                let (len, next) = read_argument(bytes, pos + 1, info)?;
                bounds(bytes, next, len)
            }
            // Text strings: UTF-8 here; NFC is the serializer's concern.
            3 => {
                let (len, next) = read_argument(bytes, pos + 1, info)?;
                let end = bounds(bytes, next, len)?;
                core::str::from_utf8(&bytes[next..end])
                    .map_err(|_| "text string is not valid UTF-8")?;
                Ok(end)
            }
            // Arrays.
            4 => {
                let (len, mut next) = read_argument(bytes, pos + 1, info)?;
                if len > bytes.len() as u64 {
                    return Err("array length exceeds input");
                }
                for _ in 0..len {
                    next = item(bytes, next, depth + 1)?;
                }
                Ok(next)
            }
            // Maps: keys must be text, ordered by (length, bytes).
            5 => {
                let (len, mut next) = read_argument(bytes, pos + 1, info)?;
                if len > bytes.len() as u64 {
                    return Err("map length exceeds input");
                }
                let mut previous: Option<&[u8]> = None;
                for _ in 0..len {
                    let key_initial = *bytes.get(next).ok_or("CBOR truncated")?;
                    if key_initial >> 5 != 3 {
                        return Err("map key must be a text string");
                    }
                    let (key_len, key_start) = read_argument(bytes, next + 1, key_initial & 0x1F)?;
                    let key_end = bounds(bytes, key_start, key_len)?;
                    let key = &bytes[key_start..key_end];
                    core::str::from_utf8(key).map_err(|_| "map key is not valid UTF-8")?;
                    if let Some(previous) = previous
                        && (key.len() < previous.len()
                            || (key.len() == previous.len() && key <= previous))
                    {
                        return Err("map keys are not in canonical order (length, then bytes)");
                    }
                    previous = Some(key);
                    next = item(bytes, key_end, depth + 1)?;
                }
                Ok(next)
            }
            // Tags: allowed, the content is checked as a regular value.
            6 => {
                let (_tag, next) = read_argument(bytes, pos + 1, info)?;
                item(bytes, next, depth + 1)
            }
            // Simple values and floats.
            7 => match info {
                0..=23 => Ok(pos + 1),
                24 => {
                    if bytes.len() < pos + 2 {
                        return Err("CBOR truncated");
                    }
                    Ok(pos + 2)
                }
                25..=27 => Err("float is not allowed in hashed structures (§5.12)"),
                _ => Err("invalid additional info"),
            },
            _ => Err("invalid major type"),
        }
    }
}

#[cfg(feature = "cbor")]
mod serde_hash32 {
    use alloc::vec::Vec;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &[u8; 32], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(value)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<[u8; 32], D::Error> {
        let bytes = Vec::<u8>::deserialize(deserializer)?;
        <[u8; 32]>::try_from(bytes.as_slice())
            .map_err(|_| serde::de::Error::custom("expected 32 bytes"))
    }
}

#[cfg(feature = "cbor")]
mod serde_facts {
    use alloc::vec::Vec;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    /// CBOR byte string without an intermediate allocation.
    struct Bytes<'a>(&'a [u8]);

    impl Serialize for Bytes<'_> {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            serializer.serialize_bytes(self.0)
        }
    }

    pub fn serialize<S: Serializer>(facts: &[[u8; 16]], serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        let mut seq = serializer.serialize_seq(Some(facts.len()))?;
        for fact in facts {
            seq.serialize_element(&Bytes(fact))?;
        }
        seq.end()
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<[u8; 16]>, D::Error> {
        let items = Vec::<Vec<u8>>::deserialize(deserializer)?;
        items
            .into_iter()
            .map(|item| {
                <[u8; 16]>::try_from(item.as_slice())
                    .map_err(|_| serde::de::Error::custom("expected a 16-byte fact_id"))
            })
            .collect()
    }
}
