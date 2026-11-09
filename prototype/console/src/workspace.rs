//! Workspaces: scope of memory, own files, own sessions (docs/PROTOTYPE.md §5).
//!
//! ```text
//! ~/.aios/workspaces/
//!   home/  config.toml  files/  sessions/  memory/
//!   work/  config.toml  files/  sessions/  memory/
//! ```
//!
//! Isolation is structural: every workspace owns a directory tree, and the
//! prototype never crosses from one tree into another. `files/` and `memory/`
//! are what card 016 will use; this card only proves that the trees exist and
//! do not overlap.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::WorkspaceConfig;
use crate::error::ConsoleError;
use crate::paths::Paths;

/// Workspaces the prototype knows about; created on first run.
pub const WORKSPACES: [&str; 2] = ["home", "work"];

/// One workspace directory.
#[derive(Debug, Clone)]
pub struct Workspace {
    name: String,
    root: PathBuf,
    config: WorkspaceConfig,
}

impl Workspace {
    /// Opens (creating if needed) a workspace directory.
    ///
    /// Creates `files/`, `sessions/`, `memory/` and a default `config.toml`.
    pub fn open(root: PathBuf, name: &str) -> Result<Self, ConsoleError> {
        for dir in ["files", "sessions", "memory"] {
            let path = root.join(dir);
            fs::create_dir_all(&path)
                .map_err(|error| ConsoleError::io(format!("создать {}", path.display()), error))?;
        }
        let config_path = root.join("config.toml");
        if !config_path.exists() {
            let default = WorkspaceConfig::default_for(name);
            fs::write(&config_path, default.to_toml(name)).map_err(|error| {
                ConsoleError::io(format!("записать {}", config_path.display()), error)
            })?;
        }
        let config = WorkspaceConfig::load(&config_path)?;
        Ok(Workspace {
            name: name.to_string(),
            root,
            config,
        })
    }

    /// Workspace name (`home`, `work`).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Workspace root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `files/` — documents this workspace may read.
    pub fn files_dir(&self) -> PathBuf {
        self.root.join("files")
    }

    /// `sessions/` — NDJSON session logs.
    pub fn sessions_dir(&self) -> PathBuf {
        self.root.join("sessions")
    }

    /// `memory/` — reserved for card 016 (facts, index).
    pub fn memory_dir(&self) -> PathBuf {
        self.root.join("memory")
    }

    /// `config.toml` of this workspace.
    pub fn config_path(&self) -> PathBuf {
        self.root.join("config.toml")
    }

    /// Parsed configuration.
    pub fn config(&self) -> &WorkspaceConfig {
        &self.config
    }

    /// File names in `files/`, sorted. Only this workspace's tree is listed.
    pub fn list_files(&self) -> Result<Vec<String>, ConsoleError> {
        list_names(&self.files_dir())
    }

    /// Session log paths in `sessions/`, sorted by file name (so by time).
    pub fn list_sessions(&self) -> Result<Vec<PathBuf>, ConsoleError> {
        let dir = self.sessions_dir();
        let mut paths = Vec::new();
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(paths),
            Err(error) => {
                return Err(ConsoleError::io(format!("читать {}", dir.display()), error));
            }
        };
        for entry in entries {
            let entry = entry.map_err(|error| ConsoleError::io("читать сессии", error))?;
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) == Some("ndjson") {
                paths.push(path);
            }
        }
        paths.sort();
        Ok(paths)
    }

    /// Newest session log of this workspace, if any.
    pub fn latest_session(&self) -> Result<Option<PathBuf>, ConsoleError> {
        Ok(self.list_sessions()?.pop())
    }
}

