//! ADR 0004's temporary index: what a commit writes through it — the
//! changelist's staged hunks and nothing else, live index and worktree
//! untouched — what the hooks see of it, and the two ways it refuses:
//! a rejecting hook and a refused apply.

use std::fs;

use crate::support::RepoFixture;
use gitchange_core::{CommitOptions, CommitOutcome, Error, HunkStage};

// Gated with its only user below, so Windows builds this file clean.
#[cfg(unix)]
use gitchange_core::ApplySite;

use super::helpers::{
    commit, dormant_owners, numbered_lines, owners, repo, stages, state_json, text,
};

#[test]
fn commit_writes_only_the_changelists_staged_hunks() {
    let fixture = RepoFixture::new();
    let head = numbered_lines(30);
    fixture.write("a.txt", &text(&head)).commit_all("init");
    let repo = repo(&fixture);

    // Changelist "one": edit line 10.
    repo.create_changelist("one").unwrap();
    repo.switch(Some("one")).unwrap();
    let mut worktree = head.clone();
    worktree[9] = "ten-one".into();
    fixture.write("a.txt", &text(&worktree));
    repo.refresh().unwrap();

    // Changelist "two": edit line 20.
    repo.create_changelist("two").unwrap();
    repo.switch(Some("two")).unwrap();
    worktree[19] = "twenty-two".into();
    fixture.write("a.txt", &text(&worktree));
    repo.refresh().unwrap();

    // Both hunks staged; the payload filter, not staging, must scope the
    // commit to "one".
    fixture.stage("a.txt");
    let outcome = commit(&repo, Some("one"), "one: ten");
    assert!(matches!(outcome, CommitOutcome::Committed { .. }));

    // HEAD holds only "one"'s hunk.
    let mut committed = head.clone();
    committed[9] = "ten-one".into();
    assert_eq!(
        fixture.head_bytes("a.txt"),
        Some(text(&committed).into_bytes())
    );
    assert_eq!(fixture.head_message(), "one: ten\n");
    // The live index was never touched: it still holds both hunks.
    assert_eq!(fixture.index_content("a.txt"), Some(text(&worktree)));

    let snapshot = repo.refresh().unwrap();
    assert_eq!(
        owners(&snapshot, "a.txt"),
        vec![Some("two".into())],
        "changelist B's hunk stays in B"
    );
    assert_eq!(
        stages(&snapshot, "a.txt"),
        vec![HunkStage::Staged],
        "changelist B's staged hunk stays staged post-commit"
    );
    assert!(snapshot.advisories.is_empty());
    // "one"'s record was fully consumed and removed, not left dormant.
    assert!(dormant_owners(&fixture).is_empty());
    // The emptied changelist is kept until explicitly deleted.
    assert!(snapshot.changelists.iter().any(|cl| cl.name == "one"));
}

#[test]
fn a_hook_sees_the_commits_true_content() {
    let fixture = RepoFixture::new();
    let head = numbered_lines(30);
    fixture.write("a.txt", &text(&head)).commit_all("init");
    let repo = repo(&fixture);

    repo.create_changelist("one").unwrap();
    repo.switch(Some("one")).unwrap();
    let mut worktree = head.clone();
    worktree[9] = "ten-one".into();
    fixture.write("a.txt", &text(&worktree));
    repo.refresh().unwrap();

    repo.create_changelist("two").unwrap();
    repo.switch(Some("two")).unwrap();
    worktree[19] = "twenty-two".into();
    fixture.write("a.txt", &text(&worktree));
    repo.refresh().unwrap();
    fixture.stage("a.txt");

    fixture.with_hook(
        "pre-commit",
        "#!/bin/sh\ngit diff --cached > \"$(git rev-parse --git-dir)/hook-saw.diff\"\n",
    );
    commit(&repo, Some("one"), "one: ten");

    // A missing file means git never executed the hook at all, which is a
    // different failure from a hook that saw the wrong content — and the
    // one a platform without a usable `sh` would produce.
    let saw = fs::read_to_string(fixture.path().join(".git/hook-saw.diff"))
        .expect("the pre-commit hook ran and wrote its capture");
    assert!(saw.contains("ten-one"), "hook sees the payload: {saw}");
    assert!(
        !saw.contains("twenty-two"),
        "hook does not see other changelists' staged hunks: {saw}"
    );
}

