//! Hunk-universe derivation (ticket 24, ADR 0003): the union of
//! diff(HEAD↔worktree) and diff(HEAD↔index), with per-hunk ○●◑ and
//! per-file ●◐○ derived — asserted through `Repo::refresh()` per ADR 0008.

use crate::support::RepoFixture;
use gitchange_core::{ChangeKind, FileStage, HunkIdentity, HunkStage, Repo};
// Mode deltas only arise where the filesystem carries an exec bit.
#[cfg(unix)]
use gitchange_core::ModeDelta;

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

    let sides = file.sides.as_ref().expect("binary sides");
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
    let sides = added.sides.as_ref().unwrap();
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

// ——— zero-hunk changes (ADR 0017) ———

#[test]
#[cfg(unix)]
fn a_mode_only_change_is_one_mode_hunk() {
    // ADR 0017: a mode-only change presents one stand-alone mode hunk —
    // whole change — so the row reads `○ 0/1` instead of the inert
    // `0/0`, and the hunk carries the dedicated mode-change identity
    // rather than the whole-file blob pair.
    let fixture = RepoFixture::new();
    fixture
        .write("tool.sh", "#!/bin/sh\n")
        .commit_all("init")
        .set_exec("tool.sh");

    let repo = Repo::discover(fixture.path()).unwrap();
    let file = &repo.refresh().unwrap().files[0];

    assert_eq!(file.path, "tool.sh");
    assert_eq!(file.kind, ChangeKind::Modified);
    assert!(!file.binary, "a shell script is text");
    assert_eq!((file.staged_hunks(), file.total_hunks()), (0, 1));
    assert_eq!(file.stage(), FileStage::Unstaged);
    assert_eq!(
        file.hunks[0].identity,
        HunkIdentity::ModeChange,
        "a mode delta presents a mode hunk, not a whole-file one"
    );
    assert!(
        !file.presents_whole_file_hunk(),
        "the whole-file treatment collapses into the mode hunk"
    );

    // The file carries the delta the placeholder reads; the blob sides a
    // whole-file hunk would anchor on are not this change's.
    assert_eq!(
        file.mode_delta,
        Some(ModeDelta::Mode {
            before: 0o100644,
            after: 0o100755
        })
    );
    assert!(file.sides.is_none(), "no whole-file hunk to anchor");
}

#[test]
#[cfg(unix)]
fn a_chmod_of_a_binary_is_a_mode_hunk_too() {
    // The boundary the mode hunk draws is content, not text-ness: a
    // binary whose bytes never moved has no whole-file change to
    // present, only the mode flip (ADR 0017). A binary *content* change
    // keeps its whole-file hunk — the case above this one.
    let fixture = RepoFixture::new();
    fixture
        .write_bytes("blob.bin", &[0u8, 1, 2, 3])
        .commit_all("init")
        .set_exec("blob.bin");

    let repo = Repo::discover(fixture.path()).unwrap();
    let file = &repo.refresh().unwrap().files[0];

    assert!(file.binary);
    assert_eq!((file.staged_hunks(), file.total_hunks()), (0, 1));
    assert_eq!(file.hunks[0].identity, HunkIdentity::ModeChange);
}

