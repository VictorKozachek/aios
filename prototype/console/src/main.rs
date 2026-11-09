//! `aios-console` — the prototype REPL (docs/PROTOTYPE.md §6).
//!
//! ```text
//! cargo run -p aios-console                 # workspace work, model from config
//! cargo run -p aios-console -- --workspace home
//! cargo run -p aios-console -- --model qwen2.5:0.5b
//! ```
//!
//! Environment:
//! * `AIOS_HOME` — state directory, defaults to `$HOME/.aios`;
//! * `AIOS_OLLAMA_URL` — ollama address, defaults to `http://127.0.0.1:11434`;
//! * `AIOS_MODEL` — model tag, overrides `config.toml` of the workspace.

use std::process::ExitCode;

use aios_console::{App, BackendFactory, Paths, repl};
use aios_modeld::ModelBackend;
use aios_modeld::ModelInfo;
use aios_modeld::ollama::{DEFAULT_BASE_URL, OllamaBackend};

const USAGE: &str = "\
использование: aios-console [--workspace home|work] [--model TAG]

переменные окружения:
  AIOS_HOME        каталог состояния (по умолчанию $HOME/.aios)
  AIOS_OLLAMA_URL  адрес ollama (по умолчанию http://127.0.0.1:11434)
  AIOS_MODEL       модель, перекрывает model из config.toml воркспейса

внутри: /help — команды, Ctrl-D — выход";

fn main() -> ExitCode {
    let mut workspace = String::from("work");
    let mut model: Option<String> = std::env::var("AIOS_MODEL")
        .ok()
        .filter(|value| !value.is_empty());

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--workspace" | "-w" => match args.next() {
                Some(value) => workspace = value,
                None => {
                    eprintln!("--workspace требует значение (home или work)");
                    return ExitCode::FAILURE;
                }
            },
            "--model" | "-m" => match args.next() {
                Some(value) => model = Some(value),
                None => {
                    eprintln!("--model требует значение (например, qwen2.5:0.5b)");
                    return ExitCode::FAILURE;
                }
            },
            "--help" | "-h" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("неизвестный аргумент: {other}\n\n{USAGE}");
                return ExitCode::FAILURE;
            }
        }
    }

    let base_url =
        std::env::var("AIOS_OLLAMA_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string());
    let paths = match Paths::from_env() {
        Ok(paths) => paths,
        Err(error) => {
            eprintln!("ошибка: {error}");
            return ExitCode::FAILURE;
        }
    };
    let factory: BackendFactory = Box::new(move |model: &str| {
        Box::new(OllamaBackend::new(base_url.clone(), model)) as Box<dyn ModelBackend>
    });

    let mut app = match App::new(factory, paths, &workspace, model) {
        Ok(app) => app,
        Err(error) => {
            eprintln!("ошибка: {error}");
            return ExitCode::FAILURE;
        }
    };

    println!("AIOS prototype console · PR-015 (modeld + workspaces)");
    println!(
        "воркспейс: {} — {}",
        app.workspace_name(),
        app.workspace_root().display()
    );
    match app.health() {
        Ok(info) => println!("модель:    {}", describe(&info)),
        Err(error) => println!("модель:    {} — {error}", app.model_name()),
    }
    println!(
        "сессия:    {} ({} сообщений)",
        app.session().path().display(),
        app.session().messages().len()
    );
    println!("/help — команды, Ctrl-D — выход\n");

    match repl::run(&mut app) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ошибка: {error}");
            ExitCode::FAILURE
        }
    }
}

fn describe(info: &ModelInfo) -> String {
    match (info.context_tokens, info.quant.as_deref()) {
        (0, _) => info.name.clone(),
        (context, Some(quant)) => format!("{} (контекст {}, квант {})", info.name, context, quant),
        (context, None) => format!("{} (контекст {})", info.name, context),
    }
}
