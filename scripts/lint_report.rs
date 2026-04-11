#!/usr/bin/env rust-script
//! lint_report — Run cargo fmt, clippy, test, and doc on a Rust workspace
//!               and write a Markdown report.
//!
//! Usage: lint_report [workspace_dir] [output_file]
//!   workspace_dir  root of the Rust workspace  (default: current directory)
//!   output_file    report path                  (default: lint_report.md)

use std::env;
use std::fmt;
use std::fs;
use std::io;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

// ── types ─────────────────────────────────────────────────────────────────────

#[derive(Debug)]
struct ToolResult {
    /// Combined stdout + stderr output from the tool.
    output: String,
    /// True if the process exited with code 0.
    exit_ok: bool,
    /// Number of unique `error[...]` diagnostics in the output.
    error_count: usize,
    /// Number of unique `warning[...]` diagnostics in the output.
    warning_count: usize,
    /// Wall-clock duration in seconds.
    elapsed_secs: f64,
}

#[derive(Debug)]
enum AppError {
    MissingTool(String),
    MissingWorkspaceDir(String),
    Io(io::Error),
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::MissingTool(name) => write!(
                f,
                "ERROR: '{}' not found on PATH. Make sure Rust/Cargo is installed.",
                name
            ),
            AppError::MissingWorkspaceDir(dir) => {
                write!(f, "ERROR: Workspace directory '{}' not found.", dir)
            }
            AppError::Io(e) => write!(f, "IO error: {}", e),
        }
    }
}

impl From<io::Error> for AppError {
    fn from(e: io::Error) -> Self {
        AppError::Io(e)
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn check_tool(name: &str) -> Result<(), AppError> {
    let status = Command::new(name)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if status.is_err() || !status.unwrap().success() {
        return Err(AppError::MissingTool(name.to_string()));
    }
    Ok(())
}

/// Run a cargo subcommand in `workspace_dir`, capturing combined stdout + stderr.
fn run_cargo(args: &[&str], workspace_dir: &str, extra_env: &[(&str, &str)]) -> ToolResult {
    let label = args.join(" ");
    eprintln!("  → cargo {} …", label);

    let start = Instant::now();
    let mut cmd = Command::new("cargo");
    cmd.args(args)
        .current_dir(workspace_dir)
        // Force coloured output off so the report is clean text.
        .env("CARGO_TERM_COLOR", "never")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    for (k, v) in extra_env {
        cmd.env(k, v);
    }

    match cmd.output() {
        Err(e) => ToolResult {
            output: format!("Failed to spawn cargo {}: {}", label, e),
            exit_ok: false,
            error_count: 0,
            warning_count: 0,
            elapsed_secs: start.elapsed().as_secs_f64(),
        },
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
            // Cargo writes diagnostics to stderr; merge both for the report.
            let combined = if stdout.is_empty() {
                stderr
            } else if stderr.is_empty() {
                stdout
            } else {
                format!("{}\n{}", stdout.trim_end(), stderr)
            };
            let exit_ok = out.status.success();
            let (error_count, warning_count) = count_diagnostics(&combined);
            ToolResult {
                output: combined,
                exit_ok,
                error_count,
                warning_count,
                elapsed_secs: start.elapsed().as_secs_f64(),
            }
        }
    }
}

/// Count per-diagnostic `error[` and `warning[` lines in cargo's output,
/// excluding cargo's own summary lines ("error: could not compile", etc.)
/// which would otherwise inflate the counts.
fn count_diagnostics(output: &str) -> (usize, usize) {
    // Summary lines emitted by cargo itself — not individual diagnostics.
    const CARGO_SUMMARY_PREFIXES: &[&str] = &[
        "error: could not compile",
        "error: aborting",
        "warning: build failed",
    ];

    let is_summary = |line: &str| CARGO_SUMMARY_PREFIXES.iter().any(|p| line.starts_with(p));

    let mut errors = 0usize;
    let mut warnings = 0usize;
    for line in output.lines() {
        let trimmed = line.trim_start();
        if is_summary(trimmed) {
            continue;
        }
        if trimmed.starts_with("error[") || trimmed.starts_with("error: ") {
            errors += 1;
        } else if trimmed.starts_with("warning[") || trimmed.starts_with("warning: ") {
            warnings += 1;
        }
    }
    (errors, warnings)
}

fn badge(result: &ToolResult) -> String {
    if result.exit_ok {
        "✅ Pass".to_string()
    } else if result.error_count > 0 {
        format!("❌ {} error(s)", result.error_count)
    } else {
        "❌ Fail".to_string()
    }
}

fn output_block(result: &ToolResult) -> &str {
    let trimmed = result.output.trim();
    if trimmed.is_empty() {
        "No output."
    } else {
        trimmed
    }
}

fn overall_status(results: &[&ToolResult]) -> &'static str {
    if results.iter().all(|r| r.exit_ok) {
        "✅ All checks passed"
    } else {
        "❌ One or more checks failed"
    }
}

/// ISO-8601 timestamp from the standard library, no subprocess needed.
fn timestamp_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Minimal formatting without pulling in chrono.
    let (s, min) = (secs % 60, (secs / 60) % 60);
    let (h, days) = ((secs / 3600) % 24, secs / 86400);
    // Days since epoch → approximate calendar date (good enough for a report header).
    let (y, mo, d) = days_to_ymd(days);
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", y, mo, d, h, min, s)
}

