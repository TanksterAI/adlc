//! Turning a directory into a comparable snapshot, and comparing two of them.
//!
//! # Design decision: bounded, and an error rather than a truncation
//!
//! Mirrors the constraint Grit's snapshot engine works under, for the same
//! reason: a diff computed from a snapshot that silently stopped partway
//! through looks exactly like a diff computed from a complete one. Exceeding
//! `Limits` is `AdlcError::SnapshotBounds`, never a shorter-than-requested
//! `StateSnapshot`. This is written fresh here rather than depending on
//! Grit's crate — see `CLAUDE.md` for why keeping the two repos' CI
//! independent won the trade against reuse.
//!
//! # Design decision: a symlink pointing outside the workspace is never captured
//!
//! Same reasoning as Grit: copying its target in as content and later
//! comparing against it would make a diff about a file the workspace does
//! not actually own.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{AdlcError, Result};

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_files: usize,
    pub max_total_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_files: 5_000,
            max_total_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileDigest {
    pub sha256: String,
    pub len: u64,
}

/// A point-in-time content map of a directory tree, keyed by path relative
/// to the root that was captured.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateSnapshot {
    pub root: PathBuf,
    pub files: BTreeMap<PathBuf, FileDigest>,
}

pub trait StateProbe {
    fn capture(&self) -> Result<StateSnapshot>;
}

pub struct FilesystemProbe {
    pub root: PathBuf,
    pub limits: Limits,
}

impl FilesystemProbe {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            limits: Limits::default(),
        }
    }
}

impl StateProbe for FilesystemProbe {
    fn capture(&self) -> Result<StateSnapshot> {
        let root = self.root.canonicalize().map_err(|source| AdlcError::Io {
            path: self.root.clone(),
            source,
        })?;

        let mut files = BTreeMap::new();
        let mut total_bytes: u64 = 0;

        for entry in walkdir::WalkDir::new(&root).follow_links(false) {
            let entry = entry.map_err(|e| AdlcError::Probe(e.to_string()))?;
            let file_type = entry.file_type();

            if file_type.is_dir() {
                continue;
            }

            let path = entry.path();

            if file_type.is_symlink() {
                match path.canonicalize() {
                    Ok(target) if contains(&root, &target) => {} // inside: fall through and read it
                    _ => continue, // outside, or broken: never captured
                }
            } else if !file_type.is_file() {
                continue; // sockets, fifos, devices: not meaningful to diff
            }

            let metadata = entry
                .metadata()
                .map_err(|e| AdlcError::Probe(e.to_string()))?;
            let len = metadata.len();

            if files.len() + 1 > self.limits.max_files {
                return Err(AdlcError::SnapshotBounds(format!(
                    "more than {} files under {}",
                    self.limits.max_files,
                    root.display()
                )));
            }
            total_bytes = total_bytes.saturating_add(len);
            if total_bytes > self.limits.max_total_bytes {
                return Err(AdlcError::SnapshotBounds(format!(
                    "more than {} bytes under {}",
                    self.limits.max_total_bytes,
                    root.display()
                )));
            }

            let bytes = std::fs::read(path).map_err(|source| AdlcError::Io {
                path: path.to_path_buf(),
                source,
            })?;
            let sha256 = format!("{:x}", Sha256::digest(&bytes));

            let relative = path
                .strip_prefix(&root)
                .map_err(|_| {
                    AdlcError::Probe(format!(
                        "{} is not under {}",
                        path.display(),
                        root.display()
                    ))
                })?
                .to_path_buf();

            files.insert(relative, FileDigest { sha256, len });
        }

        Ok(StateSnapshot { root, files })
    }
}

