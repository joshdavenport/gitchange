//! What a successful commit reports and the shapes it must still handle:
//! git's own short id, `--amend`, the unassigned tree, an unborn branch.

use crate::support::RepoFixture;
use gitchange_core::{CommitMessage, CommitOptions, CommitOutcome, HunkStage, commit_echo};

use super::helpers::{commit, numbered_lines, owners, repo, stages, state_json, text};

/// The outcome's abbreviation is git's own, not a fixed-length prefix:
/// `core.abbrev` must move it, and it must match the abbreviation the
/// snapshot hands the Commits panel. A hand-rolled `oid[..7]` passes
/// neither, and would name one commit two ways on the same screen.
#[test]
fn committed_short_id_is_gits_own_abbreviation() {
    let fixture = RepoFixture::new();
    let head = numbered_lines(20);
    fixture.write("a.txt", &text(&head)).commit_all("init");
    {
        let git = git2::Repository::open(fixture.path()).unwrap();
        git.config().unwrap().set_i32("core.abbrev", 12).unwrap();
    }
    let repo = repo(&fixture);

    let mut worktree = head.clone();
    worktree[9] = "ten".into();
    fixture.write("a.txt", &text(&worktree)).stage("a.txt");
    repo.refresh().unwrap();

    let CommitOutcome::Committed { oid, short_id } = commit(&repo, None, "ten") else {
        panic!("expected a commit");
    };
    assert_eq!(short_id.len(), 12, "core.abbrev=12 should widen it");
    assert!(
        oid.starts_with(&short_id),
        "{short_id} must abbreviate {oid}"
    );
    assert_eq!(
        repo.refresh().unwrap().snapshot.recent_commits[0].short_id,
        short_id,
        "the echo and the Commits panel must name the commit identically"
    );
}

#[test]
fn amend_reuses_the_temp_index_path() {
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
    assert_eq!(fixture.commit_count(), 2);

    repo.create_changelist("two").unwrap();
    repo.switch(Some("two")).unwrap();
    worktree[9] = "ten-two".into();
    fixture.write("a.txt", &text(&worktree));
    repo.refresh().unwrap();

    // A third changelist's staged hunk (far enough from "two"'s that the
    // index diff keeps them separate atoms) proves the amend goes through
    // the same temp-index path: staged content outside the payload must
    // stay out of the amended tip and stay staged after it.
    repo.create_changelist("three").unwrap();
    repo.switch(Some("three")).unwrap();
    worktree[29] = "thirty-three".into();
    fixture.write("a.txt", &text(&worktree)).stage("a.txt");
    repo.refresh().unwrap();

    let outcome = repo
        .commit(
            Some("two"),
            CommitMessage::Given("first and second"),
            &CommitOptions {
                amend: true,
                ..CommitOptions::default()
            },
            None,
        )
        .unwrap();
    assert!(matches!(outcome, CommitOutcome::Committed { .. }));

    // Still two commits: the tip was replaced, holding "one"'s and
    // "two"'s edits — never "three"'s.
    assert_eq!(fixture.commit_count(), 2);
    assert_eq!(fixture.head_message(), "first and second\n");
    let mut amended = head.clone();
    amended[4] = "five-one".into();
    amended[9] = "ten-two".into();
    assert_eq!(
        fixture.head_bytes("a.txt"),
        Some(text(&amended).into_bytes())
    );

    let refreshed = repo.refresh().unwrap();
    let snapshot = &refreshed.snapshot;
    assert!(refreshed.advisories.is_empty());
    assert_eq!(owners(snapshot, "a.txt"), vec![Some("three".into())]);
    assert_eq!(stages(snapshot, "a.txt"), vec![HunkStage::Staged]);
}

#[test]
fn unassigned_commits_like_any_changelist() {
    // No changelists exist: the whole dirty tree is unassigned. The
    // commit still writes the last-commit record (ADR 0004 §Aftermath),
    // so a state file does grow here — the amend that follows needs the
    // record to tell its own commit from another actor's, and the file
    // carries nothing else it was not asked to hold.
    let fixture = RepoFixture::new();
    let head = numbered_lines(10);
    fixture.write("a.txt", &text(&head)).commit_all("init");
    let repo = repo(&fixture);

    let mut worktree = head.clone();
    worktree[4] = "five-edited".into();
    fixture.write("a.txt", &text(&worktree)).stage("a.txt");

    let outcome = commit(&repo, None, "unassigned edit");
    assert!(matches!(outcome, CommitOutcome::Committed { .. }));
    assert_eq!(
        fixture.head_bytes("a.txt"),
        Some(text(&worktree).into_bytes())
    );
    let state = state_json(&fixture);
    assert_eq!(state["last_commit"]["changelist"], "unassigned");
    assert_eq!(
        (state["changelists"].as_array(), state["records"].as_array()),
        (Some(&Vec::new()), Some(&Vec::new())),
        "committing an unmanaged tree invents no changelist and claims no hunk"
    );
}

/// The no-message mode (#151): with `--no-edit` core omits `-F` entirely
/// and forwards the flag, so HEAD's message survives the amend
/// byte-identical. Passing both would deliver the file's message instead,
/// which is what makes the surviving message proof of the omission.
#[test]
fn amend_with_no_edit_keeps_heads_message() {
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
    commit(&repo, Some("one"), "the message to keep");

    worktree[14] = "fifteen-one".into();
    fixture.write("a.txt", &text(&worktree)).stage("a.txt");
    repo.refresh().unwrap();
    let outcome = repo
        .commit(
            Some("one"),
            CommitMessage::Kept,
            &CommitOptions {
                amend: true,
                ..CommitOptions::default()
            },
            None,
        )
        .unwrap();

    assert!(matches!(outcome, CommitOutcome::Committed { .. }));
    assert_eq!(fixture.head_message(), "the message to keep\n");
    assert_eq!(fixture.commit_count(), 2, "the tip was replaced, not added");
    // And the transparency echo names the flag, since git was given it
    // (ADR 0007): a mode that changes the command must be visible in the
    // line that claims to be the command.
    assert!(
        commit_echo(
            &CommitOptions {
                amend: true,
                ..CommitOptions::default()
            },
            CommitMessage::Kept,
            Some("one"),
            &repo.commit_payload(Some("one")).unwrap(),
        )
        .contains("--no-edit")
    );
    assert_eq!(
        fixture.head_bytes("a.txt"),
        Some(text(&worktree).into_bytes()),
        "the amended tip carries the second hunk too"
    );
}

#[test]
fn unborn_branch_initial_commit_works() {
    let fixture = RepoFixture::new();
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();
    repo.switch(Some("one")).unwrap();

    fixture.write("a.txt", "alpha\nbeta\n").stage("a.txt");
    repo.refresh().unwrap();

    let outcome = commit(&repo, Some("one"), "initial");
    assert!(matches!(outcome, CommitOutcome::Committed { .. }));
    assert_eq!(fixture.commit_count(), 1);
    assert_eq!(fixture.head_bytes("a.txt"), Some(b"alpha\nbeta\n".to_vec()));
    assert_eq!(state_json(&fixture)["baseline_head"], fixture.head_oid());
    let refreshed = repo.refresh().unwrap();
    assert!(refreshed.advisories.is_empty());
}
