//! The two-phase entry point: `prepare` brackets the call from before,
//! `judge` brackets it from after. Mirrors Grit's `Gate::authorise` /
//! `Gate::complete` on purpose — neither crate executes the tool call it is
//! bracketing, so both end up with the same shape for the same reason: the
//! interesting moment belongs to the host.
//!
//! ```
//! # use adlc::{Harness, RuleBasedGenerator, CaseGenerator, ToolCallSpec, Expectation};
//! # use std::path::PathBuf;
//! # let dir = tempfile::tempdir()?;
//! # let seed = ToolCallSpec {
//! #     server: "fs".into(), tool: "write_file".into(),
//! #     arguments: serde_json::json!({"path": "notes.txt"}),
//! #     workspace: dir.path().to_path_buf(),
//! #     expectation: Expectation::Creates { paths: vec![PathBuf::from("notes.txt")] },
//! # };
//! let case = RuleBasedGenerator::default().generate(&seed).remove(0); // the baseline
//! let harness = Harness::new();
//!
//! let plan = harness.prepare(case)?;
//! // host.run_the_tool_call(&plan.case().spec) — not this crate's business
//! std::fs::write(dir.path().join("notes.txt"), b"hi")?;
//! let completion = harness.judge(plan)?;
//! assert!(completion.verdict.is_pass());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use serde::Serialize;

use crate::error::Result;
use crate::mutate::AdversarialCase;
use crate::probe::{diff, FilesystemProbe, Limits, StateProbe, StateSnapshot, StateTransition};
use crate::report::{CaseReport, ReportLog};
use crate::verdict::{self, Verdict};

pub struct Harness {
    limits: Limits,
    report: Option<ReportLog>,
}

impl Default for Harness {
    fn default() -> Self {
        Self::new()
    }
}

impl Harness {
    pub fn new() -> Self {
        Self {
            limits: Limits::default(),
            report: None,
        }
    }

    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    pub fn with_report(mut self, report: ReportLog) -> Self {
        self.report = Some(report);
        self
    }

    /// Captures the "before" state. Call this immediately before the host
    /// runs the tool call — a snapshot taken any earlier can be stale by
    /// the time the call actually happens.
    pub fn prepare(&self, case: AdversarialCase) -> Result<RunPlan> {
        case.spec.validate()?;
        let probe = FilesystemProbe {
            root: case.probe_root(),
            limits: self.limits,
        };
        let before = probe.capture()?;
        Ok(RunPlan { case, before })
    }

    /// Captures "after", diffs against "before", judges the diff against
    /// the case's expectation, and appends a report record if one was
    /// configured. Always produces a `Completion` when the probe itself
    /// succeeds — a case that fails its expectation is data, not an error.
    pub fn judge(&self, plan: RunPlan) -> Result<Completion> {
        let probe = FilesystemProbe {
            root: plan.case.probe_root(),
            limits: self.limits,
        };
        let after = probe.capture()?;
        let transition = diff(&plan.before, &after);
        let verdict = verdict::judge(&plan.case.spec.expectation, &transition);

        tracing::debug!(
            case_id = %plan.case.id,
            pass = verdict.is_pass(),
            "case judged"
        );

        if let Some(report) = &self.report {
            report.append(&CaseReport::new(&plan.case, &transition, &verdict))?;
        }

        Ok(Completion {
            case_id: plan.case.id.clone(),
            strategy: plan.case.strategy,
            transition,
            verdict,
        })
    }
}

pub struct RunPlan {
    case: AdversarialCase,
    before: StateSnapshot,
}

impl RunPlan {
    /// The call the host should now actually run.
    pub fn case(&self) -> &AdversarialCase {
        &self.case
    }
}

#[derive(Debug, Serialize)]
pub struct Completion {
    pub case_id: String,
    pub strategy: crate::mutate::Strategy,
    pub transition: StateTransition,
    pub verdict: Verdict,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mutate::{CaseGenerator, RuleBasedGenerator, Strategy};
    use crate::spec::{Expectation, ToolCallSpec};
    use std::path::PathBuf;

    fn seed(workspace: PathBuf) -> ToolCallSpec {
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

    /// End-to-end: a well-behaved tool that actually respects containment
    /// must Pass every adversarial case, including the ones designed to
    /// catch an escape — proving the harness doesn't cry wolf on correct
    /// behaviour.
    #[test]
    fn a_well_behaved_tool_passes_every_generated_case() {
        let dir = tempfile::tempdir().expect("tempdir");
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("mkdir");

        let cases = RuleBasedGenerator::default().generate(&seed(workspace.clone()));
        let harness = Harness::new();

        for case in cases {
            let plan = harness.prepare(case).expect("prepare");
            // The well-behaved host: whatever the mutated argument says,
            // this stand-in tool always writes exactly notes.txt inside the
            // workspace and nothing else — the correct, contained behaviour
            // regardless of what a malicious argument asked for.
            std::fs::write(workspace.join("notes.txt"), b"hi").expect("write");
            let completion = harness.judge(plan).expect("judge");
            assert!(
                completion.verdict.is_pass(),
                "case {} ({:?}) should pass for a contained tool: {:?}",
                completion.case_id,
                completion.strategy,
                completion.verdict
            );
        }
    }

    /// The case this whole crate exists for: a tool that actually follows
    /// the malicious path out of the workspace must Fail, and the escape
    /// must be visible in the transition even though the probe never looked
    /// inside `workspace` for this case — it looked one level up.
    #[test]
    fn a_tool_that_actually_escapes_fails_the_path_traversal_case() {
        let dir = tempfile::tempdir().expect("tempdir");
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("mkdir");

        let cases = RuleBasedGenerator::default().generate(&seed(workspace.clone()));
        let traversal = cases
            .into_iter()
            .find(|c| c.strategy == Strategy::PathTraversal)
            .expect("must generate one");

        let harness = Harness::new();
        let plan = harness.prepare(traversal).expect("prepare");

        // The badly-behaved host: actually follows the mutated path and
        // writes outside the workspace, exactly what containment must catch.
        std::fs::write(dir.path().join("escaped-by-mistake.txt"), b"oops").expect("write");

        let completion = harness.judge(plan).expect("judge");
        assert!(
            !completion.verdict.is_pass(),
            "an actual escape must fail the case"
        );
    }

    #[test]
    fn a_report_log_receives_one_line_per_judged_case() {
        let dir = tempfile::tempdir().expect("tempdir");
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("mkdir");
        let report_path = dir.path().join("report.jsonl");

        let harness = Harness::new().with_report(ReportLog::new(&report_path));
        let baseline = RuleBasedGenerator::default()
            .generate(&seed(workspace.clone()))
            .into_iter()
            .find(|c| c.strategy == Strategy::Baseline)
            .expect("baseline");

        let plan = harness.prepare(baseline).expect("prepare");
        std::fs::write(workspace.join("notes.txt"), b"hi").expect("write");
        harness.judge(plan).expect("judge");

        let contents = std::fs::read_to_string(&report_path).expect("read report");
        assert_eq!(contents.lines().count(), 1);
    }
}
