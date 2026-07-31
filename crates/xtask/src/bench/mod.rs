//! RefreshJob benchmark harness (issue #29, a v0.1 exit criterion):
//! synthetic repos at graduated scales — changed files × hunks per file
//! × membership records, plus the pathological single-huge-file case —
//! timing core's real refresh end-to-end and reporting scaling shape
//! per dimension. All four ADR 0005 mitigations stay measurement-gated
//! behind this harness's numbers; the gate ticket (#36) commits a run's
//! results as the v0.1 exit record.
//!
//! Progress goes to stderr, the markdown report to stdout, so
//! `cargo xtask bench > report.md` records a run directly.

pub mod case;
mod memory;
mod report;

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use clap::Args as ClapArgs;

use case::{CaseResult, CaseSpec, DORMANT_HUNKS_PER_FILE};

#[derive(ClapArgs)]
pub struct Args {
    /// Truncated scales and iterations: a fast validation pass, not a
    /// measurement run
    #[arg(long)]
    pub quick: bool,
    /// Timed refreshes per case (default 5, or 3 under --quick)
    #[arg(long)]
    pub iterations: Option<usize>,
    /// Untimed warmup refreshes per case
    #[arg(long, default_value_t = 2)]
    pub warmup: usize,
    /// Run one dimension only: files | hunks | records | huge-file
    #[arg(long)]
    pub dimension: Option<String>,
    /// Also write report.md and results.csv into this directory
    #[arg(long)]
    pub out: Option<PathBuf>,
    /// Run despite a debug build — for exercising the harness, never
    /// for numbers worth recording
    #[arg(long)]
    pub allow_debug: bool,
}

/// The default matrix: one graduated series per dimension, varied one
/// at a time so each table reads as that dimension's scaling shape,
/// plus contrast cases that split diff cost from matcher cost.
///
/// The fixed values here are restated as prose in `report::blurb` and
/// in docs/agents/benchmarks.md's dimension table — move all three
/// together.
fn matrix(args: &Args) -> Vec<CaseSpec> {
    let iterations = args.iterations.unwrap_or(if args.quick { 3 } else { 5 });
    let base = |name: &str, dimension: &str, scale: u64| CaseSpec {
        name: name.into(),
        dimension: dimension.into(),
        scale,
        files: 0,
        hunks_per_file: 4,
        changelists: 4,
        dormant: 0,
        staged_files: 0,
        huge_lines: 0,
        touch_every_iteration: false,
        warmup: args.warmup,
        iterations,
        contrast_of: None,
    };
    let mut specs = Vec::new();

    let files_scales: &[usize] = if args.quick { &[10, 50] } else { &[10, 50, 250, 1000] };
    for &n in files_scales {
        let mut spec = base(&format!("files-{n}"), "files", n as u64);
        spec.files = n;
        specs.push(spec);
    }
    if !args.quick {
        let mut spec = base("files-1000-unassigned", "files", 1000);
        spec.files = 1000;
        spec.changelists = 0;
        spec.contrast_of = Some("files-1000".into());
        specs.push(spec);
        let mut spec = base("files-250-staged", "files", 250);
        spec.files = 250;
        spec.staged_files = 125;
        spec.contrast_of = Some("files-250".into());
        specs.push(spec);
        // A genuinely-changed refresh: one file re-edited before every
        // timed iteration, so re-match + state persist are inside the
        // timer — the save-triggered event, vs the steady-state rows.
        let mut spec = base("files-250-touched", "files", 250);
        spec.files = 250;
        spec.touch_every_iteration = true;
        spec.contrast_of = Some("files-250".into());
        specs.push(spec);
    }

    let hunk_scales: &[usize] = if args.quick { &[2, 8] } else { &[2, 8, 32, 128] };
    for &h in hunk_scales {
        let mut spec = base(&format!("hunks-{h}"), "hunks", h as u64);
        spec.files = 25;
        spec.hunks_per_file = h;
        specs.push(spec);
    }
    if !args.quick {
        let mut spec = base("hunks-128-unassigned", "hunks", 128);
        spec.files = 25;
        spec.hunks_per_file = 128;
        spec.changelists = 0;
        spec.contrast_of = Some("hunks-128".into());
        specs.push(spec);
    }

    let dormant_scales: &[usize] = if args.quick {
        &[0, 1000]
    } else {
        &[0, 1000, 4000, 16000]
    };
    for &d in dormant_scales {
        debug_assert!(d.is_multiple_of(DORMANT_HUNKS_PER_FILE));
        let mut spec = base(&format!("dormant-{d}"), "records", d as u64);
        spec.files = 50;
        spec.dormant = d;
        specs.push(spec);
    }

    let huge_scales: &[usize] = if args.quick {
        &[32_768]
    } else {
        &[32_768, 131_072, 524_288]
    };
    for &lines in huge_scales {
        let label = lines / 1024;
        let mut spec = base(&format!("huge-{label}k"), "huge-file", lines as u64);
        spec.changelists = 0;
        spec.huge_lines = lines;
        specs.push(spec);
    }
    if !args.quick {
        // Both diffs carrying the huge content at once: staged rewrite
        // plus a second worktree rewrite on top.
        let mut spec = base("huge-128k-staged", "huge-file", 131_072);
        spec.changelists = 0;
        spec.huge_lines = 131_072;
        spec.staged_files = 1;
        spec.contrast_of = Some("huge-128k".into());
        specs.push(spec);
    }

    specs
}

