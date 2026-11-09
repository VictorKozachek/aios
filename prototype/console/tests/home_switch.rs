//! End-to-end test: the real binary runs with a fake `HOME`, creates the
//! workspaces and switches between them (card 015 acceptance).
//!
//! The model endpoint points at a closed port, so the test is deterministic and
//! offline: `/status`, `/ws` and `/quit` must work without a model at all.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn run_console(home: &Path, input: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_aios-console"))
        .env("HOME", home)
        .env_remove("AIOS_HOME")
        .env("AIOS_OLLAMA_URL", "http://127.0.0.1:1")
        .env_remove("AIOS_MODEL")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn aios-console");
    {
        let stdin = child.stdin.as_mut().expect("stdin");
        stdin.write_all(input.as_bytes()).expect("write stdin");
        stdin.flush().expect("flush stdin");
    }
    drop(child.stdin.take());
    child.wait_with_output().expect("wait for console")
}

fn workspace_dir(home: &Path, name: &str) -> PathBuf {
    home.join(".aios/workspaces").join(name)
}

fn sessions(workspace: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(workspace.join("sessions"))
        .expect("sessions dir")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| path.extension().and_then(|extension| extension.to_str()) == Some("ndjson"))
        .collect();
    paths.sort();
    paths
}

#[test]
fn console_creates_workspaces_under_fake_home_and_switches() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path();

    let output = run_console(home, "/status\n/ws home\n/status\n/quit\n");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "console failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    // The prompt tells the human which workspace is active.
    assert!(stdout.contains("aios[work] >"), "stdout:\n{stdout}");
    assert!(stdout.contains("aios[home] >"), "stdout:\n{stdout}");
    // /status is informative even when ollama is unreachable.
    assert!(stdout.contains("воркспейс: work"), "stdout:\n{stdout}");
    assert!(stdout.contains("воркспейс: home"), "stdout:\n{stdout}");
    assert!(stdout.contains("токены:"), "stdout:\n{stdout}");

    for name in ["home", "work"] {
        let root = workspace_dir(home, name);
        assert!(root.join("files").is_dir(), "{name}/files");
        assert!(root.join("sessions").is_dir(), "{name}/sessions");
        assert!(root.join("config.toml").is_file(), "{name}/config.toml");
        assert!(
            !root
                .join("sessions")
                .read_dir()
                .expect("list")
                .next()
                .is_none()
        );
    }

    // Each workspace got its own log, and the log records its workspace.
    let work_sessions = sessions(&workspace_dir(home, "work"));
    let home_sessions = sessions(&workspace_dir(home, "home"));
    assert_eq!(work_sessions.len(), 1);
    assert_eq!(home_sessions.len(), 1);
    assert_ne!(work_sessions[0], home_sessions[0]);
    let home_log = std::fs::read_to_string(&home_sessions[0]).expect("home log");
    assert!(home_log.contains("\"workspace\":\"home\""), "{home_log}");
    let work_log = std::fs::read_to_string(&work_sessions[0]).expect("work log");
    assert!(work_log.contains("\"workspace\":\"work\""), "{work_log}");
}

#[test]
fn restart_resumes_the_same_session_log() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path();

    let first = run_console(home, "/status\n/quit\n");
    assert!(first.status.success());
    let before = sessions(&workspace_dir(home, "work"));
    assert_eq!(before.len(), 1);

    let second = run_console(home, "/status\n/quit\n");
    assert!(second.status.success());
    let after = sessions(&workspace_dir(home, "work"));
    assert_eq!(after, before, "restart must resume, not create a new log");

    let stdout = String::from_utf8_lossy(&second.stdout);
    assert!(stdout.contains("aios[work] >"), "stdout:\n{stdout}");
    // The banner reports the resumed session with its log path.
    assert!(stdout.contains("сессия:"), "stdout:\n{stdout}");
    assert!(
        stdout.contains(before[0].to_string_lossy().as_ref()),
        "log path missing from status:\n{stdout}"
    );
}
