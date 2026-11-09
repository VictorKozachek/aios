//! Session log: `sessions/<timestamp>.ndjson` (docs/PROTOTYPE.md §5, card 015).
//!
//! One JSON object per line: session start/resume/end, every user message, every
//! model answer with ollama usage counters, errors and commands. The file is
//! append-only and flushed after every event, so a killed console loses at most
//! the event being written.
//!
//! The log is the source of truth for "resume": on startup the console opens the
//! newest log of the workspace, restores the dialogue and keeps appending to the
//! same file. Card 016 replaces this flat format with `libs/memfmt` segments.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use aios_modeld::{GenStats, Message};
use serde_json::{Value, json};

use crate::error::ConsoleError;
use crate::time;
use crate::workspace::Workspace;

/// An open session log with the dialogue restored from it.
#[derive(Debug)]
pub struct Session {
    path: PathBuf,
    file: File,
    messages: Vec<Message>,
    prompt_tokens: u64,
    completion_tokens: u64,
    events: u64,
}

impl Session {
    /// Opens the newest session of the workspace, or starts a new one.
    pub fn open_latest_or_new(workspace: &Workspace, model: &str) -> Result<Self, ConsoleError> {
        match workspace.latest_session()? {
            Some(path) => Self::resume(&path),
            None => Self::create_at(workspace, model, time::now()),
        }
    }

    /// Starts a new session log in the workspace.
    pub fn create_at(
        workspace: &Workspace,
        model: &str,
        now: SystemTime,
    ) -> Result<Self, ConsoleError> {
        let dir = workspace.sessions_dir();
        let stamp = time::file_stamp(now);
        let mut path = dir.join(format!("{stamp}.ndjson"));
        let mut counter = 2;
        while path.exists() {
            path = dir.join(format!("{stamp}-{counter}.ndjson"));
            counter += 1;
        }
        let file = OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(&path)
            .map_err(|error| ConsoleError::io(format!("создать {}", path.display()), error))?;
        let mut session = Session {
            path,
            file,
            messages: Vec::new(),
            prompt_tokens: 0,
            completion_tokens: 0,
            events: 0,
        };
        session.write_event(
            "session_start",
            json!({ "workspace": workspace.name(), "model": model }),
        )?;
        Ok(session)
    }

    /// Reopens an existing log and restores the dialogue and counters.
    pub fn resume(path: &Path) -> Result<Self, ConsoleError> {
        let text = fs::read_to_string(path)
            .map_err(|error| ConsoleError::io(format!("читать {}", path.display()), error))?;
        let mut messages = Vec::new();
        let mut prompt_tokens = 0;
        let mut completion_tokens = 0;
        let mut events = 0;
        for line in text.lines() {
            // Unknown or half-written lines are skipped: the log must survive a
            // crash, not fail to load.
            let Ok(value) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            events += 1;
            match value.get("kind").and_then(Value::as_str) {
                Some("user") => {
                    if let Some(text) = value.get("text").and_then(Value::as_str) {
                        messages.push(Message::user(text));
                    }
                }
                Some("assistant") => {
                    if let Some(text) = value.get("text").and_then(Value::as_str) {
                        messages.push(Message::assistant(text));
                    }
                    prompt_tokens += value
                        .get("prompt_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    completion_tokens += value
                        .get("completion_tokens")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                }
                _ => {}
            }
        }
        let file = OpenOptions::new()
            .append(true)
            .open(path)
            .map_err(|error| ConsoleError::io(format!("открыть {}", path.display()), error))?;
        let mut session = Session {
            path: path.to_path_buf(),
            file,
            messages,
            prompt_tokens,
            completion_tokens,
            events,
        };
        session.write_event("session_resume", json!({}))?;
        Ok(session)
    }

    /// Path of the log file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Dialogue restored from the log plus everything said in this run.
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Prompt tokens reported by the backend across the whole session.
    pub fn prompt_tokens(&self) -> u64 {
        self.prompt_tokens
    }

    /// Completion tokens reported by the backend across the whole session.
    pub fn completion_tokens(&self) -> u64 {
        self.completion_tokens
    }

    /// Number of events in the log (all kinds).
    pub fn events(&self) -> u64 {
        self.events
    }

    /// Appends a user message.
    pub fn append_user(&mut self, text: &str) -> Result<(), ConsoleError> {
        self.write_event("user", json!({ "text": text }))?;
        self.messages.push(Message::user(text));
        Ok(())
    }

    /// Appends a model answer with the usage counters ollama reported.
    pub fn append_assistant(
        &mut self,
        text: &str,
        stats: Option<GenStats>,
        model: &str,
    ) -> Result<(), ConsoleError> {
        let mut fields = json!({ "text": text, "model": model });
        if let Some(stats) = stats {
            fields["prompt_tokens"] = json!(stats.prompt_tokens);
            fields["completion_tokens"] = json!(stats.completion_tokens);
            self.prompt_tokens += stats.prompt_tokens;
            self.completion_tokens += stats.completion_tokens;
        }
        self.write_event("assistant", fields)?;
        self.messages.push(Message::assistant(text));
        Ok(())
    }

    /// Appends a model-layer error (so failures are auditable too).
    pub fn append_error(&mut self, message: &str) -> Result<(), ConsoleError> {
        self.write_event("error", json!({ "text": message }))
    }

    /// Appends a console command.
    pub fn append_command(&mut self, command: &str) -> Result<(), ConsoleError> {
        self.write_event("command", json!({ "command": command }))
    }

    /// Closes the session with a reason (`quit`, `switch`, `new`, `eof`).
    pub fn finish(&mut self, reason: &str) -> Result<(), ConsoleError> {
        self.write_event("session_end", json!({ "reason": reason }))
    }

    fn write_event(&mut self, kind: &str, mut fields: Value) -> Result<(), ConsoleError> {
        fields["ts"] = json!(time::rfc3339(time::now()));
        fields["kind"] = json!(kind);
        writeln!(self.file, "{fields}")
            .map_err(|error| ConsoleError::io(format!("писать {}", self.path.display()), error))?;
        self.file.flush().map_err(|error| {
            ConsoleError::io(format!("сбросить {}", self.path.display()), error)
        })?;
        self.events += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Session;
    use crate::workspace::Workspace;
    use aios_modeld::GenStats;
    use std::time::{Duration, UNIX_EPOCH};

    fn workspace(dir: &std::path::Path) -> Workspace {
        Workspace::open(dir.join("home"), "home").expect("workspace")
    }

    fn time_2026() -> std::time::SystemTime {
        // 2026-11-09T18:36:07Z
        UNIX_EPOCH + Duration::from_secs(1_794_249_367)
    }

    #[test]
    fn session_log_is_ndjson_and_flushed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let workspace = workspace(dir.path());
        let mut session =
            Session::create_at(&workspace, "qwen2.5:0.5b", time_2026()).expect("session");
        session.append_user("привет").expect("user");
        session
            .append_assistant(
                "здравствуй",
                Some(GenStats {
                    prompt_tokens: 9,
                    completion_tokens: 4,
                }),
                "qwen2.5:0.5b",
            )
            .expect("assistant");

        let text = std::fs::read_to_string(session.path()).expect("read log");
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines.len(), 3, "start + user + assistant");
        for line in &lines {
            let value: serde_json::Value = serde_json::from_str(line).expect("ndjson line");
            assert!(value.get("ts").is_some());
            assert!(value.get("kind").is_some());
        }
        let last: serde_json::Value = serde_json::from_str(lines[2]).expect("line");
        assert_eq!(last["kind"], "assistant");
        assert_eq!(last["text"], "здравствуй");
        assert_eq!(last["prompt_tokens"], 9);
        assert_eq!(last["completion_tokens"], 4);
        assert_eq!(session.prompt_tokens(), 9);
        assert_eq!(session.completion_tokens(), 4);
    }

