//! Hunk-universe derivation (ticket 24, ADR 0003): the union of
//! diff(HEAD↔worktree) and diff(HEAD↔index), with per-hunk ○●◑ and
//! per-file ●◐○ derived — asserted through `Repo::refresh()` per ADR 0008.

use crate::support::RepoFixture;
use gitchange_core::{ChangeKind, FileStage, HunkIdentity, HunkStage, Repo};

/// Twenty numbered lines, with `edits` as (1-based line, replacement).
fn numbered(edits: &[(usize, &str)]) -> String {
    (1..=20)
        .map(|n| {
            edits
                .iter()
                .find(|(line, _)| *line == n)
                .map(|(_, text)| format!("{text}\n"))
                .unwrap_or_else(|| format!("line {n}\n"))
        })
        .collect()
}

#[test]
fn a_worktree_only_edit_is_a_single_unstaged_hunk() {
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", "one\n")
        .commit_all("init")
        .write("a.txt", "two\n");

    let repo = Repo::discover(fixture.path()).unwrap();
    let snapshot = repo.refresh().unwrap();

    let file = &snapshot.files[0];
    assert_eq!(file.path, "a.txt");
    assert_eq!(file.kind, ChangeKind::Modified);
    assert_eq!(file.stage(), FileStage::Unstaged);
    assert_eq!((file.staged_hunks(), file.total_hunks()), (0, 1));
    assert_eq!(file.hunks[0].stage, HunkStage::Unstaged);
}

#[test]
fn hunk_lines_carry_verbatim_content_with_origins() {
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", "one\n")
        .commit_all("init")
        .write("a.txt", "two\n");

    let repo = Repo::discover(fixture.path()).unwrap();
    let snapshot = repo.refresh().unwrap();

    let lines: Vec<(char, &str)> = snapshot.files[0].hunks[0]
        .identity
        .text_lines()
        .expect("text hunk")
        .iter()
        .map(|line| (line.origin, line.content.as_str()))
        .collect();
    assert_eq!(lines, vec![('-', "one\n"), ('+', "two\n")]);
}

#[test]
fn an_externally_staged_untouched_hunk_is_staged() {
    // ADR 0003: external `git add` is absorbed at refresh, never an error.
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", "one\n")
        .commit_all("init")
        .write("a.txt", "two\n")
        .stage("a.txt");

    let repo = Repo::discover(fixture.path()).unwrap();
    let snapshot = repo.refresh().unwrap();

    let file = &snapshot.files[0];
    assert_eq!(file.kind, ChangeKind::Modified);
    assert_eq!(file.stage(), FileStage::Staged);
    assert_eq!((file.staged_hunks(), file.total_hunks()), (1, 1));
    assert_eq!(file.hunks[0].stage, HunkStage::Staged);
}

#[test]
fn an_external_git_reset_is_absorbed_as_an_unstaged_hunk() {
    // ADR 0003's absorption rule, `git reset` half (issue #60): the index
    // is the only source of staged-ness, so an external unstage simply
    // re-derives — no error, no confirmation flow. Membership comes from
    // records instead, so it does not move with the index; a `git reset`
    // that shed the owner would be the silent membership loss ADR 0001
    // exists to prevent.
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(&[]))
        .commit_all("init")
        .write("a.txt", &numbered(&[(10, "ten!")]))
        .stage("a.txt");

    let repo = Repo::discover(fixture.path()).unwrap();
    repo.create_changelist("one").unwrap();
    repo.switch(Some("one")).unwrap();
    let staged = repo.refresh().unwrap();
    assert_eq!(staged.files[0].hunks[0].stage, HunkStage::Staged);
    assert_eq!(
        staged.files[0].hunks[0].changelist.as_deref(),
        Some("one"),
        "the owned-and-staged starting state this test resets from"
    );

    // `git reset -- a.txt` in another terminal.
    fixture.reset_path("a.txt");

    let snapshot = repo.refresh().unwrap();
    let file = &snapshot.files[0];
    assert_eq!(file.kind, ChangeKind::Modified);
    assert_eq!(file.stage(), FileStage::Unstaged);
    assert_eq!((file.staged_hunks(), file.total_hunks()), (0, 1));
    assert_eq!(file.hunks[0].stage, HunkStage::Unstaged);
    assert_eq!(
        file.hunks[0].changelist.as_deref(),
        Some("one"),
        "the reset is a staging event, not a membership one"
    );
    assert!(
        snapshot.advisories.is_empty(),
        "absorbed silently: nothing to spot-check, nothing to error over"
    );
}

