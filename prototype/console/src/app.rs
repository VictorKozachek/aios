//! Console application state: workspaces, session log, model turns.
//!
//! The terminal loop ([`crate::repl`]) is a thin shell around this type: every
//! command and every model turn is a method here, so behaviour is tested without
//! a TTY (unit tests use a stub backend and a temporary `~/.aios`).
//!
//! Layering follows docs/PROTOTYPE.md §2: the console knows about workspaces and
//! sessions, the model layer knows about tokens and streams, and nothing here
//! talks HTTP directly.

use aios_modeld::{GenChunk, GenRequest, GenStats, Message, ModelBackend, ModelError, ModelInfo};

use crate::config::WorkspaceConfig;
use crate::error::ConsoleError;
use crate::paths::Paths;
use crate::session::Session;
use crate::time;
use crate::workspace::Workspaces;

/// Builds a backend for a model tag. The console stays engine-agnostic: tests
/// inject a stub, `main` injects ollama.
pub type BackendFactory = Box<dyn Fn(&str) -> Box<dyn ModelBackend>>;

/// One finished model turn.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    /// The whole answer (also streamed to the terminal chunk by chunk).
    pub text: String,
    /// Usage counters reported by the backend, when it reports them.
    pub stats: Option<GenStats>,
}

/// What the terminal loop should do after a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    /// Text to print (may be multi-line, may be empty).
    pub output: String,
    /// `true` if the console should exit.
    pub quit: bool,
}

const HELP: &str = "\
Команды:
  /ws [home|work]  показать воркспейсы или переключиться
  /status          модель, воркспейс, токены, путь лога
  /new             начать новую сессию (старый лог сохраняется)
  /help            эта справка
  /quit            выход (или Ctrl-D)

Обычный ввод уходит модели; ответ печатается потоком.";

/// Console state.
pub struct App {
    make_backend: BackendFactory,
    backend: Box<dyn ModelBackend>,
    model_info: ModelInfo,
    paths: Paths,
    workspaces: Workspaces,
    current: String,
    config: WorkspaceConfig,
    session: Session,
    model_override: Option<String>,
    last_stats: Option<GenStats>,
}

impl App {
    /// Opens workspaces and the newest session of `initial_workspace`.
    ///
    /// `model_override` (CLI `--model` / `AIOS_MODEL`) wins over the model from
    /// the workspace `config.toml`.
    pub fn new(
        make_backend: BackendFactory,
        paths: Paths,
        initial_workspace: &str,
        model_override: Option<String>,
    ) -> Result<Self, ConsoleError> {
        let workspaces = Workspaces::open(&paths)?;
        let current = if workspaces.contains(initial_workspace) {
            initial_workspace.to_string()
        } else {
            "work".to_string()
        };
        let (config, model, session) = {
            let workspace = workspaces
                .get(&current)
                .expect("current workspace is checked above");
            let config = workspace.config().clone();
            let model = model_override
                .clone()
                .unwrap_or_else(|| config.model.clone());
            let session = Session::open_latest_or_new(workspace, &model)?;
            (config, model, session)
        };
        let backend = make_backend(&model);
        let model_info = backend.info();
        Ok(App {
            make_backend,
            backend,
            model_info,
            paths,
            workspaces,
            current,
            config,
            session,
            model_override,
            last_stats: None,
        })
    }

    /// The prompt shown to the human: `aios[work] > `.
    pub fn prompt(&self) -> String {
        format!("aios[{}] > ", self.current)
    }

    /// Current workspace name.
    pub fn workspace_name(&self) -> &str {
        &self.current
    }

    /// Root directory of the current workspace.
    pub fn workspace_root(&self) -> std::path::PathBuf {
        self.workspaces
            .get(&self.current)
            .expect("current workspace exists")
            .root()
            .to_path_buf()
    }

    /// Root of the prototype state (`~/.aios`).
    pub fn state_root(&self) -> &std::path::Path {
        self.paths.root()
    }

    /// Current session log.
    pub fn session(&self) -> &Session {
        &self.session
    }

    /// Model name the console talks to.
    pub fn model_name(&self) -> &str {
        &self.model_info.name
    }

    /// Last reported usage, if any.
    pub fn last_stats(&self) -> Option<GenStats> {
        self.last_stats
    }

    /// Probes the backend (server up, model downloaded).
    pub fn health(&self) -> Result<ModelInfo, ModelError> {
        self.backend.health()
    }

    /// Closes the session with a reason (used on EOF).
    pub fn finish(&mut self, reason: &str) -> Result<(), ConsoleError> {
        self.session.finish(reason)
    }

