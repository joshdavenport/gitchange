//! `refresh` at the binary seam (#153): the deliberate claim-now — one
//! persisting refresh whose receipt carries its decisions, and whose
//! silence is as load-bearing as its echo.
//!
//! Its own module for the verb's three subjects: what a deciding refresh
//! says (the counted echo on stdout, the decisions as `notice:` lines on
//! stderr, each exactly once), what a quiet one says (nothing at all, with
//! nothing written), and the one write that stays silent — the baseline
//! restamp an external HEAD move earns, which is bookkeeping rather than a
//! decision (ADR 0012).
//!
//! The claim model's flow tests live here too, this being the verb that
//! completes them: `switch <name>` then `refresh` claims, and `switch
//! unassigned` turns capture off. `switch`'s own half — the marker moving
//! and claiming nothing — is `switch.rs`'s.

use std::path::Path;

use crate::support::{
    committed_repo, git, gitchange, initialised_repo, owned_repo, owners, seed_state,
    seed_state_raw, state_bytes, state_path, write,
};

/// Run `refresh`, asserting only that it succeeded: its stdout and stderr
/// are the subject of every test here, so they come back rather than being
/// checked in passing.
fn refresh(dir: &Path) -> (String, String) {
    let output = gitchange(dir, &["refresh"]);
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    (String::from_utf8(output.stdout).unwrap(), stderr)
}

/// A repo with `a.txt` edited and no records — the pending pool a refresh
/// is asked to claim — with `active` holding the marker (`None` being
/// unassigned, capture off).
fn capturing_repo(active: Option<&str>) -> tempfile::TempDir {
    let dir = initialised_repo();
    let repo = dir.path();
    write(repo, "a.txt", "one\n");
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-q", "--no-verify", "-m", "init"]);
    match active {
        Some(name) => seed_state(repo, name, &["feature"]),
        None => seed_state_raw(
            repo,
            r#"{ "version": 1, "active": null, "changelists": [{ "name": "feature" }] }"#,
        ),
    }
    write(repo, "a.txt", "two\n");
    dir
}

/// The state file, parsed — for the two claims a silent refresh can only
/// be checked against inside the file: the stamp it moved, and the records
/// it did not write.
fn state_json(dir: &Path) -> serde_json::Value {
    serde_json::from_slice(&state_bytes(dir)).expect("one JSON document")
}

/// The state file's `baseline_head` — the stamp the silent restamp moves
/// (ADR 0012), which is the only fact about a silent write there is to
/// assert.
fn baseline(dir: &Path) -> Option<String> {
    state_json(dir)["baseline_head"].as_str().map(str::to_owned)
}

// --- the deciding refresh ---------------------------------------------------

#[test]
fn refresh_claims_the_pending_pool_and_reports_the_decision() {
    let dir = capturing_repo(Some("feature"));
    let repo = dir.path();

    let (stdout, stderr) = refresh(repo);

    // The echo counts what the refresh decided (#153). Fragments only —
    // core owns the wording (ADR 0006), so the subject here is that stdout
    // alone says something happened, which is what trips a re-read where a
    // harness drops stderr.
    assert!(stdout.contains("1 hunk"), "unexpected stdout: {stdout}");
    assert_eq!(stdout.lines().count(), 1, "one echo line: {stdout}");
    // The decision itself rides stderr, in this surface's dressing.
    assert_eq!(
        stderr.lines().collect::<Vec<_>>(),
        vec!["gitchange: notice: auto-captured hunk at a.txt:1 → 'feature'"]
    );
    assert_eq!(owners(repo, "a.txt"), vec![Some("feature".to_owned())]);
    // And where a human looks for it: the claim is the changelist's now,
    // so the next read has no pending pool to hint about.
    let listing = String::from_utf8(gitchange(repo, &["status"]).stdout).unwrap();
    assert_eq!(
        listing.lines().collect::<Vec<_>>(),
        vec!["* feature", "    ○ M a.txt 0/1"]
    );
}

#[test]
fn a_decision_is_reported_once_and_the_next_refresh_says_nothing() {
    // Advisory delivery (#122): the deciding refresh's receipt carries it,
    // and the record it wrote is why the next one has nothing to decide —
    // so a loop over `refresh` reports each capture exactly once.
    let dir = capturing_repo(Some("feature"));
    let repo = dir.path();
    refresh(repo);
    let after_claiming = state_bytes(repo);

    let (stdout, stderr) = refresh(repo);

    assert_eq!(stdout, "");
    assert_eq!(stderr, "");
    assert_eq!(
        state_bytes(repo),
        after_claiming,
        "a repeated refresh writes nothing"
    );
}

