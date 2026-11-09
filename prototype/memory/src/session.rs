//! Episodic memory: session logs as `libs/memfmt` segments (ARCHITECTURE §5.2).
//!
//! One session is a directory with numbered segments:
//!
//! ```text
//! sessions/20261109-190000/
//!   ├── 000001.seg   header + records (magic/len/crc32c/kind/seq/ts)
//!   └── 000002.seg   opened on rotation or when the console resumes the session
//! ```
//!
//! Every record carries the format invariants of §5.2, so a torn write is
//! recovered by the resync scan of [`memfmt::SegmentReader`]: the prefix stays
//! readable, the tail is reported as [`memfmt::ReaderStatus::Torn`]. The old
//! flat `sessions/<ts>.ndjson` log of the first prototype stage is gone; the
//! format is the same one `storaged` will use in the kernel.
//!
//! A running process cannot append to a segment it did not open (the writer
//! owns the file), so resuming a session opens the next segment in the same
//! directory with `prev_crc` of the previous file. A session is thus a *set* of
//! segments, and the file-level crc chain detects a deleted segment.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use aios_modeld::Message;
use memfmt::{ReaderStatus, RecordKind, SegmentKind, SegmentReader, Writer};
use serde::{Deserialize, Serialize};

use crate::error::MemoryError;
use crate::store;
use crate::time;

/// Records per segment before the session opens the next one.
///
/// §5.1 allows 16 MiB per segment; the prototype rotates earlier so that one
/// session visibly consists of several segments in tests and on disk.
pub const RECORDS_PER_SEGMENT: u64 = 256;

/// Segment file extension.
pub const SEGMENT_EXTENSION: &str = "seg";

/// One event of a session log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    /// A new session was created.
    SessionStart {
        /// Workspace name the session belongs to.
        workspace: String,
        /// Model that answers in this session.
        model: String,
    },
    /// The console reopened an existing session (a new segment starts).
    SessionResume {
        /// Model that answers in this session.
        model: String,
    },
    /// The session was closed.
    SessionEnd {
        /// Why it was closed: `quit`, `switch`, `new`, `eof`, ...
        reason: String,
    },
    /// Human message.
    User {
        /// Message text.
        text: String,
    },
    /// Model answer with the usage counters ollama reported.
    Assistant {
        /// Answer text.
        text: String,
        /// Model name.
        model: String,
        /// Tokens in the prompt.
        prompt_tokens: u64,
        /// Generated tokens.
        completion_tokens: u64,
    },
    /// Console command (audited, useful for retrieval too).
    Command {
        /// The command line as typed.
        command: String,
    },
    /// A model-layer error.
    Error {
        /// Error message.
        message: String,
    },
}

impl Event {
    /// Record kind of the segment format this event maps to (§5.2 table).
    pub fn record_kind(&self) -> RecordKind {
        match self {
            Event::User { .. } => RecordKind::UserIn,
            Event::Assistant { .. } => RecordKind::ModelOut,
            Event::Command { .. } => RecordKind::Policy,
            Event::Error { .. } => RecordKind::Error,
            Event::SessionStart { .. } | Event::SessionResume { .. } | Event::SessionEnd { .. } => {
                RecordKind::SessionState
            }
        }
    }

    /// Text this event contributes to the BM25 index, if any.
    pub fn indexed_text(&self) -> Option<&str> {
        match self {
            Event::User { text } => Some(text),
            Event::Assistant { text, .. } => Some(text),
            Event::Command { command } => Some(command),
            Event::Error { message } => Some(message),
            Event::SessionStart { .. } | Event::SessionResume { .. } | Event::SessionEnd { .. } => {
                None
            }
        }
    }
}

/// Serializes an event into a record payload.
fn encode_payload(event: &Event) -> Result<Vec<u8>, MemoryError> {
    store::encode_json_padded(event)
}

fn decode_payload(bytes: &[u8]) -> Result<Event, MemoryError> {
    store::decode_json_padded(bytes)
}

/// Append-only writer of one session.
#[derive(Debug)]
pub struct SessionWriter {
    dir: PathBuf,
    id: String,
    workspace: String,
    model: String,
    segment_id: u64,
    next_seq: u64,
    records_in_segment: u64,
    writer: Option<Writer<store::FileStore>>,
}