    /// Handles one `/command` line.
    pub fn handle_command(&mut self, line: &str) -> Result<Action, ConsoleError> {
        let line = line.trim();
        self.session.append_command(line)?;
        let mut parts = line.split_whitespace();
        let command = parts.next().unwrap_or_default();
        match command {
            "/help" => Ok(Action {
                output: HELP.to_string(),
                quit: false,
            }),
            "/status" => Ok(Action {
                output: self.status_report(),
                quit: false,
            }),
            "/ws" => self.handle_ws(parts.next()),
            "/new" => self.handle_new(),
            "/quit" | "/exit" => {
                self.session.finish("quit")?;
                Ok(Action {
                    output: String::new(),
                    quit: true,
                })
            }
            other => Ok(Action {
                output: format!("неизвестная команда `{other}`; список — /help"),
                quit: false,
            }),
        }
    }

    fn handle_ws(&mut self, target: Option<&str>) -> Result<Action, ConsoleError> {
        match target {
            None => Ok(Action {
                output: format!(
                    "воркспейсы: {}; текущий: {}",
                    self.workspaces.names().join(", "),
                    self.current
                ),
                quit: false,
            }),
            Some(target) => {
                if !self.workspaces.contains(target) {
                    return Ok(Action {
                        output: format!(
                            "нет воркспейса `{target}`; доступны: {}",
                            self.workspaces.names().join(", ")
                        ),
                        quit: false,
                    });
                }
                if target == self.current {
                    return Ok(Action {
                        output: format!("уже в `{target}`"),
                        quit: false,
                    });
                }
                self.switch_workspace(target)?;
                Ok(Action {
                    output: format!(
                        "воркспейс: {target}\nсессия:    {} ({} сообщений)",
                        self.session.path().display(),
                        self.session.messages().len()
                    ),
                    quit: false,
                })
            }
        }
    }

    fn handle_new(&mut self) -> Result<Action, ConsoleError> {
        self.session.finish("new")?;
        let model = self.model_info.name.clone();
        let workspace = self
            .workspaces
            .get(&self.current)
            .expect("current workspace exists");
        self.session = Session::create_at(workspace, &model, time::now())?;
        self.last_stats = None;
        Ok(Action {
            output: format!("новая сессия: {}", self.session.path().display()),
            quit: false,
        })
    }

    fn switch_workspace(&mut self, target: &str) -> Result<(), ConsoleError> {
        self.session.finish("switch")?;
        let (workspace, config, model) = {
            let workspace = self
                .workspaces
                .get(target)
                .expect("target workspace is checked")
                .clone();
            let config = workspace.config().clone();
            let model = self
                .model_override
                .clone()
                .unwrap_or_else(|| config.model.clone());
            (workspace, config, model)
        };
        if self.backend.info().name != model {
            self.backend = (self.make_backend)(&model);
            self.model_info = self.backend.info();
        }
        self.session = Session::open_latest_or_new(&workspace, &model)?;
        self.current = target.to_string();
        self.config = config;
        self.last_stats = None;
        Ok(())
    }