/// Convert days-since-Unix-epoch to (year, month, day). Handles leap years.
fn days_to_ymd(mut days: u64) -> (u64, u64, u64) {
    let mut year = 1970u64;
    loop {
        let leap = is_leap(year);
        let days_in_year = if leap { 366 } else { 365 };
        if days < days_in_year {
            break;
        }
        days -= days_in_year;
        year += 1;
    }
    let leap = is_leap(year);
    let month_days: &[u64] = if leap {
        &[31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        &[31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut month = 1u64;
    for &md in month_days {
        if days < md {
            break;
        }
        days -= md;
        month += 1;
    }
    (year, month, days + 1)
}

fn is_leap(y: u64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

// ── report ────────────────────────────────────────────────────────────────────

fn write_report(
    path: &str,
    workspace_dir: &str,
    timestamp: &str,
    fmt: &ToolResult,
    clippy: &ToolResult,
    test: &ToolResult,
    doc: &ToolResult,
) -> Result<(), AppError> {
    // Ensure the output directory exists.
    if let Some(parent) = Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }

    let all = [fmt, clippy, test, doc];

    let report = format!(
        r#"# Rust Lint Report

| | |
|---|---|
| **Generated** | {timestamp} |
| **Workspace** | `{workspace_dir}` |
| **Overall** | {status} |

---

## Summary

| Check | Status | Errors | Warnings | Time |
|-------|--------|--------|----------|------|
| `cargo fmt --check` | {fmt_badge} | {fmt_e} | {fmt_w} | {fmt_t:.2}s |
| `cargo clippy` | {clippy_badge} | {clippy_e} | {clippy_w} | {clippy_t:.2}s |
| `cargo test` | {test_badge} | {test_e} | {test_w} | {test_t:.2}s |
| `cargo doc` | {doc_badge} | {doc_e} | {doc_w} | {doc_t:.2}s |

---

## cargo fmt

> Checks that all source files match `rustfmt` formatting rules.
> Fix with: `cargo fmt --all`

```
{fmt_output}
```

---

## cargo clippy

> Lints for correctness, style, and performance issues.
> Fix with: `cargo clippy --fix`

```
{clippy_output}
```

---

## cargo test

> Runs the full test suite including doc-tests.

```
{test_output}
```

---

## cargo doc

> Verifies documentation compiles without warnings.

```
{doc_output}
```

---

*Report generated by `scripts/lint_report`*
"#,
        timestamp = timestamp,
        workspace_dir = workspace_dir,
        status = overall_status(&all),
        fmt_badge = badge(fmt),
        fmt_e = fmt.error_count,
        fmt_w = fmt.warning_count,
        fmt_t = fmt.elapsed_secs,
        clippy_badge = badge(clippy),
        clippy_e = clippy.error_count,
        clippy_w = clippy.warning_count,
        clippy_t = clippy.elapsed_secs,
        test_badge = badge(test),
        test_e = test.error_count,
        test_w = test.warning_count,
        test_t = test.elapsed_secs,
        doc_badge = badge(doc),
        doc_e = doc.error_count,
        doc_w = doc.warning_count,
        doc_t = doc.elapsed_secs,
        fmt_output = output_block(fmt),
        clippy_output = output_block(clippy),
        test_output = output_block(test),
        doc_output = output_block(doc),
    );

    fs::write(path, report)?;
    Ok(())
}

// ── main ──────────────────────────────────────────────────────────────────────

fn run() -> Result<bool, AppError> {
    let args: Vec<String> = env::args().collect();
    let workspace_dir = args.get(1).map(String::as_str).unwrap_or(".");
    let output = args.get(2).map(String::as_str).unwrap_or("lint_report.md");

    if !Path::new(workspace_dir).is_dir() {
        return Err(AppError::MissingWorkspaceDir(workspace_dir.to_string()));
    }
    check_tool("cargo")?;
    check_tool("rustfmt")?;

    let timestamp = timestamp_now();

    eprintln!("Running Rust checks on '{}' …", workspace_dir);

    // 1. Format check — fails if any file needs reformatting.
    let fmt = run_cargo(&["fmt", "--all", "--", "--check"], workspace_dir, &[]);

    // 2. Clippy — treat warnings as errors to match CI.
    //    Pass -D warnings only via the `--` flag so it applies to clippy lints
    //    only. Setting RUSTFLAGS=-D warnings would also affect all dependencies
    //    and proc-macros, causing spurious failures in third-party crates.
    let clippy = run_cargo(
        &["clippy", "--all-targets", "--", "-D", "warnings"],
        workspace_dir,
        &[],
    );

    // 3. Tests — full suite with all features.
    let test = run_cargo(&["test", "--all-features"], workspace_dir, &[]);

    // 4. Docs — fail on any rustdoc warning.
    let doc = run_cargo(
        &["doc", "--no-deps", "--all-features"],
        workspace_dir,
        &[("RUSTDOCFLAGS", "-D warnings")],
    );

    write_report(
        output,
        workspace_dir,
        &timestamp,
        &fmt,
        &clippy,
        &test,
        &doc,
    )?;
    eprintln!("Report written to: {}", output);

    Ok(fmt.exit_ok && clippy.exit_ok && test.exit_ok && doc.exit_ok)
}

fn main() {
    match run() {
        Ok(passed) => {
            if !passed {
                std::process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    }
}
