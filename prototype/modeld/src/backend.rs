//! The backend-agnostic model interface (docs/PROTOTYPE.md §3).
//!
//! Everything here is plain data: no HTTP, no threads, no engine specifics.
//! The console builds a [`GenRequest`], gets a stream of [`GenChunk`]s and
//! counts tokens with [`ModelBackend::count_tokens`] before sending.

/// Who authored a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
}

impl Role {
    /// Role name as used by the wire protocol.
    pub const fn as_str(self) -> &'static str {
        match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
        }
    }
}

/// One chat message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Message {
            role: Role::System,
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Message {
            role: Role::User,
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Message {
            role: Role::Assistant,
            content: content.into(),
        }
    }
}

/// What the backend knows about the served model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    /// Model tag, e.g. `qwen2.5:0.5b`.
    pub name: String,
    /// Context window in tokens, `0` if unknown (server did not answer).
    pub context_tokens: usize,
    /// Quantisation, e.g. `Q4_K_M`, if the backend reports it.
    pub quant: Option<String>,
}

impl ModelInfo {
    /// Info without a server probe: only the configured name is known.
    pub fn unknown(name: impl Into<String>) -> Self {
        ModelInfo {
            name: name.into(),
            context_tokens: 0,
            quant: None,
        }
    }
}

/// One generation request.
#[derive(Debug, Clone, PartialEq)]
pub struct GenRequest {
    /// Full message list: system prompt first, then the session window.
    pub messages: Vec<Message>,
    /// Sampling temperature.
    pub temperature: f32,
    /// Upper bound on generated tokens, if the caller wants one.
    pub max_tokens: Option<u32>,
}

impl GenRequest {
    /// A request with default sampling and no token cap.
    pub fn new(messages: Vec<Message>) -> Self {
        GenRequest {
            messages,
            temperature: 0.7,
            max_tokens: None,
        }
    }
}

/// Token usage reported by the engine for one generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GenStats {
    /// Tokens in the prompt (the whole context that was sent).
    pub prompt_tokens: u64,
    /// Tokens generated.
    pub completion_tokens: u64,
}

/// One streamed delta.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GenChunk {
    /// Text delta; empty for the final chunk.
    pub text: String,
    /// `true` on the final chunk.
    pub done: bool,
    /// Usage counters; usually present only on the final chunk.
    pub stats: Option<GenStats>,
}

/// A running generation: deltas arrive as they are produced.
pub type GenStream = crate::stream::GenStream;

/// The model interface every engine implements.
pub trait ModelBackend {
    /// Model description. Must not fail: an unreachable server yields
    /// [`ModelInfo::unknown`].
    fn info(&self) -> ModelInfo;

    /// Starts a streamed generation.
    fn generate(&self, req: &GenRequest) -> Result<GenStream, crate::ModelError>;

    /// Rough token count of `text` — for the context budget before sending.
    ///
    /// Prototype engines have no tokenizer; the estimate is documented in
    /// [`crate::estimate_tokens`]. Exact usage comes back in [`GenChunk::stats`].
    fn count_tokens(&self, text: &str) -> usize;

    /// Checks that the server is reachable and the model is downloaded.
    ///
    /// The default implementation is enough for backends without a
    /// separate health endpoint.
    fn health(&self) -> Result<ModelInfo, crate::ModelError> {
        Ok(self.info())
    }
}
