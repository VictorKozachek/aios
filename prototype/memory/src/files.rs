//! RAG over `files/`: chunked text files indexed by BM25 (docs/PROTOTYPE.md §4).
//!
//! Files are chunked by characters (800 with 100 overlap by default), which
//! keeps chunks stable for Russian text where byte offsets and character
//! offsets differ. Each chunk gets a label the console can print —
//! `files/report.md §2` — so an answer can be traced to a fragment instead of
//! "the model knows".

use std::fs;
use std::path::{Path, PathBuf};

use crate::bm25::{Document, RefKind};
use crate::error::MemoryError;

/// Chunk size in characters.
pub const CHUNK_CHARS: usize = 800;

/// Overlap between neighbouring chunks, characters.
pub const CHUNK_OVERLAP: usize = 100;

/// Files larger than this are skipped (prototype sanity limit).
pub const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// One indexed fragment of a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChunk {
    /// Path relative to `files/`, with `/` separators.
    pub path: String,
    /// Zero-based chunk index inside the file.
    pub chunk_index: usize,
    /// Chunk text.
    pub text: String,
}

impl FileChunk {
    /// Console label: `files/report.md §2`.
    pub fn label(&self) -> String {
        format!("files/{} §{}", self.path, self.chunk_index + 1)
    }

    /// BM25 document for this chunk.
    pub fn document(&self) -> Document {
        Document {
            id: format!("file:{}#{}", self.path, self.chunk_index),
            ref_kind: RefKind::File,
            label: self.label(),
            provenance: format!("files/{}", self.path),
            text: self.text.clone(),
        }
    }
}

/// Scans `files/` recursively and returns all text chunks, sorted by path and
/// chunk index.
///
/// Non-UTF-8 files, hidden files and files above [`MAX_FILE_BYTES`] are
/// skipped: they are not something the prototype should feed to the model.
pub fn scan(files_dir: &Path) -> Result<Vec<FileChunk>, MemoryError> {
    let mut paths = Vec::new();
    collect(files_dir, &mut paths)?;
    paths.sort();

    let mut chunks = Vec::new();
    for path in paths {
        let relative = relative_path(files_dir, &path);
        let bytes = fs::read(&path)
            .map_err(|error| MemoryError::io(format!("читать {}", path.display()), error))?;
        if bytes.len() as u64 > MAX_FILE_BYTES {
            continue;
        }
        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };
        for (chunk_index, chunk) in chunk_text(&text).into_iter().enumerate() {
            chunks.push(FileChunk {
                path: relative.clone(),
                chunk_index,
                text: chunk,
            });
        }
    }
    Ok(chunks)
}

/// Splits one text into overlapping chunks.
pub fn chunk_text(text: &str) -> Vec<String> {
    let characters: Vec<char> = text.chars().collect();
    if characters.is_empty() {
        return Vec::new();
    }
    let mut chunks = Vec::new();
    let mut start = 0;
    loop {
        let end = (start + CHUNK_CHARS).min(characters.len());
        let chunk: String = characters[start..end].iter().collect();
        // Keep the chunk verbatim (offsets stay exact); skip only whitespace.
        if !chunk.trim().is_empty() {
            chunks.push(chunk);
        }
        if end == characters.len() {
            break;
        }
        // Step back by the overlap, but always move forward.
        let next = end.saturating_sub(CHUNK_OVERLAP).max(start + 1);
        start = next;
    }
    chunks
}

fn collect(dir: &Path, paths: &mut Vec<PathBuf>) -> Result<(), MemoryError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(MemoryError::io(format!("читать {}", dir.display()), error));
        }
    };
    for entry in entries {
        let entry = entry.map_err(|error| MemoryError::io("читать файлы", error))?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            collect(&path, paths)?;
        } else if path.is_file() {
            paths.push(path);
        }
    }
    Ok(())
}

fn relative_path(root: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    let mut parts = Vec::new();
    for component in relative.components() {
        parts.push(component.as_os_str().to_string_lossy().to_string());
    }
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::{CHUNK_CHARS, CHUNK_OVERLAP, chunk_text, scan};

    #[test]
    fn short_text_is_one_chunk() {
        let chunks = chunk_text("короткий текст");
        assert_eq!(chunks, vec!["короткий текст"]);
        assert!(chunk_text("").is_empty());
        assert!(chunk_text("   \n\n  ").is_empty());
    }

    #[test]
    fn long_text_is_chunked_with_overlap() {
        let text: String = (0..CHUNK_CHARS * 2 + 50)
            .map(|index| if index % 10 == 0 { ' ' } else { 'а' })
            .collect();
        let chunks = chunk_text(&text);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].chars().count(), CHUNK_CHARS);
        assert_eq!(chunks[1].chars().count(), CHUNK_CHARS);
        // The overlap region of chunk 1 ends chunk 0 and starts chunk 1.
        let tail: String = chunks[0]
            .chars()
            .rev()
            .take(CHUNK_OVERLAP)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let head: String = chunks[1].chars().take(CHUNK_OVERLAP).collect();
        assert_eq!(tail, head);
    }

    #[test]
    fn chunking_covers_the_whole_text() {
        let text: String = (0..5000)
            .map(|index| char::from(b'a' + (index % 26) as u8))
            .collect();
        let chunks = chunk_text(&text);
        let last = chunks.last().expect("chunks");
        assert!(last.chars().count() <= CHUNK_CHARS);
        assert!(text.ends_with(last.trim()));
    }

    #[test]
    fn scan_reads_files_recursively_and_labels_them() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("sub")).expect("dir");
        std::fs::write(dir.path().join("notes.md"), "заметки про формат").expect("write");
        std::fs::write(dir.path().join("sub/plan.txt"), "план работ").expect("write");
        std::fs::write(dir.path().join(".hidden"), "секрет").expect("write");
        std::fs::write(dir.path().join("binary.bin"), [0xff, 0xfe, 0x00]).expect("write");

        let chunks = scan(dir.path()).expect("scan");
        let paths: Vec<&str> = chunks.iter().map(|chunk| chunk.path.as_str()).collect();
        assert_eq!(paths, vec!["notes.md", "sub/plan.txt"]);
        assert_eq!(chunks[0].label(), "files/notes.md §1");
        assert_eq!(chunks[1].label(), "files/sub/plan.txt §1");
    }

    #[test]
    fn missing_files_dir_is_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let chunks = scan(&dir.path().join("nope")).expect("scan");
        assert!(chunks.is_empty());
    }
}
