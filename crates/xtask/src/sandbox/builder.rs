//! Deterministic sandbox repo builder: shell-out git for repo
//! construction (like the commit path, ADR 0004), core's real sync ops
//! for gitchange state. Commits get pinned identity and stepped
//! timestamps so rebuilds reproduce identical OIDs.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use gitchange_core::{Hunk, Repo, Snapshot};

/// 2026-01-01T00:00:00Z — recent enough that the Commits panel doesn't
/// look broken, fixed so rebuilds are byte-identical.
const BASE_EPOCH: u64 = 1_767_225_600;

pub struct Sandbox {
    root: PathBuf,
    commits: u64,
}

impl Sandbox {
    /// Init a repo at `root` with pinned local config, mirroring
    /// `RepoFixture` so global git config can't distort what's eyeballed.
    pub fn init(root: &Path) -> Result<Self> {
        fs::create_dir_all(root).with_context(|| format!("create {}", root.display()))?;
        let sandbox = Self {
            root: root.to_path_buf(),
            commits: 0,
        };
        sandbox.git(&["init", "--quiet", "--initial-branch=main"])?;
        for (key, value) in [
            ("user.name", "gitchange-sandbox"),
            ("user.email", "sandbox@gitchange.invalid"),
            ("commit.gpgsign", "false"),
            ("core.hooksPath", ".git/hooks"),
            ("core.autocrlf", "false"),
        ] {
            sandbox.git(&["config", key, value])?;
        }
        Ok(sandbox)
    }

    /// Write `content` to a repo-relative path, creating parent dirs.
    pub fn write(&self, rel: &str, content: &str) -> Result<()> {
        self.write_bytes(rel, content.as_bytes())
    }

    pub fn write_bytes(&self, rel: &str, content: &[u8]) -> Result<()> {
        let path = self.root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, content).with_context(|| format!("write {}", path.display()))?;
        Ok(())
    }

    /// Replace `from` with `to` in a tracked file, erroring when the
    /// needle is absent — a silent no-op would build a wrong scenario.
    pub fn replace(&self, rel: &str, from: &str, to: &str) -> Result<()> {
        let path = self.root.join(rel);
        let content = fs::read_to_string(&path).with_context(|| format!("read {rel}"))?;
        if !content.contains(from) {
            bail!("`{rel}` does not contain the expected text: {from:?}");
        }
        self.write(rel, &content.replace(from, to))
    }

    /// Run git in the sandbox, erroring on non-zero exit.
    pub fn git(&self, args: &[&str]) -> Result<String> {
        let output = self.git_command(args).output().context("spawn git")?;
        if !output.status.success() {
            bail!(
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Run git expecting failure (e.g. a conflicting merge); errors if
    /// the command unexpectedly succeeds.
    pub fn git_expect_failure(&self, args: &[&str]) -> Result<()> {
        let output = self.git_command(args).output().context("spawn git")?;
        if output.status.success() {
            bail!("git {args:?} succeeded but the scenario expects it to fail");
        }
        Ok(())
    }

    fn git_command(&self, args: &[&str]) -> Command {
        let mut command = Command::new("git");
        command.arg("-C").arg(&self.root).args(args);
        command
    }

    /// Stage everything and commit with the next stepped timestamp.
    pub fn commit_all(&mut self, message: &str) -> Result<()> {
        self.git(&["add", "-A"])?;
        self.commits += 1;
        let date = format!("{} +0000", BASE_EPOCH + self.commits * 3600);
        let output = self
            .git_command(&["commit", "--quiet", "-m", message])
            .env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_DATE", &date)
            .output()
            .context("spawn git commit")?;
        if !output.status.success() {
            bail!(
                "git commit {message:?} failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(())
    }

    /// Open the sandbox through core — the only way scenarios touch
    /// gitchange state.
    pub fn repo(&self) -> Result<Repo> {
        Repo::discover(&self.root).map_err(|err| anyhow!("open repo via core: {err}"))
    }
}

/// Refresh through core, surfacing core errors as anyhow.
pub fn refresh(repo: &Repo) -> Result<Snapshot> {
    repo.refresh().map_err(|err| anyhow!("core refresh: {err}"))
}

fn file_hunks<'a>(snapshot: &'a Snapshot, path: &str) -> Result<&'a [Hunk]> {
    snapshot
        .files
        .iter()
        .find(|file| file.path == path)
        .map(|file| file.hunks.as_slice())
        .with_context(|| format!("snapshot has no changed file `{path}`"))
}

/// Move every hunk of `path` to `target`.
pub fn move_file(repo: &Repo, snapshot: &Snapshot, path: &str, target: &str) -> Result<()> {
    let hunks = file_hunks(snapshot, path)?;
    repo.move_hunks(path, hunks, Some(target))
        .map_err(|err| anyhow!("move `{path}` to `{target}`: {err}"))?;
    Ok(())
}

/// Move one hunk of `path` (by snapshot position) to `target`.
pub fn move_hunk(
    repo: &Repo,
    snapshot: &Snapshot,
    path: &str,
    index: usize,
    target: &str,
) -> Result<()> {
    let hunks = file_hunks(snapshot, path)?;
    let hunk = hunks
        .get(index)
        .with_context(|| format!("`{path}` has {} hunks, wanted index {index}", hunks.len()))?;
    repo.move_hunks(path, std::slice::from_ref(hunk), Some(target))
        .map_err(|err| anyhow!("move `{path}` hunk {index} to `{target}`: {err}"))?;
    Ok(())
}

/// Stage one hunk of `path` by snapshot position.
pub fn stage_hunk(repo: &Repo, snapshot: &Snapshot, path: &str, index: usize) -> Result<()> {
    let hunks = file_hunks(snapshot, path)?;
    let hunk = hunks
        .get(index)
        .with_context(|| format!("`{path}` has {} hunks, wanted index {index}", hunks.len()))?;
    repo.stage_hunk(path, hunk)
        .map_err(|err| anyhow!("stage `{path}` hunk {index}: {err}"))?;
    Ok(())
}