#[test]
fn a_staged_then_edited_hunk_is_staged_stale() {
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", "one\n")
        .commit_all("init")
        .write("a.txt", "two\n")
        .stage("a.txt")
        .write("a.txt", "three\n");

    let repo = Repo::discover(fixture.path()).unwrap();
    let snapshot = repo.refresh().unwrap();

    let file = &snapshot.files[0];
    assert_eq!(file.stage(), FileStage::PartiallyStaged);
    assert_eq!((file.staged_hunks(), file.total_hunks()), (0, 1));
    assert_eq!(file.hunks[0].stage, HunkStage::StagedStale);
}

#[test]
fn an_index_only_hunk_after_worktree_revert_is_staged_stale() {
    // The visibility invariant's hard case: staged then worktree-reverted
    // leaves nothing in the worktree diff, yet the hunk is committable.
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", "one\n")
        .commit_all("init")
        .write("a.txt", "two\n")
        .stage("a.txt")
        .write("a.txt", "one\n");

    let repo = Repo::discover(fixture.path()).unwrap();
    let snapshot = repo.refresh().unwrap();

    let file = &snapshot.files[0];
    assert_eq!(file.path, "a.txt");
    assert_eq!(file.kind, ChangeKind::Modified);
    assert_eq!(file.stage(), FileStage::PartiallyStaged);
    assert_eq!((file.staged_hunks(), file.total_hunks()), (0, 1));
    assert_eq!(file.hunks[0].stage, HunkStage::StagedStale);
}

#[test]
fn a_file_with_one_staged_and_one_unstaged_hunk_is_partially_staged() {
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(&[]))
        .commit_all("init")
        .write("a.txt", &numbered(&[(2, "edit near top")]))
        .stage("a.txt")
        .write(
            "a.txt",
            &numbered(&[(2, "edit near top"), (18, "edit near bottom")]),
        );

    let repo = Repo::discover(fixture.path()).unwrap();
    let snapshot = repo.refresh().unwrap();

    let file = &snapshot.files[0];
    assert_eq!(file.stage(), FileStage::PartiallyStaged);
    assert_eq!((file.staged_hunks(), file.total_hunks()), (1, 2));
    let stages: Vec<HunkStage> = file.hunks.iter().map(|hunk| hunk.stage).collect();
    assert_eq!(stages, vec![HunkStage::Staged, HunkStage::Unstaged]);
}

#[test]
fn an_unborn_head_diffs_against_the_empty_tree() {
    // ADR 0007: fresh `git init` works from commit zero.
    let fixture = RepoFixture::new();
    fixture
        .write("untracked.txt", "hello\n")
        .write("staged.txt", "world\n")
        .stage("staged.txt");

    let repo = Repo::discover(fixture.path()).unwrap();
    let snapshot = repo.refresh().unwrap();

    let entries: Vec<(&str, ChangeKind, FileStage)> = snapshot
        .files
        .iter()
        .map(|file| (file.path.as_str(), file.kind, file.stage()))
        .collect();
    assert_eq!(
        entries,
        vec![
            ("staged.txt", ChangeKind::Added, FileStage::Staged),
            ("untracked.txt", ChangeKind::Untracked, FileStage::Unstaged),
        ]
    );
    assert_eq!(snapshot.files[0].hunks[0].stage, HunkStage::Staged);
    assert_eq!(snapshot.files[1].hunks[0].stage, HunkStage::Unstaged);
}

