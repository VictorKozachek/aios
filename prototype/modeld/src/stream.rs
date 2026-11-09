//! NDJSON stream of ollama `/api/chat` (docs/PROTOTYPE.md §3).
//!
//! ollama answers a streaming chat request with newline-delimited JSON:
//!
//! ```text
//! {"model":"…","message":{"role":"assistant","content":"При"},"done":false}
//! {"model":"…","message":{"role":"assistant","content":"вет"},"done":false}
//! {"model":"…","message":{"role":"assistant","content":""},"done":true,
//!  "prompt_eval_count":34,"eval_count":12,"done_reason":"stop"}
//! ```
//!
//! The decoder is deliberately separated from HTTP: the fixture tests feed it
//! bytes from a file, so the wire format is covered without a network and
//! without ollama.

use std::io::{BufRead, BufReader, Read};

use serde::Deserialize;

use crate::backend::{GenChunk, GenStats};
use crate::error::ModelError;

/// One JSON line of the stream.
#[derive(Debug, Deserialize)]
struct WireChunk {
    message: Option<WireMessage>,
    done: Option<bool>,
    prompt_eval_count: Option<u64>,
    eval_count: Option<u64>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WireMessage {
    content: Option<String>,
}

/// Parses one NDJSON line of an `/api/chat` stream.
///
/// * blank lines are skipped (`Ok(None)`);
/// * `{"error": "…"}` is a [`ModelError::ServerError`];
/// * malformed JSON is a [`ModelError::BadStream`] with the offending line.
pub fn parse_line(line: &str) -> Result<Option<GenChunk>, ModelError> {
    let line = line.trim();
    if line.is_empty() {
        return Ok(None);
    }
    let wire: WireChunk = serde_json::from_str(line).map_err(|_| ModelError::BadStream {
        line: shorten(line, 120),
    })?;
    if let Some(message) = wire.error.filter(|message| !message.is_empty()) {
        return Err(ModelError::ServerError { message });
    }
    let stats = if wire.prompt_eval_count.is_some() || wire.eval_count.is_some() {
        Some(GenStats {
            prompt_tokens: wire.prompt_eval_count.unwrap_or(0),
            completion_tokens: wire.eval_count.unwrap_or(0),
        })
    } else {
        None
    };
    Ok(Some(GenChunk {
        text: wire
            .message
            .and_then(|message| message.content)
            .unwrap_or_default(),
        done: wire.done.unwrap_or(false),
        stats,
    }))
}

fn shorten(text: &str, max_chars: usize) -> String {
    let mut shortened: String = text.chars().take(max_chars).collect();
    if text.chars().count() > max_chars {
        shortened.push('…');
    }
    shortened
}

/// Iterator over the deltas of one generation.
///
/// The stream owns the HTTP body reader, so tokens are printed as they arrive
/// instead of after the whole reply is buffered.
pub struct GenStream {
    reader: BufReader<Box<dyn Read + Send + Sync>>,
    finished: bool,
    saw_done: bool,
    last_stats: Option<GenStats>,
}

impl GenStream {
    /// Wraps any reader that yields NDJSON.
    pub fn from_reader<R: Read + Send + Sync + 'static>(reader: R) -> Self {
        GenStream {
            reader: BufReader::new(Box::new(reader)),
            finished: false,
            saw_done: false,
            last_stats: None,
        }
    }

    /// The next delta, or `None` when the stream is over.
    pub fn next_chunk(&mut self) -> Option<Result<GenChunk, ModelError>> {
        while !self.finished {
            let mut line = String::new();
            match self.reader.read_line(&mut line) {
                Ok(0) => {
                    self.finished = true;
                    if !self.saw_done {
                        return Some(Err(ModelError::IncompleteStream));
                    }
                    return None;
                }
                Ok(_) => {}
                Err(error) => {
                    self.finished = true;
                    return Some(Err(ModelError::Io {
                        detail: error.to_string(),
                    }));
                }
            }

            match parse_line(&line) {
                Ok(Some(chunk)) => {
                    if chunk.done {
                        self.saw_done = true;
                    }
                    if let Some(stats) = chunk.stats {
                        self.last_stats = Some(stats);
                    }
                    return Some(Ok(chunk));
                }
                Ok(None) => continue,
                Err(error) => {
                    self.finished = true;
                    return Some(Err(error));
                }
            }
        }
        None
    }

    /// `true` if the final `done` chunk was seen.
    pub fn saw_done(&self) -> bool {
        self.saw_done
    }

    /// Usage counters from the final chunk, if it carried them.
    pub fn last_stats(&self) -> Option<GenStats> {
        self.last_stats
    }
}

impl Iterator for GenStream {
    type Item = Result<GenChunk, ModelError>;

    fn next(&mut self) -> Option<Self::Item> {
        self.next_chunk()
    }
}
