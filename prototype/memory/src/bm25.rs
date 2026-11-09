//! BM25 index (docs/PROTOTYPE.md §4: deterministic retrieval, no embeddings).
//!
//! The index is rebuilt by scanning segments, facts and files at startup and
//! updated incrementally as new records and facts appear. Scoring is plain
//! BM25 with `k1 = 1.2`, `b = 0.75`; ties are broken by document id so that two
//! identical runs always return the same order.
//!
//! Tokenisation is deliberately simple — lowercase, split on non-alphanumerics,
//! `ё → е`, drop one-character tokens. Russian and Latin text is handled the
//! same way; no stemming, because a wrong stem is worse than a missed match in
//! a prototype.

use std::cmp::Ordering;
use std::collections::HashMap;

/// BM25 term-frequency saturation.
pub const K1: f64 = 1.2;
/// BM25 length normalisation.
pub const B: f64 = 0.75;

/// Where a document came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    /// A record of an episodic session log.
    Episode,
    /// A fact of the semantic memory.
    Fact,
    /// A chunk of a file from `files/`.
    File,
}

impl RefKind {
    /// Short label for the console.
    pub fn as_str(self) -> &'static str {
        match self {
            RefKind::Episode => "ep",
            RefKind::Fact => "fact",
            RefKind::File => "file",
        }
    }
}

/// A document to index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    /// Unique document id (`ep:<session>#<seq>`, `fact:<id>`, `file:<path>#<n>`).
    pub id: String,
    /// Where it came from.
    pub ref_kind: RefKind,
    /// Console label, e.g. `files/report.md §2` or `сессия 2026… #12`.
    pub label: String,
    /// Provenance line shown next to the text.
    pub provenance: String,
    /// Searchable text.
    pub text: String,
}

/// A retrieval result.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    /// Document id.
    pub id: String,
    /// Where it came from.
    pub ref_kind: RefKind,
    /// Console label.
    pub label: String,
    /// Provenance line.
    pub provenance: String,
    /// Text of the document.
    pub text: String,
    /// BM25 score.
    pub score: f64,
}

#[derive(Debug)]
struct IndexedDocument {
    document: Document,
    term_frequencies: HashMap<String, u32>,
    length: usize,
}

/// In-memory BM25 index.
#[derive(Debug, Default)]
pub struct Bm25Index {
    documents: Vec<IndexedDocument>,
    document_frequency: HashMap<String, usize>,
    total_length: usize,
}

impl Bm25Index {
    /// Empty index.
    pub fn new() -> Self {
        Bm25Index::default()
    }

    /// Number of indexed documents.
    pub fn len(&self) -> usize {
        self.documents.len()
    }

    /// `true` if nothing is indexed.
    pub fn is_empty(&self) -> bool {
        self.documents.is_empty()
    }

    /// Adds a document; a document with the same id is replaced.
    pub fn add(&mut self, document: Document) {
        if self
            .documents
            .iter()
            .any(|indexed| indexed.document.id == document.id)
        {
            self.remove(&document.id);
        }
        let tokens = tokenize(&document.text);
        let mut term_frequencies = HashMap::new();
        for token in &tokens {
            *term_frequencies.entry(token.clone()).or_insert(0) += 1;
        }
        for token in term_frequencies.keys() {
            *self.document_frequency.entry(token.clone()).or_insert(0) += 1;
        }
        self.total_length += tokens.len();
        self.documents.push(IndexedDocument {
            document,
            term_frequencies,
            length: tokens.len(),
        });
    }

    /// Removes a document by id (no-op if it is absent).
    pub fn remove(&mut self, id: &str) {
        let Some(position) = self
            .documents
            .iter()
            .position(|indexed| indexed.document.id == id)
        else {
            return;
        };
        let removed = self.documents.remove(position);
        self.total_length -= removed.length;
        for token in removed.term_frequencies.keys() {
            if let Some(count) = self.document_frequency.get_mut(token) {
                *count -= 1;
                if *count == 0 {
                    self.document_frequency.remove(token);
                }
            }
        }
    }

    /// Searches the index; returns at most `limit` hits, best first.
    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchHit> {
        let mut terms = tokenize(query);
        terms.sort();
        terms.dedup();
        if terms.is_empty() || self.documents.is_empty() || limit == 0 {
            return Vec::new();
        }

        let document_count = self.documents.len() as f64;
        let average_length = (self.total_length as f64 / document_count).max(f64::MIN_POSITIVE);

        let mut hits = Vec::new();
        for document in &self.documents {
            let mut score = 0.0;
            for term in &terms {
                let Some(&frequency) = document.term_frequencies.get(term) else {
                    continue;
                };
                let df = *self.document_frequency.get(term).unwrap_or(&0) as f64;
                let idf = (1.0 + (document_count - df + 0.5) / (df + 0.5)).ln();
                let length = document.length as f64;
                let saturation = frequency as f64 + K1 * (1.0 - B + B * length / average_length);
                score += idf * (frequency as f64 * (K1 + 1.0)) / saturation;
            }
            if score > 0.0 {
                hits.push(SearchHit {
                    id: document.document.id.clone(),
                    ref_kind: document.document.ref_kind,
                    label: document.document.label.clone(),
                    provenance: document.document.provenance.clone(),
                    text: document.document.text.clone(),
                    score,
                });
            }
        }
        hits.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(Ordering::Equal)
                .then_with(|| left.id.cmp(&right.id))
        });
        hits.truncate(limit);
        hits
    }
}

