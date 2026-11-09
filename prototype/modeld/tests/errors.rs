//! Error mapping tests: no network needed except the "server is down" case,
//! which dials loopback port 1 and fails immediately.

use aios_modeld::ModelBackend;
use aios_modeld::ModelError;
use aios_modeld::ollama::{DEFAULT_BASE_URL, DEFAULT_MODEL, OllamaBackend, map_http_status};

#[test]
fn http_404_means_the_model_is_not_downloaded() {
    let error = map_http_status(
        "qwen2.5:0.5b",
        404,
        "{\"error\":\"model 'qwen2.5:0.5b' not found\"}",
    );
    assert!(matches!(error, ModelError::ModelNotFound { .. }));
    let message = error.to_string();
    assert!(message.contains("ollama pull qwen2.5:0.5b"), "{message}");
}

#[test]
fn http_500_is_a_generic_status_error() {
    let error = map_http_status("qwen2.5:0.5b", 500, "internal failure");
    match error {
        ModelError::HttpStatus { status, detail } => {
            assert_eq!(status, 500);
            assert_eq!(detail, "internal failure");
        }
        other => panic!("expected HttpStatus, got {other:?}"),
    }
}

#[test]
fn not_found_body_is_recognised_even_without_404() {
    let error = map_http_status("qwen2.5:0.5b", 400, "model \"x\" not found, try pulling it");
    assert!(matches!(error, ModelError::ModelNotFound { .. }));
}

#[test]
fn unreachable_server_is_a_friendly_error() {
    // Port 1 is never served: this is the "ollama is not running" path.
    let backend = OllamaBackend::new("http://127.0.0.1:1", DEFAULT_MODEL);
    let error = backend.probe().expect_err("nothing listens on port 1");
    assert!(
        matches!(error, ModelError::ServerUnavailable { .. }),
        "expected ServerUnavailable, got {error:?}"
    );
    let message = error.to_string();
    assert!(message.contains("ollama недоступен"), "{message}");
    assert!(message.contains("ollama serve"), "{message}");
    // info() never fails: it degrades to the configured name.
    assert_eq!(backend.info().name, DEFAULT_MODEL);
    assert_eq!(backend.info().context_tokens, 0);
}

#[test]
fn default_endpoint_matches_the_documented_one() {
    assert_eq!(DEFAULT_BASE_URL, "http://127.0.0.1:11434");
    assert_eq!(DEFAULT_MODEL, "qwen2.5:0.5b");
    let backend = OllamaBackend::new(DEFAULT_BASE_URL, DEFAULT_MODEL);
    assert_eq!(backend.base_url(), DEFAULT_BASE_URL);
    assert_eq!(backend.model(), DEFAULT_MODEL);
}