#[test]
fn refresh_with_unassigned_active_claims_nothing() {
    // Capture off (ADR 0015): the command is named for the mechanism, so
    // it still runs — and decides nothing, which is silence.
    let dir = capturing_repo(None);
    let repo = dir.path();

    let (stdout, stderr) = refresh(repo);

    assert_eq!(stdout, "");
    assert_eq!(stderr, "");
    assert_eq!(owners(repo, "a.txt"), vec![None]);
    let state = state_json(repo);
    let records = state["records"].as_array().map_or(0, Vec::len);
    assert_eq!(records, 0, "capture off writes no records: {state}");
}

// --- the quiet tree ---------------------------------------------------------

#[test]
fn refresh_on_a_quiet_tree_says_nothing_and_writes_nothing() {
    // Nothing to decide and nothing to stamp: the second invocation is the
    // one that proves the write too, the first having adopted the current
    // HEAD as its baseline (ADR 0012) — silently, which is the restamp's
    // own contract below.
    let repo = committed_repo();
    seed_state(repo.path(), "feature", &["feature"]);
    let (stdout, stderr) = refresh(repo.path());
    assert_eq!(stdout, "");
    assert_eq!(stderr, "");
    let stamped = state_bytes(repo.path());

    let (stdout, stderr) = refresh(repo.path());

    assert_eq!(stdout, "");
    assert_eq!(stderr, "");
    assert_eq!(state_bytes(repo.path()), stamped);
}

#[test]
fn a_refresh_in_an_unmanaged_repo_grows_no_state_file() {
    // The default-state guard (ADR 0012): a repo with no gitchange state
    // has no changelists, so capture is off and there is nothing to record
    // — and a stamp alone is not a reason to grow a file.
    let repo = committed_repo();
    write(repo.path(), "tracked.txt", "two\n");

    let (stdout, stderr) = refresh(repo.path());

    assert_eq!(stdout, "");
    assert_eq!(stderr, "");
    assert!(
        !state_path(repo.path()).exists(),
        "a refresh minted a state file"
    );
}

// --- the silent restamp -----------------------------------------------------

#[test]
fn an_external_head_move_restamps_without_a_word() {
    // Bookkeeping, not a decision (ADR 0012), so the receipt stays empty
    // even though the file moved — the one write `refresh` makes silently.
    let repo = committed_repo();
    seed_state(repo.path(), "feature", &["feature"]);
    refresh(repo.path());
    let stamped = baseline(repo.path()).expect("the first refresh adopted HEAD");

    write(repo.path(), "later.txt", "hello\n");
    git(repo.path(), &["add", "-A"]);
    git(
        repo.path(),
        &["commit", "-q", "--no-verify", "-m", "outside"],
    );
    let (stdout, stderr) = refresh(repo.path());

    assert_eq!(stdout, "");
    assert_eq!(stderr, "");
    let moved = baseline(repo.path()).expect("still stamped");
    assert_ne!(moved, stamped, "the stamp followed HEAD");
    assert_eq!(moved, git(repo.path(), &["rev-parse", "HEAD"]));
}

// --- the claim model, composed ----------------------------------------------

#[test]
fn switch_then_refresh_claims_a_pre_existing_unassigned_hunk() {
    // The composition the TUI's claim-at-switch is spelled as (#153): the
    // marker moves, and the *next* persisting refresh — this one, asked for
    // deliberately — claims the pool it moved over.
    let dir = owned_repo();
    let repo = dir.path();
    assert_eq!(
        owners(repo, "sub/c.txt"),
        vec![None],
        "the fixture's unassigned hunk"
    );

    assert_eq!(gitchange(repo, &["switch", "docs"]).status.code(), Some(0));
    let (stdout, stderr) = refresh(repo);

    assert!(stdout.contains("hunk"), "unexpected stdout: {stdout}");
    assert!(
        stderr.contains("gitchange: notice: auto-captured hunk at sub/c.txt"),
        "the claim is reported where it landed: {stderr}"
    );
    assert_eq!(owners(repo, "sub/c.txt"), vec![Some("docs".to_owned())]);
    assert_eq!(
        owners(repo, "a.txt"),
        vec![Some("feature".to_owned()), Some("docs".to_owned())],
        "the recorded hunk keeps its owner; the pending one is claimed"
    );
}

#[test]
fn switch_unassigned_then_an_edit_and_a_refresh_leaves_the_hunk_unassigned() {
    // The other half of the claim model: capture off is off for the
    // deliberate refresh too, so the escalation ladder's opener stays inert.
    let dir = capturing_repo(Some("feature"));
    let repo = dir.path();
    assert_eq!(
        gitchange(repo, &["switch", "unassigned"]).status.code(),
        Some(0)
    );
    write(repo, "a.txt", "three\n");

    let (stdout, stderr) = refresh(repo);

    assert_eq!(stdout, "");
    assert_eq!(stderr, "");
    assert_eq!(owners(repo, "a.txt"), vec![None]);
}