fn list_names(dir: &Path) -> Result<Vec<String>, ConsoleError> {
    let mut names = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(names),
        Err(error) => {
            return Err(ConsoleError::io(format!("читать {}", dir.display()), error));
        }
    };
    for entry in entries {
        let entry = entry.map_err(|error| ConsoleError::io("читать каталог", error))?;
        if entry
            .file_type()
            .map(|kind| kind.is_file())
            .unwrap_or(false)
        {
            names.push(entry.file_name().to_string_lossy().to_string());
        }
    }
    names.sort();
    Ok(names)
}

/// All prototype workspaces.
#[derive(Debug)]
pub struct Workspaces {
    map: BTreeMap<String, Workspace>,
}

impl Workspaces {
    /// Creates/opens the workspaces under `~/.aios/workspaces`.
    pub fn open(paths: &Paths) -> Result<Self, ConsoleError> {
        let base = paths.workspaces_dir();
        fs::create_dir_all(&base)
            .map_err(|error| ConsoleError::io(format!("создать {}", base.display()), error))?;
        let mut map = BTreeMap::new();
        for name in WORKSPACES {
            let workspace = Workspace::open(base.join(name), name)?;
            map.insert(name.to_string(), workspace);
        }
        Ok(Workspaces { map })
    }

    /// Looks a workspace up by name.
    pub fn get(&self, name: &str) -> Option<&Workspace> {
        self.map.get(name)
    }

    /// All workspace names, sorted.
    pub fn names(&self) -> Vec<&str> {
        self.map.keys().map(String::as_str).collect()
    }

    /// `true` if the workspace exists.
    pub fn contains(&self, name: &str) -> bool {
        self.map.contains_key(name)
    }
}

#[cfg(test)]
mod tests {
    use super::{WORKSPACES, Workspace, Workspaces};
    use crate::paths::Paths;

    fn open_test_workspaces() -> (tempfile::TempDir, Workspaces) {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::at(dir.path().join(".aios"));
        let workspaces = Workspaces::open(&paths).expect("open workspaces");
        (dir, workspaces)
    }

    #[test]
    fn workspaces_are_created_with_files_sessions_and_config() {
        let (dir, workspaces) = open_test_workspaces();
        for name in WORKSPACES {
            let root = dir.path().join(".aios/workspaces").join(name);
            assert!(workspaces.contains(name));
            assert_eq!(workspaces.get(name).expect("workspace").root(), root);
            assert!(root.join("files").is_dir());
            assert!(root.join("sessions").is_dir());
            assert!(root.join("memory").is_dir());
            assert!(root.join("config.toml").is_file());
        }
        assert_eq!(workspaces.names(), vec!["home", "work"]);
    }

    #[test]
    fn workspaces_do_not_see_each_others_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = Workspace::open(dir.path().join("home"), "home").expect("home");
        let work = Workspace::open(dir.path().join("work"), "work").expect("work");

        std::fs::write(home.files_dir().join("secret.txt"), "только для home").expect("write");
        std::fs::write(work.files_dir().join("plan.md"), "# work").expect("write");

        assert_eq!(home.list_files().expect("list home"), vec!["secret.txt"]);
        assert_eq!(work.list_files().expect("list work"), vec!["plan.md"]);
        assert_ne!(home.files_dir(), work.files_dir());
        assert_ne!(home.sessions_dir(), work.sessions_dir());
        assert!(!work.files_dir().join("secret.txt").exists());
    }

    #[test]
    fn session_dirs_are_independent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = Workspace::open(dir.path().join("home"), "home").expect("home");
        let work = Workspace::open(dir.path().join("work"), "work").expect("work");

        std::fs::write(home.sessions_dir().join("20260101-000000.ndjson"), "{}\n").expect("write");

        assert_eq!(home.list_sessions().expect("home sessions").len(), 1);
        assert!(work.list_sessions().expect("work sessions").is_empty());
        assert_eq!(
            home.latest_session()
                .expect("latest")
                .expect("some")
                .file_name()
                .and_then(|name| name.to_str()),
            Some("20260101-000000.ndjson")
        );
        assert!(work.latest_session().expect("latest").is_none());
    }
}
