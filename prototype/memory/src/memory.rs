//! Workspace memory: facts, sessions, files and the BM25 index in one place.
//!
//! Each workspace owns its memory (docs/PROTOTYPE.md §5): its own
//! `memory/facts.jsonl`, its own audit segments, its own sessions and its own
//! `files/`. [`WorkspaceMemory`] is the only entry point the console uses, so
//! there is no way for one workspace's memory to leak into another: the roots
//! are different directories and nothing is shared in process memory.

use std::path::{Path, PathBuf};

use aios_modeld::Message;

use crate::audit::{AuditEvent, AuditLog};
use crate::bm25::{Bm25Index, Document, RefKind, SearchHit};
use crate::context::{ContextPlan, build_context};
use crate::error::MemoryError;
use crate::fact::{Fact, FactStore, Source, human_fact};
use crate::files::{self, FileChunk};
use crate::session::{self, SessionMeta};
use crate::time;

/// How many hits retrieval asks for by default.
pub const DEFAULT_RETRIEVAL_LIMIT: usize = 6;

/// Memory of one workspace.
#[derive(Debug)]
pub struct WorkspaceMemory {
    root: PathBuf,
    name: String,
    facts: FactStore,
    audit: AuditLog,
    sessions: Vec<SessionMeta>,
    files: Vec<FileChunk>,
    index: Bm25Index,
}

impl WorkspaceMemory {
    /// Opens the memory of a workspace and rebuilds its index.
    ///
    /// The index is rebuilt by a deterministic scan of segments, facts and
    /// files — that is what makes retrieval reproducible after a restart.
    pub fn open(workspace_root: &Path, name: &str) -> Result<Self, MemoryError> {
        for dir in ["files", "sessions", "memory"] {
            let path = workspace_root.join(dir);
            std::fs::create_dir_all(&path)
                .map_err(|error| MemoryError::io(format!("создать {}", path.display()), error))?;
        }
        let facts = FactStore::open(workspace_root)?;
        let audit = AuditLog::open(workspace_root)?;
        let mut memory = WorkspaceMemory {
            root: workspace_root.to_path_buf(),
            name: name.to_string(),
            facts,
            audit,
            sessions: Vec::new(),
            files: Vec::new(),
            index: Bm25Index::new(),
        };
        memory.rebuild()?;
        Ok(memory)
    }

    /// Rebuilds the BM25 index from disk.
    pub fn rebuild(&mut self) -> Result<(), MemoryError> {
        let mut index = Bm25Index::new();

        let files = files::scan(&self.root.join("files"))?;
        for chunk in &files {
            index.add(chunk.document());
        }

        let sessions = session::list_sessions(&self.root.join("sessions"))?;
        for meta in &sessions {
            let loaded = session::load_session(&meta.dir)?;
            for (seq, event) in &loaded.events {
                if let Some(text) = event.indexed_text() {
                    index.add(Document {
                        id: format!("ep:{}#{}", loaded.id, seq),
                        ref_kind: RefKind::Episode,
                        label: format!("сессия {} #{}", loaded.id, seq),
                        provenance: format!("сессия {}", loaded.id),
                        text: text.to_string(),
                    });
                }
            }
        }

        for fact in self.facts.facts() {
            index.add(fact_document(fact));
        }

        self.sessions = sessions;
        self.files = files;
        self.index = index;
        Ok(())
    }

    /// Workspace name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Workspace root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// All facts.
    pub fn facts(&self) -> &[Fact] {
        self.facts.facts()
    }

    /// Number of facts.
    pub fn fact_count(&self) -> usize {
        self.facts.len()
    }

    /// Path of `memory/facts.jsonl`.
    pub fn facts_path(&self) -> &Path {
        self.facts.path()
    }

    /// Pinned, non-expired facts (always considered for the context).
    pub fn pinned_facts(&self) -> Vec<Fact> {
        self.facts
            .pinned(time::now_ns())
            .into_iter()
            .cloned()
            .collect()
    }

    /// Sessions of the workspace, oldest first.
    pub fn sessions(&self) -> &[SessionMeta] {
        &self.sessions
    }

    /// Chunks of `files/`.
    pub fn files(&self) -> &[FileChunk] {
        &self.files
    }

