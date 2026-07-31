//! One benchmark case: generate a synthetic repo at the spec's scale,
//! seed membership through core's real ops (records are never
//! hand-assembled — same rule as the sandboxes), verify the built shape
//! matches the spec, then time `Repo::refresh()` — the whole RefreshJob
//! minus the watcher (status + both diffs + matcher + persist).

use std::fs::{self, File};
use std::io::{BufWriter, Write as _};
use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result, bail, ensure};
use gitchange_core::Snapshot;
use serde::{Deserialize, Serialize};

use super::memory;
use crate::sandbox::builder::{Sandbox, refresh};

/// Lines per generated block; one block yields one hunk (the edit sits
/// mid-block, and blocks are far enough apart that the default 3-line
/// diff context can't merge neighbouring hunks).
const BLOCK_LINES: usize = 10;

/// Hunks per dormant-seed file: written, captured, then reverted so
/// their records go dormant in bulk.
pub const DORMANT_HUNKS_PER_FILE: usize = 40;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseSpec {
    pub name: String,
    /// files | hunks | records | huge-file | binaries — the report
    /// groups by this.
    pub dimension: String,
    /// The varied dimension's value: the x-axis of the scaling fit.
    pub scale: u64,
    /// Changed files in the working tree.
    pub files: usize,
    pub hunks_per_file: usize,
    /// 0 = no changelists at all: hunks stay unassigned and the matcher
    /// has no records to work with (the diff-cost-only contrast).
    pub changelists: usize,
    /// Dormant records to accumulate; must be a multiple of
    /// [`DORMANT_HUNKS_PER_FILE`].
    pub dormant: usize,
    /// Changed files to fully stage (index diff material).
    pub staged_files: usize,
    /// When > 0: the pathological case — one file of this many lines,
    /// every line rewritten, replacing the graduated layout.
    pub huge_lines: usize,
    /// When > 0: changed binary files, replacing the graduated layout —
    /// every refresh re-hashes each one's worktree bytes (ADR 0009's
    /// stated cost, deferred to this harness).
    pub binary_files: usize,
    /// Size of each binary file in KiB.
    pub binary_kib: usize,
    /// Re-edit one file's hunks before every timed refresh, so each
    /// iteration measures a genuinely-changed refresh: tier-2 re-match
    /// plus the state persist, not the steady-state no-rewrite path.
    pub touch_every_iteration: bool,
    pub warmup: usize,
    pub iterations: usize,
    /// Names the base case this one contrasts against (reported as a
    /// ratio, excluded from the dimension's shape fit).
    pub contrast_of: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseResult {
    pub spec: CaseSpec,
    pub times_ms: Vec<f64>,
    pub hunks_total: usize,
    pub live_records: usize,
    pub dormant_records: usize,
    /// Baseline + edited file bytes for huge-file and binaries cases;
    /// 0 otherwise.
    pub content_bytes: u64,
    /// Peak RSS after generation but before any refresh.
    pub peak_rss_before: Option<u64>,
    /// Peak RSS after all refreshes.
    pub peak_rss_after: Option<u64>,
}

/// Build the repo, verify its shape, and time the refreshes.
pub fn run(spec: &CaseSpec) -> Result<CaseResult> {
    ensure!(
        !spec.touch_every_iteration || (spec.files > 0 && spec.huge_lines == 0),
        "touch_every_iteration re-edits the first graduated module"
    );
    let dir = tempfile::tempdir().context("create temp repo dir")?;
    let mut sandbox = Sandbox::init(dir.path())?;
    let content_bytes = if spec.huge_lines > 0 {
        generate_huge(&mut sandbox, dir.path(), spec)?
    } else if spec.binary_files > 0 {
        generate_binaries(&mut sandbox, dir.path(), spec)?
    } else {
        generate(&mut sandbox, spec)?;
        0
    };

    let repo = sandbox.repo()?;
    let peak_rss_before = memory::peak_rss_bytes();

    // The verification refresh doubles as the first warmup: it also
    // settles any pending state write so timed iterations measure the
    // steady state (records unchanged → no rewrite, ADR 0005).
    let snapshot = refresh(&repo)?;
    let (hunks_total, live_records, dormant_records) = verify(spec, &snapshot, dir.path())?;
    for _ in 1..spec.warmup.max(1) {
        refresh(&repo)?;
    }

    let mut times_ms = Vec::with_capacity(spec.iterations);
    for i in 0..spec.iterations {
        if spec.touch_every_iteration {
            // Alternate between two edited forms so every refresh sees
            // changed anchors on this file's hunks — the write itself
            // stays outside the timer.
            let variant = if i % 2 == 0 {
                BlockVariant::Touched
            } else {
                BlockVariant::Edited
            };
            sandbox.write(&module_path(0), &module(0, spec.hunks_per_file, variant))?;
        }
        let start = Instant::now();
        refresh(&repo)?;
        times_ms.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    let peak_rss_after = memory::peak_rss_bytes();

    Ok(CaseResult {
        spec: spec.clone(),
        times_ms,
        hunks_total,
        live_records,
        dormant_records,
        content_bytes,
        peak_rss_before,
        peak_rss_after,
    })
}

// --- graduated layout --------------------------------------------------

fn module_path(index: usize) -> String {
    format!("src/mod_{index:04}.rs")
}

fn dormant_path(index: usize) -> String {
    format!("src/dormant_{index:04}.rs")
}

/// The three states a block's mid-line cycles through: baseline,
/// edited (the benchmark diff), and touched (a second distinct edit,
/// for per-iteration re-edits).
#[derive(Clone, Copy, PartialEq)]
enum BlockVariant {
    Base,
    Edited,
    Touched,
}

/// One pseudo-Rust block; the variant swaps a mid-block line so each
/// block contributes exactly one hunk with a distinct anchor.
fn block(file: usize, index: usize, variant: BlockVariant) -> String {
    let body = match variant {
        BlockVariant::Base => {
            format!("    total = total.wrapping_mul(31).wrapping_add({index});")
        }
        BlockVariant::Edited => {
            format!("    total = total.wrapping_mul(37).wrapping_add({index} + 1); // edited")
        }
        BlockVariant::Touched => {
            format!("    total = total.wrapping_mul(41).wrapping_add({index} + 2); // touched")
        }
    };
    format!(
        "fn item_{file}_{index}() -> u64 {{\n\
         \x20   let seed = {file} * 100_000 + {index};\n\
         \x20   let mut total = seed as u64;\n\
         \x20   total ^= (seed as u64) >> 3;\n\
         {body}\n\
         \x20   total = total.rotate_left(7);\n\
         \x20   total ^= 0x9e37_79b9;\n\
         \x20   total.wrapping_add({file})\n\
         }}\n\n"
    )
}

fn module(file: usize, blocks: usize, variant: BlockVariant) -> String {
    let mut out = String::with_capacity(blocks * BLOCK_LINES * 48);
    for index in 0..blocks {
        out.push_str(&block(file, index, variant));
    }
    out
}

fn generate(sandbox: &mut Sandbox, spec: &CaseSpec) -> Result<()> {
    ensure!(
        spec.dormant.is_multiple_of(DORMANT_HUNKS_PER_FILE),
        "dormant target {} must be a multiple of {DORMANT_HUNKS_PER_FILE}",
        spec.dormant
    );
    ensure!(
        spec.staged_files <= spec.files,
        "staged_files exceeds files"
    );
    let dormant_files = spec.dormant / DORMANT_HUNKS_PER_FILE;

    for f in 0..spec.files {
        sandbox.write(
            &module_path(f),
            &module(f, spec.hunks_per_file, BlockVariant::Base),
        )?;
    }
    // Dormant-seed files get a disjoint file-id space so no anchor can
    // collide with a live module's.
    for d in 0..dormant_files {
        sandbox.write(
            &dormant_path(d),
            &module(d + 100_000, DORMANT_HUNKS_PER_FILE, BlockVariant::Base),
        )?;
    }
    sandbox.commit_all("baseline")?;

    let repo = sandbox.repo()?;
    if spec.changelists > 0 {
        for k in 0..spec.changelists {
            repo.create_changelist(&format!("cl-{k}"))
                .map_err(|err| anyhow::anyhow!("create changelist: {err}"))?;
        }
        // Edit one changelist's slice of files at a time; the refresh
        // auto-captures the new hunks to the active changelist, which is
        // how records are seeded without hand-assembly.
        for k in 0..spec.changelists {
            for f in (0..spec.files).filter(|f| f % spec.changelists == k) {
                sandbox.write(
                    &module_path(f),
                    &module(f, spec.hunks_per_file, BlockVariant::Edited),
                )?;
            }
            repo.switch(&format!("cl-{k}"))
                .map_err(|err| anyhow::anyhow!("switch: {err}"))?;
            refresh(&repo)?;
        }
        if dormant_files > 0 {
            // Capture, then revert: vanished hunks leave their records
            // dormant — the stash/revert reality dormancy exists for.
            for d in 0..dormant_files {
                sandbox.write(
                    &dormant_path(d),
                    &module(d + 100_000, DORMANT_HUNKS_PER_FILE, BlockVariant::Edited),
                )?;
            }
            refresh(&repo)?;
            for d in 0..dormant_files {
                sandbox.write(
                    &dormant_path(d),
                    &module(d + 100_000, DORMANT_HUNKS_PER_FILE, BlockVariant::Base),
                )?;
            }
            refresh(&repo)?;
        }
    } else {
        ensure!(dormant_files == 0, "dormant records need changelists");
        for f in 0..spec.files {
            sandbox.write(
                &module_path(f),
                &module(f, spec.hunks_per_file, BlockVariant::Edited),
            )?;
        }
    }

    for f in 0..spec.staged_files {
        repo.stage_file(&module_path(f))
            .map_err(|err| anyhow::anyhow!("stage {}: {err}", module_path(f)))?;
    }
    Ok(())
}

// --- pathological single huge file -------------------------------------

/// Stream-written so generation itself stays small in memory — the
/// refresh, not the generator, must be what moves the RSS high-water.
fn write_huge(path: &Path, lines: usize, variant: u32) -> Result<u64> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut writer = BufWriter::new(File::create(path)?);
    let mut bytes = 0u64;
    for i in 0..lines {
        let line = format!(
            "let row_{i:07}: u64 = compute_row({i}, {variant}, \"payload-{i:07}-padding-padding\");\n"
        );
        bytes += line.len() as u64;
        writer.write_all(line.as_bytes())?;
    }
    writer.flush()?;
    Ok(bytes)
}

fn generate_huge(sandbox: &mut Sandbox, root: &Path, spec: &CaseSpec) -> Result<u64> {
    ensure!(
        spec.changelists == 0 && spec.dormant == 0 && spec.staged_files <= 1,
        "the huge-file case probes raw diff cost; keep membership out of it"
    );
    let path = root.join("data/generated.rs");
    let mut bytes = write_huge(&path, spec.huge_lines, 0)?;
    sandbox.commit_all("baseline")?;
    bytes += write_huge(&path, spec.huge_lines, 1)?;
    if spec.staged_files == 1 {
        // Stage the first rewrite, then rewrite again: HEAD, index, and
        // worktree all differ, so BOTH diffs carry the huge content at
        // once — the full both-diffs-load-in-full memory shape.
        sandbox
            .repo()?
            .stage_file("data/generated.rs")
            .map_err(|err| anyhow::anyhow!("stage huge file: {err}"))?;
        bytes += write_huge(&path, spec.huge_lines, 2)?;
    }
    Ok(bytes)
}

// --- changed binaries ---------------------------------------------------

fn binary_path(index: usize) -> String {
    format!("assets/blob_{index:04}.bin")
}

/// Deterministic binary bytes: an 8-byte NUL header (git's binary sniff
/// looks for a NUL in the first 8000 bytes) then an LCG stream keyed by
/// `seed`, so rewrites keep the size and change every byte.
fn write_binary_blob(path: &Path, kib: usize, seed: u64) -> Result<u64> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut writer = BufWriter::new(File::create(path)?);
    writer.write_all(&[0u8; 8])?;
    let mut state = seed;
    let mut chunk = [0u8; 1024];
    for _ in 0..kib {
        for byte in chunk.iter_mut() {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            *byte = (state >> 56) as u8;
        }
        writer.write_all(&chunk)?;
    }
    writer.flush()?;
    Ok(8 + kib as u64 * 1024)
}

