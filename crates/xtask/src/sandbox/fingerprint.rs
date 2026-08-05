//! Build-time fingerprints for `sandbox status`: enough to say
//! missing / pristine / modified and name the diverged layer, never to
//! say *what* changed — that's what nuke-and-rebuild is for.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use gitchange_core::Repo;
use serde::{Deserialize, Serialize};

use super::git;

#[derive(Serialize, Deserialize)]
pub struct Fingerprint {
    schema: u32,
    scenario: String,
    built_at: u64,
    /// HEAD commit OID — the history layer.
    head: String,
    /// Digest of diff(HEAD↔index) — the index layer.
    index_digest: String,
    /// Digest of the worktree diff, untracked paths, and unmerged
    /// entries — the worktree layer.
    worktree_digest: String,
    /// Digest of state.json, or "absent" — the gitchange state layer.
    state_digest: String,
}

impl Fingerprint {
    /// Layer names where `current` diverges from this stored baseline.
    pub fn diverged_layers(&self, current: &Fingerprint) -> Vec<&'static str> {
        let mut layers = Vec::new();
        if self.head != current.head {
            layers.push("history");
        }
        if self.index_digest != current.index_digest {
            layers.push("index");
        }
        if self.worktree_digest != current.worktree_digest {
            layers.push("worktree");
        }
        if self.state_digest != current.state_digest {
            layers.push("gitchange state");
        }
        layers
    }
}

pub fn capture(repo_dir: &Path, scenario: &str) -> Result<Fingerprint> {
    let head = git::run(repo_dir, &["rev-parse", "HEAD"])?
        .trim()
        .to_string();
    let index_digest = fnv1a64(git::run(repo_dir, &["diff", "--cached"])?.as_bytes());
    let worktree = [
        git::run(repo_dir, &["diff"])?,
        git::run(repo_dir, &["ls-files", "--others", "--exclude-standard"])?,
        git::run(repo_dir, &["ls-files", "--unmerged"])?,
    ]
    .join("\0");
    // Ask core where the state file is. A hand-spelled path that misses
    // reads as `absent` below, which is indistinguishable from a scenario
    // that legitimately has no state — so `status` would call the
    // gitchange-state layer pristine forever.
    let state_path = Repo::discover(repo_dir)
        .map_err(|err| anyhow!("open repo via core: {err}"))?
        .state_file_path();
    let state_digest = match fs::read(&state_path) {
        Ok(bytes) => fnv1a64(&bytes),
        // Only a missing file means "this scenario has no gitchange
        // state". Any other read failure digested as `absent` would
        // report the layer pristine because we couldn't read it.
        Err(err) if err.kind() == ErrorKind::NotFound => "absent".to_string(),
        Err(err) => {
            return Err(err).with_context(|| format!("read {}", state_path.display()));
        }
    };
    Ok(Fingerprint {
        schema: 1,
        scenario: scenario.to_string(),
        built_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        head,
        index_digest,
        worktree_digest: fnv1a64(worktree.as_bytes()),
        state_digest,
    })
}

pub fn store(meta_dir: &Path, fingerprint: &Fingerprint) -> Result<()> {
    fs::create_dir_all(meta_dir)?;
    let path = meta_dir.join(format!("{}.json", fingerprint.scenario));
    let json = serde_json::to_string_pretty(fingerprint)?;
    fs::write(&path, json).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

pub fn load(meta_dir: &Path, scenario: &str) -> Result<Option<Fingerprint>> {
    let path = meta_dir.join(format!("{scenario}.json"));
    match fs::read_to_string(&path) {
        Ok(json) => Ok(Some(
            serde_json::from_str(&json).with_context(|| format!("parse {}", path.display()))?,
        )),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("read {}", path.display())),
    }
}

fn fnv1a64(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}
