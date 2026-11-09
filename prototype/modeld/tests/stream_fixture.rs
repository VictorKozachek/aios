//! Stream decoder tests on a recorded ollama NDJSON fixture (no network).

use std::io::Cursor;

use aios_modeld::ModelError;
use aios_modeld::stream::GenStream;

const STREAM: &str = include_str!("fixtures/stream.ndjson");
const SHOW: &str = include_str!("fixtures/show.json");

#[test]
fn stream_reassembles_text_from_fixture() {
    let mut stream = GenStream::from_reader(Cursor::new(STREAM.as_bytes()));
    let mut text = String::new();
    let mut chunks = 0;
    for chunk in stream.by_ref() {
        let chunk = chunk.expect("fixture must parse");
        text.push_str(&chunk.text);
        chunks += 1;
    }
    assert_eq!(text, "Привет! Я — локальная модель.");
    // Four content chunks plus the final `done` chunk.
    assert_eq!(chunks, 5);
    assert!(stream.saw_done());
    assert!(stream.last_stats().is_some());
}

#[test]
fn stream_reports_final_stats_and_done() {
    let mut stream = GenStream::from_reader(Cursor::new(STREAM.as_bytes()));
    let mut final_chunk = None;
    for chunk in stream.by_ref() {
        let chunk = chunk.expect("fixture must parse");
        if chunk.done {
            final_chunk = Some(chunk);
        }
    }
    let final_chunk = final_chunk.expect("final chunk with done=true");
    assert_eq!(final_chunk.text, "");
    let stats = final_chunk
        .stats
        .expect("usage counters on the final chunk");
    assert_eq!(stats.prompt_tokens, 34);
    assert_eq!(stats.completion_tokens, 12);
    assert_eq!(stream.last_stats(), Some(stats));
}

#[test]
fn incomplete_stream_is_reported() {
    // Two chunks and no final `done`: the server died mid-answer.
    let body = "{\"message\":{\"content\":\"часть\"},\"done\":false}\n";
    let mut stream = GenStream::from_reader(Cursor::new(body.as_bytes()));
    let first = stream.next_chunk().expect("first chunk").expect("valid");
    assert_eq!(first.text, "часть");
    let error = stream.next_chunk().expect("error").unwrap_err();
    assert!(matches!(error, ModelError::IncompleteStream));
    assert!(stream.next_chunk().is_none());
}

#[test]
fn malformed_line_is_reported() {
    let body = "{\"message\":{\"content\":\"ok\"},\"done\":false}\n{not json}\n";
    let mut stream = GenStream::from_reader(Cursor::new(body.as_bytes()));
    assert!(stream.next_chunk().expect("chunk").is_ok());
    let error = stream.next_chunk().expect("error").unwrap_err();
    match error {
        ModelError::BadStream { line } => assert!(line.contains("not json")),
        other => panic!("expected BadStream, got {other:?}"),
    }
}

#[test]
fn server_error_line_is_reported() {
    let body = "{\"error\":\"model requires more system memory\"}\n";
    let mut stream = GenStream::from_reader(Cursor::new(body.as_bytes()));
    let error = stream.next_chunk().expect("error").unwrap_err();
    match error {
        ModelError::ServerError { message } => {
            assert!(message.contains("more system memory"));
        }
        other => panic!("expected ServerError, got {other:?}"),
    }
}

#[test]
fn blank_lines_are_skipped() {
    let body = "\n\n{\"message\":{\"content\":\"a\"},\"done\":false}\n\n{\"done\":true}\n";
    let mut stream = GenStream::from_reader(Cursor::new(body.as_bytes()));
    let chunks: Vec<_> = stream.by_ref().map(|chunk| chunk.expect("valid")).collect();
    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0].text, "a");
    assert!(chunks[1].done);
    assert!(stream.saw_done());
}

#[test]
fn show_response_gives_context_and_quant() {
    let info = aios_modeld::parse_show(SHOW, "qwen2.5:0.5b").expect("parse /api/show");
    assert_eq!(info.name, "qwen2.5:0.5b");
    assert_eq!(info.context_tokens, 32768);
    assert_eq!(info.quant.as_deref(), Some("Q4_K_M"));
}

#[test]
fn show_response_without_details_is_tolerated() {
    let info = aios_modeld::parse_show("{\"model_info\":{}}", "qwen2.5:0.5b").expect("parse");
    assert_eq!(info.context_tokens, 0);
    assert_eq!(info.quant, None);
    assert!(matches!(
        aios_modeld::parse_show("not json", "qwen2.5:0.5b"),
        Err(ModelError::BadResponse { .. })
    ));
}
