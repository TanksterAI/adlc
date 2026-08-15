//! Turning one seed tool call into a set of adversarial variants.
//!
//! # Design decision: fail-closed is the only default ADLC can defend
//!
//! For a containment probe (`PathTraversal`, `OutOfScopePath`) the correct
//! postcondition is universal: nothing should change outside the workspace,
//! whatever the tool does. For every other strategy here, ADLC cannot know
//! whether a well-designed tool *should* refuse a malformed argument or
//! coerce it and carry on — that is the tool's contract, not something a
//! generic harness can infer from the seed. So the non-containment
//! strategies default to `Expectation::NoChange`: a malformed input should
//! leave no trace. A tool that legitimately succeeds on some of these is not
//! a bug in the harness — it is a prompt to write that case by hand with the
//! expectation it actually deserves, which the two-phase `Harness` API
//! supports directly without going through this generator at all.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::spec::{Expectation, ToolCallSpec};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Strategy {
    /// The seed call, unmutated. A control: if this doesn't Pass, the
    /// mutated cases' verdicts aren't telling you anything about mutation.
    Baseline,
    /// A relative `../` sequence spliced into a path-like argument.
    PathTraversal,
    /// A path-like argument rewritten to an absolute path outside the
    /// workspace outright, no traversal needed.
    OutOfScopePath,
    /// A string argument blown up past anything the seed call implied.
    OversizedArgument,
    /// A string argument replaced with a different JSON type.
    TypeConfusion,
    /// A string argument replaced with an empty string.
    NullOrEmpty,
    /// A string argument wrapped in zero-width, RTL-override, or NUL bytes.
    UnicodeEdgeCase,
    /// A numeric argument replaced with 0, -1, or an integer extreme.
    Boundary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdversarialCase {
    pub id: String,
    pub strategy: Strategy,
    pub spec: ToolCallSpec,
}

impl AdversarialCase {
    /// Where the probe needs to look to judge this case. `NoChangeOutside`
    /// is the one expectation that is about the world beyond `workspace`, so
    /// it is the one case where the probe has to cover more than
    /// `workspace` itself — otherwise an actual escape is structurally
    /// invisible to a probe that only ever looks inside the sandbox it is
    /// checking. See `spec::ToolCallSpec::workspace_parent`.
    pub fn probe_root(&self) -> PathBuf {
        match &self.spec.expectation {
            Expectation::NoChangeOutside { .. } => self.spec.workspace_parent(),
            _ => self.spec.workspace.clone(),
        }
    }
}

pub trait CaseGenerator {
    fn generate(&self, seed: &ToolCallSpec) -> Vec<AdversarialCase>;
}

/// Deterministic, offline, no LLM call. This is the safe default — see
/// `CLAUDE.md` for why a generator that can only be exercised with a live
/// model key is not something this crate ships as its default path. Nothing
/// stops a caller from writing an LLM-backed `CaseGenerator` alongside this
/// one; the trait is the seam.
pub struct RuleBasedGenerator {
    /// Cap per strategy, not overall — an argument object with twenty
    /// string fields should not make `TypeConfusion` alone dominate the
    /// run.
    pub max_per_strategy: usize,
}

impl Default for RuleBasedGenerator {
    fn default() -> Self {
        Self {
            max_per_strategy: 3,
        }
    }
}

fn string_fields(args: &Value) -> Vec<(String, String)> {
    match args {
        Value::Object(map) => map
            .iter()
            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
            .collect(),
        _ => Vec::new(),
    }
}

fn number_fields(args: &Value) -> Vec<String> {
    match args {
        Value::Object(map) => map
            .iter()
            .filter(|(_, v)| v.is_number())
            .map(|(k, _)| k.clone())
            .collect(),
        _ => Vec::new(),
    }
}

/// A field counts as path-like if its name suggests it, or its value
/// already looks like a path. Both are heuristics, not proof — see
/// `INJECTION_PATTERNS` in Grit's `gate.rs` for the same honest caveat
/// applied to a different heuristic.
fn is_path_like(key: &str, value: &str) -> bool {
    let k = key.to_ascii_lowercase();
    k.contains("path")
        || k.contains("file")
        || k.contains("dir")
        || k.contains("root")
        || value.contains('/')
}