pub fn run(args: &Args) -> Result<()> {
    if cfg!(debug_assertions) && !args.allow_debug {
        bail!(
            "benchmarking a debug build would record misleading numbers — \
             use `cargo xbench` (release), or pass --allow-debug to exercise \
             the harness anyway"
        );
    }
    let specs: Vec<CaseSpec> = matrix(args)
        .into_iter()
        .filter(|spec| {
            args.dimension
                .as_deref()
                .is_none_or(|dim| spec.dimension == dim)
        })
        .collect();
    if specs.is_empty() {
        bail!(
            "no cases match dimension `{}` (files | hunks | records | huge-file)",
            args.dimension.as_deref().unwrap_or_default()
        );
    }

    eprintln!(
        "bench: {} cases, {} timed iterations after {} warmup, one subprocess per case",
        specs.len(),
        specs[0].iterations,
        args.warmup
    );
    let mut results = Vec::with_capacity(specs.len());
    for spec in &specs {
        eprint!("  {:<24}", spec.name);
        let result = run_case_in_subprocess(spec)?;
        eprintln!("median {:>8.2} ms", report::median(&result.times_ms));
        results.push(result);
    }

    let meta = report::Meta {
        date: capture("date", &["-u", "+%Y-%m-%d"]).unwrap_or_else(|| "unknown".into()),
        host: host_description(),
        commit: capture("git", &["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".into()),
        iterations: specs[0].iterations,
        warmup: args.warmup,
    };
    let markdown = report::render_markdown(&results, &meta);
    println!("{markdown}");

    if let Some(dir) = &args.out {
        fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        fs::write(dir.join("report.md"), &markdown)?;
        fs::write(dir.join("results.csv"), report::render_csv(&results))?;
        eprintln!(
            "wrote {} and {}",
            dir.join("report.md").display(),
            dir.join("results.csv").display()
        );
    }
    Ok(())
}

/// The hidden `bench-case` entry: run one case in this process and
/// emit its result as one JSON line on stdout for the parent.
pub fn run_child(spec_json: &str) -> Result<()> {
    let spec: CaseSpec = serde_json::from_str(spec_json).context("parse case spec")?;
    let result = case::run(&spec)?;
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}

/// Each case runs in a fresh subprocess: peak RSS is per-case (the
/// memory probe's whole point) and no allocator warmth leaks between
/// cases.
fn run_case_in_subprocess(spec: &CaseSpec) -> Result<CaseResult> {
    let exe = std::env::current_exe().context("locate xtask binary")?;
    let output = Command::new(exe)
        .arg("bench-case")
        .arg(serde_json::to_string(spec)?)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .output()
        .context("spawn bench-case subprocess")?;
    if !output.status.success() {
        bail!("case `{}` failed ({})", spec.name, output.status);
    }
    let stdout = String::from_utf8(output.stdout).context("bench-case stdout not UTF-8")?;
    let line = stdout
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .with_context(|| format!("case `{}` produced no result line", spec.name))?;
    serde_json::from_str(line).with_context(|| format!("parse case `{}` result", spec.name))
}

fn capture(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program)
        .args(args)
        .current_dir(crate::sandbox::project_root())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn host_description() -> String {
    let cpu = capture("sysctl", &["-n", "machdep.cpu.brand_string"]);
    let uname = capture("uname", &["-sm"]);
    match (cpu, uname) {
        (Some(cpu), Some(uname)) => format!("{cpu} ({uname})"),
        (Some(cpu), None) => cpu,
        (None, Some(uname)) => uname,
        (None, None) => "unknown".into(),
    }
}
