//! End-to-end tests of the binary: output and the 0/1/2 exit-code
//! contract (ticket 13). Fixtures are built with the git CLI so git2
//! stays a gitchange-core-only dependency (ADR 0006).

use std::path::Path;
use std::process::{Command, Output};

fn gitchange(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_gitchange"))
        .current_dir(dir)
        .args(args)
        .output()
        .expect("run gitchange")
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(dir)
        .args(args)
        .status()
        .expect("run git");
    assert!(status.success(), "git {args:?} failed");
}

fn dirty_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q"]);
    git(dir.path(), &["config", "user.name", "gitchange-tests"]);
    git(
        dir.path(),
        &["config", "user.email", "tests@gitchange.invalid"],
    );
    std::fs::write(dir.path().join("tracked.txt"), "one\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-q", "--no-verify", "-m", "init"]);
    std::fs::write(dir.path().join("tracked.txt"), "two\n").unwrap();
    std::fs::write(dir.path().join("untracked.txt"), "hello\n").unwrap();
    dir
}

#[test]
fn status_lists_changed_files_and_exits_0() {
    let repo = dirty_repo();
    let output = gitchange(repo.path(), &["status"]);

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines, vec!["M tracked.txt", "? untracked.txt"]);
}

#[test]
fn status_outside_a_repo_exits_1_with_message_on_stderr() {
    let dir = tempfile::tempdir().unwrap();
    let output = gitchange(dir.path(), &["status"]);

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("not a git repository"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn unknown_subcommand_exits_2() {
    let dir = tempfile::tempdir().unwrap();
    let output = gitchange(dir.path(), &["frobnicate"]);

    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn bare_invocation_prints_tui_stub_and_exits_0() {
    let repo = dirty_repo();
    let output = gitchange(repo.path(), &[]);

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("TUI not yet built"),
        "unexpected stdout: {stdout}"
    );
}