fn generate_binaries(sandbox: &mut Sandbox, root: &Path, spec: &CaseSpec) -> Result<u64> {
    ensure!(
        spec.changelists == 0
            && spec.dormant == 0
            && spec.staged_files == 0
            && spec.files == 0
            && !spec.touch_every_iteration,
        "the binaries case probes raw hashing cost; keep membership out of it"
    );
    ensure!(spec.binary_kib > 0, "binary files need a size");
    let mut bytes = 0u64;
    for b in 0..spec.binary_files {
        bytes += write_binary_blob(&root.join(binary_path(b)), spec.binary_kib, b as u64)?;
    }
    sandbox.commit_all("baseline")?;
    // Rewrite every blob, same size, different bytes: each refresh's
    // worktree diff re-hashes all of this content from disk.
    for b in 0..spec.binary_files {
        bytes += write_binary_blob(
            &root.join(binary_path(b)),
            spec.binary_kib,
            b as u64 + 1_000_000,
        )?;
    }
    Ok(bytes)
}

// --- verification -------------------------------------------------------

/// Counts of (live, dormant) records in the state file. Read-only
/// introspection via generic JSON — a benchmark of the wrong shape must
/// fail loudly, not report plausible numbers.
fn record_counts(root: &Path) -> Result<(usize, usize)> {
    let path = root.join(".git/gitchange/state.json");
    if !path.exists() {
        return Ok((0, 0));
    }
    let value: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path)?)
        .context("parse state.json for verification")?;
    let records = value.get("records").and_then(|r| r.as_array()).context(
        "state.json has no `records` array — schema drift? update the bench verification",
    )?;
    let dormant = records
        .iter()
        .filter(|record| {
            record
                .get("dormant_since")
                .is_some_and(|since| !since.is_null())
        })
        .count();
    Ok((records.len() - dormant, dormant))
}

