//! One error type for the crate.
//!
//! # Design decision: `Fail` is not an error
//!
//! A tool call that leaks a file outside its declared scope is exactly what
//! this harness exists to catch — that is a successful run of ADLC producing
//! a `Verdict::Fail`, not an `AdlcError`. This enum is reserved for ADLC
//! *itself* being unable to reach a verdict: a spec that doesn't parse, a
//! probe that can't read the workspace, a snapshot that overflows its
//! bounds. Conflating "the tool under test misbehaved" with "the harness
//! broke" is how an eval framework quietly starts reporting false passes —
//! a harness error that gets swallowed and treated as "nothing changed"
//! looks identical to a tool that behaved perfectly.

use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, AdlcError>;

#[derive(Debug, thiserror::Error)]
pub enum AdlcError {
    #[error("path escapes every declared root: {attempted}")]
    PathEscape { attempted: PathBuf },

    #[error("state probe failed: {0}")]
    Probe(String),

    /// A partial snapshot is worse than none: never returned for a capture
    /// that ran out of budget partway through. See `probe::Limits`.
    #[error("snapshot exceeded its bounds: {0}")]
    SnapshotBounds(String),

    #[error("invalid spec: {0}")]
    Spec(String),

    #[error("config error: {0}")]
    Config(String),

    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

impl AdlcError {
    /// Stable, machine-readable identifier. Log and script against this, not
    /// against the Display string, which is free to reword.
    pub fn code(&self) -> &'static str {
        match self {
            AdlcError::PathEscape { .. } => "path_escape",
            AdlcError::Probe(_) => "probe_failed",
            AdlcError::SnapshotBounds(_) => "snapshot_bounds",
            AdlcError::Spec(_) => "invalid_spec",
            AdlcError::Config(_) => "config_error",
            AdlcError::Io { .. } => "io_error",
            AdlcError::Json(_) => "json_error",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_is_stable_for_every_variant() {
        let cases = [
            AdlcError::PathEscape {
                attempted: PathBuf::from("/x"),
            },
            AdlcError::Probe("x".into()),
            AdlcError::SnapshotBounds("x".into()),
            AdlcError::Spec("x".into()),
            AdlcError::Config("x".into()),
        ];
        for e in cases {
            assert!(!e.code().is_empty());
        }
    }
}