    /// Number of indexed documents.
    pub fn index_len(&self) -> usize {
        self.index.len()
    }

    /// BM25 search over episodes, facts and files.
    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchHit> {
        self.index.search(query, limit)
    }

    /// Stores a human fact (`/remember`) and audits it.
    pub fn remember(
        &mut self,
        subject: &str,
        key: &str,
        value: &str,
        source: Source,
    ) -> Result<Fact, MemoryError> {
        let fact = human_fact(&self.name, subject, key, value, source);
        self.facts.add(fact.clone())?;
        self.index.add(fact_document(&fact));
        self.audit.append("remember", &fact.id, "human")?;
        Ok(fact)
    }

    /// Forgets a fact (`/forget`), audits it and returns the removed fact.
    pub fn forget(&mut self, id: &str, by: &str) -> Result<Option<Fact>, MemoryError> {
        let removed = self.facts.remove(id)?;
        if let Some(fact) = &removed {
            self.index.remove(&format!("fact:{}", fact.id));
            self.audit.append("forget", &fact.id, by)?;
        }
        Ok(removed)
    }

    /// Reads the audit segments.
    pub fn audit_events(&self) -> Result<Vec<(u64, AuditEvent)>, MemoryError> {
        self.audit.events()
    }

    /// Builds the context for one turn: system + facts + tail + retrieved.
    pub fn build_context(
        &self,
        system_prompt: &str,
        budget: usize,
        query: &str,
        history: &[Message],
        count_tokens: &dyn Fn(&str) -> usize,
    ) -> ContextPlan {
        let hits = self.search(query, DEFAULT_RETRIEVAL_LIMIT);
        let pinned = self.pinned_facts();
        build_context(system_prompt, budget, &pinned, &hits, history, count_tokens)
    }
}

fn fact_document(fact: &Fact) -> Document {
    Document {
        id: format!("fact:{}", fact.id),
        ref_kind: RefKind::Fact,
        label: format!("факт {}", fact.id),
        provenance: fact.provenance(),
        text: fact.indexed_text(),
    }
}

#[cfg(test)]
mod tests {
    use super::WorkspaceMemory;
    use crate::bm25::RefKind;
    use crate::fact::Source;
    use aios_modeld::Message;

    /// `chars/4` estimate, the same rule the model layer uses.
    fn count(text: &str) -> usize {
        if text.is_empty() {
            0
        } else {
            text.chars().count().div_ceil(4)
        }
    }

    fn open(root: &std::path::Path) -> WorkspaceMemory {
        std::fs::create_dir_all(root).expect("create root");
        WorkspaceMemory::open(root, "work").expect("open memory")
    }

    fn source() -> Source {
        Source {
            session: "20261109-190000".into(),
            seq: 5,
        }
    }

    #[test]
    fn remembered_fact_survives_reopen_and_is_searchable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let fact = {
            let mut memory = open(dir.path());
            memory
                .remember(
                    "project:aios",
                    "snapshot_format",
                    "сегменты с magic и crc32c",
                    source(),
                )
                .expect("remember")
        };

        let memory = open(dir.path());
        assert_eq!(memory.fact_count(), 1);
        let stored = &memory.facts()[0];
        assert_eq!(stored.id, fact.id);
        assert_eq!(stored.value, "сегменты с magic и crc32c");
        assert_eq!(memory.pinned_facts().len(), 1);