#[test]
fn an_untracked_file_presents_its_content_as_one_unstaged_hunk() {
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", "one\n")
        .commit_all("init")
        .write("new.txt", "alpha\nbeta\n");

    let repo = Repo::discover(fixture.path()).unwrap();
    let snapshot = repo.refresh().unwrap();

    let file = &snapshot.files[0];
    assert_eq!(file.path, "new.txt");
    assert_eq!(file.kind, ChangeKind::Untracked);
    assert_eq!((file.staged_hunks(), file.total_hunks()), (0, 1));
    let lines: Vec<(char, &str)> = file.hunks[0]
        .identity
        .text_lines()
        .expect("text hunk")
        .iter()
        .map(|line| (line.origin, line.content.as_str()))
        .collect();
    assert_eq!(lines, vec![('+', "alpha\n"), ('+', "beta\n")]);
}

#[test]
fn a_deleted_file_presents_removals_as_one_hunk() {
    let fixture = RepoFixture::new();
    fixture.write("doomed.txt", "gone\n").commit_all("init");
    std::fs::remove_file(fixture.path().join("doomed.txt")).unwrap();

    let repo = Repo::discover(fixture.path()).unwrap();
    let snapshot = repo.refresh().unwrap();

    let file = &snapshot.files[0];
    assert_eq!(file.kind, ChangeKind::Deleted);
    assert_eq!((file.staged_hunks(), file.total_hunks()), (0, 1));
    assert_eq!(
        file.hunks[0].identity.text_lines().expect("text hunk")[0].origin,
        '-'
    );
}

#[test]
fn a_staged_new_file_removed_from_the_worktree_reads_deleted_and_stale() {
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", "one\n")
        .commit_all("init")
        .write("new.txt", "content\n")
        .stage("new.txt");
    std::fs::remove_file(fixture.path().join("new.txt")).unwrap();

    let repo = Repo::discover(fixture.path()).unwrap();
    let snapshot = repo.refresh().unwrap();

    let file = &snapshot.files[0];
    assert_eq!(file.path, "new.txt");
    assert_eq!(file.kind, ChangeKind::Deleted);
    assert_eq!(file.hunks[0].stage, HunkStage::StagedStale);
}

#[test]
fn a_rename_presents_as_delete_plus_untracked() {
    // ADR 0011: rename detection is off in v0.1 — a rename is a deleted
    // old path plus an untracked new path, by decision not accident.
    let fixture = RepoFixture::new();
    fixture
        .write("old.txt", "content\n")
        .commit_all("init")
        .rename("old.txt", "new.txt");

    let repo = Repo::discover(fixture.path()).unwrap();
    let snapshot = repo.refresh().unwrap();

    let entries: Vec<(&str, ChangeKind)> = snapshot
        .files
        .iter()
        .map(|file| (file.path.as_str(), file.kind))
        .collect();
    assert_eq!(
        entries,
        vec![
            ("new.txt", ChangeKind::Untracked),
            ("old.txt", ChangeKind::Deleted),
        ]
    );
}

#[test]
fn a_changed_binary_file_is_one_whole_file_hunk() {
    // ADR 0009: a changed binary is one degenerate whole-file hunk with
    // a blob-OID-pair anchor, not an error and not hunk-less.
    let fixture = RepoFixture::new();
    fixture
        .write_bytes("blob.bin", &[0u8, 1, 2, 3])
        .commit_all("init")
        .write_bytes("blob.bin", &[0u8, 9, 9, 9, 9]);

    let repo = Repo::discover(fixture.path()).unwrap();
    let snapshot = repo.refresh().unwrap();

    let file = &snapshot.files[0];
    assert_eq!(file.path, "blob.bin");
    assert_eq!(file.kind, ChangeKind::Modified);
    assert!(file.binary);
    assert_eq!(file.total_hunks(), 1);
    assert_eq!(file.stage(), FileStage::Unstaged);
    assert_eq!(file.hunks[0].stage, HunkStage::Unstaged);
    // The whole-file identity carries no lines by construction, so the
    // pattern match *is* the old "no lines" assertion (ADR 0009).
    let HunkIdentity::WholeFile { oids: anchor } = &file.hunks[0].identity else {
        panic!("a changed binary presents a whole-file hunk");
    };
    assert!(anchor.head.is_some(), "HEAD-side blob OID");
    assert!(anchor.changed.is_some(), "changed-side content hash");
    assert_ne!(anchor.head, anchor.changed);

    let sides = file.binary_sides.as_ref().expect("binary sides");
    assert_eq!(sides.head.as_ref().map(|blob| blob.size), Some(4));
    assert_eq!(sides.changed.as_ref().map(|blob| blob.size), Some(5));
}

