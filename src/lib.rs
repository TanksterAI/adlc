//! ADLC — adversarial test-case generation for MCP tool calls, judged by a
//! real filesystem state-transition diff instead of the text a tool
//! happened to print.
//!
//! # What this honestly is not
//!
//! **Not a general correctness verifier.** A `Verdict` is a diff comparison
//! against a declared `Expectation`, nothing more. It has no model of
//! whether the *content* written is right — only whether the *set of paths*
//! that changed matches what was declared. A tool that overwrites
//! `notes.txt` with garbage instead of the intended text passes the same
//! `Modifies` check as one that wrote it correctly. Pair this with
//! content-level assertions of your own; it is not a substitute for them.
//!
//! **Filesystem only, in this version.** Jack's own framing of this pillar
//! names "filesystem, DB, API" as the domains state-transition validation
//! should cover. `StateProbe` is the seam for that — `FilesystemProbe` is
//! the one real implementation shipped here. A database probe or an API
//! probe is a real extension, not a stub pretending to be one; nothing here
//! claims coverage it doesn't have.
//!
//! **The mutation strategies are rule-based, not an LLM.** `CaseGenerator`
//! is the seam; `RuleBasedGenerator` is deterministic and needs no model
//! key, so `cargo test` exercises the real thing rather than a fake. A
//! model-backed generator that proposes strategies an eight-strategy fixed
//! list can't anticipate is a legitimate future implementation of the same
//! trait — see `tankster_core.llm`'s `LLMClient` / `DryRunLLM` split in the
//! sibling `tankster-core` repo for the same shape applied to a live model
//! call, which this crate deliberately does not attempt to duplicate.
//!
//! **Does not execute the tool call.** Same posture as Grit's `Gate`:
//! `Harness::prepare` brackets the call from before, `Harness::judge` from
//! after, and the call itself happens in between, run by the host. See
//! `harness` for why.

pub mod error;
pub mod harness;
pub mod mutate;
pub mod probe;
pub mod report;
pub mod spec;
pub mod verdict;

pub use error::{AdlcError, Result};
pub use harness::{Completion, Harness, RunPlan};
pub use mutate::{AdversarialCase, CaseGenerator, RuleBasedGenerator, Strategy};
pub use probe::{
    diff, FileDigest, FilesystemProbe, Limits, StateProbe, StateSnapshot, StateTransition,
};
pub use report::{CaseReport, ReportLog};
pub use spec::{Expectation, ToolCallSpec};
pub use verdict::Verdict;
