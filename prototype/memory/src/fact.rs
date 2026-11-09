//! Semantic memory: facts with provenance (docs/PROTOTYPE.md §4).
//!
//! A fact answers "who said it, where, when and how sure we are":
//!
//! ```text
//! { id, scope, subject, key, value,
//!   source { session, seq }, author, confidence, state,
//!   pinned, created_ns, ttl_ns }
//! ```
//!
//! Storage is `memory/facts.jsonl` of the workspace, one fact per line, written
//! atomically (temp file + rename), so `/remember` and `/forget` never leave a
//! half-written file. The prototype id is a FNV-1a digest of the content —
//! ARCHITECTURE §5.7 defines `keyed_blake3` for the real thing; the shape of the
//! field (`[16]` → 32 hex chars) is already the same.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::MemoryError;
use crate::time;

/// Who wrote the fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Author {
    /// A human typed `/remember`.
    Human,
    /// The model proposed the fact (always unverified until confirmed).
    Model,
}

impl Author {
    /// Short label for the console.
    pub fn as_str(self) -> &'static str {
        match self {
            Author::Human => "human",
            Author::Model => "model",
        }
    }
}

/// Fact lifecycle state (§5.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Confirmed by a human or by an executed tool.
    Verified,
    /// Proposed by the model, not confirmed.
    Unverified,
    /// Conflicting evidence was found.
    Disputed,
}

impl State {
    /// Short label for the console.
    pub fn as_str(self) -> &'static str {
        match self {
            State::Verified => "verified",
            State::Unverified => "unverified",
            State::Disputed => "disputed",
        }
    }
}

/// Where the fact came from: a session and a record `seq` inside it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    /// Session id the fact was learned in.
    pub session: String,
    /// Record `seq` of the message/command that produced it.
    pub seq: u64,
}

/// A fact of the workspace semantic memory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fact {
    /// Stable id (32 hex chars in the prototype, ARCHITECTURE §5.7 `[16]`).
    pub id: String,
    /// Scope: the prototype uses the workspace name.
    pub scope: String,
    /// Subject, e.g. `project:aios` or `user:victor`.
    pub subject: String,
    /// Key, e.g. `snapshot_format`.
    pub key: String,
    /// Value as text (JSON-tagged values are a later card).
    pub value: String,
    /// Provenance: session and record.
    pub source: Source,
    /// Author.
    pub author: Author,
    /// Confidence 0..=1 (`1.0` for human facts).
    #[serde(default = "default_confidence")]
    pub confidence: f32,
    /// Lifecycle state.
    pub state: State,
    /// Pinned facts are always considered for the context.
    #[serde(default)]
    pub pinned: bool,
    /// Creation time, nanoseconds since the Unix epoch.
    pub created_ns: u64,
    /// Optional time-to-live in nanoseconds.
    #[serde(default)]
    pub ttl_ns: Option<u64>,
}

fn default_confidence() -> f32 {
    1.0
}

impl Fact {
    /// Text used by BM25 and by the context builder.
    pub fn indexed_text(&self) -> String {
        format!("{} {} {}", self.subject, self.key, self.value)
    }

    /// One-line provenance label: `subject/key (author, session #seq)`.
    pub fn provenance(&self) -> String {
        format!(
            "{}/{} (автор: {}, сессия {} #{})",
            self.subject,
            self.key,
            self.author.as_str(),
            self.source.session,
            self.source.seq
        )
    }

    /// `true` if the TTL has expired relative to `now_ns`.
    pub fn is_expired(&self, now_ns: u64) -> bool {
        match self.ttl_ns {
            Some(ttl) => now_ns.saturating_sub(self.created_ns) > ttl,
            None => false,
        }
    }
}

/// `memory/facts.jsonl` of one workspace.
#[derive(Debug)]
pub struct FactStore {
    path: PathBuf,
    facts: Vec<Fact>,
}