/// One staged hunk in changelist "one" — the starting point the three
/// commit-failure tests below share. Shared rather than repeated so
/// "same fixture, one difference" is structural: the `--no-verify`
/// bypass is only evidence about the flag if the rejection it walks past
/// is demonstrably the same rejection. Returns the worktree lines.
fn one_staged_hunk_in_one(fixture: &RepoFixture) -> Vec<String> {
    let head = numbered_lines(20);
    fixture.write("a.txt", &text(&head)).commit_all("init");
    let repo = repo(fixture);
    repo.create_changelist("one").unwrap();
    repo.switch(Some("one")).unwrap();
    let mut worktree = head;
    worktree[9] = "ten-one".into();
    fixture.write("a.txt", &text(&worktree)).stage("a.txt");
    repo.refresh().unwrap();
    worktree
}

#[test]
fn hook_rejection_changes_nothing() {
    let fixture = RepoFixture::new();
    let worktree = one_staged_hunk_in_one(&fixture);
    let repo = repo(&fixture);

    let state_before =
        fs::read_to_string(fixture.path().join(".git/gitchange/state.json")).unwrap();
    let index_before = fixture.index_content("a.txt");

    fixture.with_hook("pre-commit", "#!/bin/sh\necho nope >&2\nexit 1\n");
    let err = repo
        .commit(Some("one"), "one: ten", &CommitOptions::default(), None)
        .unwrap_err();
    match err {
        Error::HookRejected { stderr } => {
            assert!(stderr.contains("nope"), "hook stderr surfaced: {stderr}")
        }
        other => panic!("expected HookRejected, got {other:?}"),
    }

    assert_eq!(fixture.commit_count(), 1, "no commit was created");
    assert_eq!(fixture.index_content("a.txt"), index_before);
    assert_eq!(
        fs::read_to_string(fixture.path().join("a.txt")).unwrap(),
        text(&worktree)
    );
    assert_eq!(
        fs::read_to_string(fixture.path().join(".git/gitchange/state.json")).unwrap(),
        state_before,
        "state file untouched by the failed commit"
    );
    // The rejection lands *after* the temp index and message file are
    // written, so this is where the discard-on-failure guarantee is
    // actually exercised (ADR 0004).
    assert_eq!(
        fixture.state_dir_entries(),
        vec!["state.json"],
        "the temp index and message file are discarded"
    );
}

#[test]
fn no_verify_commits_past_a_rejecting_hook() {
    // The same fixture, hook and payload as the rejection above, with
    // `no_verify` the only difference — the pair is what shows the flag
    // is doing the work. What it produces is an ordinary commit, not a
    // special case: HEAD moves, the payload lands, the record goes.
    let fixture = RepoFixture::new();
    let worktree = one_staged_hunk_in_one(&fixture);
    let repo = repo(&fixture);

    fixture.with_hook("pre-commit", "#!/bin/sh\necho nope >&2\nexit 1\n");
    let outcome = repo
        .commit(
            Some("one"),
            "one: ten",
            &CommitOptions {
                no_verify: true,
                amend: false,
            },
            None,
        )
        .unwrap();
    assert!(matches!(outcome, CommitOutcome::Committed { .. }));

    assert_eq!(fixture.commit_count(), 2, "the commit was created");
    assert_eq!(
        fixture.head_bytes("a.txt"),
        Some(text(&worktree).into_bytes()),
        "the payload landed in HEAD"
    );
    assert_eq!(fixture.head_message(), "one: ten\n");

    let snapshot = repo.refresh().unwrap();
    assert!(
        !snapshot.files.iter().any(|file| file.path == "a.txt"),
        "the payload was fully consumed, so nothing is left changed"
    );
    assert!(snapshot.advisories.is_empty());
    // Fully consumed: the record is removed outright, not left dormant.
    assert!(
        state_json(&fixture)["records"]
            .as_array()
            .unwrap()
            .is_empty(),
        "the consumed record is gone: {}",
        state_json(&fixture)["records"]
    );
}