fn verify(spec: &CaseSpec, snapshot: &Snapshot, root: &Path) -> Result<(usize, usize, usize)> {
    let (expected_files, expected_hunks) = if spec.huge_lines > 0 {
        (1, 1)
    } else if spec.binary_files > 0 {
        // One whole-file degenerate hunk per changed binary (ADR 0009).
        (spec.binary_files, spec.binary_files)
    } else {
        (spec.files, spec.files * spec.hunks_per_file)
    };
    let hunks_total: usize = snapshot.files.iter().map(|f| f.hunks.len()).sum();
    if snapshot.files.len() != expected_files || hunks_total != expected_hunks {
        bail!(
            "case `{}` built the wrong shape: {} files / {} hunks, expected {} / {}",
            spec.name,
            snapshot.files.len(),
            hunks_total,
            expected_files,
            expected_hunks
        );
    }
    // A full rewrite misdetected as text would also be one hunk, so the
    // counts alone can't prove the hash cost is being exercised.
    let flagged_binary = snapshot.files.iter().filter(|f| f.binary).count();
    if flagged_binary != spec.binary_files {
        bail!(
            "case `{}`: {flagged_binary} files detected as binary, expected {} — \
             the worktree hash isn't being exercised as intended",
            spec.name,
            spec.binary_files
        );
    }
    let owned = snapshot
        .files
        .iter()
        .flat_map(|f| &f.hunks)
        .filter(|h| h.changelist.is_some())
        .count();
    let expected_owned = if spec.changelists > 0 {
        expected_hunks
    } else {
        0
    };
    if owned != expected_owned {
        bail!(
            "case `{}`: {owned} hunks owned, expected {expected_owned}",
            spec.name
        );
    }
    let (live, dormant) = record_counts(root)?;
    if live != expected_owned || dormant != spec.dormant {
        bail!(
            "case `{}`: {live} live / {dormant} dormant records, expected {expected_owned} / {}",
            spec.name,
            spec.dormant
        );
    }
    Ok((hunks_total, live, dormant))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(name: &str) -> CaseSpec {
        CaseSpec {
            name: name.into(),
            dimension: "files".into(),
            scale: 3,
            files: 3,
            hunks_per_file: 2,
            changelists: 2,
            dormant: 0,
            staged_files: 0,
            huge_lines: 0,
            binary_files: 0,
            binary_kib: 0,
            touch_every_iteration: false,
            warmup: 1,
            iterations: 1,
            contrast_of: None,
        }
    }

    #[test]
    fn graduated_case_builds_and_measures() {
        let spec = spec("smoke-graduated");
        let result = run(&spec).unwrap();
        assert_eq!(result.hunks_total, 6);
        assert_eq!(result.live_records, 6);
        assert_eq!(result.dormant_records, 0);
        assert_eq!(result.times_ms.len(), 1);
        assert!(result.times_ms[0] > 0.0);
    }

    #[test]
    fn dormant_and_staged_seeding_reach_their_targets() {
        let mut spec = spec("smoke-dormant");
        spec.dormant = DORMANT_HUNKS_PER_FILE;
        spec.staged_files = 1;
        let result = run(&spec).unwrap();
        assert_eq!(result.hunks_total, 6);
        assert_eq!(result.live_records, 6);
        assert_eq!(result.dormant_records, DORMANT_HUNKS_PER_FILE);
    }

    #[test]
    fn huge_case_is_one_whole_file_hunk() {
        let mut spec = spec("smoke-huge");
        spec.changelists = 0;
        spec.huge_lines = 400;
        let result = run(&spec).unwrap();
        assert_eq!(result.hunks_total, 1);
        assert_eq!(result.live_records, 0);
        assert!(result.content_bytes > 0);
    }

    #[test]
    fn staged_huge_case_carries_both_diffs() {
        let mut spec = spec("smoke-huge-staged");
        spec.changelists = 0;
        spec.huge_lines = 400;
        spec.staged_files = 1;
        let result = run(&spec).unwrap();
        // Index (v1) and worktree (v2) rewrites pair on the same
        // HEAD-side span: one staged-stale whole-file hunk.
        assert_eq!(result.hunks_total, 1);
        // Three versions written: baseline + staged + worktree.
        assert!(result.content_bytes > 2 * 400 * 60);
    }

    #[test]
    fn binary_case_is_whole_file_hunks_flagged_binary() {
        let mut spec = spec("smoke-binary");
        spec.changelists = 0;
        spec.files = 0;
        spec.binary_files = 2;
        spec.binary_kib = 8;
        let result = run(&spec).unwrap();
        // verify() also asserts both files were detected as binary —
        // the whole point of the dimension.
        assert_eq!(result.hunks_total, 2);
        assert_eq!(result.live_records, 0);
        // Baseline + rewrite for each file, 8 KiB + NUL header each.
        assert_eq!(result.content_bytes, 2 * 2 * (8 * 1024 + 8));
    }

    #[test]
    fn touched_case_measures_changed_refreshes() {
        let mut spec = spec("smoke-touched");
        spec.touch_every_iteration = true;
        spec.iterations = 2;
        let result = run(&spec).unwrap();
        assert_eq!(result.hunks_total, 6);
        assert_eq!(result.times_ms.len(), 2);
    }
}