impl FactStore {
    /// Opens (creating the directory if needed) the fact store of a workspace.
    pub fn open(workspace_root: &Path) -> Result<Self, MemoryError> {
        let memory_dir = workspace_root.join("memory");
        fs::create_dir_all(&memory_dir)
            .map_err(|error| MemoryError::io(format!("создать {}", memory_dir.display()), error))?;
        let path = memory_dir.join("facts.jsonl");
        let mut facts = Vec::new();
        if path.exists() {
            let text = fs::read_to_string(&path)
                .map_err(|error| MemoryError::io(format!("читать {}", path.display()), error))?;
            for (number, line) in text.lines().enumerate() {
                if line.trim().is_empty() {
                    continue;
                }
                let fact: Fact = serde_json::from_str(line).map_err(|error| {
                    MemoryError::json(
                        format!(
                            "разобрать факт (строка {}) в {}",
                            number + 1,
                            path.display()
                        ),
                        error,
                    )
                })?;
                facts.push(fact);
            }
        }
        Ok(FactStore { path, facts })
    }

    /// Path of the backing file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// All facts in insertion order.
    pub fn facts(&self) -> &[Fact] {
        &self.facts
    }

    /// Number of facts.
    pub fn len(&self) -> usize {
        self.facts.len()
    }

    /// `true` if the store is empty.
    pub fn is_empty(&self) -> bool {
        self.facts.is_empty()
    }

    /// Looks a fact up by id.
    pub fn get(&self, id: &str) -> Option<&Fact> {
        self.facts.iter().find(|fact| fact.id == id)
    }

    /// Pinned, non-expired facts.
    pub fn pinned(&self, now_ns: u64) -> Vec<&Fact> {
        self.facts
            .iter()
            .filter(|fact| fact.pinned && !fact.is_expired(now_ns))
            .collect()
    }

    /// Adds a fact and persists the whole file atomically.
    ///
    /// Re-remembering the same `(scope, subject, key)` replaces the value: the
    /// key is unique per workspace, so "remember" means "set", not "append a
    /// duplicate".
    pub fn add(&mut self, fact: Fact) -> Result<(), MemoryError> {
        self.facts.retain(|existing| {
            !(existing.scope == fact.scope
                && existing.subject == fact.subject
                && existing.key == fact.key)
                && existing.id != fact.id
        });
        self.facts.push(fact);
        self.save()
    }

    /// Removes a fact by id, returning it.
    pub fn remove(&mut self, id: &str) -> Result<Option<Fact>, MemoryError> {
        let Some(index) = self.facts.iter().position(|fact| fact.id == id) else {
            return Ok(None);
        };
        let removed = self.facts.remove(index);
        self.save()?;
        Ok(Some(removed))
    }

    /// Rewrites `facts.jsonl` through a temporary file and a rename.
    pub fn save(&self) -> Result<(), MemoryError> {
        let temporary = self.path.with_extension("jsonl.tmp");
        {
            let mut file = File::create(&temporary).map_err(|error| {
                MemoryError::io(format!("создать {}", temporary.display()), error)
            })?;
            for fact in &self.facts {
                let line = serde_json::to_string(fact)
                    .map_err(|error| MemoryError::json("сериализовать факт", error))?;
                writeln!(file, "{line}").map_err(|error| {
                    MemoryError::io(format!("писать {}", temporary.display()), error)
                })?;
            }
            file.sync_all().map_err(|error| {
                MemoryError::io(format!("сбросить {}", temporary.display()), error)
            })?;
        }
        fs::rename(&temporary, &self.path)
            .map_err(|error| MemoryError::io(format!("заменить {}", self.path.display()), error))?;
        Ok(())
    }
}

/// Computes the prototype fact id: two FNV-1a 64 digests over the content.
///
/// ARCHITECTURE §5.7 uses `keyed_blake3(device_key, …)[0..16]`; until the
/// prototype has a device key, FNV-1a gives the same 16-byte, hex-encoded shape
/// and is deterministic.
pub fn make_id(scope: &str, subject: &str, key: &str, value: &str, created_ns: u64) -> String {
    let content = format!("{scope}\u{1f}{subject}\u{1f}{key}\u{1f}{value}\u{1f}{created_ns}");
    let first = fnv1a_64(content.as_bytes(), 0xcbf2_9ce4_8422_2325);
    let second = fnv1a_64(content.as_bytes(), 0x9e37_79b9_7f4a_7c15);
    format!("{first:016x}{second:016x}")
}

