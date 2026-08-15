# ADLC Execution Harness

Adversarial test-case generation for MCP tool calls, judged by a real
filesystem diff instead of the text a tool happened to print. It does four
things:

1. **Generates adversarial variants of a seed call.** `RuleBasedGenerator`
   takes one `ToolCallSpec` and mutates its arguments — path traversal,
   out-of-scope paths, oversized values, wrong types, empty/null values,
   Unicode edge cases, numeric boundaries — deterministically, offline, no
   model call required.
2. **Snapshots the filesystem before and after.** `FilesystemProbe` captures
   a SHA-256 digest of every file under a root, bounded the same way Grit's
   snapshot engine is bounded: exceeding the bound is an error, never a
   partial capture.
3. **Judges the diff against a declared expectation**, not the tool's
   output. `Expectation::NoChangeOutside` is the one every adversarial case
   built by the generator's containment strategies actually needs — see
   below for why it probes one directory higher than the workspace itself.
4. **Reports outcomes, never payloads.** One JSON line per judged case:
   strategy, verdict, path counts. No arguments, no file contents, no
   workspace-specific paths.

## What this honestly is not

**Not a general correctness verifier.** A verdict is "did the right *set of
paths* change", not "was the *content* right". A tool that overwrites a file
with garbage instead of the intended text passes the same `Modifies` check as
one that wrote it correctly. Bring your own content-level assertions; this
replaces the "did anything unexpected happen to the filesystem" check, not
every check.

**Filesystem only, in this version.** State-transition validation should
eventually cover database and API state as well as the filesystem —
`StateProbe` is the trait that seam is built on, `FilesystemProbe` is the one
implementation that ships. A database or API probe is real, unbuilt future
work, not a stub standing in for one.

**The mutation strategies are a fixed rule-based list, not an LLM.** Eight
strategies, deterministic, no model key needed — which is also why `cargo
test` can exercise the real generator instead of a fake one. `CaseGenerator`
is a trait for a reason: a model-backed generator that proposes strategies
this list can't anticipate is a legitimate second implementation of it.

**Does not execute the tool call.** `Harness::prepare` brackets a case from
before, `Harness::judge` from after; the call itself happens in between, run
by whatever host is driving the MCP tool. The CLI's `run` subcommand shells
out as a scripting convenience — the library never does.

## Why containment cases probe one directory higher

A `FilesystemProbe` can only see what is under its own root. If a
`PathTraversal` case only watched `workspace`, an actual escape would be
invisible to it by construction — the probe never looks anywhere the escape
could land. So the two containment strategies (`PathTraversal`,
`OutOfScopePath`) probe `workspace`'s **parent** instead, and the expectation
becomes "nothing changes outside `workspace`" over that wider tree — an
escape shows up as an unexplained create or modify one level up. Every other
strategy probes `workspace` itself.

This means the harness needs `workspace` to live inside a directory it
doesn't otherwise share with anything large or unrelated — a fresh temp
directory containing just the workspace is the right shape. Pointing
`workspace` straight at, say, a home directory makes the containment probe's
bound (`Limits`, 5,000 files / 64 MB by default) trip on unrelated files
that have nothing to do with the case.

## Quick start

```bash
cargo build --release

cat > spec.json <<'JSON'
{
  "server": "fs", "tool": "write_file",
  "arguments": {"path": "notes.txt", "content": "hello"},
  "workspace": "/tmp/adlc-demo/workspace",
  "expectation": {"kind": "creates", "paths": ["notes.txt"]}
}
JSON

mkdir -p /tmp/adlc-demo/workspace
adlc generate spec.json > cases.json
python3 -c "import json; print(len(json.load(open('cases.json'))), 'cases')"

# Run one case end-to-end: prepare, exec, judge, in one step.
python3 -c "import json; print(json.dumps(json.load(open('cases.json'))[0]))" > case0.json
adlc run case0.json --exec "cp /dev/null /tmp/adlc-demo/workspace/notes.txt" --report report.jsonl
```

## Using it as a library

```rust
let seed = ToolCallSpec { /* ... */ };
let cases = RuleBasedGenerator::default().generate(&seed);
let harness = Harness::new().with_report(ReportLog::new("report.jsonl"));

for case in cases {
    let plan = harness.prepare(case)?;
    host.run_the_tool_call(&plan.case().spec);   // not this crate's business
    let completion = harness.judge(plan)?;
    if !completion.verdict.is_pass() {
        eprintln!("{}: {:?}", completion.case_id, completion.verdict);
    }
}
```

## Expectation shapes

| Kind | Meaning | Probes |
|---|---|---|
| `no_change_outside` | Nothing changes except inside `allowed`. | `workspace`'s parent |
| `creates` | Exactly these paths are newly created, nothing else changes. | `workspace` |
| `modifies` | Exactly these paths change content, nothing else. | `workspace` |
| `deletes` | Exactly these paths are removed, nothing else changes. | `workspace` |
| `no_change` | The tree comes back byte-identical. | `workspace` |

All comparisons are exact-match, not "at least contains" — a call that
creates the one expected file *and* two unexpected ones fails, because two
unexpected creates is exactly the kind of thing this harness exists to
catch.

## Strategies

`Baseline` (the seed call, unmutated — a control), `PathTraversal`,
`OutOfScopePath`, `OversizedArgument`, `TypeConfusion`, `NullOrEmpty`,
`UnicodeEdgeCase`, `Boundary`. The two containment strategies get
`NoChangeOutside`; every other mutation defaults to `NoChange` — a
fail-closed posture, since ADLC cannot know whether a given tool *should*
coerce a malformed argument and succeed. A tool that legitimately does isn't
wrong; write that case by hand with the expectation it actually deserves
instead of going through the generator for it.

## Development

```bash
cargo test                                  # unit + integration + doctest
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```