        let hits = memory.search("формат снапшотов crc32c", 3);
        assert!(
            hits.iter()
                .any(|hit| hit.ref_kind == RefKind::Fact && hit.label.contains(&fact.id))
        );
    }

    #[test]
    fn remembering_the_same_key_updates_the_value() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut memory = open(dir.path());
        memory
            .remember("project", "format", "v1", source())
            .expect("first");
        memory
            .remember("project", "format", "v2", source())
            .expect("second");
        assert_eq!(memory.fact_count(), 1);
        assert_eq!(memory.facts()[0].value, "v2");
    }

    #[test]
    fn forget_removes_the_fact_from_store_and_index_and_audits_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut memory = open(dir.path());
        let fact = memory
            .remember("s", "k", "уникальноеслово", source())
            .expect("remember");
        assert!(
            memory
                .search("уникальноеслово", 3)
                .iter()
                .any(|hit| hit.label.contains(&fact.id))
        );

        let removed = memory
            .forget(&fact.id, "human")
            .expect("forget")
            .expect("found");
        assert_eq!(removed.id, fact.id);
        assert_eq!(memory.fact_count(), 0);
        assert!(memory.search("уникальноеслово", 3).is_empty());

        let events = memory.audit_events().expect("audit");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].1.action, "remember");
        assert_eq!(events[1].1.action, "forget");
        assert_eq!(events[1].1.fact_id, fact.id);
    }

    #[test]
    fn workspaces_do_not_share_facts_sessions_or_index() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home_root = dir.path().join("home");
        let work_root = dir.path().join("work");
        std::fs::create_dir_all(&home_root).expect("home");
        std::fs::create_dir_all(&work_root).expect("work");

        let mut home = WorkspaceMemory::open(&home_root, "home").expect("home memory");
        let work = WorkspaceMemory::open(&work_root, "work").expect("work memory");

        home.remember("личное", "заметка", "не показывать на работе", source())
            .expect("remember");

        assert_eq!(home.fact_count(), 1);
        assert_eq!(
            work.fact_count(),
            0,
            "facts must not leak between workspaces"
        );
        assert!(work.search("не показывать на работе", 5).is_empty());
        assert!(work.facts_path() != home.facts_path());

        // A file in home/files is invisible to the work index.
        std::fs::write(home_root.join("files/secret.md"), "секретный документ").expect("write");
        std::fs::create_dir_all(work_root.join("files")).expect("work files");
        std::fs::write(work_root.join("files/plan.md"), "рабочий план").expect("write");

        let home = WorkspaceMemory::open(&home_root, "home").expect("home again");
        let work = WorkspaceMemory::open(&work_root, "work").expect("work again");
        assert!(
            home.search("секретный документ", 5)
                .iter()
                .any(|hit| hit.label.contains("secret.md"))
        );
        assert!(work.search("секретный документ", 5).is_empty());
        assert!(
            work.search("рабочий план", 5)
                .iter()
                .any(|hit| hit.label.contains("plan.md"))
        );
    }

    #[test]
    fn context_includes_pinned_fact_and_file_fragment() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("files")).expect("files");
        std::fs::write(
            dir.path().join("files/report.md"),
            "Отчёт: снапшоты адресуются парой SnapshotKey и prefix_hash.",
        )
        .expect("write");

        let mut memory = open(dir.path());
        let fact = memory
            .remember(
                "project:aios",
                "snapshot",
                "ключ SnapshotKey + prefix_hash",
                source(),
            )
            .expect("remember");
        let history = vec![Message::user("как адресуются снапшоты?")];
        let plan = memory.build_context(
            "Ты — AIOS.",
            1000,
            "как адресуются снапшоты SnapshotKey",
            &history,
            &count,
        );

        assert!(plan.fits_budget());
        assert!(plan.facts_included.contains(&fact.id));
        assert!(
            plan.file_refs
                .iter()
                .any(|label| label == "files/report.md §1"),
            "file fragment must be referenced, got {:?}",
            plan.file_refs
        );
        let joined = plan
            .messages
            .iter()
            .map(|message| message.content.clone())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("SnapshotKey"), "{joined}");
        assert!(joined.contains("project:aios/snapshot"), "{joined}");
    }

    #[test]
    fn long_history_and_files_respect_the_budget() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("files")).expect("files");
        let long_text = "длинный фрагмент текста про память. ".repeat(200);
        std::fs::write(dir.path().join("files/long.md"), long_text).expect("write");

        let memory = open(dir.path());
        let mut history = Vec::new();
        for index in 0..50 {
            history.push(Message::user(format!(
                "вопрос {index} {}",
                "контекст ".repeat(40)
            )));
            history.push(Message::assistant(format!(
                "ответ {index} {}",
                "подробности ".repeat(40)
            )));
        }
        let plan = memory.build_context("system prompt", 600, "память", &history, &count);
        assert!(
            plan.fits_budget(),
            "total {} > 600: {}",
            plan.total(),
            plan.composition()
        );
        assert!(plan.dropped_messages > 0);
        assert!(plan.was_truncated());
    }
}