impl SessionWriter {
    /// Starts a new session under `sessions_dir`.
    pub fn create(
        sessions_dir: &Path,
        workspace: &str,
        model: &str,
        now: SystemTime,
    ) -> Result<Self, MemoryError> {
        fs::create_dir_all(sessions_dir).map_err(|error| {
            MemoryError::io(format!("создать {}", sessions_dir.display()), error)
        })?;
        let id = unique_session_id(sessions_dir, now);
        let dir = sessions_dir.join(&id);
        fs::create_dir_all(&dir)
            .map_err(|error| MemoryError::io(format!("создать {}", dir.display()), error))?;
        let mut session = SessionWriter {
            dir,
            id,
            workspace: workspace.to_string(),
            model: model.to_string(),
            segment_id: 1,
            next_seq: 1,
            records_in_segment: 0,
            writer: None,
        };
        session.writer = Some(open_segment(&session.dir, 1, 1, now, 0)?);
        session.append(&Event::SessionStart {
            workspace: workspace.to_string(),
            model: model.to_string(),
        })?;
        Ok(session)
    }

    /// Reopens an existing session directory, starting its next segment.
    pub fn resume(
        session_dir: &Path,
        workspace: &str,
        model: &str,
        now: SystemTime,
    ) -> Result<Self, MemoryError> {
        let (last_segment_id, last_seq, prev_crc) = last_segment_state(session_dir)?;
        let id = session_dir
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        let mut session = SessionWriter {
            dir: session_dir.to_path_buf(),
            id,
            workspace: workspace.to_string(),
            model: model.to_string(),
            segment_id: last_segment_id + 1,
            next_seq: last_seq + 1,
            records_in_segment: 0,
            writer: None,
        };
        session.writer = Some(open_segment(
            &session.dir,
            session.segment_id,
            session.next_seq,
            now,
            prev_crc,
        )?);
        session.append(&Event::SessionResume {
            model: model.to_string(),
        })?;
        Ok(session)
    }

    /// Opens the newest session of the workspace or starts a new one.
    ///
    /// Returns the writer and the session as it was on disk before this run.
    pub fn open_latest_or_new(
        sessions_dir: &Path,
        workspace: &str,
        model: &str,
    ) -> Result<(Self, LoadedSession), MemoryError> {
        let now = time::now();
        match latest_session_dir(sessions_dir)? {
            Some(dir) => {
                let loaded = load_session(&dir)?;
                let writer = SessionWriter::resume(&dir, workspace, model, now)?;
                Ok((writer, loaded))
            }
            None => {
                let writer = SessionWriter::create(sessions_dir, workspace, model, now)?;
                let loaded = load_session(writer.dir())?;
                Ok((writer, loaded))
            }
        }
    }

    /// Appends an event and returns its `seq`.
    pub fn append(&mut self, event: &Event) -> Result<u64, MemoryError> {
        if self.records_in_segment >= RECORDS_PER_SEGMENT {
            self.rotate()?;
        }
        let payload = encode_payload(event)?;
        let writer = self
            .writer
            .as_mut()
            .ok_or_else(|| MemoryError::corrupt("записать событие", "сегмент сессии закрыт"))?;
        let appended = writer
            .append(event.record_kind(), 0, time::now_ns(), &payload)
            .map_err(|error| MemoryError::segment("записать событие сессии", error))?;
        self.records_in_segment += 1;
        self.next_seq = appended.seq + 1;
        Ok(appended.seq)
    }

    /// Closes the session with a reason.
    pub fn finish(&mut self, reason: &str) -> Result<(), MemoryError> {
        self.append(&Event::SessionEnd {
            reason: reason.to_string(),
        })?;
        Ok(())
    }

    fn rotate(&mut self) -> Result<(), MemoryError> {
        let prev_crc = self
            .writer
            .as_ref()
            .map(|writer| writer.segment_crc())
            .unwrap_or(0);
        let next_id = self.segment_id + 1;
        let writer = open_segment(&self.dir, next_id, self.next_seq, time::now(), prev_crc)?;
        self.writer = Some(writer);
        self.segment_id = next_id;
        self.records_in_segment = 0;
        Ok(())
    }