/// ADR 0004's apply-failure abort: a refused temp-index apply aborts
/// before any commit exists and changes nothing.
///
/// The refusal is forced by an unwritable object store, not by a
/// mismatching payload — that shape is unreachable through the public
/// ops, since the diff applied is computed from HEAD's tree against the
/// live index and applied straight back to that tree (issue #58). The
/// refusal maps to [`Error::ApplyFailed`] at the commit site — the
/// apply tripwire's second sensor (ADR 0003) — carrying libgit2's
/// message verbatim and stating ADR 0004's abort guarantee. Like the
/// staging twin, an unwritable odb is not the tripwire's trigger (#55);
/// what this certifies is the reporting path. The state dir stays clean
/// because the abort precedes the temp index being written, so it is
/// [`hook_rejection_changes_nothing`] that covers discarding files which
/// do exist.
// Unix-only by necessity: no Windows equivalent of a 0o500 directory, so
// the refusal has no way to happen there (ADR 0008).
#[cfg(unix)]
#[test]
fn a_refused_temp_index_apply_aborts_before_any_commit_exists() {
    let fixture = RepoFixture::new();
    let worktree = one_staged_hunk_in_one(&fixture);
    let repo = repo(&fixture);

    let state_before =
        fs::read_to_string(fixture.path().join(".git/gitchange/state.json")).unwrap();
    let index_before = fixture.index_content("a.txt");

    let _odb = fixture.unwritable_odb();
    let err = repo
        .commit(Some("one"), "one: ten", &CommitOptions::default(), None)
        .unwrap_err();
    // The variant pins the refusal to the apply itself: everything
    // else in the commit path the same mode breaks stays `Backend`.
    match &err {
        Error::ApplyFailed { path, detail, site } => {
            assert_eq!(path, "a.txt", "the error names the file");
            assert_eq!(*site, ApplySite::CommitTempIndex, "and the commit site");
            assert!(
                detail.contains("Permission denied"),
                "libgit2's own refusal, verbatim: {detail}"
            );
        }
        other => panic!("expected ApplyFailed, got {other:?}"),
    }
    assert!(
        err.to_string().contains("nothing was committed"),
        "the abort guarantee is stated: {err}"
    );

    assert_eq!(fixture.commit_count(), 1, "no commit was created");
    assert_eq!(fixture.index_content("a.txt"), index_before);
    assert_eq!(
        fs::read_to_string(fixture.path().join("a.txt")).unwrap(),
        text(&worktree),
        "the worktree is never touched"
    );
    assert_eq!(
        fs::read_to_string(fixture.path().join(".git/gitchange/state.json")).unwrap(),
        state_before,
        "state file untouched by the aborted commit"
    );
    assert_eq!(
        fixture.state_dir_entries(),
        vec!["state.json"],
        "no temp index left behind"
    );
}

#[test]
fn a_changelist_containing_a_binary_commits_it_whole() {
    // ADR 0009: the temp-index commit receives the staged blob
    // whole-file; other changelists' content stays behind.
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &text(&numbered_lines(5)))
        .write_bytes("logo.png", &[0u8, 1, 2, 3])
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("art").unwrap();
    repo.switch(Some("art")).unwrap();
    fixture
        .write_bytes("logo.png", &[0u8, 9, 9])
        .stage("logo.png");
    repo.refresh().unwrap();

    // A second changelist owns a staged text edit that must not commit.
    repo.create_changelist("other").unwrap();
    repo.switch(Some("other")).unwrap();
    let mut lines = numbered_lines(5);
    lines[0] = "edited".into();
    fixture.write("a.txt", &text(&lines)).stage("a.txt");
    repo.refresh().unwrap();

    let payload = repo.commit_payload(Some("art")).unwrap();
    assert_eq!(payload.file_count(), 1);
    assert_eq!(payload.staged_hunks(), 1, "the binary counts as one hunk");
    assert_eq!(payload.stale_hunks(), 0);

    commit(&repo, Some("art"), "binary whole-file");
    assert_eq!(fixture.head_bytes("logo.png").unwrap(), vec![0u8, 9, 9]);
    assert_eq!(
        fixture.head_bytes("a.txt").unwrap(),
        text(&numbered_lines(5)).into_bytes(),
        "the other changelist's staged edit stays out of the commit"
    );
    assert_eq!(
        fixture.index_content("a.txt").unwrap(),
        text(&lines),
        "the live index is untouched"
    );

    // The consumed record is removed; the path drops out of the diff.
    let snapshot = repo.refresh().unwrap();
    assert!(!snapshot.files.iter().any(|file| file.path == "logo.png"));
}