/// Tokenises text for indexing and querying.
///
/// Lowercase, split on anything that is not alphanumeric, `ё → е`, drop tokens
/// shorter than two characters.
pub fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for character in text.chars() {
        if character.is_alphanumeric() {
            for lower in character.to_lowercase() {
                current.push(if lower == 'ё' { 'е' } else { lower });
            }
        } else if !current.is_empty() {
            push_token(&mut tokens, &mut current);
        }
    }
    if !current.is_empty() {
        push_token(&mut tokens, &mut current);
    }
    tokens
}

fn push_token(tokens: &mut Vec<String>, current: &mut String) {
    if current.chars().count() >= 2 {
        tokens.push(std::mem::take(current));
    } else {
        current.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::{Bm25Index, Document, RefKind, tokenize};

    fn document(id: &str, kind: RefKind, label: &str, text: &str) -> Document {
        Document {
            id: id.to_string(),
            ref_kind: kind,
            label: label.to_string(),
            provenance: label.to_string(),
            text: text.to_string(),
        }
    }

    fn fixture() -> Bm25Index {
        let mut index = Bm25Index::new();
        index.add(document(
            "ep:1#3",
            RefKind::Episode,
            "сессия 1 #3",
            "Пользователь спросил про формат снапшотов KV-кэша и префикс-хеши",
        ));
        index.add(document(
            "ep:1#4",
            RefKind::Episode,
            "сессия 1 #4",
            "Обсудили формат сегментов: заголовок, magic, crc32c и resync",
        ));
        index.add(document(
            "fact:abc",
            RefKind::Fact,
            "факт abc",
            "project/format = сегменты с magic и crc32c",
        ));
        index.add(document(
            "file:report.md#0",
            RefKind::File,
            "files/report.md §1",
            "Отчёт про память и BM25 индекс, без эмбеддингов",
        ));
        index
    }

    #[test]
    fn tokenizer_handles_case_punctuation_and_yo() {
        assert_eq!(
            tokenize("Привет, МИР! ёлка — 42, ok?"),
            vec!["привет", "мир", "елка", "42", "ok"]
        );
        assert!(tokenize("a — b").is_empty());
        assert!(tokenize("").is_empty());
    }

    #[test]
    fn search_finds_the_most_relevant_document() {
        let index = fixture();
        let hits = index.search("формат сегментов", 3);
        assert!(!hits.is_empty());
        assert_eq!(hits[0].id, "ep:1#4");
        assert!(hits[0].score > 0.0);

        let hits = index.search("снапшот kv", 2);
        assert_eq!(hits[0].id, "ep:1#3");
    }

    #[test]
    fn search_matches_facts_and_files() {
        let index = fixture();
        let hits = index.search("bm25 эмбеддингов", 2);
        assert_eq!(hits[0].id, "file:report.md#0");
        assert_eq!(hits[0].ref_kind, RefKind::File);

        let hits = index.search("project format", 3);
        assert_eq!(hits[0].id, "fact:abc");
        assert_eq!(hits[0].ref_kind, RefKind::Fact);
    }

    #[test]
    fn search_is_deterministic_and_limited() {
        let index = fixture();
        let first = index.search("формат память", 2);
        let second = index.search("формат память", 2);
        assert_eq!(first, second);
        assert_eq!(first.len(), 2);
        let all = index.search("формат память", 10);
        assert!(all.len() >= first.len());
        // Ties are broken by id, so ordering is stable.
        for window in all.windows(2) {
            if (window[0].score - window[1].score).abs() < f64::EPSILON {
                assert!(window[0].id <= window[1].id);
            }
        }
    }

    #[test]
    fn empty_query_and_empty_index_return_nothing() {
        assert!(fixture().search("", 5).is_empty());
        assert!(fixture().search("!!!", 5).is_empty());
        assert!(Bm25Index::new().search("что-нибудь", 5).is_empty());
    }

    #[test]
    fn remove_drops_the_document_from_results() {
        let mut index = fixture();
        index.remove("file:report.md#0");
        assert_eq!(index.len(), 3);
        let hits = index.search("bm25 эмбеддингов", 3);
        assert!(hits.iter().all(|hit| hit.id != "file:report.md#0"));
        // Removing an unknown id is a no-op.
        index.remove("nope");
        assert_eq!(index.len(), 3);
    }
}
