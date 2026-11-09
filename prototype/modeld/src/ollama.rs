//! ollama backend (docs/PROTOTYPE.md §3).
//!
//! Plain HTTP/1.1 to `http://127.0.0.1:11434`, no TLS: ollama serves only on
//! loopback in the prototype. Two endpoints are used:
//!
//! * `POST /api/show` — is the server up and is the model downloaded; also
//!   gives the context window and quantisation for `/status`;
//! * `POST /api/chat` with `stream: true` — NDJSON token stream, see
//!   [`crate::stream`].

use std::cell::RefCell;
use std::time::Duration;

use serde_json::json;

use crate::backend::{GenRequest, ModelBackend, ModelInfo};
use crate::error::ModelError;
use crate::stream::GenStream;
use crate::tokens::estimate_tokens;

/// Default ollama address.
pub const DEFAULT_BASE_URL: &str = "http://127.0.0.1:11434";

/// Default model for the prototype.
pub const DEFAULT_MODEL: &str = "qwen2.5:0.5b";

/// ollama-served model.
pub struct OllamaBackend {
    base_url: String,
    model: String,
    agent: ureq::Agent,
    cached_info: RefCell<Option<ModelInfo>>,
}

impl OllamaBackend {
    /// Creates a backend; no I/O happens until the first request.
    pub fn new(base_url: impl Into<String>, model: impl Into<String>) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(2))
            .build();
        OllamaBackend {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            model: model.into(),
            agent,
            cached_info: RefCell::new(None),
        }
    }

    /// Base URL this backend talks to.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Model tag this backend asks for.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Checks the server and the model, returning the model description.
    pub fn probe(&self) -> Result<ModelInfo, ModelError> {
        let url = format!("{}/api/show", self.base_url);
        let body = json!({ "name": self.model }).to_string();
        let response = self
            .agent
            .post(&url)
            .set("Content-Type", "application/json")
            .send_string(&body);
        match response {
            Ok(response) => {
                let text = response.into_string().map_err(|error| ModelError::Io {
                    detail: error.to_string(),
                })?;
                let info = parse_show(&text, &self.model)?;
                *self.cached_info.borrow_mut() = Some(info.clone());
                Ok(info)
            }
            Err(ureq::Error::Status(status, response)) => {
                let detail = response.into_string().unwrap_or_default();
                Err(map_http_status(&self.model, status, &detail))
            }
            Err(ureq::Error::Transport(transport)) => Err(ModelError::ServerUnavailable {
                url: self.base_url.clone(),
                detail: transport.to_string(),
            }),
        }
    }
}

impl ModelBackend for OllamaBackend {
    fn info(&self) -> ModelInfo {
        if let Some(info) = self.cached_info.borrow().clone() {
            return info;
        }
        self.probe()
            .unwrap_or_else(|_| ModelInfo::unknown(&self.model))
    }

    fn health(&self) -> Result<ModelInfo, ModelError> {
        self.probe()
    }

    fn generate(&self, req: &GenRequest) -> Result<GenStream, ModelError> {
        let messages: Vec<serde_json::Value> = req
            .messages
            .iter()
            .map(|message| {
                json!({
                    "role": message.role.as_str(),
                    "content": message.content,
                })
            })
            .collect();
        let mut options = serde_json::Map::new();
        options.insert("temperature".to_string(), json!(req.temperature));
        if let Some(max_tokens) = req.max_tokens {
            options.insert("num_predict".to_string(), json!(max_tokens));
        }
        let body = json!({
            "model": self.model,
            "messages": messages,
            "stream": true,
            "options": options,
        })
        .to_string();

        let url = format!("{}/api/chat", self.base_url);
        let response = self
            .agent
            .post(&url)
            .set("Content-Type", "application/json")
            .send_string(&body);
        match response {
            Ok(response) => Ok(GenStream::from_reader(response.into_reader())),
            Err(ureq::Error::Status(status, response)) => {
                let detail = response.into_string().unwrap_or_default();
                Err(map_http_status(&self.model, status, &detail))
            }
            Err(ureq::Error::Transport(transport)) => Err(ModelError::ServerUnavailable {
                url: self.base_url.clone(),
                detail: transport.to_string(),
            }),
        }
    }

    fn count_tokens(&self, text: &str) -> usize {
        estimate_tokens(text)
    }
}

/// Maps a non-2xx HTTP response to a user-facing error.
///
/// Kept public and pure so the mapping is covered by tests without a network:
/// 404 and `"… not found"` bodies become [`ModelError::ModelNotFound`] (the
/// model is simply not downloaded yet), everything else is a generic status
/// error.
pub fn map_http_status(model: &str, status: u16, detail: &str) -> ModelError {
    let detail = detail.trim();
    let detail: String = detail.chars().take(240).collect();
    if status == 404 || detail.contains("not found") {
        ModelError::ModelNotFound {
            model: model.to_string(),
            detail,
        }
    } else {
        ModelError::HttpStatus { status, detail }
    }
}

/// Parses the `/api/show` response: context window and quantisation.
///
/// Context length lives in a model-dependent key such as
/// `"qwen2.context_length"` inside `model_info`, so the key is matched by
/// suffix. Unknown fields are tolerated: the console shows what it got.
pub fn parse_show(body: &str, model: &str) -> Result<ModelInfo, ModelError> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|error| ModelError::BadResponse {
            detail: error.to_string(),
        })?;
    if let Some(error) = value.get("error").and_then(|value| value.as_str()) {
        return Err(ModelError::BadResponse {
            detail: error.to_string(),
        });
    }
    let context_tokens = value
        .get("model_info")
        .and_then(|info| info.as_object())
        .and_then(|info| {
            info.iter()
                .find(|(key, _)| key.ends_with(".context_length"))
                .and_then(|(_, value)| value.as_u64())
        })
        .unwrap_or(0) as usize;
    let quant = value
        .get("details")
        .and_then(|details| details.get("quantization_level"))
        .and_then(|value| value.as_str())
        .map(str::to_string);
    Ok(ModelInfo {
        name: model.to_string(),
        context_tokens,
        quant,
    })
}
