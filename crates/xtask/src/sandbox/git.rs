//! One shell-out git runner for the sandbox tooling — the builder that
//! constructs scenario repos and the fingerprinter that reads them back.
//! Both want the same thing: git scoped to a repo dir, non-zero exit
//! surfaced with git's own stderr.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

/// A git invocation scoped to `repo_dir`, unspawned — for callers that
/// must set env vars first or want to check the status themselves.
pub(crate) fn command(repo_dir: &Path, args: &[&str]) -> Command {
    let mut command = Command::new("git");
    command.arg("-C").arg(repo_dir).args(args);
    command
}

/// Run git in `repo_dir`, returning stdout and erroring on non-zero exit.
pub(crate) fn run(repo_dir: &Path, args: &[&str]) -> Result<String> {
    stdout_or_fail(command(repo_dir, args), args)
}

/// Spawn a prepared invocation and take its stdout, erroring on non-zero
/// exit. `args` is carried separately only to name the command in errors.
pub(crate) fn stdout_or_fail(mut command: Command, args: &[&str]) -> Result<String> {
    let output = command.output().context("spawn git")?;
    if !output.status.success() {
        bail!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
