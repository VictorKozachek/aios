//! Audit segment: append-only `libs/memfmt` log of memory operations.
//!
//! `/remember` and `/forget` change what the model will see, so both leave a
//! record in `memory/audit/…seg` — the prototype version of the hash-chained
//! audit log of ARCHITECTURE §7.4 (the blake3 chain itself is a later card).
//! The segment uses [`memfmt::SegmentKind::Audit`]; records are ordinary
//! `mem_write` entries with a JSON payload.

use std::fs;
use std::path::{Path, PathBuf};

use memfmt::{RecordKind, SegmentKind, SegmentReader, Writer};
use serde::{Deserialize, Serialize};

use crate::error::MemoryError;
use crate::store::{self, FileStore};
use crate::time;

/// Directory of the audit segments inside `memory/`.
pub const AUDIT_DIR: &str = "audit";

/// Records per audit segment before rotation.
pub const RECORDS_PER_SEGMENT: u64 = 4096;

/// One audited memory operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEvent {
    /// `remember` / `forget`.
    pub action: String,
    /// Fact id the action touched.
    pub fact_id: String,
    /// Who did it (`human`, `model`, `console`).
    pub by: String,
    /// Time of the action, nanoseconds since the Unix epoch.
    pub ts_ns: u64,
}

/// Append-only audit log of one workspace.
#[derive(Debug)]
pub struct AuditLog {
    dir: PathBuf,
    next_segment_id: u64,
    next_seq: u64,
    prev_crc: u32,
    records_in_segment: u64,
    writer: Option<Writer<FileStore>>,
}

impl AuditLog {
    /// Opens the audit log of a workspace, ready for appending.
    pub fn open(workspace_root: &Path) -> Result<Self, MemoryError> {
        let dir = workspace_root.join("memory").join(AUDIT_DIR);
        fs::create_dir_all(&dir)
            .map_err(|error| MemoryError::io(format!("создать {}", dir.display()), error))?;
        let (last_segment_id, last_seq, prev_crc) = last_state(&dir)?;
        Ok(AuditLog {
            dir,
            next_segment_id: last_segment_id + 1,
            next_seq: last_seq + 1,
            prev_crc,
            records_in_segment: 0,
            writer: None,
        })
    }

    /// Appends an audit event and returns its record `seq`.
    pub fn append(&mut self, action: &str, fact_id: &str, by: &str) -> Result<u64, MemoryError> {
        if self.writer.is_none() {
            self.open_segment()?;
        }
        if self.records_in_segment >= RECORDS_PER_SEGMENT {
            self.rotate()?;
        }
        let event = AuditEvent {
            action: action.to_string(),
            fact_id: fact_id.to_string(),
            by: by.to_string(),
            ts_ns: time::now_ns(),
        };
        let payload = store::encode_json_padded(&event)?;
        let writer = self
            .writer
            .as_mut()
            .ok_or_else(|| MemoryError::corrupt("записать аудит", "сегмент аудита закрыт"))?;
        let appended = writer
            .append(RecordKind::MemWrite, 0, time::now_ns(), &payload)
            .map_err(|error| MemoryError::segment("записать аудит", error))?;
        self.records_in_segment += 1;
        self.next_seq = appended.seq + 1;
        Ok(appended.seq)
    }

    /// Reads all audit events of all segments, in record order.
    pub fn events(&self) -> Result<Vec<(u64, AuditEvent)>, MemoryError> {
        let mut events = Vec::new();
        for path in segment_files(&self.dir)? {
            let bytes = fs::read(&path)
                .map_err(|error| MemoryError::io(format!("читать {}", path.display()), error))?;
            let mut reader = SegmentReader::new(&bytes);
            for record in reader.by_ref() {
                if let Ok(event) = store::decode_json_padded::<AuditEvent>(record.payload) {
                    events.push((record.header.seq, event));
                }
            }
        }
        Ok(events)
    }

    /// Directory of the audit segments (useful for `/status`).
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn open_segment(&mut self) -> Result<(), MemoryError> {
        let segment_id = self.next_segment_id;
        let path = self.dir.join(format!("{segment_id:06}.seg"));
        let store = FileStore::create_new(&path)?;
        let writer = Writer::create(
            store,
            SegmentKind::Audit,
            segment_id,
            self.next_seq,
            time::now_ns(),
            self.prev_crc,
        )
        .map_err(|error| MemoryError::segment(format!("открыть {}", path.display()), error))?;
        self.writer = Some(writer);
        self.records_in_segment = 0;
        Ok(())
    }

    fn rotate(&mut self) -> Result<(), MemoryError> {
        self.prev_crc = self
            .writer
            .as_ref()
            .map(|writer| writer.segment_crc())
            .unwrap_or(self.prev_crc);
        self.next_segment_id += 1;
        self.writer = None;
        self.open_segment()
    }
}

fn segment_files(dir: &Path) -> Result<Vec<PathBuf>, MemoryError> {
    let mut files = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(files),
        Err(error) => {
            return Err(MemoryError::io(format!("читать {}", dir.display()), error));
        }
    };
    for entry in entries {
        let entry = entry.map_err(|error| MemoryError::io("читать аудит", error))?;
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) == Some("seg") {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn last_state(dir: &Path) -> Result<(u64, u64, u32), MemoryError> {
    let files = segment_files(dir)?;
    let Some(path) = files.last() else {
        return Ok((0, 0, 0));
    };
    let segment_id = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .and_then(|stem| stem.parse::<u64>().ok())
        .ok_or_else(|| {
            MemoryError::corrupt(
                "прочитать аудит",
                format!("неожиданное имя файла {}", path.display()),
            )
        })?;
    let bytes = fs::read(path)
        .map_err(|error| MemoryError::io(format!("читать {}", path.display()), error))?;
    let mut reader = SegmentReader::new(&bytes);
    let mut last_seq = 0;
    if let Ok(header) = reader.header() {
        last_seq = header.seq_first.saturating_sub(1);
    }
    for record in reader.by_ref() {
        last_seq = record.header.seq;
    }
    Ok((segment_id, last_seq, memfmt::crc32c(&bytes)))
}

#[cfg(test)]
mod tests {
    use super::AuditLog;

    #[test]
    fn remember_and_forget_are_audited_and_readable_after_reopen() {
        let dir = tempfile::tempdir().expect("tempdir");
        {
            let mut log = AuditLog::open(dir.path()).expect("open");
            log.append("remember", "abc", "human").expect("remember");
            log.append("forget", "abc", "human").expect("forget");
        }

        let log = AuditLog::open(dir.path()).expect("reopen");
        let events = log.events().expect("events");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].1.action, "remember");
        assert_eq!(events[0].1.fact_id, "abc");
        assert_eq!(events[1].1.action, "forget");
        assert_eq!(events[0].0 + 1, events[1].0, "seq is continuous");
        let segment = std::fs::read_dir(log.dir())
            .expect("list")
            .next()
            .expect("file")
            .expect("entry")
            .path();
        let bytes = std::fs::read(&segment).expect("bytes");
        assert_eq!(&bytes[0..8], b"AIOSSEG1");
    }

    #[test]
    fn appends_continue_in_a_new_segment_after_reopen() {
        let dir = tempfile::tempdir().expect("tempdir");
        {
            let mut log = AuditLog::open(dir.path()).expect("open");
            log.append("remember", "one", "human").expect("first");
        }
        let mut log = AuditLog::open(dir.path()).expect("reopen");
        log.append("remember", "two", "human").expect("second");
        let events = log.events().expect("events");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].0 + 1, events[1].0);
    }
}
