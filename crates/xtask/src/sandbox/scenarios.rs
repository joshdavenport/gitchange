//! The scenario catalogue: each is a script — write files, commit a
//! baseline, edit, then drive core's real ops. Membership records are
//! never hand-assembled.
//!
//! The fake project ("tempo", a task timer) deliberately has no
//! Cargo.toml: cargo run from inside a sandbox must resolve the
//! gitchange workspace manifest, not a nested fake one.

use anyhow::Result;

use super::builder::{Sandbox, assign_file, assign_hunk, refresh, stage_hunk};

pub struct Scenario {
    pub name: &'static str,
    pub build: fn(&mut Sandbox) -> Result<()>,
}

pub fn all() -> Vec<Scenario> {
    vec![
        Scenario {
            name: "fresh",
            build: fresh,
        },
        Scenario {
            name: "unassigned-only",
            build: unassigned_only,
        },
        Scenario {
            name: "sorted",
            build: sorted,
        },
        Scenario {
            name: "mid-staging",
            build: mid_staging,
        },
        Scenario {
            name: "conflicted",
            build: conflicted,
        },
        Scenario {
            name: "binary",
            build: binary,
        },
        Scenario {
            name: "large",
            build: large,
        },
    ]
}

// --- baseline fake project -------------------------------------------

const README: &str = "\
# tempo

A tiny task timer for the terminal.

Start a timer with `tempo <label>`, stop it with ctrl-c, and get a
summary line. Configuration lives in `tempo.toml`.
";

const MAIN_RS: &str = "\
mod config;
mod report;
mod timer;

use std::env;
use std::path::Path;

use config::Config;
use timer::Timer;

