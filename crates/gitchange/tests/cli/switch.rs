//! `switch` at the binary seam: the receipt it prints, its refusals, and
//! the active marker it moves. The marker is the subject either way — a
//! switch is only observable through what the next read says about it, so
//! each test here pairs the mutation with the `status` that reveals it.
//!
//! Its own module because these are the suite's only mutations that
//! succeed: what a write says on stdout (core composes the echo, #138),
//! what a refused one leaves behind, and how the `*` travels between a
//! changelist and `unassigned` (#52 / ADR 0015). Contention over the same
//! write is `locking.rs`'s; `status`'s own faces are `status.rs`'s.

use crate::support::{CAPTURE_PENDING_HINT, dirty_repo, gitchange, seed_state};

#[test]
fn switch_then_status_round_trip_the_active_marker() {
    let repo = dirty_repo();
    seed_state(repo.path(), "feature", &["feature", "bugfix"]);

    let output = gitchange(repo.path(), &["switch", "bugfix"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    // Core's echo, printed verbatim: the receipt's stdout line is
    // composed beside the write (ADR 0006/0007), never here (#138).
    assert_eq!(stdout, "switched to 'bugfix'\n");

    // A separate invocation sees the persisted marker. The dirty files
    // stay unassigned: a read claims nothing (#156), so the newly active
    // changelist is where capture *will* flow, not where it has.
    let output = gitchange(repo.path(), &["status"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        vec![
            "  feature",
            "* bugfix",
            "  unassigned",
            CAPTURE_PENDING_HINT,
            "    ○ M tracked.txt 0/1",
            "    ○ ? untracked.txt 0/1",
        ]
    );
}

#[test]
fn switch_to_unknown_name_exits_1_with_message_on_stderr() {
    let repo = dirty_repo();
    seed_state(repo.path(), "feature", &["feature"]);

    let output = gitchange(repo.path(), &["switch", "nope"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        output.stdout,
        Vec::<u8>::new(),
        "a failed command leaves stdout empty (#122)"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("no changelist named 'nope'"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn switching_to_the_already_active_changelist_says_nothing() {
    // Nothing decided, nothing printed (#122): git's "Already on 'x'"
    // comfort text is not borrowed, so stdout carries decisions only.
    let repo = dirty_repo();
    seed_state(repo.path(), "feature", &["feature"]);

    let output = gitchange(repo.path(), &["switch", "feature"]);

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, Vec::<u8>::new());
    assert_eq!(output.stderr, Vec::<u8>::new());
}

#[test]
fn switch_without_a_name_exits_2() {
    let repo = dirty_repo();
    let output = gitchange(repo.path(), &["switch"]);

    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn switch_unassigned_turns_capture_off_and_marks_the_group() {
    // #52 / ADR 0015: `unassigned` is a switch target, and the existing
    // `*` on its group is the whole indicator.
    let repo = dirty_repo();
    seed_state(repo.path(), "feature", &["feature"]);

    let output = gitchange(repo.path(), &["switch", "unassigned"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "switched to 'unassigned'\n"
    );

    let output = gitchange(repo.path(), &["status"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        vec![
            "  feature",
            "* unassigned",
            "    ○ M tracked.txt 0/1",
            "    ○ ? untracked.txt 0/1",
        ],
        "capture is off: the dirty tree is unassigned with nothing pending, so no hint"
    );

    // And back: the changelist reclaims the marker, and capture is on
    // again — pending, since the read that shows it never performs it.
    let output = gitchange(repo.path(), &["switch", "feature"]);
    assert_eq!(output.status.code(), Some(0));
    let output = gitchange(repo.path(), &["status"]);
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        vec![
            "* feature",
            "  unassigned",
            CAPTURE_PENDING_HINT,
            "    ○ M tracked.txt 0/1",
            "    ○ ? untracked.txt 0/1",
        ]
    );
}
