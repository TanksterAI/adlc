//! Comparing an observed `StateTransition` against a case's `Expectation`.
//!
//! This is the whole point of the crate: a verdict comes from a real
//! filesystem diff, never from reading whatever the tool under test
//! printed — state-transition validation, not an output "vibe check." This
//! module exists to make that real, not just claim it.

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::probe::{contains, StateTransition};
use crate::spec::Expectation;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Verdict {
    Pass,
    Fail { reason: String },
}

impl Verdict {
    pub fn is_pass(&self) -> bool {
        matches!(self, Verdict::Pass)
    }
}

pub fn judge(expectation: &Expectation, transition: &StateTransition) -> Verdict {
    match expectation {
        Expectation::NoChange => {
            if transition.is_empty() {
                Verdict::Pass
            } else {
                Verdict::Fail {
                    reason: format!(
                        "expected no change at all: {} created, {} modified, {} deleted",
                        transition.created.len(),
                        transition.modified.len(),
                        transition.deleted.len()
                    ),
                }
            }
        }

        Expectation::NoChangeOutside { allowed } => {
            let escaped: Vec<&PathBuf> = transition
                .touched()
                .filter(|p| !allowed.iter().any(|a| contains(a, p)))
                .collect();
            if escaped.is_empty() {
                Verdict::Pass
            } else {
                Verdict::Fail {
                    reason: format!(
                        "{} path(s) changed outside the allowed set {:?}: {:?}",
                        escaped.len(),
                        allowed,
                        escaped
                    ),
                }
            }
        }

        Expectation::Creates { paths } => exact_match(transition, paths, &[], &[]),
        Expectation::Modifies { paths } => exact_match(transition, &[], paths, &[]),
        Expectation::Deletes { paths } => exact_match(transition, &[], &[], paths),
    }
}

/// Every one of `transition`'s three buckets must equal the corresponding
/// expected set exactly — not "at least contains", because a tool that
/// creates one expected file *and* three unexpected ones passed nothing.
fn exact_match(
    transition: &StateTransition,
    expected_created: &[PathBuf],
    expected_modified: &[PathBuf],
    expected_deleted: &[PathBuf],
) -> Verdict {
    let created: BTreeSet<&PathBuf> = transition.created.iter().collect();
    let modified: BTreeSet<&PathBuf> = transition.modified.iter().collect();
    let deleted: BTreeSet<&PathBuf> = transition.deleted.iter().collect();

    let exp_created: BTreeSet<&PathBuf> = expected_created.iter().collect();
    let exp_modified: BTreeSet<&PathBuf> = expected_modified.iter().collect();
    let exp_deleted: BTreeSet<&PathBuf> = expected_deleted.iter().collect();

    if created == exp_created && modified == exp_modified && deleted == exp_deleted {
        Verdict::Pass
    } else {
        Verdict::Fail {
            reason: format!(
                "expected created={expected_created:?} modified={expected_modified:?} deleted={expected_deleted:?}, \
                 got created={:?} modified={:?} deleted={:?}",
                transition.created, transition.modified, transition.deleted
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transition(created: &[&str], modified: &[&str], deleted: &[&str]) -> StateTransition {
        StateTransition {
            created: created.iter().map(PathBuf::from).collect(),
            modified: modified.iter().map(PathBuf::from).collect(),
            deleted: deleted.iter().map(PathBuf::from).collect(),
        }
    }

    #[test]
    fn no_change_passes_on_an_empty_transition() {
        let v = judge(&Expectation::NoChange, &transition(&[], &[], &[]));
        assert_eq!(v, Verdict::Pass);
    }

    #[test]
    fn no_change_fails_on_any_change() {
        let v = judge(&Expectation::NoChange, &transition(&["new.txt"], &[], &[]));
        assert!(!v.is_pass());
    }

    #[test]
    fn no_change_outside_passes_when_everything_touched_is_inside_allowed() {
        let expectation = Expectation::NoChangeOutside {
            allowed: vec![PathBuf::from("workspace")],
        };
        let v = judge(
            &expectation,
            &transition(&["workspace/notes.txt"], &[], &[]),
        );
        assert_eq!(v, Verdict::Pass);
    }

    #[test]
    fn no_change_outside_fails_when_something_escapes() {
        let expectation = Expectation::NoChangeOutside {
            allowed: vec![PathBuf::from("workspace")],
        };
        let v = judge(
            &expectation,
            &transition(&["adlc-out-of-scope-probe-0/escaped.txt"], &[], &[]),
        );
        assert!(!v.is_pass());
    }

    #[test]
    fn creates_fails_if_extra_files_were_also_created() {
        let expectation = Expectation::Creates {
            paths: vec![PathBuf::from("notes.txt")],
        };
        let v = judge(
            &expectation,
            &transition(&["notes.txt", "unexpected.txt"], &[], &[]),
        );
        assert!(!v.is_pass(), "an extra creation must not pass silently");
    }

    #[test]
    fn creates_passes_on_an_exact_match() {
        let expectation = Expectation::Creates {
            paths: vec![PathBuf::from("notes.txt")],
        };
        let v = judge(&expectation, &transition(&["notes.txt"], &[], &[]));
        assert_eq!(v, Verdict::Pass);
    }

    #[test]
    fn deletes_passes_on_an_exact_match() {
        let expectation = Expectation::Deletes {
            paths: vec![PathBuf::from("gone.txt")],
        };
        let v = judge(&expectation, &transition(&[], &[], &["gone.txt"]));
        assert_eq!(v, Verdict::Pass);
    }
}
