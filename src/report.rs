//! An append-only record of what was judged, never of what was said.
//!
//! Same discipline as Grit's `audit.rs`, for the same reason: a report that
//! carries the tool's arguments or the workspace's actual file names starts
//! doubling as a leak of whatever the workspace under test contains. Counts,
//! outcomes, and reasons travel; paths and payloads do not.

use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{AdlcError, Result};
use crate::mutate::{AdversarialCase, Strategy};
use crate::probe::StateTransition;
use crate::verdict::Verdict;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseReport {
    pub case_id: String,
    pub server: String,
    pub tool: String,
    pub strategy: Strategy,
    pub outcome: String, // "pass" | "fail"
    pub reason: Option<String>,
    pub paths_created: usize,
    pub paths_modified: usize,
    pub paths_deleted: usize,
}

impl CaseReport {
    pub fn new(case: &AdversarialCase, transition: &StateTransition, verdict: &Verdict) -> Self {
        let (outcome, reason) = match verdict {
            Verdict::Pass => ("pass".to_string(), None),
            Verdict::Fail { reason } => ("fail".to_string(), Some(reason.clone())),
        };
        Self {
            case_id: case.id.clone(),
            server: case.spec.server.clone(),
            tool: case.spec.tool.clone(),
            strategy: case.strategy,
            outcome,
            reason,
            paths_created: transition.created.len(),
            paths_modified: transition.modified.len(),
            paths_deleted: transition.deleted.len(),
        }
    }
}

/// Appends one JSON object per line. Opened and closed on every write, like
/// Grit's `AuditLog` — a killed process loses at most the record in flight,
/// never a held-open handle's worth of buffered history.
pub struct ReportLog {
    path: std::path::PathBuf,
}

impl ReportLog {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
        }
    }

    pub fn append(&self, report: &CaseReport) -> Result<()> {
        let line = serde_json::to_string(report)?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|source| AdlcError::Io {
                path: self.path.clone(),
                source,
            })?;
        writeln!(file, "{line}").map_err(|source| AdlcError::Io {
            path: self.path.clone(),
            source,
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::{Expectation, ToolCallSpec};
    use std::path::PathBuf;

    fn case() -> AdversarialCase {
        AdversarialCase {
            id: "fs/write_file::path_traversal::0".into(),
            strategy: Strategy::PathTraversal,
            spec: ToolCallSpec {
                server: "fs".into(),
                tool: "write_file".into(),
                arguments: serde_json::json!({"path": "../../etc/passwd", "content": "top secret credentials"}),
                workspace: PathBuf::from("/tmp/workspace"),
                expectation: Expectation::NoChangeOutside {
                    allowed: vec![PathBuf::from("workspace")],
                },
            },
        }
    }

    #[test]
    fn a_report_never_carries_arguments_or_workspace_paths() {
        let c = case();
        let transition = StateTransition::default();
        let verdict = Verdict::Fail {
            reason: "escaped".into(),
        };
        let report = CaseReport::new(&c, &transition, &verdict);
        let json = serde_json::to_string(&report).expect("serialize");

        assert!(!json.contains("top secret credentials"));
        assert!(!json.contains("arguments"));
        assert!(!json.contains("/tmp/workspace"));
    }

    #[test]
    fn append_writes_one_line_per_call_and_survives_reopen() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log = ReportLog::new(dir.path().join("report.jsonl"));
        let c = case();
        let transition = StateTransition::default();

        log.append(&CaseReport::new(&c, &transition, &Verdict::Pass))
            .expect("append 1");
        log.append(&CaseReport::new(&c, &transition, &Verdict::Pass))
            .expect("append 2");

        let contents = std::fs::read_to_string(dir.path().join("report.jsonl")).expect("read");
        assert_eq!(contents.lines().count(), 2);
    }
}
