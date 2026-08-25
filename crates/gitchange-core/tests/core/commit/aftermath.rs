//! What one locked update writes after an own commit: ADR 0012's record
//! aftermath — consumed records removed, surviving same-file records
//! commuted by the committed deltas, retained `◑` records rewritten
//! against the new HEAD, the baseline stamped so the #39 guard never
//! fires — and ADR 0004's last-commit record, the reference an amend's
//! foreign-head guard reads back.

use std::fs;

use crate::support::RepoFixture;

use super::helpers::{
    amend, commit, dormant_owners, numbered_lines, owners, repo, state_json, text,
};

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

#[test]
fn every_commit_records_itself_as_the_last_gitchange_commit() {
    // ADR 0004 §Aftermath: one record for the repo, written in the same
    // locked update as the membership bookkeeping. Both the persisted
    // shape and the reading the amend guard does (#151) are asserted —
    // a record written under the wrong name reads back as no record at
    // all, and only the shape says which of the two went wrong.
    let fixture = RepoFixture::new();
    let head = numbered_lines(20);
    fixture.write("a.txt", &text(&head)).commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();
    repo.switch(Some("one")).unwrap();

    let mut worktree = head.clone();
    worktree[4] = "five-one".into();
    fixture.write("a.txt", &text(&worktree)).stage("a.txt");
    repo.refresh().unwrap();

    commit(&repo, Some("one"), "one: five");

    assert_eq!(
        state_json(&fixture)["last_commit"],
        serde_json::json!({ "oid": fixture.head_oid(), "changelist": "one" }),
        "the oid is the new HEAD and the name is the scope committed"
    );
    assert!(repo.head_is_own_last_commit(Some("one")).unwrap());
    assert!(
        !repo.head_is_own_last_commit(None).unwrap(),
        "the record names one scope: unassigned did not commit this"
    );
}

#[test]
fn an_unassigned_scope_commit_records_unassigned() {
    // Entailed by the every-commit-records rule (ADR 0004 §Aftermath):
    // unassigned is a scope a commit can take, so the record spells it
    // with its label rather than leaving the name out — the amend guard
    // compares names, and an absent one would read as no commit.
    let fixture = RepoFixture::new();
    let head = numbered_lines(10);
    fixture.write("a.txt", &text(&head)).commit_all("init");
    let repo = repo(&fixture);

    let mut worktree = head.clone();
    worktree[4] = "five-edited".into();
    fixture.write("a.txt", &text(&worktree)).stage("a.txt");

    commit(&repo, None, "unassigned edit");

    assert_eq!(
        state_json(&fixture)["last_commit"]["changelist"],
        "unassigned"
    );
    assert!(repo.head_is_own_last_commit(None).unwrap());
}

#[test]
fn amend_re_records_the_last_commit() {
    // The loop ADR 0004 §Amend leaves unguarded: commit, notice a miss,
    // amend — and amend again. Each amend replaces the record, so HEAD
    // stays the changelist's own last commit however many times it moves.
    let fixture = RepoFixture::new();
    let head = numbered_lines(40);
    fixture.write("a.txt", &text(&head)).commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();
    repo.switch(Some("one")).unwrap();

    let mut worktree = head.clone();
    worktree[4] = "five-one".into();
    fixture.write("a.txt", &text(&worktree)).stage("a.txt");
    repo.refresh().unwrap();
    commit(&repo, Some("one"), "first");
    let first = fixture.head_oid();

    worktree[19] = "twenty-one".into();
    fixture.write("a.txt", &text(&worktree)).stage("a.txt");
    repo.refresh().unwrap();
    amend(&repo, Some("one"), "first, and twenty");
    assert_ne!(fixture.head_oid(), first, "the tip was replaced");
    assert!(repo.head_is_own_last_commit(Some("one")).unwrap());

    worktree[29] = "thirty-one".into();
    fixture.write("a.txt", &text(&worktree)).stage("a.txt");
    repo.refresh().unwrap();
    amend(&repo, Some("one"), "first, twenty, and thirty");
    assert_eq!(
        state_json(&fixture)["last_commit"]["oid"],
        fixture.head_oid(),
        "amend-after-amend keeps the record current"
    );
    assert!(repo.head_is_own_last_commit(Some("one")).unwrap());
}