fn with_field(args: &Value, key: &str, value: Value) -> Value {
    let mut out = args.clone();
    if let Value::Object(map) = &mut out {
        map.insert(key.to_string(), value);
    }
    out
}

impl RuleBasedGenerator {
    fn case(
        &self,
        seed: &ToolCallSpec,
        strategy: Strategy,
        n: usize,
        arguments: Value,
        expectation: Expectation,
    ) -> AdversarialCase {
        AdversarialCase {
            id: format!("{}::{:?}::{n}", seed.key(), strategy).to_lowercase(),
            strategy,
            spec: ToolCallSpec {
                arguments,
                expectation,
                ..seed.clone()
            },
        }
    }

    fn containment_case(
        &self,
        seed: &ToolCallSpec,
        strategy: Strategy,
        n: usize,
        arguments: Value,
    ) -> AdversarialCase {
        let probe_root = seed.workspace_parent();
        let allowed = seed
            .workspace
            .strip_prefix(&probe_root)
            .unwrap_or(&seed.workspace)
            .to_path_buf();
        self.case(
            seed,
            strategy,
            n,
            arguments,
            Expectation::NoChangeOutside {
                allowed: vec![allowed],
            },
        )
    }

    fn no_change_case(
        &self,
        seed: &ToolCallSpec,
        strategy: Strategy,
        n: usize,
        arguments: Value,
    ) -> AdversarialCase {
        self.case(seed, strategy, n, arguments, Expectation::NoChange)
    }
}

impl CaseGenerator for RuleBasedGenerator {
    fn generate(&self, seed: &ToolCallSpec) -> Vec<AdversarialCase> {
        let mut cases = vec![AdversarialCase {
            id: format!("{}::baseline", seed.key()).to_lowercase(),
            strategy: Strategy::Baseline,
            spec: seed.clone(),
        }];

        let strings = string_fields(&seed.arguments);
        let numbers = number_fields(&seed.arguments);
        let path_like: Vec<_> = strings.iter().filter(|(k, v)| is_path_like(k, v)).collect();

        for (i, (key, _)) in path_like.iter().take(self.max_per_strategy).enumerate() {
            let mutated = with_field(
                &seed.arguments,
                key,
                Value::String(format!("../../../../../../etc/adlc-traversal-probe-{i}")),
            );
            cases.push(self.containment_case(seed, Strategy::PathTraversal, i, mutated));
        }

        for (i, (key, _)) in path_like.iter().take(self.max_per_strategy).enumerate() {
            let target = seed
                .workspace_parent()
                .join(format!("adlc-out-of-scope-probe-{i}"));
            let mutated = with_field(
                &seed.arguments,
                key,
                Value::String(target.to_string_lossy().into_owned()),
            );
            cases.push(self.containment_case(seed, Strategy::OutOfScopePath, i, mutated));
        }

        for (i, (key, _)) in strings.iter().take(self.max_per_strategy).enumerate() {
            let mutated = with_field(&seed.arguments, key, Value::String("A".repeat(2_000_000)));
            cases.push(self.no_change_case(seed, Strategy::OversizedArgument, i, mutated));
        }

        let type_substitutes = [
            Value::Null,
            Value::Bool(true),
            serde_json::json!(12345),
            serde_json::json!(["x", "y"]),
        ];
        let mut n = 0;
        'type_confusion: for (key, _) in &strings {
            for sub in &type_substitutes {
                if n >= self.max_per_strategy {
                    break 'type_confusion;
                }
                let mutated = with_field(&seed.arguments, key, sub.clone());
                cases.push(self.no_change_case(seed, Strategy::TypeConfusion, n, mutated));
                n += 1;
            }
        }

        for (i, (key, _)) in strings.iter().take(self.max_per_strategy).enumerate() {
            let mutated = with_field(&seed.arguments, key, Value::String(String::new()));
            cases.push(self.no_change_case(seed, Strategy::NullOrEmpty, i, mutated));
        }