    /// Session id (the directory name).
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Session directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// `seq` that the next record will get.
    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    /// `seq` of the last written record, `0` if none.
    pub fn last_seq(&self) -> u64 {
        self.next_seq.saturating_sub(1)
    }

    /// Number of open segments so far.
    pub fn segment_id(&self) -> u64 {
        self.segment_id
    }

    /// Workspace this session belongs to.
    pub fn workspace(&self) -> &str {
        &self.workspace
    }

    /// Model that answers in this session.
    pub fn model(&self) -> &str {
        &self.model
    }
}

/// A session as loaded from disk.
#[derive(Debug, Clone)]
pub struct LoadedSession {
    /// Session id (directory name).
    pub id: String,
    /// Session directory.
    pub dir: PathBuf,
    /// Dialogue restored from the log.
    pub messages: Vec<Message>,
    /// All decoded events with their record `seq`.
    pub events: Vec<(u64, Event)>,
    /// Prompt tokens reported by the backend over the session.
    pub prompt_tokens: u64,
    /// Completion tokens reported by the backend over the session.
    pub completion_tokens: u64,
    /// Number of segment files.
    pub segments: usize,
    /// Reader status per segment, in file order.
    pub statuses: Vec<ReaderStatus>,
    /// Number of valid records read.
    pub records: usize,
    /// Number of `seq` gaps spotted across segments.
    pub seq_gaps: usize,
    /// Creation time from the first segment header.
    pub created_ns: Option<u64>,
}

impl LoadedSession {
    /// `true` if no segment was damaged (a torn tail is a normal crash case).
    pub fn is_healthy(&self) -> bool {
        !self.statuses.iter().any(|status| status.is_damaged())
    }

    /// `true` if any segment has a torn tail or lost records.
    pub fn has_losses(&self) -> bool {
        self.seq_gaps > 0 || self.statuses.iter().any(|status| !status.is_clean())
    }

    /// Human-readable path of the segment directory.
    pub fn dir_display(&self) -> String {
        self.dir.display().to_string()
    }
}

/// Summary used by `/resume` and `/status`.
#[derive(Debug, Clone)]
pub struct SessionMeta {
    /// Session id.
    pub id: String,
    /// Session directory.
    pub dir: PathBuf,
    /// Number of messages (user + assistant).
    pub messages: usize,
    /// Number of segments.
    pub segments: usize,
    /// Creation time from the first segment header.
    pub created_ns: Option<u64>,
    /// `true` if no segment was damaged.
    pub healthy: bool,
    /// `true` if a torn tail or a `seq` gap was found.
    pub has_losses: bool,
}

/// Loads all segments of one session.
pub fn load_session(dir: &Path) -> Result<LoadedSession, MemoryError> {
    let files = segment_files(dir)?;
    let mut messages = Vec::new();
    let mut events = Vec::new();
    let mut prompt_tokens = 0;
    let mut completion_tokens = 0;
    let mut statuses = Vec::new();
    let mut records = 0_usize;
    let mut seq_gaps = 0_usize;
    let mut previous_seq: Option<u64> = None;
    let mut created_ns = None;

    for path in &files {
        let bytes = fs::read(path)
            .map_err(|error| MemoryError::io(format!("читать {}", path.display()), error))?;
        let mut reader = SegmentReader::new(&bytes);
        match reader.header() {
            Ok(header) => {
                if created_ns.is_none() {
                    created_ns = Some(header.created_ns);
                }
            }
            Err(_) => {
                // A header that does not parse is damage, not a torn write.
                statuses.push(ReaderStatus::Damaged { at: 0 });
                continue;
            }
        }
        for record in reader.by_ref() {
            records += 1;
            if let Some(previous) = previous_seq
                && record.header.seq != previous + 1
            {
                seq_gaps += 1;
            }
            previous_seq = Some(record.header.seq);
            if let Ok(event) = decode_payload(record.payload) {
                match &event {
                    Event::User { text } => messages.push(Message::user(text.clone())),
                    Event::Assistant {
                        text,
                        prompt_tokens: prompt,
                        completion_tokens: completion,
                        ..
                    } => {
                        messages.push(Message::assistant(text.clone()));
                        prompt_tokens += prompt;
                        completion_tokens += completion;
                    }
                    _ => {}
                }
                events.push((record.header.seq, event));
            }
        }
        statuses.push(reader.status());
    }

    Ok(LoadedSession {
        id: dir
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default(),
        dir: dir.to_path_buf(),
        messages,
        events,
        prompt_tokens,
        completion_tokens,
        segments: files.len(),
        statuses,
        records,
        seq_gaps,
        created_ns,
    })
}