    #[test]
    fn resume_restores_dialogue_and_counters() {
        let dir = tempfile::tempdir().expect("tempdir");
        let workspace = workspace(dir.path());
        let path = {
            let mut session =
                Session::create_at(&workspace, "qwen2.5:0.5b", time_2026()).expect("session");
            session.append_user("вопрос").expect("user");
            session
                .append_assistant(
                    "ответ",
                    Some(GenStats {
                        prompt_tokens: 5,
                        completion_tokens: 2,
                    }),
                    "qwen2.5:0.5b",
                )
                .expect("assistant");
            session.finish("quit").expect("finish");
            session.path().to_path_buf()
        };

        let restored = Session::resume(&path).expect("resume");
        assert_eq!(restored.path(), path.as_path());
        assert_eq!(restored.messages().len(), 2);
        assert_eq!(restored.messages()[0].content, "вопрос");
        assert_eq!(restored.messages()[1].content, "ответ");
        assert_eq!(restored.prompt_tokens(), 5);
        assert_eq!(restored.completion_tokens(), 2);
    }

    #[test]
    fn open_latest_or_new_reuses_the_newest_log() {
        let dir = tempfile::tempdir().expect("tempdir");
        let workspace = workspace(dir.path());
        let first = Session::create_at(&workspace, "m", time_2026()).expect("first");
        let first_path = first.path().to_path_buf();
        drop(first);

        let second = Session::open_latest_or_new(&workspace, "m").expect("second");
        assert_eq!(second.path(), first_path.as_path());
        assert_eq!(workspace.list_sessions().expect("sessions").len(), 1);
    }

    #[test]
    fn sessions_do_not_collide_within_one_second() {
        let dir = tempfile::tempdir().expect("tempdir");
        let workspace = workspace(dir.path());
        let first = Session::create_at(&workspace, "m", time_2026()).expect("first");
        let second = Session::create_at(&workspace, "m", time_2026()).expect("second");
        assert_ne!(first.path(), second.path());
        assert_eq!(workspace.list_sessions().expect("sessions").len(), 2);
    }

    #[test]
    fn broken_lines_do_not_break_resume() {
        let dir = tempfile::tempdir().expect("tempdir");
        let workspace = workspace(dir.path());
        let path = workspace.sessions_dir().join("20260101-000000.ndjson");
        std::fs::write(
            &path,
            "{\"kind\":\"user\",\"text\":\"ok\",\"ts\":\"x\"}\n{broken\n\n",
        )
        .expect("write");
        let restored = Session::resume(&path).expect("resume");
        assert_eq!(restored.messages().len(), 1);
        assert_eq!(restored.messages()[0].content, "ok");
    }
}
