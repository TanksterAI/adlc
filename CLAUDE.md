# adlc — app constitution

The canonical constitution is `CLAUDE.md` in **`TanksterAI/tankster-core`**.
Everything there applies. This adds only what is specific to ADLC.

## What this is

The ADLC Execution Harness: adversarial test-case generation for MCP tool
calls, judged by a real filesystem before/after diff rather than by reading
what the tool printed. Pillar 2 of the three-pillar Tankster AI portfolio
narrative — Pillar 1 is `grit`, Pillar 3 is the Agentic Data Lake.

## What this is *not* — read before changing anything

- **Not a general correctness verifier.** A verdict answers "did the right
  *set of paths* change", never "was the content right". Do not let a
  verdict get described anywhere as validating semantic correctness; it
  validates a set-of-paths diff against a declared expectation and nothing
  more.
- **Not multi-domain yet.** State-transition validation should eventually
  cover filesystem, database and API state alike. `StateProbe` is the seam;
  `FilesystemProbe` is the only real implementation. Do not describe
  database or API state-transition validation as shipped — it is a real,
  unbuilt extension of the trait.
- **Not LLM-backed generation.** `RuleBasedGenerator` is deterministic and
  needs no model key. `CaseGenerator` is a trait so a model-backed
  generator can be a second implementation later; it is not a stub for one
  today.
- **Not an executor.** `Harness` never runs the tool call it is bracketing —
  same posture as Grit's `Gate`, and for the same reason: the interesting
  moment belongs to the host. The CLI's `run` subcommand shells out as a
  scripting convenience; the library does not.
- **Not dependent on `grit`, and not a reason to make `grit` depend on this.**
  Both crates independently implement a small bounded, symlink-aware,
  copy-based directory snapshot and a component-wise path-containment
  check, because the alternative — a Cargo git dependency across two private
  repos in the same org — couples each crate's CI to the other's repo
  visibility settings for a few hundred lines of genuinely stable logic.
  Revisit if a third crate needs the same primitive; two independent
  implementations of a well-tested twenty-line function is a fine trade
  against that CI coupling, three might not be.

## Hard rules

- **No `unwrap()`, `expect()` or `panic!` outside `#[cfg(test)]`.** Enforced
  by `cargo clippy --all-targets -- -D warnings` and the shared CI's
  panic-guard step.
- **A containment case probes `workspace`'s parent, never `workspace`
  itself.** A probe rooted at `workspace` cannot observe a path that
  escaped it — the escape is structurally outside what the probe ever
  looks at. `AdversarialCase::probe_root` is the one place this decision is
  made; do not add a second place that decides it differently.
- **Expectations are exact-match, never "at least contains".** A call that
  creates the one expected file and two unexpected ones must fail — two
  unexpected creates is exactly the failure mode this harness exists to
  catch. `verdict::exact_match` has a test for this; if you touch it, that
  test is the specification.
- **A partial snapshot is an error, never a truncation.** Exceeding
  `Limits` returns `AdlcError::SnapshotBounds`, never a `StateSnapshot`
  shorter than what was actually there.
- **The report carries outcomes, not payloads.** No arguments, no file
  contents, no workspace-specific path strings — counts only. `report.rs`
  has a test asserting a serialized report never contains the case's own
  argument values.
- **`Verdict` has no `Error` variant.** A verdict is only ever produced when
  judging *succeeded* at reaching a conclusion. A probe failure is an
  `AdlcError` propagated through `Result`, never folded into `Verdict` —
  conflating "the tool under test misbehaved" with "the harness broke" is
  how a false pass gets reported.
- **The non-containment mutation strategies default to `Expectation::NoChange`.**
  This is a stated, fail-closed assumption, not a discovered truth about
  every tool: ADLC cannot know whether a given tool should refuse a
  malformed argument or coerce it and succeed. Do not "fix" a strategy to
  assume success is fine by default; hand-author the specific case that
  needs a different expectation instead.

## Naming

`infrastructure-conventions.md` in the project knowledge base is the naming
source of truth and needs a row for this service — added in the same
change that adds this file, not as a follow-up.

## CI

Uses the shared `TanksterAI/.github` reusable workflows (`rust-ci.yml`,
`security.yml`) rather than a hand-rolled pipeline — see that repo's
`README.md` for the convention. The repo-specific addition is a guard step
that fails the build if the README or source describes database or API
state-transition validation as already implemented; see "not multi-domain
yet" above for why that claim needs to stay qualified.