/// Lists sessions of a workspace, oldest first.
///
/// A session whose segments cannot be read is still listed (marked unhealthy),
/// because `/resume` must show what exists on disk.
pub fn list_sessions(sessions_dir: &Path) -> Result<Vec<SessionMeta>, MemoryError> {
    let mut sessions = Vec::new();
    let entries = match fs::read_dir(sessions_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(sessions),
        Err(error) => {
            return Err(MemoryError::io(
                format!("читать {}", sessions_dir.display()),
                error,
            ));
        }
    };
    for entry in entries {
        let entry = entry.map_err(|error| MemoryError::io("читать сессии", error))?;
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let id = entry.file_name().to_string_lossy().to_string();
        let meta = match load_session(&dir) {
            Ok(loaded) => SessionMeta {
                id,
                dir,
                messages: loaded.messages.len(),
                segments: loaded.segments,
                created_ns: loaded.created_ns,
                healthy: loaded.is_healthy(),
                has_losses: loaded.has_losses(),
            },
            Err(_) => SessionMeta {
                id,
                dir,
                messages: 0,
                segments: 0,
                created_ns: None,
                healthy: false,
                has_losses: true,
            },
        };
        sessions.push(meta);
    }
    sessions.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(sessions)
}

/// Loads the newest session of the workspace, if any.
pub fn load_latest(sessions_dir: &Path) -> Result<Option<LoadedSession>, MemoryError> {
    match latest_session_dir(sessions_dir)? {
        Some(dir) => Ok(Some(load_session(&dir)?)),
        None => Ok(None),
    }
}

fn latest_session_dir(sessions_dir: &Path) -> Result<Option<PathBuf>, MemoryError> {
    Ok(list_sessions(sessions_dir)?.pop().map(|meta| meta.dir))
}

fn unique_session_id(sessions_dir: &Path, now: SystemTime) -> String {
    let stamp = time::file_stamp(now);
    let mut id = stamp.clone();
    let mut counter = 2;
    while sessions_dir.join(&id).exists() {
        id = format!("{stamp}-{counter}");
        counter += 1;
    }
    id
}

