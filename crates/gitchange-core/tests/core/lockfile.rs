//! Lock contention resolution (ADR 0002): what a writer that finds the
//! lockfile already taken is told about its holder, and which of those
//! answers may advise removing it. The primitive's fail-fast and
//! never-stolen properties are asserted here too — they belong to the
//! lockfile, not to any one operation, and `tests/core/persistence.rs`
//! carries ADR 0002's other file-level properties.
//!
//! `create_changelist` stands in for every mutating op: all of them take
//! the same lock through one `update_state` cycle, so what is asserted
//! below is the lockfile's behaviour rather than the changelist op's.
//!
//! The recording half — that taking the lock writes the writer's PID — is
//! a unit test in `src/state_file.rs`: the file exists only while the lock
//! is held, and every public op releases before it returns, so this seam
//! can observe the resolution but never the write.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::support::RepoFixture;
use gitchange_core::{Error, LockHolder, Repo};

/// A repo whose lockfile holds `contents`, standing in for another
/// writer's hold. One changelist is created first so the state dir and a
/// real state file already exist — contention, not a first write.
fn repo_holding(contents: &str) -> (RepoFixture, Repo, PathBuf) {
    let fixture = RepoFixture::new();
    let repo = Repo::discover(fixture.path()).unwrap();
    repo.create_changelist("feature").unwrap();

    let lock_path = fixture.path().join(".git/gitchange/state.json.lock");
    fs::write(&lock_path, contents).unwrap();
    (fixture, repo, lock_path)
}

/// The PID of a process that has exited and been reaped — one the OS
/// answers "gone" for. Recycling could in principle hand the number to a
/// new process before the assertion runs; nothing portable rules that out,
/// and the window is microseconds.
fn reaped_pid() -> u32 {
    let mut child = Command::new("git")
        .arg("--version")
        .stdout(Stdio::null())
        .spawn()
        .expect("git is on PATH for this suite");
    let pid = child.id();
    child.wait().unwrap();
    pid
}

#[test]
fn a_live_holder_is_resolved_alive_and_never_advises_removal() {
    // This test process is the holder: the only PID certain to still be
    // running when the assertion reads it.
    let pid = std::process::id();
    let (_fixture, repo, lock_path) = repo_holding(&format!("{pid}\n"));

    let err = repo.create_changelist("bugfix").unwrap_err();
    assert!(
        matches!(err, Error::LockContention { holder: LockHolder::Alive { pid: held }, .. } if held == pid),
        "{err:?}"
    );

    let message = err.to_string();
    assert!(
        message.contains(&pid.to_string()),
        "the live holder is named: {message}"
    );
    assert!(
        !message.contains("remove"),
        "removing a live holder's lock breaks its session, so the text \
         cannot suggest it: {message}"
    );
    assert!(
        !message.contains(&lock_path.display().to_string()),
        "and the lockfile goes unnamed, for the same reason: {message}"
    );

    // Fail-fast, and never stolen (ADR 0002): the hold survived the
    // refusal.
    assert!(lock_path.exists());
}

#[test]
fn a_dead_holder_is_resolved_dead_and_the_refusal_names_the_lockfile() {
    let pid = reaped_pid();
    let (_fixture, repo, lock_path) = repo_holding(&format!("{pid}\n"));

    let err = repo.create_changelist("bugfix").unwrap_err();
    assert!(
        matches!(err, Error::LockContention { holder: LockHolder::Dead { pid: held }, .. } if held == pid),
        "{err:?}"
    );

    let message = err.to_string();
    assert!(
        message.contains("remove") && message.contains(&lock_path.display().to_string()),
        "a holder known to be gone is the one case where removal is the \
         accurate resolution, and it names the file to remove: {message}"
    );

    // Still no auto-break: gitchange reports the stale lock rather than
    // clearing it, so the never-silently-stolen invariant holds even when
    // stealing would be safe.
    assert!(lock_path.exists());

    // The advice is genuinely the resolution.
    fs::remove_file(&lock_path).unwrap();
    repo.create_changelist("bugfix").unwrap();
}

#[test]
fn an_unreadable_holder_is_assumed_alive() {
    // Three ways the PID can fail to identify anyone: a hold whose write
    // was cut short (or a lockfile from a gitchange predating the
    // recording), something that is not a number, and PID 0, which names
    // no process. All take the fail-safe branch — assuming alive costs a
    // retry, while assuming dead costs another session's state.
    for contents in ["", "not-a-pid\n", "0\n"] {
        let (_fixture, repo, lock_path) = repo_holding(contents);

        let err = repo.create_changelist("bugfix").unwrap_err();
        assert!(
            matches!(
                err,
                Error::LockContention {
                    holder: LockHolder::Unreadable,
                    ..
                }
            ),
            "lockfile {contents:?}: {err:?}"
        );
        let message = err.to_string();
        assert!(
            !message.contains("remove"),
            "lockfile {contents:?}: assumed-alive means retry, never remove: {message}"
        );
        assert!(
            message.contains(&lock_path.display().to_string()),
            "lockfile {contents:?}: this is the one refusal that names the \
             file without advising anything be done to it — its retry can go \
             on being refused, and then the file is all there is to inspect: \
             {message}"
        );
        assert!(lock_path.exists());
    }
}
