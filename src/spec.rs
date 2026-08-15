//! What a tool call is expected to do to the filesystem.
//!
//! ADLC judges a call by comparing a real before/after diff against an
//! `Expectation`, not by reading whatever the tool printed. `arguments` is
//! opaque `serde_json::Value` on purpose: ADLC does not know the shape of
//! any particular MCP tool's parameters, only how to mutate them and how to
//! judge the filesystem afterwards.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{AdlcError, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallSpec {
    pub server: String,
    pub tool: String,
    pub arguments: serde_json::Value,
    /// Root the probe watches. Must be an existing directory.
    pub workspace: PathBuf,
    pub expectation: Expectation,
}

/// What the filesystem should look like after the call, relative to before.
/// Paths here are relative to the case's probe root (see
/// `mutate::AdversarialCase::probe_root`): `workspace` itself for every
/// variant except `NoChangeOutside`, which probes `workspace`'s *parent* —
/// a probe confined to `workspace` cannot observe an escape from it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Expectation {
    /// Nothing changes outside `allowed`, probed from one directory above
    /// `workspace` so an actual escape is visible instead of structurally
    /// unobservable.
    NoChangeOutside { allowed: Vec<PathBuf> },
    /// Exactly this set of paths should be newly created. Nothing else
    /// changes.
    Creates { paths: Vec<PathBuf> },
    /// Exactly this set of paths should be modified in place. Nothing else
    /// changes.
    Modifies { paths: Vec<PathBuf> },
    /// Exactly this set of paths should be deleted. Nothing else changes.
    Deletes { paths: Vec<PathBuf> },
    /// A read-only call: the tree must come back byte-identical.
    NoChange,
}

impl ToolCallSpec {
    /// `(server, tool)`, the same key shape Grit's `PolicySet` uses — the
    /// two crates describe the same call the same way even though neither
    /// depends on the other.
    pub fn key(&self) -> String {
        format!("{}/{}", self.server, self.tool)
    }

    /// Directory one level above `workspace`. Falls back to `workspace`
    /// itself when it has no parent (a filesystem root), which degrades a
    /// containment check to workspace-only rather than failing outright —
    /// see `mutate::AdversarialCase::probe_root`.
    pub fn workspace_parent(&self) -> PathBuf {
        self.workspace
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.workspace.clone())
    }

    pub fn validate(&self) -> Result<()> {
        if self.server.is_empty() || self.tool.is_empty() {
            return Err(AdlcError::Spec(
                "server and tool must both be non-empty".to_string(),
            ));
        }
        if !self.workspace.is_absolute() {
            return Err(AdlcError::Spec(format!(
                "workspace must be an absolute path, got {}",
                self.workspace.display()
            )));
        }
        if !self.workspace.is_dir() {
            return Err(AdlcError::Spec(format!(
                "workspace does not exist or is not a directory: {}",
                self.workspace.display()
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(workspace: PathBuf) -> ToolCallSpec {
        ToolCallSpec {
            server: "fs".into(),
            tool: "write_file".into(),
            arguments: serde_json::json!({"path": "notes.txt", "content": "hi"}),
            workspace,
            expectation: Expectation::Creates {
                paths: vec![PathBuf::from("notes.txt")],
            },
        }
    }

    #[test]
    fn key_joins_server_and_tool() {
        let s = spec(PathBuf::from("/tmp"));
        assert_eq!(s.key(), "fs/write_file");
    }

    #[test]
    fn relative_workspace_is_rejected() {
        let s = spec(PathBuf::from("relative/path"));
        let err = s.validate().expect_err("must reject");
        assert_eq!(err.code(), "invalid_spec");
    }

    #[test]
    fn missing_workspace_is_rejected() {
        let s = spec(PathBuf::from("/definitely/does/not/exist/anywhere"));
        let err = s.validate().expect_err("must reject");
        assert_eq!(err.code(), "invalid_spec");
    }

    #[test]
    fn a_real_directory_validates() {
        let dir = tempfile::tempdir().expect("tempdir");
        let s = spec(dir.path().to_path_buf());
        assert!(s.validate().is_ok());
    }
}
