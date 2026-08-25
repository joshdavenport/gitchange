//! Lock contention as the binary resolves it (#122/#135): what a CLI
//! writer that finds the state lockfile already taken does about it, and
//! which of the three holder classifications — live, unreadable, dead —
//! reaches the caller as the transient exit 3 rather than an ordinary
//! exit 1. Core's own resolution is tested in `gitchange-core`; what is
//! asserted here is the retry budget the binary spends on top of it, and
//! the diagnostics it prints when the budget runs out.
//!
//! Its own module because contention needs a fixture no other test wants —
//! a lockfile seeded with a chosen PID — and because it is the only place
//! that spawns the binary without waiting for it. `switch bugfix` stands
//! in for every mutation: they all contend through the same one cycle.

use std::process::{Command, Stdio};
use std::time::Duration;

use crate::support::{gitchange, lock_path, reaped_pid, repo_holding, state_path};

#[test]
fn a_hold_released_within_the_budget_never_surfaces() {
    // The routine concurrency the retry exists for (#122): a live TUI
    // holds the lock across one write, and the CLI absorbs it. The
    // holder is this test process, the only PID certain to be running.
    let repo = repo_holding(&format!("{}\n", std::process::id()));

    let child = Command::new(env!("CARGO_BIN_EXE_gitchange"))
        .current_dir(repo.path())
        .args(["switch", "bugfix"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run gitchange");
    // Long enough that the child is past its first contended attempt
    // even on a cold debug binary — that attempt is what makes the
    // release load-bearing rather than incidental. An overshoot on a
    // loaded runner is safe by a wide margin: the child's budget only
    // starts once it has discovered the repo and contended, so a release
    // near a second late still lands well inside it.
    std::thread::sleep(Duration::from_millis(700));
    std::fs::remove_file(lock_path(repo.path())).unwrap();

    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "switched to 'bugfix'\n"
    );
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "",
        "absorbed contention is not an error, so nothing is said about it"
    );
}

#[test]
fn a_live_hold_outlasting_the_budget_exits_3() {
    // Budget spent against a holder still running: transient, so the
    // answer is the retry code and never removal advice — following that
    // advice would break the live holder's own load-mutate-save cycle.
    let pid = std::process::id();
    let repo = repo_holding(&format!("{pid}\n"));
    let state_before = std::fs::read(state_path(repo.path())).unwrap();

    let output = gitchange(repo.path(), &["switch", "bugfix"]);

    assert_eq!(output.status.code(), Some(3));
    assert_eq!(
        output.stdout,
        Vec::<u8>::new(),
        "a failed command leaves stdout empty"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains(&pid.to_string()) && stderr.contains("run the same command again"),
        "the live holder is named and the retry is the same command: {stderr}"
    );
    assert!(
        !stderr.contains("remove"),
        "removing a live holder's lock breaks its session: {stderr}"
    );
    assert!(
        stderr.lines().all(|line| line.starts_with("gitchange: ")),
        "every diagnostic carries the prefix: {stderr}"
    );
    assert_eq!(
        std::fs::read(state_path(repo.path())).unwrap(),
        state_before,
        "the refused switch wrote nothing"
    );
    assert!(
        lock_path(repo.path()).exists(),
        "gitchange never breaks a lock it did not take"
    );
}

#[test]
fn an_unreadable_hold_outlasting_the_budget_exits_3() {
    // No PID to read — a hold whose write was cut short, or a lockfile
    // gitchange did not write. Assumed running (the fail-safe direction),
    // so it takes the live holder's exit code and its silence about
    // removal.
    let repo = repo_holding("");

    let output = gitchange(repo.path(), &["switch", "bugfix"]);

    assert_eq!(output.status.code(), Some(3));
    assert_eq!(output.stdout, Vec::<u8>::new());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        !stderr.contains("remove"),
        "assumed-alive means retry, never remove: {stderr}"
    );
    assert!(
        stderr.contains("run the same command again"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn a_dead_hold_outlasting_the_budget_exits_1_naming_the_lockfile() {
    // A holder proven gone will never release, so this is no longer
    // transient: an ordinary refusal, carrying the one resolution that is
    // accurate here. The file is named rather than removed — gitchange
    // never breaks a lock (ADR 0002).
    let repo = repo_holding(&format!("{}\n", reaped_pid()));

    let output = gitchange(repo.path(), &["switch", "bugfix"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, Vec::<u8>::new());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        // The file is matched by name, not by full path: git hands core a
        // forward-slashed git dir even on Windows, so the printed path
        // differs from a hand-spelled one by separator alone (#178).
        stderr.contains("remove") && stderr.contains("state.json.lock"),
        "the refusal names the file to remove: {stderr}"
    );
    assert!(
        !stderr.contains("run the same command again"),
        "retrying into a stale lock is not the resolution: {stderr}"
    );
    assert!(lock_path(repo.path()).exists());

    // The advice is genuinely the resolution.
    std::fs::remove_file(lock_path(repo.path())).unwrap();
    let output = gitchange(repo.path(), &["switch", "bugfix"]);
    assert_eq!(output.status.code(), Some(0));
}