fn main() {
    let config = Config::load(Path::new(\"tempo.toml\"));
    let label = env::args()
        .nth(1)
        .unwrap_or_else(|| config.default_label.clone());
    let mut timer = Timer::new(&label);
    timer.start();
    // Stand-in for the interactive loop.
    timer.stop();
    report::print_summary(&timer, &config);
}
";

const TIMER_RS: &str = "\
use std::time::{Duration, Instant};

pub struct Timer {
    started: Option<Instant>,
    elapsed: Duration,
    label: String,
}

impl Timer {
    pub fn new(label: &str) -> Self {
        Self {
            started: None,
            elapsed: Duration::ZERO,
            label: label.to_string(),
        }
    }

    pub fn start(&mut self) {
        if self.started.is_none() {
            self.started = Some(Instant::now());
        }
    }

    pub fn stop(&mut self) {
        if let Some(started) = self.started.take() {
            self.elapsed += started.elapsed();
        }
    }

    pub fn elapsed(&self) -> Duration {
        match self.started {
            Some(started) => self.elapsed + started.elapsed(),
            None => self.elapsed,
        }
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn reset(&mut self) {
        self.started = None;
        self.elapsed = Duration::ZERO;
    }
}
";

const CONFIG_RS: &str = "\
use std::fs;
use std::path::Path;

pub struct Config {
    pub default_label: String,
    pub report_precision: u8,
}

impl Config {
    pub fn load(path: &Path) -> Self {
        let raw = fs::read_to_string(path).unwrap_or_default();
        let mut config = Self::default();
        for line in raw.lines() {
            match line.split_once('=') {
                Some((\"default_label\", value)) => {
                    config.default_label = value.trim().to_string();
                }
                Some((\"report_precision\", value)) => {
                    config.report_precision = value.trim().parse().unwrap_or(2);
                }
                _ => {}
            }
        }
        config
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_label: \"work\".to_string(),
            report_precision: 2,
        }
    }
}
";

const REPORT_RS: &str = "\
use crate::config::Config;
use crate::timer::Timer;

pub fn print_summary(timer: &Timer, config: &Config) {
    let secs = timer.elapsed().as_secs_f64();
    println!(
        \"{}: {:.prec$}s\",
        timer.label(),
        secs,
        prec = config.report_precision as usize
    );
}
";

const TEMPO_TOML: &str = "\
default_label = focus
report_precision = 2
";

/// Three commits of plausible history shared by most scenarios.
fn baseline(sandbox: &mut Sandbox) -> Result<()> {
    sandbox.write("README.md", README)?;
    sandbox.write("src/timer.rs", TIMER_RS)?;
    sandbox.commit_all("Initial project skeleton")?;
    sandbox.write("src/config.rs", CONFIG_RS)?;
    sandbox.write("tempo.toml", TEMPO_TOML)?;
    sandbox.commit_all("Add config loading")?;
    sandbox.write("src/report.rs", REPORT_RS)?;
    sandbox.write("src/main.rs", MAIN_RS)?;
    sandbox.commit_all("Wire up main and report output")?;
    Ok(())
}

// --- shared edits ------------------------------------------------------

/// timer.rs region A (`stop()`, upper half) — the fix-timeout-retry hunk,
/// and the region mid-staging re-edits to go staged-stale.
fn edit_timer_region_a(sandbox: &Sandbox) -> Result<()> {
    sandbox.replace(
        "src/timer.rs",
        "self.elapsed += started.elapsed();",
        "self.elapsed = self.elapsed.saturating_add(started.elapsed());",
    )
}

/// timer.rs region B (`reset()`, lower half) — the debug-logging hunk.
/// Far enough from region A that the two stay separate hunks.
fn edit_timer_region_b(sandbox: &Sandbox) -> Result<()> {
    sandbox.replace(
        "src/timer.rs",
        "    pub fn reset(&mut self) {\n        self.started = None;",
        "    pub fn reset(&mut self) {\n        eprintln!(\"[debug] timer {} reset\", self.label);\n        self.started = None;",
    )
}

fn edit_main_debug_logging(sandbox: &Sandbox) -> Result<()> {
    sandbox.replace(
        "src/main.rs",
        "    let label = env::args()",
        "    eprintln!(\n        \"[debug] config: label={} precision={}\",\n        config.default_label, config.report_precision\n    );\n    let label = env::args()",
    )
}

fn edit_config_timeout(sandbox: &Sandbox) -> Result<()> {
    sandbox.replace(
        "src/config.rs",
        "    pub report_precision: u8,\n}",
        "    pub report_precision: u8,\n    pub timeout_secs: u64,\n}",
    )?;
    sandbox.replace(
        "src/config.rs",
        "            report_precision: 2,\n        }",
        "            report_precision: 2,\n            timeout_secs: 0,\n        }",
    )
}

fn edit_report_cleanup(sandbox: &Sandbox) -> Result<()> {
    sandbox.replace(
        "src/report.rs",
        "pub fn print_summary(timer: &Timer, config: &Config) {\n    let secs = timer.elapsed().as_secs_f64();\n    println!(",
        "pub fn print_summary(timer: &Timer, config: &Config) {\n    println!(\"{}\", summary_line(timer, config));\n}\n\npub fn summary_line(timer: &Timer, config: &Config) -> String {\n    let secs = timer.elapsed().as_secs_f64();\n    format!(",
    )?;
    sandbox.replace(
        "src/report.rs",
        "        prec = config.report_precision as usize\n    );\n}",
        "        prec = config.report_precision as usize\n    )\n}",
    )
}

/// The `sorted` working state: three changelists, hunks distributed,
/// `src/timer.rs` split across two of them, `fix-timeout-retry` active.
fn build_sorted_state(sandbox: &mut Sandbox) -> Result<()> {
    baseline(sandbox)?;
    edit_timer_region_a(sandbox)?;
    edit_timer_region_b(sandbox)?;
    edit_main_debug_logging(sandbox)?;
    edit_config_timeout(sandbox)?;
    edit_report_cleanup(sandbox)?;

    let repo = sandbox.repo()?;
    // First created becomes active; everything auto-captures to it on
    // the first refresh, then moves distribute the rest.
    repo.create_changelist("fix-timeout-retry")
        .map_err(|err| anyhow::anyhow!("create changelist: {err}"))?;
    repo.create_changelist("debug-logging")
        .map_err(|err| anyhow::anyhow!("create changelist: {err}"))?;
    repo.create_changelist("api-cleanup")
        .map_err(|err| anyhow::anyhow!("create changelist: {err}"))?;
    let snapshot = refresh(&repo)?;
    assign_file(&repo, &snapshot, "src/main.rs", "debug-logging")?;
    assign_hunk(&repo, &snapshot, "src/timer.rs", 1, "debug-logging")?;
    assign_file(&repo, &snapshot, "src/report.rs", "api-cleanup")?;
    refresh(&repo)?;
    Ok(())
}

// --- scenarios ---------------------------------------------------------

/// Clean repo with history, no dirty tree, no state file: the
/// first-launch experience.
fn fresh(sandbox: &mut Sandbox) -> Result<()> {
    baseline(sandbox)
}

/// Dirty tree, no changelists: the pre-adoption warning-state view.
/// Deliberately never touches core — no state file exists.
fn unassigned_only(sandbox: &mut Sandbox) -> Result<()> {
    baseline(sandbox)?;
    edit_timer_region_a(sandbox)?;
    edit_main_debug_logging(sandbox)?;
    sandbox.write(
        "src/notes.rs",
        "//! Scratch notes — untracked on purpose.\n",
    )?;
    Ok(())
}

fn sorted(sandbox: &mut Sandbox) -> Result<()> {
    build_sorted_state(sandbox)
}

/// `sorted` plus mixed staged states: ● config.rs, ◐ timer.rs (region B
/// staged, region A staged-stale ◑), ○ main.rs and report.rs.
fn mid_staging(sandbox: &mut Sandbox) -> Result<()> {
    build_sorted_state(sandbox)?;
    let repo = sandbox.repo()?;
    let snapshot = refresh(&repo)?;
    // ● fully staged file.
    repo.stage_file("src/config.rs")
        .map_err(|err| anyhow::anyhow!("stage config.rs: {err}"))?;
    // ◐ partially staged file: stage region B, leave region A unstaged.
    stage_hunk(&repo, &snapshot, "src/timer.rs", 1)?;
    // ◑ staged-stale: stage region A, then edit the same region again.
    stage_hunk(&repo, &snapshot, "src/timer.rs", 0)?;
    sandbox.replace(
        "src/timer.rs",
        "self.elapsed = self.elapsed.saturating_add(started.elapsed());",
        "self.elapsed = self\n                .elapsed\n                .checked_add(started.elapsed())\n                .unwrap_or(Duration::MAX);",
    )?;
    // src/main.rs and src/report.rs stay ○ unstaged.
    refresh(&repo)?;
    Ok(())
}

/// Mid-merge with a conflicted file quarantined and a changelist
/// holding unrelated dirty work.
fn conflicted(sandbox: &mut Sandbox) -> Result<()> {
    baseline(sandbox)?;
    sandbox.git(&["switch", "--quiet", "-c", "feature/units"])?;
    sandbox.replace(
        "src/report.rs",
        "\"{}: {:.prec$}s\",",
        "\"{} took {:.prec$}s\",",
    )?;
    sandbox.commit_all("Spell out units in the summary line")?;
    sandbox.git(&["switch", "--quiet", "main"])?;
    sandbox.replace(
        "src/report.rs",
        "\"{}: {:.prec$}s\",",
        "\"[{}] {:.prec$}s\",",
    )?;
    sandbox.commit_all("Bracket the label in the summary line")?;

    // Unrelated dirty work sorted into a changelist before the merge.
    edit_timer_region_a(sandbox)?;
    let repo = sandbox.repo()?;
    repo.create_changelist("fix-timeout-retry")
        .map_err(|err| anyhow::anyhow!("create changelist: {err}"))?;
    refresh(&repo)?;

    sandbox.git_expect_failure(&["merge", "feature/units"])?;
    refresh(&repo)?;
    Ok(())
}

/// A changed binary as a whole-file hunk in a changelist, next to a
/// text change for contrast.
fn binary(sandbox: &mut Sandbox) -> Result<()> {
    baseline(sandbox)?;
    sandbox.write_bytes("assets/logo.png", &png_bytes(0x11, 12_698))?;
    sandbox.commit_all("Add logo asset")?;
    sandbox.write_bytes("assets/logo.png", &png_bytes(0x2e, 15_462))?;
    sandbox.replace("README.md", "A tiny task timer", "A tiny, fast task timer")?;
    let repo = sandbox.repo()?;
    repo.create_changelist("assets-refresh")
        .map_err(|err| anyhow::anyhow!("create changelist: {err}"))?;
    refresh(&repo)?;
    Ok(())
}

/// Deterministic pseudo-PNG of exactly `len` bytes: a real PNG signature
/// followed by patterned bytes (with NULs so git treats it as binary).
/// Not a valid image — nothing in the TUI decodes it. `len` is a
/// parameter because the two revisions must differ in size for the
/// sized placeholder (ADR 0009) to have anything to show.
fn png_bytes(seed: u8, len: usize) -> Vec<u8> {
    let mut bytes = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
    debug_assert!(len >= bytes.len(), "len must fit the signature");
    for i in 0..(len - bytes.len()) as u32 {
        bytes.push((i as u8).wrapping_mul(seed));
    }
    bytes
}

/// Many changed files for scroll, refresh feel, and layout under
/// volume: 45 generated modules plus the split timer.rs.
fn large(sandbox: &mut Sandbox) -> Result<()> {
    baseline(sandbox)?;
    for i in 0..45 {
        sandbox.write(&gen_path(i), &gen_module(i, 31))?;
    }
    sandbox.commit_all("Add generated modules")?;
    for i in 0..45 {
        sandbox.write(&gen_path(i), &gen_module(i, 37))?;
    }
    edit_timer_region_a(sandbox)?;
    edit_timer_region_b(sandbox)?;

    let repo = sandbox.repo()?;
    repo.create_changelist("wide-refactor")
        .map_err(|err| anyhow::anyhow!("create changelist: {err}"))?;
    repo.create_changelist("misc-fixes")
        .map_err(|err| anyhow::anyhow!("create changelist: {err}"))?;
    let snapshot = refresh(&repo)?;
    for i in 30..45 {
        assign_file(&repo, &snapshot, &gen_path(i), "misc-fixes")?;
    }
    assign_hunk(&repo, &snapshot, "src/timer.rs", 1, "misc-fixes")?;
    refresh(&repo)?;
    Ok(())
}

fn gen_path(i: u32) -> String {
    format!("src/gen/mod_{i:02}.rs")
}

fn gen_module(i: u32, multiplier: u64) -> String {
    format!(
        "//! Generated module {i:02} — synthetic surface for the large scenario.\n\
         \n\
         pub fn compute_{i:02}(input: u64) -> u64 {{\n\
         \x20   let mut acc = input;\n\
         \x20   for step in 0..{i} {{\n\
         \x20       acc = acc.wrapping_mul({multiplier}).wrapping_add(step);\n\
         \x20   }}\n\
         \x20   acc\n\
         }}\n\
         \n\
         pub fn describe_{i:02}() -> String {{\n\
         \x20   format!(\"module {i:02}: {{}}\", compute_{i:02}({i}))\n\
         }}\n"
    )
}
