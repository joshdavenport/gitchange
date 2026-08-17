//! The commit payload: what `commit_payload` snapshots at confirm time,
//! its staged and stale counts, the drift check that sends a stale
//! confirmation back, and the `align`/`stage_all` ops that shape it.

use crate::support::RepoFixture;
use gitchange_core::{Advisory, CommitOptions, CommitOutcome, Error, HunkStage};

use super::helpers::{commit, numbered_lines, owners, repo, stages, text};

#[test]
fn payload_drift_returns_to_the_confirm_step() {
    let fixture = RepoFixture::new();
    let head = numbered_lines(20);
    fixture.write("a.txt", &text(&head)).commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();
    repo.switch(Some("one")).unwrap();

    let mut worktree = head.clone();
    worktree[9] = "ten-v1".into();
    fixture.write("a.txt", &text(&worktree)).stage("a.txt");
    let confirmed = repo.commit_payload(Some("one")).unwrap();

    // The staged payload changes between confirm and commit.
    worktree[9] = "ten-v2".into();
    fixture.write("a.txt", &text(&worktree)).stage("a.txt");

    let outcome = repo
        .commit(
            Some("one"),
            "one: ten",
            &CommitOptions::default(),
            Some(&confirmed),
        )
        .unwrap();
    let CommitOutcome::Drifted { payload } = outcome else {
        panic!("expected Drifted, got {outcome:?}");
    };
    assert_ne!(payload, confirmed);
    assert_eq!(fixture.commit_count(), 1, "nothing was committed");

    // Re-confirming with the fresh payload commits.
    let outcome = repo
        .commit(
            Some("one"),
            "one: ten",
            &CommitOptions::default(),
            Some(&payload),
        )
        .unwrap();
    assert!(matches!(outcome, CommitOutcome::Committed { .. }));
    assert_eq!(fixture.commit_count(), 2);
}

#[test]
fn zero_staged_hunks_is_an_error_not_a_commit() {
    let fixture = RepoFixture::new();
    let head = numbered_lines(20);
    fixture.write("a.txt", &text(&head)).commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();

    // An owned but unstaged hunk: nothing to commit.
    let mut worktree = head.clone();
    worktree[9] = "ten-unstaged".into();
    fixture.write("a.txt", &text(&worktree));
    repo.refresh().unwrap();

    let err = repo
        .commit(Some("one"), "one: ten", &CommitOptions::default(), None)
        .unwrap_err();
    assert!(matches!(err, Error::NothingStaged));
    assert_eq!(fixture.commit_count(), 1);

    let err = repo
        .commit(Some("missing"), "?", &CommitOptions::default(), None)
        .unwrap_err();
    assert!(matches!(err, Error::UnknownChangelist { .. }));
}

#[test]
fn payload_counts_cover_both_stale_flavours() {
    let fixture = RepoFixture::new();
    let head = numbered_lines(20);
    fixture
        .write("a.txt", &text(&head))
        .write("b.txt", &text(&head))
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();
    repo.switch(Some("one")).unwrap();

    // a.txt: one cleanly staged hunk and one edited-flavour ◑ hunk.
    let mut worktree = head.clone();
    worktree[4] = "five-staged".into();
    worktree[14] = "fifteen-staged".into();
    fixture.write("a.txt", &text(&worktree)).stage("a.txt");
    worktree[14] = "fifteen-final".into();
    fixture.write("a.txt", &text(&worktree));

    // b.txt: an index-only ◑ hunk (staged then reverted in the worktree).
    let mut staged_b = head.clone();
    staged_b[9] = "ten-staged".into();
    fixture
        .write("b.txt", &text(&staged_b))
        .stage("b.txt")
        .write("b.txt", &text(&head));

    let payload = repo.commit_payload(Some("one")).unwrap();
    assert_eq!(payload.file_count(), 2);
    assert_eq!(payload.staged_hunks(), 1);
    assert_eq!(payload.stale_hunks(), 2, "both ◑ flavours count");
    assert!(!payload.is_empty());
}

