//! ADR 0012's record aftermath on an own commit: consumed records
//! removed, surviving same-file records commuted by the committed deltas,
//! retained `◑` records rewritten against the new HEAD, and the baseline
//! stamped in the same locked update so the #39 guard never fires.

use crate::support::RepoFixture;

use super::helpers::{commit, dormant_owners, numbered_lines, owners, repo, state_json, text};

#[test]
fn committing_one_changelist_commutes_same_file_records() {
    // The own-commit half of ADR 0012: `commit()` shifts surviving
    // same-file records by the committed deltas, so a later anchor-broken
    // edit still inherits via tier-2 — the external-commit flavour of
    // this exact scenario goes dormant instead (tests/core/head_moves.rs).
    let fixture = RepoFixture::new();
    let head = numbered_lines(60);
    fixture.write("a.txt", &text(&head)).commit_all("init");
    let repo = repo(&fixture);

    // Changelist "two": replace lines 20..=31 with one line (delta -11).
    repo.create_changelist("two").unwrap();
    repo.switch(Some("two")).unwrap();
    let mut worktree = head.clone();
    worktree.splice(19..31, ["twenty!".into()]);
    fixture.write("a.txt", &text(&worktree));
    repo.refresh().unwrap();

    // Changelist "one": edit original line 40, record old range [37, 44).
    repo.create_changelist("one").unwrap();
    repo.switch(Some("one")).unwrap();
    worktree[28] = "forty-v1".into();
    fixture.write("a.txt", &text(&worktree));
    let snapshot = repo.refresh().unwrap().snapshot;
    assert_eq!(
        owners(&snapshot, "a.txt"),
        vec![Some("two".into()), Some("one".into())]
    );

    // Stage and commit only "two"'s hunk through gitchange.
    let file = &snapshot.files[0];
    repo.stage_hunk("a.txt", &file.hunks[0]).unwrap();
    // A third changelist is active, so re-attachment can't hide behind
    // active-capture landing on "one" by luck.
    repo.create_changelist("three").unwrap();
    repo.switch(Some("three")).unwrap();
    commit(&repo, Some("two"), "two: twenty");

    // Keep editing "one"'s hunk: anchor broken, tier-2 must inherit via
    // the commuted record.
    worktree[28] = "forty-v2".into();
    fixture.write("a.txt", &text(&worktree));

    let refreshed = repo.refresh().unwrap();
    let snapshot = &refreshed.snapshot;
    assert_eq!(
        owners(snapshot, "a.txt"),
        vec![Some("one".into())],
        "the commuted record keeps the shifted hunk in its changelist"
    );
    assert!(
        refreshed.advisories.is_empty(),
        "no dormancy notice on an own commit"
    );
    assert!(dormant_owners(&fixture).is_empty());
}

#[test]
fn a_residual_stale_hunk_reattaches_after_an_own_commit() {
    // Committing a ◑ hunk as-is leaves a residual worktree diff; the
    // retained record is rewritten against the new HEAD so the residual
    // re-attaches to its changelist — the external flavour goes dormant
    // (tests/core/head_moves.rs pins that contrast).
    let fixture = RepoFixture::new();
    let head = numbered_lines(20);
    fixture.write("a.txt", &text(&head)).commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();
    repo.switch(Some("one")).unwrap();

    let mut worktree = head.clone();
    worktree[9] = "ten-staged".into();
    fixture.write("a.txt", &text(&worktree)).stage("a.txt");
    // Edit further: the hunk is now staged-stale (◑).
    worktree[9] = "ten-final".into();
    fixture.write("a.txt", &text(&worktree));
    repo.refresh().unwrap();

    repo.create_changelist("two").unwrap();
    repo.switch(Some("two")).unwrap();
    commit(&repo, Some("one"), "one: ten (staged version)");

    let mut committed = head.clone();
    committed[9] = "ten-staged".into();
    assert_eq!(
        fixture.head_bytes("a.txt"),
        Some(text(&committed).into_bytes())
    );

    // Residual hunk: committed "ten-staged" ↔ worktree "ten-final".
    let refreshed = repo.refresh().unwrap();
    let snapshot = &refreshed.snapshot;
    assert_eq!(
        owners(snapshot, "a.txt"),
        vec![Some("one".into())],
        "the rewritten record re-attaches the residual to its changelist"
    );
    assert!(refreshed.advisories.is_empty());
    assert!(dormant_owners(&fixture).is_empty());
}

#[test]
fn a_residual_stale_hunk_reattaches_when_the_payload_shifts_it() {
    // The hardest aftermath case (ADR 0012's "shifting alone is not
    // enough"): the same commit's other hunk shrinks the file above the
    // residual, so the retained record needs both new coordinates and a
    // re-derived anchor whose old side is the committed content.
    let fixture = RepoFixture::new();
    let head = numbered_lines(60);
    fixture.write("a.txt", &text(&head)).commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();
    repo.switch(Some("one")).unwrap();

    // Changelist "one", two hunks: replace lines 10..=21 with one line
    // (delta -11), and edit original line 40 — both staged.
    let mut worktree = head.clone();
    worktree.splice(9..21, ["ten!".into()]);
    worktree[28] = "forty-staged".into();
    fixture.write("a.txt", &text(&worktree)).stage("a.txt");
    // Edit the second hunk further: staged-stale (◑).
    worktree[28] = "forty-final".into();
    fixture.write("a.txt", &text(&worktree));
    repo.refresh().unwrap();

    repo.create_changelist("two").unwrap();
    repo.switch(Some("two")).unwrap();
    commit(&repo, Some("one"), "one: both hunks, staged versions");

    let mut committed = head.clone();
    committed.splice(9..21, ["ten!".into()]);
    committed[28] = "forty-staged".into();
    assert_eq!(
        fixture.head_bytes("a.txt"),
        Some(text(&committed).into_bytes())
    );

    let refreshed = repo.refresh().unwrap();
    let snapshot = &refreshed.snapshot;
    assert_eq!(
        owners(snapshot, "a.txt"),
        vec![Some("one".into())],
        "the shifted residual still re-attaches to its changelist"
    );
    assert!(refreshed.advisories.is_empty());
    // The consumed record is removed; the retained one stays live.
    assert!(dormant_owners(&fixture).is_empty());
}

#[test]
fn commit_stamps_the_baseline_in_the_same_update() {
    let fixture = RepoFixture::new();
    let head = numbered_lines(20);
    fixture.write("a.txt", &text(&head)).commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();
    repo.switch(Some("one")).unwrap();

    let mut worktree = head.clone();
    worktree[9] = "ten-one".into();
    fixture.write("a.txt", &text(&worktree)).stage("a.txt");
    repo.refresh().unwrap();

    commit(&repo, Some("one"), "one: ten");
    // Before any follow-up refresh, the baseline already names the new
    // HEAD — the #39 guard can never arm on an own commit.
    assert_eq!(state_json(&fixture)["baseline_head"], fixture.head_oid());
}