fn segment_file_name(segment_id: u64) -> String {
    format!("{segment_id:06}.{SEGMENT_EXTENSION}")
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
        let entry = entry.map_err(|error| MemoryError::io("читать сегменты", error))?;
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) == Some(SEGMENT_EXTENSION) {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn open_segment(
    dir: &Path,
    segment_id: u64,
    seq_first: u64,
    created: SystemTime,
    prev_crc: u32,
) -> Result<Writer<store::FileStore>, MemoryError> {
    let path = dir.join(segment_file_name(segment_id));
    let store = store::FileStore::create_new(&path)?;
    Writer::create(
        store,
        SegmentKind::Session,
        segment_id,
        seq_first,
        time::to_nanos(created),
        prev_crc,
    )
    .map_err(|error| MemoryError::segment(format!("открыть сегмент {}", path.display()), error))
}

/// Returns `(last segment id, last seq, crc32c of the last segment file)`.
fn last_segment_state(dir: &Path) -> Result<(u64, u64, u32), MemoryError> {
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
                "прочитать сегмент",
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
    use super::{
        Event, LoadedSession, RECORDS_PER_SEGMENT, SessionWriter, list_sessions, load_latest,
        load_session,
    };
    use std::time::{Duration, UNIX_EPOCH};

    fn time_2026() -> std::time::SystemTime {
        // 2026-11-09T18:36:07Z
        UNIX_EPOCH + Duration::from_secs(1_794_249_367)
    }

    fn sessions_dir(dir: &tempfile::TempDir) -> std::path::PathBuf {
        dir.path().join("sessions")
    }

    #[test]
    fn session_roundtrip_through_segments() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sessions = sessions_dir(&dir);
        let mut writer = SessionWriter::create(&sessions, "work", "qwen2.5:0.5b", time_2026())
            .expect("create session");
        writer
            .append(&Event::User {
                text: "привет".into(),
            })
            .expect("user");
        writer
            .append(&Event::Assistant {
                text: "здравствуй".into(),
                model: "qwen2.5:0.5b".into(),
                prompt_tokens: 9,
                completion_tokens: 4,
            })
            .expect("assistant");
        writer.finish("quit").expect("finish");
        let session_dir = writer.dir().to_path_buf();
        drop(writer);

        // The segment file really is the memfmt format of ARCHITECTURE §5.2.
        let segment = session_dir.join("000001.seg");
        let bytes = std::fs::read(&segment).expect("segment bytes");
        assert!(bytes.len() >= memfmt::SEGMENT_HEADER_LEN);
        assert_eq!(&bytes[0..8], b"AIOSSEG1");

        let loaded = load_session(&session_dir).expect("load session");
        assert_eq!(loaded.messages.len(), 2);
        assert_eq!(loaded.messages[0].content, "привет");
        assert_eq!(loaded.messages[1].content, "здравствуй");
        assert_eq!(loaded.prompt_tokens, 9);
        assert_eq!(loaded.completion_tokens, 4);
        assert_eq!(loaded.segments, 1);
        assert!(loaded.statuses.iter().all(|status| status.is_clean()));
        assert!(loaded.is_healthy());
        assert!(!loaded.has_losses());
        assert_eq!(loaded.seq_gaps, 0);
        // session_start, user, assistant, session_end
        assert_eq!(loaded.records, 4);
    }

    #[test]
    fn rotation_opens_the_next_segment_with_a_crc_chain() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sessions = sessions_dir(&dir);
        let mut writer =
            SessionWriter::create(&sessions, "work", "m", time_2026()).expect("create");
        for index in 0..RECORDS_PER_SEGMENT {
            writer
                .append(&Event::User {
                    text: format!("сообщение {index}"),
                })
                .expect("append");
        }
        // session_start + 255 user records fill the first segment; the 256th
        // user record opens the second one.
        assert_eq!(writer.segment_id(), 2);
        writer.finish("quit").expect("finish");
        let session_dir = writer.dir().to_path_buf();
        drop(writer);

        // RECORDS_PER_SEGMENT + session_start fills one segment, then rotation.
        let segments: Vec<_> = std::fs::read_dir(&session_dir)
            .expect("list")
            .map(|entry| entry.expect("entry").path())
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("seg"))
            .collect();
        assert_eq!(segments.len(), 2);

        let loaded = load_session(&session_dir).expect("load");
        assert_eq!(loaded.segments, 2);
        assert!(loaded.statuses.iter().all(|status| status.is_clean()));
        assert_eq!(loaded.seq_gaps, 0, "seq must be continuous across segments");
        assert_eq!(loaded.messages.len() as u64, RECORDS_PER_SEGMENT);

        // Second segment knows the crc of the first one (file-level chain).
        let first = std::fs::read(session_dir.join("000001.seg")).expect("first");
        let second = std::fs::read(session_dir.join("000002.seg")).expect("second");
        let header = memfmt::SegmentHeader::decode(&second).expect("header");
        assert_eq!(header.prev_crc, memfmt::crc32c(&first));
    }

    #[test]
    fn resume_continues_the_session_in_a_new_segment() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sessions = sessions_dir(&dir);
        let mut writer =
            SessionWriter::create(&sessions, "work", "m", time_2026()).expect("create");
        writer
            .append(&Event::User {
                text: "до".into()
            })
            .expect("user");
        let session_dir = writer.dir().to_path_buf();
        let id = writer.id().to_string();
        writer.finish("quit").expect("finish");
        drop(writer);

        let mut resumed =
            SessionWriter::resume(&session_dir, "work", "m", time_2026()).expect("resume");
        assert_eq!(resumed.id(), id);
        assert_eq!(resumed.segment_id(), 2, "resume opens a new segment");
        resumed
            .append(&Event::User {
                text: "после".into(),
            })
            .expect("user");
        resumed.finish("quit").expect("finish");
        drop(resumed);

        let loaded = load_session(&session_dir).expect("load");
        assert_eq!(loaded.segments, 2);
        assert_eq!(loaded.messages.len(), 2);
        assert_eq!(loaded.messages[0].content, "до");
        assert_eq!(loaded.messages[1].content, "после");
        assert_eq!(loaded.seq_gaps, 0);
    }

    #[test]
    fn torn_tail_keeps_the_prefix_and_is_reported() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sessions = sessions_dir(&dir);
        let mut writer =
            SessionWriter::create(&sessions, "work", "m", time_2026()).expect("create");
        writer
            .append(&Event::User {
                text: "первое".into(),
            })
            .expect("user");
        writer
            .append(&Event::User {
                text: "второе".into(),
            })
            .expect("user");
        writer
            .append(&Event::User {
                text: "третье".into(),
            })
            .expect("user");
        let session_dir = writer.dir().to_path_buf();
        drop(writer);

        // Cut the tail of the last record — exactly what a crash leaves behind.
        let segment = session_dir.join("000001.seg");
        let bytes = std::fs::read(&segment).expect("bytes");
        std::fs::write(&segment, &bytes[..bytes.len() - 5]).expect("truncate");

        let loaded = load_session(&session_dir).expect("load");
        // The cut record is lost; everything before it stays readable.
        assert_eq!(loaded.messages.len(), 2, "the whole prefix stays readable");
        assert!(
            loaded.statuses.iter().any(|status| status.is_torn()),
            "torn tail must be reported, got {:?}",
            loaded.statuses
        );
        assert!(loaded.is_healthy(), "a torn tail is not mid-file damage");
        assert!(loaded.has_losses());
    }

    #[test]
    fn damaged_middle_loses_one_record_and_is_reported() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sessions = sessions_dir(&dir);
        let mut writer =
            SessionWriter::create(&sessions, "work", "m", time_2026()).expect("create");
        for text in ["раз", "два", "три", "четыре"] {
            writer
                .append(&Event::User { text: text.into() })
                .expect("user");
        }
        let session_dir = writer.dir().to_path_buf();
        drop(writer);

        // Corrupt a payload byte in the middle of the file.
        let segment = session_dir.join("000001.seg");
        let mut bytes = std::fs::read(&segment).expect("bytes");
        let position = bytes.len() / 2;
        bytes[position] ^= 0x01;
        std::fs::write(&segment, &bytes).expect("write");

        let loaded = load_session(&session_dir).expect("load");
        assert!(
            loaded.statuses.iter().any(|status| status.is_damaged()),
            "damage must be reported, got {:?}",
            loaded.statuses
        );
        assert!(!loaded.is_healthy());
        assert!(!loaded.messages.is_empty());
        assert!(loaded.messages.len() < 4, "one record is lost");
    }

    #[test]
    fn sessions_are_listed_oldest_first_and_latest_resumes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sessions = sessions_dir(&dir);
        let first = SessionWriter::create(&sessions, "work", "m", time_2026()).expect("first");
        let first_dir = first.dir().to_path_buf();
        drop(first);

        std::thread::sleep(std::time::Duration::from_millis(1100));
        let mut second =
            SessionWriter::create(&sessions, "work", "m", crate::time::now()).expect("second");
        second
            .append(&Event::User {
                text: "новее".into(),
            })
            .expect("user");
        let second_dir = second.dir().to_path_buf();
        drop(second);

        let listed = list_sessions(&sessions).expect("list");
        assert_eq!(listed.len(), 2);
        assert!(listed[0].id < listed[1].id);
        assert_eq!(listed[1].dir, second_dir);

        let latest: LoadedSession = load_latest(&sessions).expect("latest").expect("some");
        assert_eq!(latest.dir, second_dir);
        assert_ne!(latest.dir, first_dir);
        assert_eq!(latest.messages.len(), 1);
    }

    #[test]
    fn session_ids_do_not_collide_within_one_second() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sessions = sessions_dir(&dir);
        let first = SessionWriter::create(&sessions, "work", "m", time_2026()).expect("first");
        let second = SessionWriter::create(&sessions, "work", "m", time_2026()).expect("second");
        assert_ne!(first.id(), second.id());
        assert_eq!(list_sessions(&sessions).expect("list").len(), 2);
    }
}