#[test]
fn align_sets_index_to_worktree_for_the_changelists_stale_hunks() {
    let fixture = RepoFixture::new();
    let head = numbered_lines(20);
    fixture
        .write("a.txt", &text(&head))
        .write("b.txt", &text(&head))
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();
    repo.switch(Some("one")).unwrap();

    // a.txt: edited-flavour ◑ — align re-stages the edited version.
    let mut worktree_a = head.clone();
    worktree_a[4] = "five-staged".into();
    fixture.write("a.txt", &text(&worktree_a)).stage("a.txt");
    worktree_a[4] = "five-final".into();
    fixture.write("a.txt", &text(&worktree_a));

    // b.txt: index-only ◑ — align discards the staged content.
    let mut staged_b = head.clone();
    staged_b[9] = "ten-staged".into();
    fixture
        .write("b.txt", &text(&staged_b))
        .stage("b.txt")
        .write("b.txt", &text(&head));

    let advisories = repo.align(Some("one")).unwrap().advisories;
    assert_eq!(advisories, Vec::<Advisory>::new());

    assert_eq!(fixture.index_content("a.txt"), Some(text(&worktree_a)));
    assert_eq!(fixture.index_content("b.txt"), Some(text(&head)));

    let snapshot = repo.refresh().unwrap();
    assert_eq!(stages(&snapshot, "a.txt"), vec![HunkStage::Staged]);
    assert!(
        !snapshot.files.iter().any(|file| file.path == "b.txt"),
        "the discarded index-only hunk leaves no diff at all"
    );
}

#[test]
fn stage_all_stages_only_the_changelists_unstaged_hunks() {
    let fixture = RepoFixture::new();
    let head = numbered_lines(20);
    fixture
        .write("a.txt", &text(&head))
        .write("b.txt", &text(&head))
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();
    repo.switch(Some("one")).unwrap();

    // Both edits land in the active changelist; b.txt's hunk is then
    // released to unassigned so stage_all must leave it behind. The
    // release happens under capture-off (ADR 0016): with 'one' still
    // active, stage_all's own refresh would recapture it.
    let mut worktree_a = head.clone();
    worktree_a[4] = "five-edited".into();
    fixture.write("a.txt", &text(&worktree_a));
    let mut worktree_b = head.clone();
    worktree_b[9] = "ten-edited".into();
    fixture.write("b.txt", &text(&worktree_b));
    let snapshot = repo.refresh().unwrap();
    let b_hunks = snapshot
        .files
        .iter()
        .find(|file| file.path == "b.txt")
        .unwrap()
        .hunks
        .clone();
    repo.switch(None).unwrap();
    repo.assign_hunks("b.txt", &b_hunks, None).unwrap();

    let outcome = repo.stage_all(Some("one")).unwrap();
    assert_eq!(outcome.echo.as_deref(), Some("staged 1 hunk — 'one'"));
    assert_eq!(outcome.advisories, Vec::<Advisory>::new());

    assert_eq!(fixture.index_content("a.txt"), Some(text(&worktree_a)));
    assert_eq!(
        fixture.index_content("b.txt"),
        Some(text(&head)),
        "unassigned hunk left unstaged"
    );

    let snapshot = repo.refresh().unwrap();
    assert_eq!(stages(&snapshot, "a.txt"), vec![HunkStage::Staged]);
    assert_eq!(stages(&snapshot, "b.txt"), vec![HunkStage::Unstaged]);

    assert!(matches!(
        repo.stage_all(Some("missing")),
        Err(Error::UnknownChangelist { .. })
    ));
}