    /// Sends a chat message and streams the answer through `on_token`.
    ///
    /// The user message is logged before the request (so a crash leaves a trace),
    /// the assistant answer after the stream ends. Errors are logged too.
    pub fn send(
        &mut self,
        text: &str,
        on_token: &mut dyn FnMut(&str),
    ) -> Result<Reply, ConsoleError> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(Reply {
                text: String::new(),
                stats: None,
            });
        }
        self.session.append_user(text)?;
        let request = GenRequest {
            messages: self.build_messages(),
            temperature: self.config.temperature,
            max_tokens: None,
        };
        let mut stream = match self.backend.generate(&request) {
            Ok(stream) => stream,
            Err(error) => {
                self.session.append_error(&error.to_string())?;
                return Err(ConsoleError::Model(error));
            }
        };

        let mut reply = String::new();
        let mut stats = None;
        while let Some(chunk) = stream.next_chunk() {
            match chunk {
                Ok(GenChunk {
                    text,
                    stats: chunk_stats,
                    ..
                }) => {
                    if !text.is_empty() {
                        reply.push_str(&text);
                        on_token(&text);
                    }
                    if let Some(chunk_stats) = chunk_stats {
                        stats = Some(chunk_stats);
                    }
                }
                Err(error) => {
                    self.session.append_error(&error.to_string())?;
                    return Err(ConsoleError::Model(error));
                }
            }
        }

        self.session
            .append_assistant(&reply, stats, &self.model_info.name)?;
        self.last_stats = stats;
        Ok(Reply { text: reply, stats })
    }

    /// Message list for the next request: system prompt plus a tail of the
    /// dialogue that fits the workspace context budget.
    fn build_messages(&self) -> Vec<Message> {
        let system = Message::system(&self.config.system_prompt);
        let budget = self.config.max_context_tokens.max(256);
        let mut used = self.backend.count_tokens(&system.content) + 4;
        let mut window: Vec<Message> = Vec::new();
        for message in self.session.messages().iter().rev() {
            let cost = self.backend.count_tokens(&message.content) + 4;
            if used + cost > budget && !window.is_empty() {
                break;
            }
            used += cost;
            window.push(message.clone());
        }
        window.reverse();
        let mut messages = Vec::with_capacity(window.len() + 1);
        messages.push(system);
        messages.extend(window);
        messages
    }

    fn context_estimate(&self) -> usize {
        self.build_messages()
            .iter()
            .map(|message| self.backend.count_tokens(&message.content) + 4)
            .sum()
    }

    /// Human-readable `/status` block.
    pub fn status_report(&self) -> String {
        let workspace = self
            .workspaces
            .get(&self.current)
            .expect("current workspace");
        let info = &self.model_info;
        let model_line = match (info.context_tokens, info.quant.as_deref()) {
            (0, _) => format!("{} (контекст неизвестен — ollama не ответил)", info.name),
            (context, Some(quant)) => {
                format!("{} (контекст {}, квант {})", info.name, context, quant)
            }
            (context, None) => format!("{} (контекст {})", info.name, context),
        };
        let stats_line = match self.last_stats {
            Some(stats) => format!(
                "; последний ответ: prompt {} / completion {} (ollama)",
                stats.prompt_tokens, stats.completion_tokens
            ),
            None => String::new(),
        };
        [
            format!(
                "воркспейс: {} — {}",
                self.current,
                workspace.root().display()
            ),
            format!("модель:    {model_line}"),
            format!(
                "сессия:    сообщений {}, лог: {}",
                self.session.messages().len(),
                self.session.path().display()
            ),
            format!(
                "токены:    контекст ~{} (оценка); сессия: prompt {} / completion {}{stats_line}",
                self.context_estimate(),
                self.session.prompt_tokens(),
                self.session.completion_tokens()
            ),
            format!(
                "файлы:     files/ — {}; sessions/ — {}; память — карточка 016",
                count(workspace.list_files()),
                count(workspace.list_sessions())
            ),
        ]
        .join("\n")
    }
}

