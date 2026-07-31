//! Build-time fingerprints for `sandbox status`: enough to say
//! missing / pristine / modified and name the diverged layer, never to
//! say *what* changed — that's what nuke-and-rebuild is for.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

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
    let head = git(repo_dir, &["rev-parse", "HEAD"])?.trim().to_string();
    let index_digest = fnv1a64(git(repo_dir, &["diff", "--cached"])?.as_bytes());
    let worktree = [
        git(repo_dir, &["diff"])?,
        git(repo_dir, &["ls-files", "--others", "--exclude-standard"])?,
        git(repo_dir, &["ls-files", "--unmerged"])?,
    ]
    .join("\0");
    let state_path = repo_dir.join(".git/gitchange/state.json");
    let state_digest = match fs::read(&state_path) {
        Ok(bytes) => fnv1a64(&bytes),
        Err(_) => "absent".to_string(),
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

fn git(repo_dir: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_dir)
        .args(args)
        .output()
        .context("spawn git")?;
    if !output.status.success() {
        bail!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn fnv1a64(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}