#[test]
fn binary_staging_is_derived_by_oid_compare() {
    // The `●` case: staged, worktree unchanged since. The changed-side
    // hash equals the staged blob's OID, so the file reads fully staged
    // — the "staged binary derives ○ 0/0" gap this ticket closes.
    let fixture = RepoFixture::new();
    fixture
        .write_bytes("blob.bin", &[0u8, 1, 2, 3])
        .commit_all("init")
        .write_bytes("blob.bin", &[0u8, 9, 9])
        .stage("blob.bin");

    let repo = Repo::discover(fixture.path()).unwrap();
    let file = &repo.refresh().unwrap().files[0];
    assert_eq!(file.stage(), FileStage::Staged);
    assert_eq!(file.hunks[0].stage, HunkStage::Staged);
    assert_eq!((file.staged_hunks(), file.total_hunks()), (1, 1));

    // The edited `◑` flavour: index and worktree hold different blobs.
    fixture.write_bytes("blob.bin", &[0u8, 7, 7, 7]);
    let file = &repo.refresh().unwrap().files[0];
    assert_eq!(file.hunks[0].stage, HunkStage::StagedStale);
    assert!(!file.hunks[0].index_only);
    assert_eq!((file.staged_hunks(), file.total_hunks()), (0, 1));

    // The index-only `◑` flavour: staged then worktree-reverted.
    fixture.write_bytes("blob.bin", &[0u8, 1, 2, 3]);
    let file = &repo.refresh().unwrap().files[0];
    assert_eq!(file.hunks[0].stage, HunkStage::StagedStale);
    assert!(file.hunks[0].index_only);
}

#[test]
fn added_and_deleted_binaries_have_one_sided_anchors() {
    let fixture = RepoFixture::new();
    fixture
        .write_bytes("old.bin", &[0u8, 1, 2])
        .commit_all("init")
        .write_bytes("new.bin", &[0u8, 3, 4, 5, 6]);
    std::fs::remove_file(fixture.path().join("old.bin")).unwrap();

    let repo = Repo::discover(fixture.path()).unwrap();
    let snapshot = repo.refresh().unwrap();

    let added = snapshot.files.iter().find(|f| f.path == "new.bin").unwrap();
    assert_eq!(added.kind, ChangeKind::Untracked);
    let HunkIdentity::WholeFile { oids: anchor } = &added.hunks[0].identity else {
        panic!("whole-file hunk");
    };
    assert!(anchor.head.is_none());
    assert!(anchor.changed.is_some());
    let sides = added.binary_sides.as_ref().unwrap();
    assert_eq!(sides.changed.as_ref().map(|blob| blob.size), Some(5));

    let deleted = snapshot.files.iter().find(|f| f.path == "old.bin").unwrap();
    assert_eq!(deleted.kind, ChangeKind::Deleted);
    let HunkIdentity::WholeFile { oids: anchor } = &deleted.hunks[0].identity else {
        panic!("whole-file hunk");
    };
    assert!(anchor.head.is_some());
    assert!(anchor.changed.is_none());
}

#[test]
fn a_conflicted_binary_stays_quarantined() {
    // Quarantine precedence (ADR 0007) survives whole-file hunks: a
    // conflicted binary never derives one.
    let fixture = RepoFixture::new();
    fixture
        .write_bytes("blob.bin", &[0u8, 1])
        .commit_all("init")
        .add_index_conflict("blob.bin");

    let repo = Repo::discover(fixture.path()).unwrap();
    let snapshot = repo.refresh().unwrap();

    let file = &snapshot.files[0];
    assert_eq!(file.kind, ChangeKind::Conflicted);
    assert!(file.hunks.is_empty());
}