fn count<T>(items: Result<Vec<T>, ConsoleError>) -> String {
    match items {
        Ok(items) => items.len().to_string(),
        Err(_) => "?".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{App, BackendFactory};
    use crate::paths::Paths;
    use aios_modeld::{GenRequest, GenStats, GenStream, ModelBackend, ModelError, ModelInfo, Role};

    /// Echo backend: answers `эхо: <last user message>` in two chunks and
    /// reports deterministic usage counters.
    struct StubBackend {
        model: String,
    }

    impl ModelBackend for StubBackend {
        fn info(&self) -> ModelInfo {
            ModelInfo {
                name: self.model.clone(),
                context_tokens: 4096,
                quant: Some("Q4_K_M".to_string()),
            }
        }

        fn generate(&self, request: &GenRequest) -> Result<GenStream, ModelError> {
            let last = request
                .messages
                .iter()
                .rev()
                .find(|message| message.role == Role::User)
                .map(|message| message.content.clone())
                .unwrap_or_default();
            let content = serde_json::to_string(&format!("эхо: {last}")).expect("json");
            let body = format!(
                "{{\"message\":{{\"role\":\"assistant\",\"content\":{content}}},\"done\":false}}\n\
                 {{\"message\":{{\"role\":\"assistant\",\"content\":\"\"}},\"done\":true,\
                 \"prompt_eval_count\":7,\"eval_count\":3}}\n"
            );
            Ok(GenStream::from_reader(std::io::Cursor::new(body)))
        }

        fn count_tokens(&self, text: &str) -> usize {
            aios_modeld::estimate_tokens(text)
        }
    }

    fn factory() -> BackendFactory {
        Box::new(|model: &str| {
            Box::new(StubBackend {
                model: model.to_string(),
            }) as Box<dyn ModelBackend>
        })
    }

    fn app(dir: &std::path::Path) -> App {
        App::new(
            factory(),
            Paths::at(dir.join(".aios")),
            "work",
            Some("stub:1b".to_string()),
        )
        .expect("app")
    }

    #[test]
    fn send_streams_answer_and_logs_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = app(dir.path());
        let mut streamed = String::new();
        let reply = app
            .send("привет, кто ты?", &mut |token| {
                streamed.push_str(token)
            })
            .expect("send");

        assert_eq!(streamed, "эхо: привет, кто ты?");
        assert_eq!(reply.text, streamed);
        assert_eq!(
            reply.stats,
            Some(GenStats {
                prompt_tokens: 7,
                completion_tokens: 3
            })
        );
        assert_eq!(app.last_stats(), reply.stats);
        assert_eq!(app.session().messages().len(), 2);
        assert_eq!(app.session().prompt_tokens(), 7);
        assert_eq!(app.session().completion_tokens(), 3);

        let log = std::fs::read_to_string(app.session().path()).expect("log");
        assert!(log.contains("эхо: привет, кто ты?"));
        assert!(log.contains("\"prompt_tokens\":7"));
        assert_eq!(
            log.lines().filter(|line| !line.trim().is_empty()).count(),
            3
        );
    }

    #[test]
    fn restart_restores_the_session_and_dialogue() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = {
            let mut app = app(dir.path());
            app.send("запомни это", &mut |_| {}).expect("send");
            app.session().path().to_path_buf()
        };

        let mut restarted = app(dir.path());
        assert_eq!(restarted.session().path(), path.as_path());
        assert_eq!(restarted.session().messages().len(), 2);
        assert_eq!(restarted.session().messages()[0].content, "запомни это");
        // Продолжение диалога видно модели: окно контекста содержит историю.
        let mut streamed = String::new();
        restarted
            .send("продолжим", &mut |token| streamed.push_str(token))
            .expect("send");
        assert_eq!(streamed, "эхо: продолжим");
        assert_eq!(restarted.session().messages().len(), 4);
    }

    #[test]
    fn switching_workspace_resets_and_restores_history() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = app(dir.path());
        app.send("рабочий вопрос", &mut |_| {}).expect("send");

        let action = app.handle_command("/ws home").expect("ws home");
        assert!(!action.quit);
        assert!(action.output.contains("home"), "{}", action.output);
        assert_eq!(app.prompt(), "aios[home] > ");
        assert!(
            app.session().messages().is_empty(),
            "home must not see work dialogue"
        );

        app.send("домашний вопрос", &mut |_| {}).expect("send");
        assert_eq!(app.session().messages().len(), 2);
        let home_log = app.session().path().to_path_buf();
        assert!(home_log.to_string_lossy().contains("home/sessions"));

        app.handle_command("/ws work").expect("ws work");
        assert_eq!(app.prompt(), "aios[work] > ");
        assert_eq!(app.session().messages().len(), 2);
        assert_eq!(app.session().messages()[0].content, "рабочий вопрос");
        assert!(
            app.session()
                .path()
                .to_string_lossy()
                .contains("work/sessions")
        );
    }

    #[test]
    fn status_reports_model_workspace_tokens_and_log() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = app(dir.path());
        app.send("привет", &mut |_| {}).expect("send");

        let status = app.handle_command("/status").expect("status").output;
        assert!(status.contains("work"), "{status}");
        assert!(status.contains("stub:1b"), "{status}");
        assert!(status.contains("контекст 4096"), "{status}");
        assert!(status.contains("prompt 7"), "{status}");
        assert!(status.contains("completion 3"), "{status}");
        assert!(status.contains("sessions"), "{status}");
        assert!(
            status.contains(app.session().path().to_string_lossy().as_ref()),
            "{status}"
        );
    }

    #[test]
    fn new_starts_a_fresh_session_and_keeps_the_old_log() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = app(dir.path());
        app.send("старое", &mut |_| {}).expect("send");
        let previous = app.session().path().to_path_buf();

        let action = app.handle_command("/new").expect("new");
        assert!(action.output.contains("новая сессия"), "{}", action.output);
        assert_ne!(app.session().path(), previous.as_path());
        assert!(previous.exists(), "old log must stay on disk");
        assert!(app.session().messages().is_empty());

        let sessions = app.session().path().parent().expect("dir").to_path_buf();
        assert_eq!(std::fs::read_dir(sessions).expect("list").count(), 2);
    }

    #[test]
    fn ws_and_unknown_commands_are_reported() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = app(dir.path());

        let listed = app.handle_command("/ws").expect("ws list").output;
        assert!(
            listed.contains("home") && listed.contains("work"),
            "{listed}"
        );
        assert_eq!(app.prompt(), "aios[work] > ");

        let missing = app.handle_command("/ws nope").expect("ws missing").output;
        assert!(missing.contains("нет воркспейса"), "{missing}");
        assert_eq!(app.prompt(), "aios[work] > ");

        let unknown = app.handle_command("/wat").expect("unknown").output;
        assert!(unknown.contains("/help"), "{unknown}");
    }

    #[test]
    fn help_lists_commands_and_quit_exits() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = app(dir.path());

        let help = app.handle_command("/help").expect("help").output;
        for command in ["/ws", "/status", "/new", "/quit", "/help"] {
            assert!(help.contains(command), "{command} missing from help");
        }

        let action = app.handle_command("/quit").expect("quit");
        assert!(action.quit);
        let log = std::fs::read_to_string(app.session().path()).expect("log");
        assert!(log.contains("\"reason\":\"quit\""));
    }
}