#[test]
fn a_stale_binary_warns_and_commits_the_staged_blob() {
    // ADR 0004's warn-and-confirm applies unchanged: the `◑` binary
    // counts as a stale hunk and the commit carries the staged blob,
    // not the worktree's newer bytes.
    let fixture = RepoFixture::new();
    fixture
        .write_bytes("logo.png", &[0u8, 1])
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("art").unwrap();
    repo.switch(Some("art")).unwrap();
    fixture
        .write_bytes("logo.png", &[0u8, 2, 2])
        .stage("logo.png")
        .write_bytes("logo.png", &[0u8, 3, 3, 3]);

    let payload = repo.commit_payload(Some("art")).unwrap();
    assert_eq!(payload.staged_hunks(), 0);
    assert_eq!(payload.stale_hunks(), 1, "the ◑ binary is the warn count");

    commit(&repo, Some("art"), "stale binary");
    assert_eq!(fixture.head_bytes("logo.png").unwrap(), vec![0u8, 2, 2]);

    // The retained record was rewritten against the new HEAD: the
    // residual worktree change keeps its membership.
    let snapshot = repo.refresh().unwrap();
    assert_eq!(owners(&snapshot, "logo.png"), vec![Some("art".into())]);
    assert_eq!(stages(&snapshot, "logo.png"), vec![HunkStage::Unstaged]);
}

#[test]
fn a_restaged_binary_blob_drifts_the_confirmation() {
    // The payload's whole-file OID participates in drift equality
    // (ADR 0004's freshness guard): re-staging new bytes between
    // confirm and commit returns Drifted, nothing committed.
    let fixture = RepoFixture::new();
    fixture
        .write_bytes("logo.png", &[0u8, 1])
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("art").unwrap();
    repo.switch(Some("art")).unwrap();
    fixture
        .write_bytes("logo.png", &[0u8, 2, 2])
        .stage("logo.png");

    let confirmed = repo.commit_payload(Some("art")).unwrap();
    fixture
        .write_bytes("logo.png", &[0u8, 4, 4, 4, 4])
        .stage("logo.png");

    let outcome = repo
        .commit(
            Some("art"),
            "drifted",
            &CommitOptions::default(),
            Some(&confirmed),
        )
        .unwrap();
    assert!(matches!(outcome, CommitOutcome::Drifted { .. }));
    assert_eq!(fixture.commit_count(), 1, "nothing was committed");
}

#[test]
#[cfg(unix)]
fn a_staged_zero_hunk_change_is_in_the_payload() {
    // The hole issue #98 opened with: with both files staged by raw
    // `git add`, `commit_payload` returned `files: []` and gitchange
    // could not commit them at all. They carry a whole-file hunk now
    // (ADR 0017), so the payload names them.
    let fixture = RepoFixture::new();
    fixture
        .write("tool.sh", "#!/bin/sh\n")
        .commit_all("init")
        .set_exec("tool.sh")
        .stage("tool.sh")
        .write_bytes("empty.txt", b"")
        .stage("empty.txt");
    let repo = repo(&fixture);
    repo.create_changelist("work").unwrap();
    repo.switch(Some("work")).unwrap();
    repo.refresh().unwrap();

    let payload = repo.commit_payload(Some("work")).unwrap();
    let paths: Vec<&str> = payload
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    assert_eq!(paths, vec!["empty.txt", "tool.sh"]);
    assert_eq!(payload.staged_hunks(), 2);
    assert_eq!(payload.stale_hunks(), 0);

    commit(&repo, Some("work"), "zero-hunk changes");
    assert_eq!(fixture.head_mode("tool.sh"), Some(0o100755));
    assert_eq!(fixture.head_bytes("empty.txt"), Some(Vec::new()));
    assert!(
        repo.refresh().unwrap().files.is_empty(),
        "nothing left over"
    );
}

