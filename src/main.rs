//! CLI. The library's `Harness` never executes a tool call — see
//! `harness` for why — but a command-line tool with nothing to shell out to
//! isn't useful on its own, so `run` is the one place this crate breaks
//! that rule, deliberately and only here, for scripting convenience.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use adlc::{
    diff, AdlcError, AdversarialCase, CaseGenerator, FilesystemProbe, Harness, ReportLog,
    RuleBasedGenerator, StateProbe, StateSnapshot, ToolCallSpec, Verdict,
};

fn main() -> ExitCode {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let args: Vec<String> = env::args().collect();
    match run(&args[1..]) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("adlc: {e}");
            ExitCode::from(2)
        }
    }
}

fn run(args: &[String]) -> adlc::Result<ExitCode> {
    match args.first().map(String::as_str) {
        Some("generate") => cmd_generate(&args[1..]),
        Some("snapshot") => cmd_snapshot(&args[1..]),
        Some("diff") => cmd_diff(&args[1..]),
        Some("run") => cmd_run(&args[1..]),
        _ => {
            print_usage();
            Ok(ExitCode::from(2))
        }
    }
}

fn print_usage() {
    eprintln!(
        "usage:\n  \
         adlc generate <spec.json> [--max-per-strategy N]\n  \
         adlc snapshot <dir>\n  \
         adlc diff <before.json> <after.json>\n  \
         adlc run <case.json> --exec \"<shell command>\" [--report report.jsonl]\n\n\
         generate turns one seed ToolCallSpec into adversarial AdversarialCase\n\
         values (see the crate docs for the JSON shape). run is the only\n\
         subcommand that executes anything; the library's Harness never does."
    );
}

fn read_json<T: serde::de::DeserializeOwned>(path: &str) -> adlc::Result<T> {
    let text = fs::read_to_string(path).map_err(|source| AdlcError::Io {
        path: PathBuf::from(path),
        source,
    })?;
    Ok(serde_json::from_str(&text)?)
}

fn cmd_generate(args: &[String]) -> adlc::Result<ExitCode> {
    let spec_path = args
        .first()
        .ok_or_else(|| AdlcError::Config("generate needs a spec.json path".to_string()))?;

    let mut max_per_strategy = 3usize;
    let mut i = 1;
    while i < args.len() {
        if args[i] == "--max-per-strategy" {
            if let Some(v) = args.get(i + 1).and_then(|v| v.parse().ok()) {
                max_per_strategy = v;
            }
            i += 2;
        } else {
            i += 1;
        }
    }

    let seed: ToolCallSpec = read_json(spec_path)?;
    seed.validate()?;

    let cases = RuleBasedGenerator { max_per_strategy }.generate(&seed);
    println!("{}", serde_json::to_string_pretty(&cases)?);
    eprintln!("{} case(s) generated for {}", cases.len(), seed.key());
    Ok(ExitCode::from(0))
}

fn cmd_snapshot(args: &[String]) -> adlc::Result<ExitCode> {
    let dir = args
        .first()
        .ok_or_else(|| AdlcError::Config("snapshot needs a directory".to_string()))?;
    let probe = FilesystemProbe::new(PathBuf::from(dir));
    let snapshot = probe.capture()?;
    println!("{}", serde_json::to_string_pretty(&snapshot)?);
    Ok(ExitCode::from(0))
}

fn cmd_diff(args: &[String]) -> adlc::Result<ExitCode> {
    let usage = "diff needs <before.json> <after.json>";
    let before_path = args
        .first()
        .ok_or_else(|| AdlcError::Config(usage.to_string()))?;
    let after_path = args
        .get(1)
        .ok_or_else(|| AdlcError::Config(usage.to_string()))?;

    let before: StateSnapshot = read_json(before_path)?;
    let after: StateSnapshot = read_json(after_path)?;
    println!("{}", serde_json::to_string_pretty(&diff(&before, &after))?);
    Ok(ExitCode::from(0))
}

fn cmd_run(args: &[String]) -> adlc::Result<ExitCode> {
    let case_path = args.first().ok_or_else(|| {
        AdlcError::Config("run needs a case.json path and --exec \"<command>\"".to_string())
    })?;

    let mut exec: Option<String> = None;
    let mut report_path: Option<String> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--exec" => {
                exec = args.get(i + 1).cloned();
                i += 2;
            }
            "--report" => {
                report_path = args.get(i + 1).cloned();
                i += 2;
            }
            _ => i += 1,
        }
    }
    let exec =
        exec.ok_or_else(|| AdlcError::Config("run needs --exec \"<command>\"".to_string()))?;

    let case: AdversarialCase = read_json(case_path)?;

    let mut harness = Harness::new();
    if let Some(rp) = &report_path {
        harness = harness.with_report(ReportLog::new(rp));
    }

    let plan = harness.prepare(case)?;

    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(&exec)
        .status()
        .map_err(|source| AdlcError::Io {
            path: PathBuf::from("sh"),
            source,
        })?;
    if !status.success() {
        eprintln!("adlc: exec command exited with {status}");
    }

    let completion = harness.judge(plan)?;
    println!("{}", serde_json::to_string_pretty(&completion)?);

    match completion.verdict {
        Verdict::Pass => Ok(ExitCode::from(0)),
        Verdict::Fail { .. } => Ok(ExitCode::from(1)),
    }
}