/// Component-wise containment, not a string prefix comparison — see Grit's
/// `containment.rs` for the sibling-directory case this avoids
/// (`/a/project` vs `/a/project-secrets`). Both crates need this exact
/// check; neither is positioned to be "the" place it lives yet.
///
/// `pub(crate)` because `verdict::judge` reuses it for `NoChangeOutside`.
pub(crate) fn contains(root: &Path, candidate: &Path) -> bool {
    let mut r = root.components();
    let mut c = candidate.components();
    loop {
        match (r.next(), c.next()) {
            (None, _) => return true,
            (Some(_), None) => return false,
            (Some(a), Some(b)) if a == b => continue,
            _ => return false,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StateTransition {
    pub created: Vec<PathBuf>,
    pub modified: Vec<PathBuf>,
    pub deleted: Vec<PathBuf>,
}

impl StateTransition {
    pub fn is_empty(&self) -> bool {
        self.created.is_empty() && self.modified.is_empty() && self.deleted.is_empty()
    }

    /// Every path this transition touched, created or modified or deleted.
    pub fn touched(&self) -> impl Iterator<Item = &PathBuf> {
        self.created
            .iter()
            .chain(&self.modified)
            .chain(&self.deleted)
    }
}

pub fn diff(before: &StateSnapshot, after: &StateSnapshot) -> StateTransition {
    let mut created = Vec::new();
    let mut modified = Vec::new();

    for (path, after_digest) in &after.files {
        match before.files.get(path) {
            None => created.push(path.clone()),
            Some(before_digest) if before_digest != after_digest => modified.push(path.clone()),
            Some(_) => {}
        }
    }

    let mut deleted: Vec<PathBuf> = before
        .files
        .keys()
        .filter(|path| !after.files.contains_key(*path))
        .cloned()
        .collect();

    created.sort();
    modified.sort();
    deleted.sort();

    StateTransition {
        created,
        modified,
        deleted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn a_freshly_created_file_shows_up_as_created() {
        let dir = tempfile::tempdir().expect("tempdir");
        let probe = FilesystemProbe::new(dir.path().to_path_buf());
        let before = probe.capture().expect("capture");

        fs::write(dir.path().join("new.txt"), b"hello").expect("write");
        let after = probe.capture().expect("capture");

        let d = diff(&before, &after);
        assert_eq!(d.created, vec![PathBuf::from("new.txt")]);
        assert!(d.modified.is_empty());
        assert!(d.deleted.is_empty());
    }

    #[test]
    fn a_content_change_shows_up_as_modified_not_created_and_deleted() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("f.txt"), b"v1").expect("write");
        let probe = FilesystemProbe::new(dir.path().to_path_buf());
        let before = probe.capture().expect("capture");

        fs::write(dir.path().join("f.txt"), b"v2-longer").expect("write");
        let after = probe.capture().expect("capture");

        let d = diff(&before, &after);
        assert_eq!(d.modified, vec![PathBuf::from("f.txt")]);
        assert!(d.created.is_empty());
        assert!(d.deleted.is_empty());
    }

    #[test]
    fn a_removed_file_shows_up_as_deleted() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("gone.txt"), b"bye").expect("write");
        let probe = FilesystemProbe::new(dir.path().to_path_buf());
        let before = probe.capture().expect("capture");

        fs::remove_file(dir.path().join("gone.txt")).expect("remove");
        let after = probe.capture().expect("capture");

        let d = diff(&before, &after);
        assert_eq!(d.deleted, vec![PathBuf::from("gone.txt")]);
    }

    #[test]
    fn an_untouched_tree_diffs_to_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("stable.txt"), b"unchanged").expect("write");
        let probe = FilesystemProbe::new(dir.path().to_path_buf());
        let before = probe.capture().expect("capture");
        let after = probe.capture().expect("capture");

        assert!(diff(&before, &after).is_empty());
    }

    #[test]
    fn a_symlink_pointing_outside_the_root_is_never_captured() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("work");
        let outside = dir.path().join("outside");
        fs::create_dir_all(&root).expect("mkdir");
        fs::create_dir_all(&outside).expect("mkdir");
        fs::write(outside.join("secret.txt"), b"shh").expect("write");

        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.join("secret.txt"), root.join("link.txt"))
            .expect("symlink");

        #[cfg(unix)]
        {
            let probe = FilesystemProbe::new(root);
            let snap = probe.capture().expect("capture");
            assert!(
                snap.files.is_empty(),
                "escaping symlink must not be captured"
            );
        }
    }

    #[test]
    fn exceeding_the_file_count_limit_is_an_error_not_a_partial_snapshot() {
        let dir = tempfile::tempdir().expect("tempdir");
        for i in 0..5 {
            fs::write(dir.path().join(format!("f{i}.txt")), b"x").expect("write");
        }
        let probe = FilesystemProbe {
            root: dir.path().to_path_buf(),
            limits: Limits {
                max_files: 2,
                max_total_bytes: u64::MAX,
            },
        };
        let err = probe.capture().expect_err("must refuse, not truncate");
        assert_eq!(err.code(), "snapshot_bounds", "{err}");
    }

    #[test]
    fn exceeding_the_byte_limit_is_an_error_not_a_partial_snapshot() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("big.txt"), vec![0u8; 1024]).expect("write");
        let probe = FilesystemProbe {
            root: dir.path().to_path_buf(),
            limits: Limits {
                max_files: 100,
                max_total_bytes: 10,
            },
        };
        let err = probe.capture().expect_err("must refuse, not truncate");
        assert_eq!(err.code(), "snapshot_bounds", "{err}");
    }
}