        let edge_markers = ["\u{200B}", "\u{202E}", "\0"];
        let mut n = 0;
        'unicode: for (key, val) in &strings {
            for marker in &edge_markers {
                if n >= self.max_per_strategy {
                    break 'unicode;
                }
                let mutated = with_field(
                    &seed.arguments,
                    key,
                    Value::String(format!("{marker}{val}{marker}")),
                );
                cases.push(self.no_change_case(seed, Strategy::UnicodeEdgeCase, n, mutated));
                n += 1;
            }
        }

        let boundary_values = [
            serde_json::json!(0),
            serde_json::json!(-1),
            serde_json::json!(i64::MAX),
            serde_json::json!(i64::MIN),
        ];
        let mut n = 0;
        'boundary: for key in &numbers {
            for bv in &boundary_values {
                if n >= self.max_per_strategy {
                    break 'boundary;
                }
                let mutated = with_field(&seed.arguments, key, bv.clone());
                cases.push(self.no_change_case(seed, Strategy::Boundary, n, mutated));
                n += 1;
            }
        }

        cases
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn seed(workspace: PathBuf) -> ToolCallSpec {
        ToolCallSpec {
            server: "fs".into(),
            tool: "write_file".into(),
            arguments: serde_json::json!({"path": "notes.txt", "content": "hi", "retries": 3}),
            workspace,
            expectation: Expectation::Creates {
                paths: vec![PathBuf::from("notes.txt")],
            },
        }
    }

    #[test]
    fn generate_always_includes_exactly_one_baseline_case() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cases = RuleBasedGenerator::default().generate(&seed(dir.path().to_path_buf()));
        let baselines = cases
            .iter()
            .filter(|c| c.strategy == Strategy::Baseline)
            .count();
        assert_eq!(baselines, 1);
    }

    #[test]
    fn path_traversal_targets_the_path_like_field_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cases = RuleBasedGenerator::default().generate(&seed(dir.path().to_path_buf()));
        let traversal = cases
            .iter()
            .find(|c| c.strategy == Strategy::PathTraversal)
            .expect("must generate at least one");
        let mutated_path = traversal
            .spec
            .arguments
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        assert!(mutated_path.contains(".."), "{mutated_path}");
        // The untouched field survives the mutation.
        assert_eq!(
            traversal
                .spec
                .arguments
                .get("content")
                .and_then(|v| v.as_str()),
            Some("hi")
        );
    }

    #[test]
    fn containment_strategies_probe_the_workspace_parent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("mkdir");
        let cases = RuleBasedGenerator::default().generate(&seed(workspace.clone()));
        let traversal = cases
            .iter()
            .find(|c| c.strategy == Strategy::PathTraversal)
            .unwrap();
        assert_eq!(traversal.probe_root(), workspace.parent().unwrap());
    }

    #[test]
    fn non_containment_strategies_probe_the_workspace_itself() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cases = RuleBasedGenerator::default().generate(&seed(dir.path().to_path_buf()));
        let oversized = cases
            .iter()
            .find(|c| c.strategy == Strategy::OversizedArgument)
            .unwrap();
        assert_eq!(oversized.probe_root(), dir.path());
    }

    #[test]
    fn max_per_strategy_bounds_type_confusion_output() {
        let dir = tempfile::tempdir().expect("tempdir");
        let gen = RuleBasedGenerator {
            max_per_strategy: 2,
        };
        let cases = gen.generate(&seed(dir.path().to_path_buf()));
        let confusion = cases
            .iter()
            .filter(|c| c.strategy == Strategy::TypeConfusion)
            .count();
        assert_eq!(confusion, 2);
    }

    #[test]
    fn oversized_argument_case_still_expects_no_change() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cases = RuleBasedGenerator::default().generate(&seed(dir.path().to_path_buf()));
        let oversized = cases
            .iter()
            .find(|c| c.strategy == Strategy::OversizedArgument)
            .unwrap();
        assert!(matches!(oversized.spec.expectation, Expectation::NoChange));
    }

    #[test]
    fn a_seed_with_no_string_arguments_still_yields_only_the_baseline() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut s = seed(dir.path().to_path_buf());
        s.arguments = serde_json::json!({});
        let cases = RuleBasedGenerator::default().generate(&s);
        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].strategy, Strategy::Baseline);
    }
}
