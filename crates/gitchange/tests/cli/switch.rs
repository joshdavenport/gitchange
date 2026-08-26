//! `switch` at the binary seam: the receipt it prints, its refusals, and
//! the active marker it moves. The marker is the subject either way — a
//! switch is only observable through what the next read says about it, so
//! each test here pairs the mutation with the `status` that reveals it.
//!
//! Its own module for the verb's three subjects: what the write says on
//! stdout (core composes the echo, #138), what a refused one leaves behind
//! (#172 — the candidates listed, nothing created), and how the `*` travels
//! between a changelist and `unassigned` (#52 / ADR 0015). The deliberate
//! *absence* is here too: a switch runs no refresh, so the pending pool it
//! moves the marker over stays pending (#153). Contention over the same
//! write is `locking.rs`'s; `status`'s own faces are `status.rs`'s.

use crate::support::{
    CAPTURE_PENDING_HINT, committed_repo, dirty_repo, gitchange, owned_repo, owners, seed_state,
    state_bytes, state_path,
};

/// The membership records, parsed — what a switch must leave alone even
/// though the marker beside them moves, so the file's bytes cannot be
/// the subject the way they are everywhere else here.
fn records(dir: &std::path::Path) -> serde_json::Value {
    let state: serde_json::Value =
        serde_json::from_slice(&state_bytes(dir)).expect("one JSON document");
    state["records"].clone()
}

#[test]
fn switch_then_status_round_trip_the_active_marker() {
    let repo = dirty_repo();
    seed_state(repo.path(), "feature", &["feature", "bugfix"]);

    let output = gitchange(repo.path(), &["switch", "bugfix"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    // Core's echo, printed verbatim: the receipt's stdout line is
    // composed beside the write (ADR 0006/0007), never here (#138). The
    // fragment is the target it names — the prose around it is a display,
    // so pinning it whole would make core's wording this suite's business.
    assert!(stdout.contains("'bugfix'"), "unexpected stdout: {stdout}");
    assert_eq!(stdout.lines().count(), 1, "one echo line: {stdout}");

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
fn switch_to_unknown_name_refuses_listing_the_valid_targets() {
    // gh's error shape (#122/#172): the name that matched nothing, then
    // every target a retry can name — `unassigned` among them, since it is
    // a switch target (ADR 0015). A typo costs one round trip.
    let repo = dirty_repo();
    seed_state(repo.path(), "feature", &["feature", "bugfix"]);
    let before = state_bytes(repo.path());

    let output = gitchange(repo.path(), &["switch", "nope"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        output.stdout,
        Vec::<u8>::new(),
        "a failed command leaves stdout empty (#122)"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    for fragment in [
        "no changelist named 'nope'",
        "unassigned",
        "'feature'",
        "'bugfix'",
    ] {
        assert!(
            stderr.contains(fragment),
            "no {fragment:?} in stderr: {stderr}"
        );
    }

    // A refusal never creates: the typo'd name is not minted, and the
    // marker has not moved.
    assert_eq!(state_bytes(repo.path()), before);
}

#[test]
fn switch_all_refuses_as_an_ordinary_unrecognised_name() {
    // No changelist can hold a reserved name, so `all` matches nothing —
    // the same refusal a typo earns, and creating nothing either.
    let repo = dirty_repo();
    seed_state(repo.path(), "feature", &["feature"]);
    let before = state_bytes(repo.path());

    let output = gitchange(repo.path(), &["switch", "all"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, Vec::<u8>::new());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("no changelist named 'all'") && stderr.contains("'feature'"),
        "unexpected stderr: {stderr}"
    );
    assert_eq!(state_bytes(repo.path()), before);
}

#[test]
fn a_refused_switch_in_an_unmanaged_repo_grows_no_state_file() {
    // The strongest reading of "the refusal never creates" (#172): where
    // there is no state file at all, a typo does not mint one — so the
    // default-state guard survives a failed switch, and the refusal still
    // names the one scope such a repo has.
    let repo = committed_repo();

    let output = gitchange(repo.path(), &["switch", "nope"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, Vec::<u8>::new());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("no changelist named 'nope'") && stderr.contains("unassigned"),
        "unexpected stderr: {stderr}"
    );
    assert!(
        !state_path(repo.path()).exists(),
        "a refusal wrote a state file"
    );
}

#[test]
fn switching_to_the_already_active_changelist_says_nothing() {
    // Nothing decided, nothing printed (#122): git's "Already on 'x'"
    // comfort text is not borrowed, so stdout carries decisions only —
    // and nothing decided is nothing written, so the file is untouched.
    let repo = dirty_repo();
    seed_state(repo.path(), "feature", &["feature"]);
    let before = state_bytes(repo.path());

    let output = gitchange(repo.path(), &["switch", "feature"]);

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, Vec::<u8>::new());
    assert_eq!(output.stderr, Vec::<u8>::new());
    assert_eq!(state_bytes(repo.path()), before);
}

#[test]
fn switch_claims_nothing_from_the_pending_pool() {
    // The marker move is the whole act (#153): a bare write runs no
    // refresh, so the unassigned hunks sitting there — another actor's
    // fresh work included — stay unassigned across it. Claim-now is the
    // composition `switch <name>` then `refresh`, never the switch.
    let repo = owned_repo();
    let before = records(repo.path());
    assert_eq!(
        owners(repo.path(), "a.txt"),
        vec![Some("feature".to_owned()), None],
        "the fixture's second hunk is the pending pool"
    );

    let output = gitchange(repo.path(), &["switch", "docs"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        output.stderr,
        Vec::<u8>::new(),
        "no refresh ran, so no advisory can ride the receipt"
    );

    // Records untouched — the pending hunk is still nobody's, and
    // 'feature' still holds the one it held.
    assert_eq!(records(repo.path()), before);
    assert_eq!(
        owners(repo.path(), "a.txt"),
        vec![Some("feature".to_owned()), None]
    );
    assert_eq!(owners(repo.path(), "sub/c.txt"), vec![None]);

    // And the marker did move: the listing is where a switch is visible.
    let listing = String::from_utf8(gitchange(repo.path(), &["changelist"]).stdout).unwrap();
    assert!(listing.contains("* docs"), "unexpected listing: {listing}");
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
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("unassigned"), "unexpected stdout: {stdout}");

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
