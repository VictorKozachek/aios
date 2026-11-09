//! Per-workspace configuration: `config.toml` (docs/PROTOTYPE.md §5).
//!
//! The file is created with defaults on the first run of a workspace and can be
//! edited by hand: model, temperature, context budget and the workspace system
//! prompt. Unknown keys are ignored so that later cards (memory, retrieval) can
//! add settings without breaking older files.

use std::path::Path;

use serde::Deserialize;

use crate::error::ConsoleError;

/// Settings of one workspace.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct WorkspaceConfig {
    /// Model tag served by the backend, e.g. `qwen2.5:0.5b`.
    pub model: String,
    /// Sampling temperature.
    pub temperature: f32,
    /// Token budget for the context window built before each request.
    pub max_context_tokens: usize,
    /// System prompt of this workspace.
    pub system_prompt: String,
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        WorkspaceConfig::default_for("work")
    }
}

impl WorkspaceConfig {
    /// Defaults for a named workspace: `home` and `work` get different system
    /// prompts, everything else gets the neutral one.
    pub fn default_for(workspace: &str) -> Self {
        let system_prompt = match workspace {
            "home" => {
                "Ты — AIOS, локальный ассистент в домашнем воркспейсе. \
                 Отвечай дружелюбно и кратко, помогай с заметками и повседневными вопросами."
            }
            "work" => {
                "Ты — AIOS, локальный ассистент в рабочем воркспейсе. \
                 Отвечай кратко и по делу, помогай с кодом, документами и решениями."
            }
            _ => "Ты — AIOS, локальный ассистент. Отвечай кратко и по делу.",
        };
        WorkspaceConfig {
            model: "qwen2.5:0.5b".to_string(),
            temperature: 0.7,
            max_context_tokens: 3000,
            system_prompt: system_prompt.to_string(),
        }
    }

    /// Reads a workspace config; missing keys fall back to defaults.
    pub fn load(path: &Path) -> Result<Self, ConsoleError> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| ConsoleError::io(format!("читать {}", path.display()), error))?;
        toml::from_str(&text).map_err(|error| ConsoleError::Config {
            path: path.to_path_buf(),
            detail: error.to_string(),
        })
    }

    /// Renders the config as the file that is written on first run.
    pub fn to_toml(&self, workspace: &str) -> String {
        format!(
            "# AIOS workspace: {workspace}\n\
             # Модель, сэмплирование и системный промпт этого воркспейса.\n\
             # Правьте руками; неизвестные ключи игнорируются.\n\
             \n\
             model = \"{}\"\n\
             temperature = {}\n\
             max_context_tokens = {}\n\
             system_prompt = \"{}\"\n",
            escape(&self.model),
            self.temperature,
            self.max_context_tokens,
            escape(&self.system_prompt),
        )
    }
}

fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

#[cfg(test)]
mod tests {
    use super::WorkspaceConfig;

    #[test]
    fn home_and_work_prompts_differ() {
        let home = WorkspaceConfig::default_for("home");
        let work = WorkspaceConfig::default_for("work");
        assert_ne!(home.system_prompt, work.system_prompt);
        assert!(home.system_prompt.contains("домашнем"));
        assert!(work.system_prompt.contains("рабочем"));
        assert_eq!(home.model, "qwen2.5:0.5b");
        assert_eq!(work.temperature, 0.7);
    }

    #[test]
    fn missing_keys_fall_back_to_defaults() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "model = \"tiny:1b\"\n").expect("write");
        let config = WorkspaceConfig::load(&path).expect("parse");
        assert_eq!(config.model, "tiny:1b");
        assert_eq!(config.temperature, 0.7);
        assert_eq!(config.max_context_tokens, 3000);
        assert!(!config.system_prompt.is_empty());
    }

    #[test]
    fn to_toml_roundtrips_through_load() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        let mut original = WorkspaceConfig::default_for("work");
        original.system_prompt = "строка с \"кавычками\" и \\ слэшем".to_string();
        original.temperature = 0.2;
        original.max_context_tokens = 1024;
        std::fs::write(&path, original.to_toml("work")).expect("write");
        let loaded = WorkspaceConfig::load(&path).expect("parse");
        assert_eq!(loaded, original);
    }

    #[test]
    fn broken_config_is_reported_with_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "model = [not a string]\n").expect("write");
        let error = WorkspaceConfig::load(&path).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("config.toml"), "{message}");
    }
}