#[test]
fn a_staged_binary_deletion_commits_the_removal() {
    let fixture = RepoFixture::new();
    fixture
        .write("keep.txt", "content\n")
        .write_bytes("logo.png", &[0u8, 1])
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("art").unwrap();
    repo.switch(Some("art")).unwrap();
    fs::remove_file(fixture.path().join("logo.png")).unwrap();
    fixture.stage_removal("logo.png");

    let payload = repo.commit_payload(Some("art")).unwrap();
    assert_eq!(payload.file_count(), 1);
    assert_eq!(payload.staged_hunks(), 1);

    commit(&repo, Some("art"), "drop binary");
    assert!(fixture.head_bytes("logo.png").is_none());
    assert!(fixture.head_bytes("keep.txt").is_some());
}

#[cfg(unix)]
#[test]
fn a_whole_file_commit_leaves_another_changelists_mode_flip_behind() {
    // ADR 0017: the whole-file payload's temp-index entry takes its mode
    // from the changelist's own mode hunk, or HEAD's when none is owned —
    // never the live entry's. Here the flip is another changelist's, so
    // the commit lands HEAD's 644 over the new bytes.
    let fixture = RepoFixture::new();
    fixture
        .write_bytes("logo.png", &[0u8, 1, 2, 3])
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("art").unwrap();
    repo.switch(Some("art")).unwrap();
    fixture
        .write_bytes("logo.png", &[0u8, 9, 9])
        .stage("logo.png");
    repo.refresh().unwrap();

    // A second changelist owns the chmod, staged.
    repo.create_changelist("chores").unwrap();
    repo.switch(Some("chores")).unwrap();
    fixture.set_exec("logo.png").stage("logo.png");
    let snapshot = repo.refresh().unwrap();
    assert_eq!(
        owners(&snapshot, "logo.png"),
        vec![Some("chores".to_string()), Some("art".to_string())],
        "the mode hunk sits first, owned by chores"
    );

    commit(&repo, Some("art"), "art: new bytes");
    assert_eq!(fixture.head_bytes("logo.png").unwrap(), vec![0u8, 9, 9]);
    assert_eq!(
        fixture.head_mode("logo.png"),
        Some(0o100644),
        "the mode hunk chores owns stays out of art's commit"
    );
    assert_eq!(
        fixture.index_mode("logo.png"),
        Some(0o100755),
        "the live index keeps the staged flip"
    );
}

#[cfg(unix)]
#[test]
fn a_whole_file_commit_lands_the_mode_flip_it_owns() {
    // The mirror: one changelist owns both hunks, so its payload carries
    // the mode hunk and HEAD gets the flip.
    let fixture = RepoFixture::new();
    fixture
        .write_bytes("logo.png", &[0u8, 1, 2, 3])
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("art").unwrap();
    repo.switch(Some("art")).unwrap();
    fixture
        .write_bytes("logo.png", &[0u8, 9, 9])
        .set_exec("logo.png")
        .stage("logo.png");

    let payload = repo.commit_payload(Some("art")).unwrap();
    assert_eq!(payload.staged_hunks(), 2, "the mode hunk and the bytes");

    commit(&repo, Some("art"), "art: new bytes, executable");
    assert_eq!(fixture.head_bytes("logo.png").unwrap(), vec![0u8, 9, 9]);
    assert_eq!(fixture.head_mode("logo.png"), Some(0o100755));
}

#[test]
fn a_binary_worktree_over_staged_text_commits_the_staged_text() {
    // The mixed case: text content staged, then the worktree file
    // replaced with binary bytes. Both deltas are the universe's, each
    // `◑` — the worktree's binary rewrite as a whole-file hunk, the
    // staged edit as an index-only text hunk (ADR 0017: no side's delta
    // is dropped because the other side's hunks survived pairing).
    // Committing carries the staged text blob whole-file rather than
    // silently excluding the path.
    let fixture = RepoFixture::new();
    fixture.write("notes.txt", "original\n").commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("work").unwrap();
    repo.switch(Some("work")).unwrap();
    fixture
        .write("notes.txt", "staged text\n")
        .stage("notes.txt");
    fixture.write_bytes("notes.txt", &[0u8, 1, 2, 3]);

    let payload = repo.commit_payload(Some("work")).unwrap();
    assert_eq!(payload.file_count(), 1);
    assert_eq!(payload.stale_hunks(), 2, "index and worktree differ: ◑");

    commit(&repo, Some("work"), "mixed staged text");
    assert_eq!(
        fixture.head_bytes("notes.txt").unwrap(),
        b"staged text\n".to_vec()
    );
    assert_eq!(
        std::fs::read(fixture.path().join("notes.txt")).unwrap(),
        vec![0u8, 1, 2, 3],
        "the worktree is untouched"
    );
}