#[test]
#[cfg(unix)]
fn mode_only_staging_is_derived_by_mode_compare() {
    // A mode hunk carries no content evidence at all, so the mode bits
    // are what derive its ○●◑ — the two diffs' changed-side modes
    // compared, as text hunks compare lines (ADR 0017).
    let fixture = RepoFixture::new();
    fixture
        .write("tool.sh", "#!/bin/sh\n")
        .commit_all("init")
        .set_exec("tool.sh")
        .stage("tool.sh");

    let repo = Repo::discover(fixture.path()).unwrap();
    let file = &repo.refresh().unwrap().files[0];
    assert_eq!(file.hunks[0].identity, HunkIdentity::ModeChange);
    assert_eq!(file.hunks[0].stage, HunkStage::Staged);
    assert_eq!((file.staged_hunks(), file.total_hunks()), (1, 1));
    assert_eq!(file.stage(), FileStage::Staged);

    // Staged, then reverted in the worktree: the index-only `◑` flavour,
    // which the ADR names as reachable for a mode flip.
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(
        fixture.path().join("tool.sh"),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    let file = &repo.refresh().unwrap().files[0];
    assert_eq!(file.hunks[0].stage, HunkStage::StagedStale);
    assert!(file.hunks[0].index_only);
    assert_eq!((file.staged_hunks(), file.total_hunks()), (0, 1));
}

#[test]
#[cfg(unix)]
fn a_mode_hunk_sits_beside_content_hunks() {
    // A chmod alongside worktree edits: the mode delta is its own hunk,
    // first, and the edits are theirs — a chmod+edit counts `0/3` where
    // the bare edits count `0/2` (ADR 0017).
    let fixture = RepoFixture::new();
    fixture
        .write("tool.sh", &numbered(&[]))
        .commit_all("init")
        .write(
            "tool.sh",
            &numbered(&[(4, "EDIT four"), (16, "EDIT sixteen")]),
        )
        .set_exec("tool.sh");

    let repo = Repo::discover(fixture.path()).unwrap();
    let file = &repo.refresh().unwrap().files[0];

    assert_eq!((file.staged_hunks(), file.total_hunks()), (0, 3));
    assert_eq!(file.hunks[0].identity, HunkIdentity::ModeChange);
    assert!(
        file.hunks[1..]
            .iter()
            .all(|hunk| hunk.identity.text_lines().is_some()),
        "the edits keep their text hunks"
    );
    assert_eq!(file.stage(), FileStage::Unstaged);
    assert_eq!(
        file.mode_delta,
        Some(ModeDelta::Mode {
            before: 0o100644,
            after: 0o100755
        })
    );
}

#[test]
#[cfg(unix)]
fn a_worktree_mode_flip_survives_index_only_content_hunks() {
    // Issue #101's forward corner: an edit staged, reverted in the
    // worktree, then a chmod. The index diff's hunks survive pairing as
    // index-only, and the worktree diff — mode delta, no hunks — must
    // still contribute it. Before ADR 0017's amendment the +x appeared
    // nowhere at all.
    let fixture = RepoFixture::new();
    fixture
        .write("tool.sh", "one\n")
        .commit_all("init")
        .write("tool.sh", "two\n")
        .stage("tool.sh")
        .write("tool.sh", "one\n")
        .set_exec("tool.sh");

    let repo = Repo::discover(fixture.path()).unwrap();
    let file = &repo.refresh().unwrap().files[0];

    assert_eq!((file.staged_hunks(), file.total_hunks()), (0, 2));
    assert_eq!(file.hunks[0].identity, HunkIdentity::ModeChange);
    assert_eq!(
        file.hunks[0].stage,
        HunkStage::Unstaged,
        "the index holds HEAD's mode"
    );
    assert!(!file.hunks[0].index_only);
    assert_eq!(file.hunks[1].stage, HunkStage::StagedStale);
    assert!(
        file.hunks[1].index_only,
        "the staged edit the worktree reverted"
    );
    assert_eq!(file.stage(), FileStage::PartiallyStaged);
    assert_eq!(
        file.mode_delta,
        Some(ModeDelta::Mode {
            before: 0o100644,
            after: 0o100755
        })
    );
}

#[test]
#[cfg(unix)]
fn a_staged_mode_flip_survives_a_worktree_edit() {
    // Issue #101's mirror corner: the flip staged, then the worktree
    // edited. Both diffs carry the mode delta, so the mode hunk is `●`
    // beside the `○` edit — the file reads `◐`, never wholly `○`, which
    // is what made the staged flip invisible.
    let fixture = RepoFixture::new();
    fixture
        .write("tool.sh", "one\n")
        .commit_all("init")
        .set_exec("tool.sh")
        .stage("tool.sh")
        .write("tool.sh", "two\n");

    let repo = Repo::discover(fixture.path()).unwrap();
    let file = &repo.refresh().unwrap().files[0];

    assert_eq!((file.staged_hunks(), file.total_hunks()), (1, 2));
    assert_eq!(file.hunks[0].identity, HunkIdentity::ModeChange);
    assert_eq!(file.hunks[0].stage, HunkStage::Staged);
    assert_eq!(file.hunks[1].stage, HunkStage::Unstaged);
    assert_eq!(file.stage(), FileStage::PartiallyStaged);
}

#[test]
#[cfg(unix)]
fn a_staged_mode_flip_reverted_in_the_worktree_is_index_only_beside_an_edit() {
    // The third pairing arm with content alongside: only the index diff
    // carries the mode delta, so the mode hunk is the index-only `◑`
    // flavour — committable, invisible in the worktree.
    let fixture = RepoFixture::new();
    fixture
        .write("tool.sh", "one\n")
        .commit_all("init")
        .set_exec("tool.sh")
        .stage("tool.sh")
        .clear_exec("tool.sh")
        .write("tool.sh", "two\n");

    let repo = Repo::discover(fixture.path()).unwrap();
    let file = &repo.refresh().unwrap().files[0];

    assert_eq!((file.staged_hunks(), file.total_hunks()), (0, 2));
    assert_eq!(file.hunks[0].identity, HunkIdentity::ModeChange);
    assert_eq!(file.hunks[0].stage, HunkStage::StagedStale);
    assert!(file.hunks[0].index_only);
    assert_eq!(file.hunks[1].stage, HunkStage::Unstaged);
}

#[test]
#[cfg(unix)]
fn a_chmodded_binary_edit_presents_both_hunks() {
    // Content and mode are separate deltas even where the content has no
    // lines to address: the binary keeps its whole-file hunk (ADR 0009)
    // and the chmod gets its mode hunk beside it (ADR 0017).
    let fixture = RepoFixture::new();
    fixture
        .write_bytes("blob.bin", &[0u8, 1, 2, 3])
        .commit_all("init")
        .write_bytes("blob.bin", &[0u8, 9, 9, 9, 9])
        .set_exec("blob.bin");

    let repo = Repo::discover(fixture.path()).unwrap();
    let file = &repo.refresh().unwrap().files[0];

    assert!(file.binary);
    assert_eq!((file.staged_hunks(), file.total_hunks()), (0, 2));
    assert_eq!(file.hunks[0].identity, HunkIdentity::ModeChange);
    assert!(
        matches!(file.hunks[1].identity, HunkIdentity::WholeFile { .. }),
        "the bytes moved, so the whole-file hunk stays"
    );
    assert!(file.presents_whole_file_hunk());
    assert!(
        file.sides.is_some(),
        "the placeholder's sizes come from the sides"
    );
}

#[test]
fn a_staged_binary_over_a_worktree_edit_keeps_both_deltas() {
    // The same rule with no mode in sight: binary bytes staged, then the
    // worktree edited as text. The index side has no hunks to pair, so
    // its rewrite comes through as a whole-file hunk beside the worktree
    // edit's text hunk — neither delta is dropped for the other
    // (ADR 0017). Its anchor comes from the index side, the worktree
    // side carrying no sides for a file with text hunks.
    let fixture = RepoFixture::new();
    fixture
        .write("notes.txt", "original\n")
        .commit_all("init")
        .write_bytes("notes.txt", &[0u8, 1, 2, 3])
        .stage("notes.txt")
        .write("notes.txt", "edited\n");

    let repo = Repo::discover(fixture.path()).unwrap();
    let file = &repo.refresh().unwrap().files[0];

    assert_eq!(file.total_hunks(), 2);
    let HunkIdentity::WholeFile { oids: anchor } = &file.hunks[0].identity else {
        panic!("whole-file hunk for the staged binary");
    };
    assert!(anchor.changed.is_some(), "the staged blob");
    assert_eq!(file.hunks[0].stage, HunkStage::StagedStale);
    assert_eq!(file.hunks[1].stage, HunkStage::Unstaged);
}

#[test]
fn empty_file_adds_and_deletes_are_whole_file_hunks() {
    // The other zero-hunk shapes (ADR 0017), discriminated by which side
    // of the anchor exists — the same rule added and deleted binaries
    // follow.
    let fixture = RepoFixture::new();
    fixture
        .write_bytes("gone.txt", b"")
        .commit_all("init")
        .write_bytes("fresh.txt", b"");
    std::fs::remove_file(fixture.path().join("gone.txt")).unwrap();

    let repo = Repo::discover(fixture.path()).unwrap();
    let snapshot = repo.refresh().unwrap();

    let added = snapshot
        .files
        .iter()
        .find(|f| f.path == "fresh.txt")
        .unwrap();
    assert_eq!(added.kind, ChangeKind::Untracked);
    assert_eq!(added.total_hunks(), 1);
    let HunkIdentity::WholeFile { oids: anchor } = &added.hunks[0].identity else {
        panic!("whole-file hunk");
    };
    assert!(anchor.head.is_none());
    assert!(anchor.changed.is_some(), "the empty blob");

    let deleted = snapshot
        .files
        .iter()
        .find(|f| f.path == "gone.txt")
        .unwrap();
    assert_eq!(deleted.kind, ChangeKind::Deleted);
    assert_eq!(deleted.total_hunks(), 1);
    let HunkIdentity::WholeFile { oids: anchor } = &deleted.hunks[0].identity else {
        panic!("whole-file hunk");
    };
    assert!(anchor.head.is_some());
    assert!(anchor.changed.is_none());
}

#[test]
#[cfg(unix)]
fn an_added_or_deleted_executable_carries_no_mode_hunk() {
    // ADR 0017's boundary: a file coming or going has its mode as part of
    // the add/delete whole, so there is no second hunk for it — which is
    // what keeps mode-versus-content dependency rules out of gitchange
    // entirely. Both directions have one side absent, so no delta to
    // compare.
    // `empty.sh` and `gone.sh` are the pointed pair: zero-hunk changes
    // where one side is absent, so the degenerate branch has a mode on
    // one side and nothing to compare it against.
    let fixture = RepoFixture::new();
    fixture
        .write("old.sh", "#!/bin/sh\n")
        .write_bytes("gone.sh", b"")
        .commit_all("init")
        .set_exec("old.sh")
        .set_exec("gone.sh")
        .stage("old.sh")
        .stage("gone.sh")
        .commit_all("make them executable")
        .remove("old.sh")
        .remove("gone.sh")
        .write("new.sh", "#!/bin/sh\n")
        .write_bytes("empty.sh", b"");
    fixture.set_exec("new.sh").set_exec("empty.sh");

    let repo = Repo::discover(fixture.path()).unwrap();
    let snapshot = repo.refresh().unwrap();

    for path in ["empty.sh", "gone.sh", "new.sh", "old.sh"] {
        let file = snapshot
            .files
            .iter()
            .find(|file| file.path == path)
            .unwrap_or_else(|| panic!("{path} in the universe"));
        assert_eq!(file.total_hunks(), 1, "{path}");
        assert!(
            !file.hunks[0].is_mode_change(),
            "{path} presents its whole change as one hunk, mode included"
        );
    }
    for path in ["empty.sh", "gone.sh"] {
        let file = snapshot
            .files
            .iter()
            .find(|file| file.path == path)
            .expect("in the universe");
        assert!(
            file.presents_whole_file_hunk(),
            "{path} has no line content either, so it is the whole-file hunk"
        );
    }
}

#[test]
fn an_embedded_repository_stays_out_of_the_universe() {
    // git reports a nested clone or linked worktree as one untracked
    // directory — no blob, no hunks, and no index write gitchange makes.
    // It is not a zero-hunk change, so it never enters the universe and
    // the "every change carries a hunk" invariant holds (ADR 0017).
    let fixture = RepoFixture::new();
    fixture.write("a.txt", "content\n").commit_all("init");
    fixture.add_worktree("side");

    let repo = Repo::discover(fixture.path()).unwrap();
    let snapshot = repo.refresh().unwrap();

    assert!(snapshot.files.is_empty(), "{:?}", snapshot.files);
}

#[test]
#[cfg(unix)]
fn a_type_change_is_a_zero_hunk_change_too() {
    // git reports a file↔symlink swap with mode bits alone and no
    // hunks, whatever the file holds, so ADR 0017's invariant takes it
    // in: one whole-file hunk, membership and staging like any other.
    // How a type change should *present* and match is its own issue
    // (#100); this pins that it no longer falls out of everything.
    let fixture = RepoFixture::new();
    fixture
        .write("thing", "content\n")
        .write("target.txt", "elsewhere\n")
        .commit_all("init")
        .remove("thing")
        .symlink("thing", "target.txt");

    let repo = Repo::discover(fixture.path()).unwrap();
    let file = repo
        .refresh()
        .unwrap()
        .files
        .into_iter()
        .find(|file| file.path == "thing")
        .expect("thing in the universe");

    assert_eq!(file.kind, ChangeKind::TypeChanged);
    assert_eq!((file.staged_hunks(), file.total_hunks()), (0, 1));
    assert!(file.presents_whole_file_hunk());
    assert!(file.sides.is_some(), "type-change sides");
    assert_eq!(
        file.mode_delta,
        Some(ModeDelta::Type {
            before: 0o100644,
            after: 0o120000
        }),
        "the symlink mode, named a type change rather than a chmod"
    );
}

#[test]
#[cfg(unix)]
fn a_symlink_to_file_swap_is_the_same_zero_hunk_shape() {
    // The reverse direction (#100): git emits no hunks here either, even
    // when the replacement file holds several lines, so the same
    // whole-file hunk takes it in. Both sides are real blobs — the old
    // target string and the new content — so the anchor carries both
    // OIDs and exact-match has content to rest on.
    let fixture = RepoFixture::new();
    fixture
        .write("target.txt", "elsewhere\n")
        .commit_all("seed")
        .symlink("thing", "target.txt")
        .commit_all("link")
        .remove("thing")
        .write("thing", "line one\nline two\nline three\n");

    let repo = Repo::discover(fixture.path()).unwrap();
    let file = repo
        .refresh()
        .unwrap()
        .files
        .into_iter()
        .find(|file| file.path == "thing")
        .expect("thing in the universe");

    assert_eq!(file.kind, ChangeKind::TypeChanged);
    assert_eq!((file.staged_hunks(), file.total_hunks()), (0, 1));
    assert!(file.presents_whole_file_hunk());
    let HunkIdentity::WholeFile { oids: anchor } = &file.hunks[0].identity else {
        panic!("whole-file hunk");
    };
    assert!(anchor.head.is_some(), "the target-string blob");
    assert!(anchor.changed.is_some(), "the content blob");
    assert_eq!(
        file.mode_delta,
        Some(ModeDelta::Type {
            before: 0o120000,
            after: 0o100644
        }),
        "away from the symlink mode"
    );
}