#[test]
#[cfg(unix)]
fn a_staged_mode_flip_commits_alone_beside_an_unstaged_edit() {
    // Issue #101's mirror corner through commit: the flip is staged, the
    // worktree edit is not. The payload carries the mode hunk and no
    // content, so the commit lands the mode over HEAD's blob — before
    // ADR 0017's amendment the path fell out of the payload entirely.
    let fixture = RepoFixture::new();
    fixture
        .write("tool.sh", "one\n")
        .commit_all("init")
        .set_exec("tool.sh")
        .stage("tool.sh")
        .write("tool.sh", "two\n");
    let repo = repo(&fixture);

    let payload = repo.commit_payload(None).unwrap();
    assert_eq!(payload.file_count(), 1);
    assert_eq!(payload.staged_hunks(), 1, "the mode hunk alone");
    assert_eq!(payload.stale_hunks(), 0);

    commit(&repo, None, "make it executable");
    assert_eq!(fixture.head_mode("tool.sh"), Some(0o100755));
    assert_eq!(
        fixture.head_bytes("tool.sh"),
        Some(b"one\n".to_vec()),
        "the unstaged edit stayed out"
    );

    // What is left is the edit, unstaged, and no mode hunk: the modes
    // agree again.
    let file = &repo.refresh().unwrap().files[0];
    assert_eq!(file.total_hunks(), 1);
    assert_eq!(file.hunks[0].stage, HunkStage::Unstaged);
    assert!(!file.hunks[0].is_mode_change());
}

#[test]
#[cfg(unix)]
fn a_staged_mode_flip_and_edit_commit_together() {
    // The mode hunk beside a committed content hunk: one temp-index
    // entry carries both, the staged lines and the staged mode.
    let fixture = RepoFixture::new();
    fixture
        .write("tool.sh", "one\n")
        .commit_all("init")
        .write("tool.sh", "two\n")
        .set_exec("tool.sh")
        .stage("tool.sh");
    let repo = repo(&fixture);

    let payload = repo.commit_payload(None).unwrap();
    assert_eq!(payload.staged_hunks(), 2, "the mode hunk and the edit");

    commit(&repo, None, "edit and chmod");
    assert_eq!(fixture.head_mode("tool.sh"), Some(0o100755));
    assert_eq!(fixture.head_bytes("tool.sh"), Some(b"two\n".to_vec()));
    assert!(
        repo.refresh().unwrap().files.is_empty(),
        "nothing left over"
    );
}

#[test]
#[cfg(unix)]
fn a_mode_flip_reverted_since_confirmation_drifts_the_commit() {
    // The mode hunk's freshness guard (ADR 0004): the payload names the
    // staged mode, so a flip undone between confirm and commit refuses
    // rather than committing a mode the user never confirmed.
    let fixture = RepoFixture::new();
    fixture
        .write("tool.sh", "one\n")
        .commit_all("init")
        .set_exec("tool.sh")
        .stage("tool.sh")
        .write("tool.sh", "two\n");
    let repo = repo(&fixture);

    let confirmed = repo.commit_payload(None).unwrap();
    fixture.clear_exec("tool.sh").stage("tool.sh");

    let outcome = repo
        .commit(None, "drifted", &CommitOptions::default(), Some(&confirmed))
        .unwrap();
    assert!(matches!(outcome, CommitOutcome::Drifted { .. }));
    assert_eq!(fixture.commit_count(), 1, "nothing was committed");
}

#[test]
#[cfg(unix)]
fn a_committed_type_change_writes_the_symlink_tree_entry() {
    // End-to-end for the type-change shape (#100): a staged file→symlink
    // swap commits as a symlink — HEAD's tree entry carries link mode
    // and the target string, not a regular file that happens to hold it.
    let fixture = RepoFixture::new();
    fixture
        .write("thing", "content\n")
        .write("target.txt", "elsewhere\n")
        .commit_all("init")
        .remove("thing")
        .symlink("thing", "target.txt");

    let repo = repo(&fixture);
    repo.stage_owned_hunks("thing", None).unwrap();

    commit(&repo, None, "swap to symlink");
    assert_eq!(fixture.head_mode("thing"), Some(0o120000), "link mode");
    assert_eq!(fixture.head_bytes("thing"), Some(b"target.txt".to_vec()));
    assert!(
        repo.refresh().unwrap().files.is_empty(),
        "nothing left over"
    );
}