#[test]
fn another_scopes_commit_and_an_outside_commit_are_not_the_record() {
    // The three shapes of the hazard the CLI's amend guard fires on
    // (ADR 0004 §Amend): no record at all, another changelist's commit,
    // and a commit made outside gitchange. Commits carry no provenance,
    // so this record is the only thing that tells them from an own one.
    let fixture = RepoFixture::new();
    let head = numbered_lines(40);
    fixture.write("a.txt", &text(&head)).commit_all("init");
    let repo = repo(&fixture);
    assert!(
        !repo.head_is_own_last_commit(None).unwrap(),
        "a repo that never committed through gitchange reads as absent, not an error"
    );

    repo.create_changelist("one").unwrap();
    repo.create_changelist("two").unwrap();
    repo.switch(Some("one")).unwrap();
    let mut worktree = head.clone();
    worktree[4] = "five-one".into();
    fixture.write("a.txt", &text(&worktree)).stage("a.txt");
    repo.refresh().unwrap();
    commit(&repo, Some("one"), "one: five");

    repo.switch(Some("two")).unwrap();
    worktree[19] = "twenty-two".into();
    fixture.write("a.txt", &text(&worktree)).stage("a.txt");
    repo.refresh().unwrap();
    commit(&repo, Some("two"), "two: twenty");
    assert!(repo.head_is_own_last_commit(Some("two")).unwrap());
    assert!(
        !repo.head_is_own_last_commit(Some("one")).unwrap(),
        "one's commit is no longer HEAD, and amending it would fold two's in"
    );

    worktree[34] = "thirty-five".into();
    fixture
        .write("a.txt", &text(&worktree))
        .commit_all("outside");
    assert!(
        !repo.head_is_own_last_commit(Some("two")).unwrap(),
        "an external commit moved HEAD past the record"
    );
}

#[test]
fn the_record_survives_the_state_files_other_writers() {
    // The record shares one document with the membership records and the
    // active marker (ADR 0002), and every later write loads and saves the
    // whole of it — so a field the round-trip dropped would leave the
    // amend guard reading a repo that had never committed.
    let fixture = RepoFixture::new();
    let head = numbered_lines(40);
    fixture.write("a.txt", &text(&head)).commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();
    repo.switch(Some("one")).unwrap();

    let mut worktree = head.clone();
    worktree[4] = "five-one".into();
    fixture.write("a.txt", &text(&worktree)).stage("a.txt");
    // A second, unstaged edit stays behind as a live record, so the
    // record coexists with membership rather than standing alone.
    worktree[29] = "thirty-one".into();
    fixture.write("a.txt", &text(&worktree));
    repo.refresh().unwrap();
    commit(&repo, Some("one"), "one: five");

    repo.refresh().unwrap();
    repo.create_changelist("two").unwrap();
    repo.switch(Some("two")).unwrap();
    repo.rename_changelist("two", "three").unwrap();

    assert!(
        !state_json(&fixture)["records"]
            .as_array()
            .expect("records")
            .is_empty(),
        "the surviving edit's record is still there to coexist with"
    );
    assert!(repo.head_is_own_last_commit(Some("one")).unwrap());
}

#[test]
fn a_state_file_written_before_the_record_reads_as_absent() {
    // Schema 1 predates the record, so files written by earlier builds
    // carry no `last_commit` key at all (ADR 0002's serde-default rule,
    // as for `records` and `baseline_head`). Such a file must load and
    // read as "no commit gitchange knows of" — the honest answer — rather
    // than failing the read or claiming a commit it has never seen.
    let fixture = RepoFixture::new();
    let head = numbered_lines(10);
    fixture.write("a.txt", &text(&head)).commit_all("init");
    let dir = fixture.path().join(".git/gitchange");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("state.json"),
        r#"{ "version": 1, "active": "one", "changelists": [{ "name": "one" }] }"#,
    )
    .unwrap();
    let repo = repo(&fixture);

    assert!(
        !repo.head_is_own_last_commit(Some("one")).unwrap(),
        "an absent key is an absent record, not an error"
    );

    let mut worktree = head.clone();
    worktree[4] = "five-one".into();
    fixture.write("a.txt", &text(&worktree)).stage("a.txt");
    repo.refresh().unwrap();
    commit(&repo, Some("one"), "one: five");
    assert!(
        repo.head_is_own_last_commit(Some("one")).unwrap(),
        "and the next commit fills it in"
    );
}
