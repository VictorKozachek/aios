//! Where the prototype keeps its state (`~/.aios`).
//!
//! `HOME` is the only input on a normal machine; `AIOS_HOME` overrides the
//! whole `~/.aios` directory and exists for tests and experiments. The tests
//! deliberately go through `HOME` (spawned binary) to prove that no absolute
//! path leaks into the code.

use std::path::{Path, PathBuf};

use crate::error::ConsoleError;

/// Root of the prototype state directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    root: PathBuf,
}

impl Paths {
    /// Uses `AIOS_HOME`, or `$HOME/.aios`.
    pub fn from_env() -> Result<Self, ConsoleError> {
        if let Some(root) = std::env::var_os("AIOS_HOME").filter(|value| !value.is_empty()) {
            return Ok(Paths::at(PathBuf::from(root)));
        }
        let home = std::env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .ok_or(ConsoleError::NoHome)?;
        Ok(Paths::at(PathBuf::from(home).join(".aios")))
    }

    /// Uses an explicit root directory (tests, embedding).
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Paths { root: root.into() }
    }

    /// `~/.aios` itself.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `~/.aios/workspaces`.
    pub fn workspaces_dir(&self) -> PathBuf {
        self.root.join("workspaces")
    }
}

#[cfg(test)]
mod tests {
    use super::Paths;

    #[test]
    fn root_and_workspaces_dir_are_derived_from_the_root() {
        let paths = Paths::at("/tmp/aios-home");
        assert_eq!(paths.root(), std::path::Path::new("/tmp/aios-home"));
        assert_eq!(
            paths.workspaces_dir(),
            std::path::PathBuf::from("/tmp/aios-home/workspaces")
        );
    }
}