/// Builds a human-authored fact (what `/remember` does).
pub fn human_fact(scope: &str, subject: &str, key: &str, value: &str, source: Source) -> Fact {
    let created_ns = time::now_ns();
    Fact {
        id: make_id(scope, subject, key, value, created_ns),
        scope: scope.to_string(),
        subject: subject.to_string(),
        key: key.to_string(),
        value: value.to_string(),
        source,
        author: Author::Human,
        confidence: 1.0,
        state: State::Verified,
        pinned: true,
        created_ns,
        ttl_ns: None,
    }
}

fn fnv1a_64(bytes: &[u8], seed: u64) -> u64 {
    let mut hash = seed;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::{Author, FactStore, Source, State, human_fact, make_id};

    fn store(dir: &tempfile::TempDir) -> FactStore {
        FactStore::open(dir.path()).expect("open fact store")
    }

    fn source() -> Source {
        Source {
            session: "20261109-190000".into(),
            seq: 7,
        }
    }

    #[test]
    fn remember_persists_and_reloads() {
        let dir = tempfile::tempdir().expect("tempdir");
        {
            let mut store = store(&dir);
            let fact = human_fact("work", "project:aios", "snapshot", "сегменты", source());
            store.add(fact.clone()).expect("add");
            assert_eq!(store.len(), 1);
            assert_eq!(store.get(&fact.id).expect("get"), &fact);
            assert_eq!(store.pinned(crate::time::now_ns()).len(), 1);
        }

        let reopened = store(&dir);
        assert_eq!(reopened.len(), 1);
        let fact = &reopened.facts()[0];
        assert_eq!(fact.subject, "project:aios");
        assert_eq!(fact.key, "snapshot");
        assert_eq!(fact.value, "сегменты");
        assert_eq!(fact.author, Author::Human);
        assert_eq!(fact.state, State::Verified);
        assert_eq!(fact.confidence, 1.0);
        assert_eq!(fact.source.seq, 7);
        assert_eq!(
            std::fs::read_to_string(reopened.path())
                .expect("file")
                .lines()
                .count(),
            1
        );
    }

    #[test]
    fn same_subject_and_key_replaces_the_value() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut store = store(&dir);
        store
            .add(human_fact("work", "project", "format", "v1", source()))
            .expect("first");
        store
            .add(human_fact("work", "project", "format", "v2", source()))
            .expect("second");
        assert_eq!(store.len(), 1);
        assert_eq!(store.facts()[0].value, "v2");
    }

    #[test]
    fn forget_removes_the_fact_and_rewrites_the_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut store = store(&dir);
        let fact = human_fact("work", "s", "k", "v", source());
        let id = fact.id.clone();
        store.add(fact).expect("add");
        let removed = store.remove(&id).expect("remove").expect("found");
        assert_eq!(removed.id, id);
        assert!(store.is_empty());
        assert!(store.remove(&id).expect("remove again").is_none());
        let text = std::fs::read_to_string(store.path()).expect("file");
        assert!(text.trim().is_empty());
    }

    #[test]
    fn ids_are_stable_and_distinct() {
        let first = make_id("work", "s", "k", "v", 1);
        let same = make_id("work", "s", "k", "v", 1);
        let other = make_id("work", "s", "k", "v", 2);
        assert_eq!(first, same);
        assert_ne!(first, other);
        assert_eq!(first.len(), 32);
        assert!(first.chars().all(|ch| ch.is_ascii_hexdigit()));
    }

    #[test]
    fn expired_facts_are_not_pinned() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut store = store(&dir);
        let mut fact = human_fact("work", "s", "k", "v", source());
        fact.created_ns = 1_000;
        fact.ttl_ns = Some(10);
        store.add(fact).expect("add");
        assert!(store.pinned(1_005).len() == 1);
        assert!(store.pinned(1_011).is_empty());
    }
}
