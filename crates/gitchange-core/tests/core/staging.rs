//! Write-through staging ops (ticket 26, ADR 0003): stage/unstage of a
//! hunk and of a Files row's owned hunks perform a real apply on the
//! live index — asserted through public `Repo` ops against real index
//! content per ADR 0008. Stale hunks fail soft with a notice datum
//! (ADR 0005's validate-at-apply).

use crate::support::RepoFixture;
use gitchange_core::{Advisory, Error, FileStage, Hunk, HunkStage, Repo, Snapshot};

// Gated with its only user below, so Windows builds this file clean.
#[cfg(unix)]
use gitchange_core::ApplySite;

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

/// A committed base file plus worktree edits at lines 2 and 18 — two
/// well-separated hunks.
fn two_hunk_fixture() -> RepoFixture {
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(&[]))
        .commit_all("init")
        .write(
            "a.txt",
            &numbered(&[(2, "edit near top"), (18, "edit near bottom")]),
        );
    fixture
}

#[test]
fn staging_one_hunk_of_a_multi_hunk_file_leaves_the_other_unstaged() {
    let fixture = two_hunk_fixture();
    let repo = Repo::discover(fixture.path()).unwrap();
    let snapshot = repo.refresh().unwrap().snapshot;
    let hunk = snapshot.files[0].hunks[0].clone();

    let advisories = repo.stage_hunk("a.txt", &hunk).unwrap().advisories;

    assert_eq!(advisories, vec![]);
    // Ground truth: the real index holds only the first edit.
    assert_eq!(
        fixture.index_content("a.txt").as_deref(),
        Some(numbered(&[(2, "edit near top")]).as_str())
    );
    let snapshot = repo.refresh().unwrap().snapshot;
    let file = &snapshot.files[0];
    assert_eq!(file.stage(), FileStage::PartiallyStaged);
    assert_eq!((file.staged_hunks(), file.total_hunks()), (1, 2));
    let stages: Vec<HunkStage> = file.hunks.iter().map(|hunk| hunk.stage).collect();
    assert_eq!(stages, vec![HunkStage::Staged, HunkStage::Unstaged]);
}

#[test]
fn unstaging_one_hunk_leaves_the_sibling_staged() {
    let fixture = two_hunk_fixture();
    fixture.stage("a.txt");
    let repo = Repo::discover(fixture.path()).unwrap();
    let snapshot = repo.refresh().unwrap().snapshot;
    let hunk = snapshot.files[0].hunks[0].clone();

    let advisories = repo.unstage_hunk("a.txt", &hunk).unwrap().advisories;

    assert_eq!(advisories, vec![]);
    assert_eq!(
        fixture.index_content("a.txt").as_deref(),
        Some(numbered(&[(18, "edit near bottom")]).as_str())
    );
    let snapshot = repo.refresh().unwrap().snapshot;
    let stages: Vec<HunkStage> = snapshot.files[0]
        .hunks
        .iter()
        .map(|hunk| hunk.stage)
        .collect();
    assert_eq!(stages, vec![HunkStage::Unstaged, HunkStage::Staged]);
}

#[test]
fn a_stale_hunk_fails_soft_with_a_notice_and_no_index_write() {
    let fixture = two_hunk_fixture();
    let repo = Repo::discover(fixture.path()).unwrap();
    let snapshot = repo.refresh().unwrap().snapshot;
    let hunk = snapshot.files[0].hunks[0].clone();

    // The worktree moves on after the snapshot: the hunk's content no
    // longer exists in the live tree.
    fixture.write(
        "a.txt",
        &numbered(&[(2, "a different edit"), (18, "edit near bottom")]),
    );

    let advisories = repo.stage_hunk("a.txt", &hunk).unwrap().advisories;

    assert_eq!(
        advisories,
        vec![Advisory::StaleHunk {
            path: "a.txt".into(),
            new_start: hunk.new_start,
        }]
    );
    // Nothing half-applied: the index still matches HEAD.
    assert_eq!(
        fixture.index_content("a.txt").as_deref(),
        Some(numbered(&[]).as_str())
    );
    let snapshot = repo.refresh().unwrap().snapshot;
    assert_eq!(snapshot.files[0].stage(), FileStage::Unstaged);
}

#[test]
fn a_stale_hunk_fails_soft_on_unstage_too() {
    let fixture = two_hunk_fixture();
    fixture.stage("a.txt");
    let repo = Repo::discover(fixture.path()).unwrap();
    let snapshot = repo.refresh().unwrap().snapshot;
    let hunk = snapshot.files[0].hunks[0].clone();

    // External `git reset` between snapshot and keypress: the staged
    // hunk the snapshot showed is gone from the index and the worktree
    // content at line 2 changed as well.
    fixture.write(
        "a.txt",
        &numbered(&[(2, "a different edit"), (18, "edit near bottom")]),
    );
    fixture.stage("a.txt");
    fixture.write(
        "a.txt",
        &numbered(&[(2, "a third edit"), (18, "edit near bottom")]),
    );

    let advisories = repo.unstage_hunk("a.txt", &hunk).unwrap().advisories;

    assert_eq!(
        advisories,
        vec![Advisory::StaleHunk {
            path: "a.txt".into(),
            new_start: hunk.new_start,
        }]
    );
    assert_eq!(
        fixture.index_content("a.txt").as_deref(),
        Some(numbered(&[(2, "a different edit"), (18, "edit near bottom")]).as_str())
    );
}

#[test]
fn space_on_a_staged_stale_hunk_sets_index_to_worktree() {
    // ADR 0003: `space` on `◑` re-stages the edited hunk.
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(&[]))
        .commit_all("init")
        .write("a.txt", &numbered(&[(2, "first version")]))
        .stage("a.txt")
        .write("a.txt", &numbered(&[(2, "second version")]));
    let repo = Repo::discover(fixture.path()).unwrap();
    let snapshot = repo.refresh().unwrap().snapshot;
    let hunk = snapshot.files[0].hunks[0].clone();
    assert_eq!(hunk.stage, HunkStage::StagedStale);

    repo.stage_hunk("a.txt", &hunk).unwrap();

    assert_eq!(
        fixture.index_content("a.txt").as_deref(),
        Some(numbered(&[(2, "second version")]).as_str())
    );
    let snapshot = repo.refresh().unwrap().snapshot;
    assert_eq!(snapshot.files[0].hunks[0].stage, HunkStage::Staged);
}

#[test]
fn space_on_an_index_only_hunk_discards_it_from_the_index() {
    // ADR 0003: `space` on the reverted-in-worktree `◑` flavour sets
    // index := worktree, i.e. discards the staged content.
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(&[]))
        .commit_all("init")
        .write("a.txt", &numbered(&[(2, "staged then reverted")]))
        .stage("a.txt")
        .write("a.txt", &numbered(&[]));
    let repo = Repo::discover(fixture.path()).unwrap();
    let snapshot = repo.refresh().unwrap().snapshot;
    let hunk = snapshot.files[0].hunks[0].clone();
    assert_eq!(hunk.stage, HunkStage::StagedStale);

    repo.stage_hunk("a.txt", &hunk).unwrap();

    assert_eq!(
        fixture.index_content("a.txt").as_deref(),
        Some(numbered(&[]).as_str())
    );
    let snapshot = repo.refresh().unwrap().snapshot;
    assert!(snapshot.files.is_empty());
}

#[test]
fn unstaging_a_staged_stale_hunk_restores_head_content() {
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(&[]))
        .commit_all("init")
        .write("a.txt", &numbered(&[(2, "first version")]))
        .stage("a.txt")
        .write("a.txt", &numbered(&[(2, "second version")]));
    let repo = Repo::discover(fixture.path()).unwrap();
    let snapshot = repo.refresh().unwrap().snapshot;
    let hunk = snapshot.files[0].hunks[0].clone();

    repo.unstage_hunk("a.txt", &hunk).unwrap();

    assert_eq!(
        fixture.index_content("a.txt").as_deref(),
        Some(numbered(&[]).as_str())
    );
    let snapshot = repo.refresh().unwrap().snapshot;
    assert_eq!(snapshot.files[0].hunks[0].stage, HunkStage::Unstaged);
}

#[test]
fn a_moved_hunk_still_stages_at_its_fresh_position() {
    // Validate-at-apply matches content, not coordinates: an edit above
    // the hunk between snapshot and keypress shifts it, not stales it.
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(&[]))
        .commit_all("init")
        .write("a.txt", &numbered(&[(18, "edit near bottom")]));
    let repo = Repo::discover(fixture.path()).unwrap();
    let snapshot = repo.refresh().unwrap().snapshot;
    let hunk = snapshot.files[0].hunks[0].clone();

    let shifted = format!("inserted line\n{}", numbered(&[(18, "edit near bottom")]));
    fixture.write("a.txt", &shifted);

    let advisories = repo.stage_hunk("a.txt", &hunk).unwrap().advisories;

    assert_eq!(advisories, vec![]);
    assert_eq!(
        fixture.index_content("a.txt").as_deref(),
        Some(numbered(&[(18, "edit near bottom")]).as_str())
    );
}

#[test]
fn staging_a_staged_hunk_is_a_no_op() {
    let fixture = two_hunk_fixture();
    fixture.stage("a.txt");
    let repo = Repo::discover(fixture.path()).unwrap();
    let snapshot = repo.refresh().unwrap().snapshot;
    let hunk = snapshot.files[0].hunks[0].clone();

    let advisories = repo.stage_hunk("a.txt", &hunk).unwrap().advisories;

    assert_eq!(advisories, vec![]);
    assert_eq!(
        fixture.index_content("a.txt").as_deref(),
        Some(numbered(&[(2, "edit near top"), (18, "edit near bottom")]).as_str())
    );
}

#[test]
fn unstaging_an_unstaged_hunk_is_a_no_op() {
    let fixture = two_hunk_fixture();
    let repo = Repo::discover(fixture.path()).unwrap();
    let snapshot = repo.refresh().unwrap().snapshot;
    let hunk = snapshot.files[0].hunks[0].clone();

    let advisories = repo.unstage_hunk("a.txt", &hunk).unwrap().advisories;

    assert_eq!(advisories, vec![]);
    assert_eq!(
        fixture.index_content("a.txt").as_deref(),
        Some(numbered(&[]).as_str())
    );
}

#[test]
fn stage_owned_hunks_stages_every_hunk_of_a_single_owner_file() {
    // The common case: one changelist owns the whole file, so the row's
    // scope and the whole file coincide (issue #97).
    let fixture = two_hunk_fixture();
    let repo = Repo::discover(fixture.path()).unwrap();
    repo.refresh().unwrap();

    let echo = repo.stage_owned_hunks("a.txt", None).unwrap().echo;

    assert_eq!(
        echo.as_deref(),
        Some("staged 2 hunks — a.txt in 'unassigned'")
    );
    assert_eq!(
        fixture.index_content("a.txt").as_deref(),
        Some(numbered(&[(2, "edit near top"), (18, "edit near bottom")]).as_str())
    );
    let snapshot = repo.refresh().unwrap().snapshot;
    let file = &snapshot.files[0];
    assert_eq!(file.stage(), FileStage::Staged);
    assert_eq!((file.staged_hunks(), file.total_hunks()), (2, 2));
}

#[test]
fn unstage_owned_hunks_resets_every_hunk_of_a_single_owner_file() {
    let fixture = two_hunk_fixture();
    fixture.stage("a.txt");
    let repo = Repo::discover(fixture.path()).unwrap();
    repo.refresh().unwrap();

    let echo = repo.unstage_owned_hunks("a.txt", None).unwrap().echo;

    assert_eq!(
        echo.as_deref(),
        Some("unstaged 2 hunks — a.txt in 'unassigned'")
    );
    assert_eq!(
        fixture.index_content("a.txt").as_deref(),
        Some(numbered(&[]).as_str())
    );
    let snapshot = repo.refresh().unwrap().snapshot;
    assert_eq!(snapshot.files[0].stage(), FileStage::Unstaged);
}

/// A file split between two changelists: the row op stages one side and
/// leaves the other out of the index — the whole point of issue #97.
#[test]
fn stage_owned_hunks_leaves_another_changelists_hunks_out_of_the_index() {
    let fixture = two_hunk_fixture();
    let repo = Repo::discover(fixture.path()).unwrap();
    repo.create_changelist("fixes").unwrap();
    let snapshot = repo.refresh().unwrap().snapshot;
    let first = snapshot.files[0].hunks[0].clone();
    repo.assign_hunks("a.txt", &[first], Some("fixes")).unwrap();

    let echo = repo.stage_owned_hunks("a.txt", Some("fixes")).unwrap().echo;

    assert_eq!(echo.as_deref(), Some("staged 1 hunk — a.txt in 'fixes'"));
    assert_eq!(
        fixture.index_content("a.txt").as_deref(),
        Some(numbered(&[(2, "edit near top")]).as_str()),
        "only the hunk 'fixes' owns reached the index"
    );
}

/// The other row over the same path is a distinct target, and staging it
/// adds to — never replaces — what the first row put in the index.
#[test]
fn the_unassigned_row_of_a_split_file_stages_only_the_unowned_hunk() {
    let fixture = two_hunk_fixture();
    let repo = Repo::discover(fixture.path()).unwrap();
    repo.create_changelist("fixes").unwrap();
    let snapshot = repo.refresh().unwrap().snapshot;
    let first = snapshot.files[0].hunks[0].clone();
    repo.assign_hunks("a.txt", &[first], Some("fixes")).unwrap();

    let echo = repo.stage_owned_hunks("a.txt", None).unwrap().echo;

    assert_eq!(
        echo.as_deref(),
        Some("staged 1 hunk — a.txt in 'unassigned'")
    );
    assert_eq!(
        fixture.index_content("a.txt").as_deref(),
        Some(numbered(&[(18, "edit near bottom")]).as_str())
    );
}

/// A row that owns nothing to move still answers (ADR 0007) — and names
/// the file it stayed inside.
#[test]
fn stage_owned_hunks_answers_when_it_moves_nothing() {
    let fixture = two_hunk_fixture();
    fixture.stage("a.txt");
    let repo = Repo::discover(fixture.path()).unwrap();
    repo.refresh().unwrap();

    let echo = repo.stage_owned_hunks("a.txt", None).unwrap().echo;

    assert_eq!(
        echo.as_deref(),
        Some("nothing to stage — a.txt in 'unassigned'")
    );
}

#[test]
fn stage_owned_hunks_names_an_unknown_changelist_as_an_error() {
    let fixture = two_hunk_fixture();
    let repo = Repo::discover(fixture.path()).unwrap();

    let result = repo.stage_owned_hunks("a.txt", Some("ghost"));

    assert!(
        matches!(result, Err(Error::UnknownChangelist { ref name }) if name == "ghost"),
        "unknown changelist is a hard error: {result:?}"
    );
}

#[test]
fn stage_owned_hunks_stages_an_untracked_file() {
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", "one\n")
        .commit_all("init")
        .write("new.txt", "alpha\nbeta\n");
    let repo = Repo::discover(fixture.path()).unwrap();

    repo.stage_owned_hunks("new.txt", None).unwrap();

    assert_eq!(
        fixture.index_content("new.txt").as_deref(),
        Some("alpha\nbeta\n")
    );
    let snapshot = repo.refresh().unwrap().snapshot;
    assert_eq!(snapshot.files[0].stage(), FileStage::Staged);
}

#[test]
fn stage_hunk_stages_an_untracked_file_whole() {
    // An untracked file presents as one hunk; staging it is staging the
    // file.
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", "one\n")
        .commit_all("init")
        .write("new.txt", "alpha\nbeta\n");
    let repo = Repo::discover(fixture.path()).unwrap();
    let snapshot = repo.refresh().unwrap().snapshot;
    let hunk = snapshot.files[0].hunks[0].clone();

    let advisories = repo.stage_hunk("new.txt", &hunk).unwrap().advisories;

    assert_eq!(advisories, vec![]);
    assert_eq!(
        fixture.index_content("new.txt").as_deref(),
        Some("alpha\nbeta\n")
    );
}

#[test]
fn unstage_hunk_removes_a_staged_new_file_from_the_index() {
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", "one\n")
        .commit_all("init")
        .write("new.txt", "alpha\nbeta\n")
        .stage("new.txt");
    let repo = Repo::discover(fixture.path()).unwrap();
    let snapshot = repo.refresh().unwrap().snapshot;
    let hunk = snapshot.files[0].hunks[0].clone();

    let advisories = repo.unstage_hunk("new.txt", &hunk).unwrap().advisories;

    assert_eq!(advisories, vec![]);
    assert_eq!(fixture.index_content("new.txt"), None);
    let snapshot = repo.refresh().unwrap().snapshot;
    assert_eq!(snapshot.files[0].stage(), FileStage::Unstaged);
}

#[test]
fn stage_owned_hunks_stages_a_deletion() {
    let fixture = RepoFixture::new();
    fixture.write("doomed.txt", "gone\n").commit_all("init");
    std::fs::remove_file(fixture.path().join("doomed.txt")).unwrap();
    let repo = Repo::discover(fixture.path()).unwrap();

    repo.stage_owned_hunks("doomed.txt", None).unwrap();

    assert_eq!(fixture.index_content("doomed.txt"), None);
    let snapshot = repo.refresh().unwrap().snapshot;
    assert_eq!(snapshot.files[0].stage(), FileStage::Staged);
}

#[test]
fn unstage_owned_hunks_restores_a_staged_deletion() {
    let fixture = RepoFixture::new();
    fixture.write("doomed.txt", "gone\n").commit_all("init");
    fixture.stage_removal("doomed.txt");
    std::fs::remove_file(fixture.path().join("doomed.txt")).unwrap();
    let repo = Repo::discover(fixture.path()).unwrap();

    repo.unstage_owned_hunks("doomed.txt", None).unwrap();

    assert_eq!(
        fixture.index_content("doomed.txt").as_deref(),
        Some("gone\n")
    );
    let snapshot = repo.refresh().unwrap().snapshot;
    assert_eq!(snapshot.files[0].stage(), FileStage::Unstaged);
}

#[test]
fn stage_hunk_stages_a_deletion_whole() {
    // A deleted file presents as one hunk of removals; staging it stages
    // the deletion.
    let fixture = RepoFixture::new();
    fixture.write("doomed.txt", "gone\n").commit_all("init");
    std::fs::remove_file(fixture.path().join("doomed.txt")).unwrap();
    let repo = Repo::discover(fixture.path()).unwrap();
    let snapshot = repo.refresh().unwrap().snapshot;
    let hunk = snapshot.files[0].hunks[0].clone();

    let advisories = repo.stage_hunk("doomed.txt", &hunk).unwrap().advisories;

    assert_eq!(advisories, vec![]);
    assert_eq!(fixture.index_content("doomed.txt"), None);
}

#[test]
fn staging_a_hunk_of_a_non_utf8_text_file_keeps_the_bytes_verbatim() {
    // The #25 heads-up: `HunkLine.content` is lossily decoded (invalid
    // bytes → U+FFFD), so the apply must come from raw bytes — staging
    // must never write replacement chars into the blob.
    let line = |n: usize, text: &str| -> Vec<u8> {
        let mut bytes = format!("line {n} caf").into_bytes();
        bytes.push(0xE9); // latin-1 'é': invalid UTF-8
        bytes.extend_from_slice(format!(" {text}\n").as_bytes());
        bytes
    };
    let content = |edit: &str| -> Vec<u8> {
        (1..=20)
            .flat_map(|n| line(n, if n == 2 { edit } else { "base" }))
            .collect()
    };
    let fixture = RepoFixture::new();
    fixture
        .write_bytes("latin1.txt", &content("base"))
        .commit_all("init")
        .write_bytes("latin1.txt", &content("edited"));
    let repo = Repo::discover(fixture.path()).unwrap();
    let snapshot = repo.refresh().unwrap().snapshot;
    let hunk = snapshot.files[0].hunks[0].clone();

    let advisories = repo.stage_hunk("latin1.txt", &hunk).unwrap().advisories;

    assert_eq!(advisories, vec![]);
    assert_eq!(
        fixture.index_bytes("latin1.txt").as_deref(),
        Some(content("edited").as_slice())
    );
}

#[test]
fn identical_hunks_stage_the_one_at_the_requested_position() {
    // Two byte-identical hunks (a repeated code block edited the same
    // way twice): the op tie-breaks by proximity to the snapshot
    // position, staging the hunk the user pointed at, not the first
    // content match in the file.
    let block = |edit: &str| format!("alpha\nbravo\ncharlie\n{edit}\necho\nfoxtrot\ngolf\nhotel\n");
    let file = |first: &str, second: &str| {
        let separators: String = (1..=7).map(|n| format!("separator {n}\n")).collect();
        format!("{}{separators}{}", block(first), block(second))
    };
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &file("delta", "delta"))
        .commit_all("init")
        .write("a.txt", &file("EDIT", "EDIT"));
    let repo = Repo::discover(fixture.path()).unwrap();
    let snapshot = repo.refresh().unwrap().snapshot;
    let hunks = snapshot.files[0].hunks.clone();
    assert_eq!(hunks.len(), 2);
    assert_eq!(hunks[0].identity, hunks[1].identity);

    let advisories = repo.stage_hunk("a.txt", &hunks[1]).unwrap().advisories;

    assert_eq!(advisories, vec![]);
    assert_eq!(
        fixture.index_content("a.txt").as_deref(),
        Some(file("delta", "EDIT").as_str())
    );
}

#[test]
fn staging_a_binary_whole_file_hunk_is_a_whole_file_index_write() {
    // ADR 0009: `space` on an unstaged binary = plain `git add`
    // semantics through the whole-file hunk, no apply machinery.
    let fixture = RepoFixture::new();
    fixture
        .write_bytes("blob.bin", &[0u8, 1, 2, 3])
        .commit_all("init")
        .write_bytes("blob.bin", &[0u8, 9, 9]);
    let repo = Repo::discover(fixture.path()).unwrap();

    let snapshot = repo.refresh().unwrap().snapshot;
    let hunk = snapshot.files[0].hunks[0].clone();
    let advisories = repo.stage_hunk("blob.bin", &hunk).unwrap().advisories;
    assert!(advisories.is_empty());
    assert_eq!(fixture.index_bytes("blob.bin").unwrap(), vec![0u8, 9, 9]);
    assert_eq!(
        repo.refresh().unwrap().snapshot.files[0].stage(),
        FileStage::Staged,
        "a staged binary derives ● — the pre-ticket-35 0/0 gap is closed"
    );

    // Unstage: index entry back to HEAD's blob.
    let snapshot = repo.refresh().unwrap().snapshot;
    let hunk = snapshot.files[0].hunks[0].clone();
    repo.unstage_hunk("blob.bin", &hunk).unwrap();
    assert_eq!(fixture.index_bytes("blob.bin").unwrap(), vec![0u8, 1, 2, 3]);
}

/// A binary's Files row holds its one whole-file hunk (ADR 0009), so the
/// row op reduces to that same whole-file index write — unchanged by
/// issue #97, and pinned here rather than left to composition.
#[test]
fn a_binary_row_toggle_is_the_whole_file_index_write() {
    let fixture = RepoFixture::new();
    fixture
        .write_bytes("blob.bin", &[0u8, 1, 2, 3])
        .commit_all("init")
        .write_bytes("blob.bin", &[0u8, 9, 9]);
    let repo = Repo::discover(fixture.path()).unwrap();

    let echo = repo.stage_owned_hunks("blob.bin", None).unwrap().echo;

    assert_eq!(
        echo.as_deref(),
        Some("staged 1 hunk — blob.bin in 'unassigned'")
    );
    assert_eq!(fixture.index_bytes("blob.bin").unwrap(), vec![0u8, 9, 9]);
    assert_eq!(
        repo.refresh().unwrap().snapshot.files[0].stage(),
        FileStage::Staged
    );

    repo.unstage_owned_hunks("blob.bin", None).unwrap();

    assert_eq!(fixture.index_bytes("blob.bin").unwrap(), vec![0u8, 1, 2, 3]);
}

#[test]
fn staging_an_untracked_binary_and_unstaging_drops_the_entry() {
    let fixture = RepoFixture::new();
    fixture
        .write("keep.txt", "content\n")
        .commit_all("init")
        .write_bytes("new.bin", &[0u8, 4, 4]);
    let repo = Repo::discover(fixture.path()).unwrap();

    let snapshot = repo.refresh().unwrap().snapshot;
    let file = snapshot
        .files
        .iter()
        .find(|file| file.path == "new.bin")
        .unwrap();
    let hunk = file.hunks[0].clone();
    repo.stage_hunk("new.bin", &hunk).unwrap();
    assert_eq!(fixture.index_bytes("new.bin").unwrap(), vec![0u8, 4, 4]);

    // Unstage a newly added file: the entry is dropped, not reset.
    let snapshot = repo.refresh().unwrap().snapshot;
    let file = snapshot
        .files
        .iter()
        .find(|file| file.path == "new.bin")
        .unwrap();
    let hunk = file.hunks[0].clone();
    repo.unstage_hunk("new.bin", &hunk).unwrap();
    assert!(fixture.index_bytes("new.bin").is_none());
}

#[test]
fn space_on_a_stale_binary_restages_the_worktree_blob() {
    // Both `◑` flavours: `space` sets index := worktree (ADR 0009).
    let fixture = RepoFixture::new();
    fixture
        .write_bytes("blob.bin", &[0u8, 1])
        .commit_all("init")
        .write_bytes("blob.bin", &[0u8, 2, 2])
        .stage("blob.bin")
        .write_bytes("blob.bin", &[0u8, 3, 3, 3]);
    let repo = Repo::discover(fixture.path()).unwrap();

    let snapshot = repo.refresh().unwrap().snapshot;
    let hunk = snapshot.files[0].hunks[0].clone();
    assert_eq!(hunk.stage, HunkStage::StagedStale);
    repo.stage_hunk("blob.bin", &hunk).unwrap();
    assert_eq!(fixture.index_bytes("blob.bin").unwrap(), vec![0u8, 3, 3, 3]);

    // Index-only flavour: worktree reverted to HEAD; `space` discards
    // the staged blob (index := worktree = HEAD).
    fixture.write_bytes("blob.bin", &[0u8, 5]).stage("blob.bin");
    fixture.write_bytes("blob.bin", &[0u8, 1]);
    let snapshot = repo.refresh().unwrap().snapshot;
    let hunk = snapshot.files[0].hunks[0].clone();
    assert!(hunk.index_only);
    repo.stage_hunk("blob.bin", &hunk).unwrap();
    assert_eq!(fixture.index_bytes("blob.bin").unwrap(), vec![0u8, 1]);
}

/// `ApplyFailed`'s contract, driven by the one refusal reachable without
/// a test-only seam: an unwritable object store (issue #58). A
/// mismatching payload cannot be it — the apply is handed a diff
/// computed moments earlier from the very index it applies back to, so
/// preimage drift, `git apply`'s dominant failure cause, is structurally
/// unreachable here.
///
/// What that certifies is the variant, not a payload shape: the refusal
/// is mapped rather than swallowed into `Backend`, names the path, and
/// carries libgit2's message verbatim, and the index is untouched —
/// the all-or-nothing guarantee ADR 0003's held shell-out fallback was
/// to provide, now asserted rather than read off libgit2's source.
///
/// This is emphatically **not** that fallback's trigger (#55), which is
/// an `ApplyFailed` where `git apply --cached` would have succeeded on
/// the same selection. An odb nothing can write to fails both equally.
/// What it does secure is the reporting path the trigger has to travel:
/// were the mapping broken, a real refusal would arrive as an opaque
/// `Backend` and the condition #55 waits on could never be observed.
// Unix-only by necessity: no Windows equivalent of a 0o500 directory, so
// the refusal has no way to happen there (ADR 0008).
#[cfg(unix)]
#[test]
fn a_refused_apply_reports_apply_failed_and_stages_nothing() {
    let fixture = two_hunk_fixture();
    let repo = Repo::discover(fixture.path()).unwrap();
    let snapshot = repo.refresh().unwrap().snapshot;
    let hunk = snapshot.files[0].hunks[0].clone();
    let index_before = fixture.index_content("a.txt");

    let _odb = fixture.unwritable_odb();
    match repo.stage_hunk("a.txt", &hunk).unwrap_err() {
        Error::ApplyFailed { path, detail, site } => {
            assert_eq!(path, "a.txt", "the error names the file");
            assert_eq!(site, ApplySite::Index, "and the staging site");
            assert!(
                detail.contains("Permission denied"),
                "libgit2's own message, verbatim: {detail}"
            );
        }
        other => panic!("expected ApplyFailed, got {other:?}"),
    }
    assert_eq!(
        fixture.index_content("a.txt"),
        index_before,
        "a refused apply stages nothing"
    );
}

// ── bulk staging: `space` on a changelist (issue #90) ───────────────

/// Each hunk's staging state for `path`, in file order.
fn stages(snapshot: &Snapshot, path: &str) -> Vec<HunkStage> {
    let file = snapshot
        .files
        .iter()
        .find(|file| file.path == path)
        .unwrap_or_else(|| panic!("{path} not in snapshot"));
    file.hunks.iter().map(|hunk| hunk.stage).collect()
}

/// A repo where `one` owns a.txt's top hunk and b.txt's only hunk,
/// `two` owns a.txt's bottom hunk, and c.txt's hunk is unassigned. The
/// split file is the point: a bulk op must move its own changelist's
/// hunks and leave the other's in place. Capture stays off after the
/// setup (ADR 0016): with a changelist active, the released c.txt hunk
/// would be recaptured on the next refresh.
fn bulk_fixture() -> (RepoFixture, Repo) {
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(&[]))
        .write("b.txt", &numbered(&[]))
        .write("c.txt", &numbered(&[]))
        .commit_all("init")
        .write(
            "a.txt",
            &numbered(&[(2, "edit near top"), (18, "edit near bottom")]),
        )
        .write("b.txt", &numbered(&[(2, "b edit")]))
        .write("c.txt", &numbered(&[(2, "c edit")]));
    let repo = Repo::discover(fixture.path()).unwrap();
    repo.create_changelist("one").unwrap();
    repo.create_changelist("two").unwrap();
    repo.switch(Some("one")).unwrap();
    // Everything auto-captures into `one` (the active changelist);
    // a.txt's bottom hunk then moves to `two` and c.txt's is released
    // to unassigned under capture-off, where it stays recordless.
    let snapshot = repo.refresh().unwrap().snapshot;
    let bottom = hunks(&snapshot, "a.txt")[1].clone();
    repo.assign_hunks("a.txt", &[bottom], Some("two")).unwrap();
    repo.switch(None).unwrap();
    let released = hunks(&snapshot, "c.txt");
    repo.assign_hunks("c.txt", &released, None).unwrap();
    (fixture, repo)
}

fn hunks(snapshot: &Snapshot, path: &str) -> Vec<Hunk> {
    snapshot
        .files
        .iter()
        .find(|file| file.path == path)
        .unwrap_or_else(|| panic!("{path} not in snapshot"))
        .hunks
        .clone()
}

#[test]
fn staging_a_changelist_takes_its_unstaged_and_staged_stale_hunks() {
    let (fixture, repo) = bulk_fixture();
    // b.txt's hunk is staged and then edited again — `◑`, which the
    // stage direction re-stages alongside a plain `○`.
    let b = hunks(&repo.refresh().unwrap().snapshot, "b.txt")[0].clone();
    repo.stage_hunk("b.txt", &b).unwrap();
    fixture.write("b.txt", &numbered(&[(2, "b edit, again")]));
    assert_eq!(
        stages(&repo.refresh().unwrap().snapshot, "b.txt"),
        vec![HunkStage::StagedStale]
    );

    let outcome = repo.stage_changelist(Some("one")).unwrap();

    assert_eq!(outcome.echo.as_deref(), Some("staged 2 hunks — 'one'"));
    assert_eq!(outcome.advisories, vec![]);
    // Ground truth: the index holds `one`'s two hunks and nobody else's.
    assert_eq!(
        fixture.index_content("a.txt").as_deref(),
        Some(numbered(&[(2, "edit near top")]).as_str()),
        "the split file's other changelist stays out of the index"
    );
    assert_eq!(
        fixture.index_content("b.txt").as_deref(),
        Some(numbered(&[(2, "b edit, again")]).as_str()),
        "the ◑ hunk is re-staged at its worktree content"
    );
    assert_eq!(
        fixture.index_content("c.txt").as_deref(),
        Some(numbered(&[]).as_str()),
        "the unassigned hunk is untouched"
    );
    let snapshot = repo.refresh().unwrap().snapshot;
    assert_eq!(
        stages(&snapshot, "a.txt"),
        vec![HunkStage::Staged, HunkStage::Unstaged]
    );
    assert_eq!(stages(&snapshot, "b.txt"), vec![HunkStage::Staged]);
}

#[test]
fn unstaging_a_changelist_leaves_every_other_changelists_hunk_staged() {
    let (fixture, repo) = bulk_fixture();
    repo.stage_changelist(Some("one")).unwrap();
    repo.stage_changelist(Some("two")).unwrap();

    let outcome = repo.unstage_changelist(Some("one")).unwrap();

    assert_eq!(outcome.echo.as_deref(), Some("unstaged 2 hunks — 'one'"));
    assert_eq!(outcome.advisories, vec![]);
    assert_eq!(
        fixture.index_content("a.txt").as_deref(),
        Some(numbered(&[(18, "edit near bottom")]).as_str()),
        "`two`'s hunk of the split file stays staged"
    );
    assert_eq!(
        fixture.index_content("b.txt").as_deref(),
        Some(numbered(&[]).as_str())
    );
    let snapshot = repo.refresh().unwrap().snapshot;
    assert_eq!(
        stages(&snapshot, "a.txt"),
        vec![HunkStage::Unstaged, HunkStage::Staged]
    );
    assert_eq!(stages(&snapshot, "b.txt"), vec![HunkStage::Unstaged]);
}

#[test]
fn bulk_staging_reaches_the_unassigned_hunks() {
    let (fixture, repo) = bulk_fixture();

    let outcome = repo.stage_changelist(None).unwrap();

    assert_eq!(
        outcome.echo.as_deref(),
        Some("staged 1 hunk — 'unassigned'")
    );
    assert_eq!(
        fixture.index_content("c.txt").as_deref(),
        Some(numbered(&[(2, "c edit")]).as_str())
    );
    assert_eq!(
        fixture.index_content("a.txt").as_deref(),
        Some(numbered(&[]).as_str()),
        "no named changelist's hunk went with it"
    );

    let outcome = repo.unstage_changelist(None).unwrap();

    assert_eq!(
        outcome.echo.as_deref(),
        Some("unstaged 1 hunk — 'unassigned'")
    );
    assert_eq!(
        fixture.index_content("c.txt").as_deref(),
        Some(numbered(&[]).as_str())
    );
}

#[test]
fn a_bulk_op_that_moves_nothing_still_says_so() {
    let (_fixture, repo) = bulk_fixture();
    repo.create_changelist("empty").unwrap();

    assert_eq!(
        repo.stage_changelist(Some("empty"))
            .unwrap()
            .echo
            .as_deref(),
        Some("nothing to stage — 'empty'")
    );
    assert_eq!(
        repo.unstage_changelist(Some("one"))
            .unwrap()
            .echo
            .as_deref(),
        Some("nothing to unstage — 'one'"),
        "nothing of `one` is staged yet"
    );
    assert!(matches!(
        repo.stage_changelist(Some("missing")),
        Err(Error::UnknownChangelist { .. })
    ));
    assert!(matches!(
        repo.unstage_changelist(Some("missing")),
        Err(Error::UnknownChangelist { .. })
    ));
}

#[test]
fn the_commit_flows_stage_all_still_leaves_staged_stale_hunks_alone() {
    let (fixture, repo) = bulk_fixture();
    let b = hunks(&repo.refresh().unwrap().snapshot, "b.txt")[0].clone();
    repo.stage_hunk("b.txt", &b).unwrap();
    fixture.write("b.txt", &numbered(&[(2, "b edit, again")]));

    let outcome = repo.stage_all(Some("one")).unwrap();

    assert_eq!(
        outcome.echo.as_deref(),
        Some("staged 1 hunk — 'one'"),
        "only the ○ hunk; the ◑ one is the dialog's align option to make"
    );
    assert_eq!(
        fixture.index_content("b.txt").as_deref(),
        Some(numbered(&[(2, "b edit")]).as_str()),
        "the ◑ hunk keeps its earlier index content"
    );
    assert_eq!(
        stages(&repo.refresh().unwrap().snapshot, "b.txt"),
        vec![HunkStage::StagedStale]
    );
}

#[test]
#[cfg(unix)]
fn staging_a_mode_hunk_writes_the_mode_and_no_object() {
    // The mode-only index write (ADR 0017): the entry's mode is set and
    // its blob kept, so no apply machinery runs and no blob is written —
    // a mode flip has no new content to store. The object count is the
    // evidence that the write is the entry rewrite and not a
    // content-bearing apply.
    let fixture = RepoFixture::new();
    fixture
        .write("tool.sh", "#!/bin/sh\n")
        .commit_all("init")
        .set_exec("tool.sh");
    let head_blob = fixture.index_bytes("tool.sh");

    let repo = Repo::discover(fixture.path()).unwrap();
    let hunk = repo.refresh().unwrap().snapshot.files[0].hunks[0].clone();
    let objects = fixture.odb_object_count();

    repo.stage_hunk("tool.sh", &hunk).unwrap();
    assert_eq!(fixture.index_mode("tool.sh"), Some(0o100755));
    assert_eq!(fixture.index_bytes("tool.sh"), head_blob, "same blob");
    assert_eq!(
        fixture.odb_object_count(),
        objects,
        "a mode flip writes no git object"
    );

    let hunk = repo.refresh().unwrap().snapshot.files[0].hunks[0].clone();
    repo.unstage_hunk("tool.sh", &hunk).unwrap();
    assert_eq!(
        fixture.index_mode("tool.sh"),
        Some(0o100644),
        "HEAD's mode back"
    );
    assert_eq!(fixture.index_bytes("tool.sh"), head_blob, "same blob");
    assert_eq!(fixture.odb_object_count(), objects);
}

#[test]
#[cfg(unix)]
fn a_bulk_stage_carries_the_mode_hunk_like_any_other_hunk() {
    // Issue #103: the mode hunk is a hunk, so every bulk scope counts it
    // and moves it. A chmod+edit is two hunks, and `space` on the
    // changelist stages both — the mode by its own write, the lines by
    // the apply.
    let fixture = RepoFixture::new();
    fixture
        .write("tool.sh", "#!/bin/sh\n")
        .commit_all("init")
        .write("tool.sh", "#!/bin/sh\nset -e\n")
        .set_exec("tool.sh");

    let repo = Repo::discover(fixture.path()).unwrap();
    let outcome = repo.stage_changelist(None).unwrap();

    assert_eq!(outcome.echo.unwrap(), "staged 2 hunks — 'unassigned'");
    assert!(outcome.advisories.is_empty());
    assert_eq!(fixture.index_mode("tool.sh"), Some(0o100755));
    assert_eq!(
        fixture.index_content("tool.sh").as_deref(),
        Some("#!/bin/sh\nset -e\n")
    );

    let outcome = repo.unstage_changelist(None).unwrap();
    assert_eq!(outcome.echo.unwrap(), "unstaged 2 hunks — 'unassigned'");
    assert_eq!(fixture.index_mode("tool.sh"), Some(0o100644));
    assert_eq!(
        fixture.index_content("tool.sh").as_deref(),
        Some("#!/bin/sh\n"),
        "HEAD's content back"
    );
}

#[test]
#[cfg(unix)]
fn the_commit_flows_stage_all_offer_takes_the_mode_hunk_too() {
    // `stage_all` stages what the index does not hold (ADR 0004's
    // offer), and an unstaged mode hunk is exactly that — the offer
    // would otherwise open the commit dialog with the chmod left behind.
    let fixture = RepoFixture::new();
    fixture
        .write("tool.sh", "#!/bin/sh\n")
        .commit_all("init")
        .write("tool.sh", "#!/bin/sh\nset -e\n")
        .set_exec("tool.sh");

    let repo = Repo::discover(fixture.path()).unwrap();
    let outcome = repo.stage_all(None).unwrap();

    assert_eq!(outcome.echo.unwrap(), "staged 2 hunks — 'unassigned'");
    assert_eq!(fixture.index_mode("tool.sh"), Some(0o100755));
    assert_eq!(
        fixture.index_content("tool.sh").as_deref(),
        Some("#!/bin/sh\nset -e\n")
    );
}

#[test]
#[cfg(unix)]
fn align_takes_an_index_only_mode_hunk_and_leaves_staged_content_alone() {
    // `align` sets index := worktree for the changelist's `◑` hunks
    // (ADR 0004). Here that is the mode hunk alone — a flip staged then
    // reverted in the worktree — so the flip is discarded and the
    // staged edit, which the worktree still matches, stays put.
    let fixture = RepoFixture::new();
    fixture
        .write("tool.sh", "one\n")
        .commit_all("init")
        .write("tool.sh", "two\n")
        .set_exec("tool.sh")
        .stage("tool.sh")
        .clear_exec("tool.sh");

    let repo = Repo::discover(fixture.path()).unwrap();
    let stages: Vec<HunkStage> = repo.refresh().unwrap().snapshot.files[0]
        .hunks
        .iter()
        .map(|hunk| hunk.stage)
        .collect();
    assert_eq!(stages, vec![HunkStage::StagedStale, HunkStage::Staged]);

    let outcome = repo.align(None).unwrap();

    assert!(outcome.advisories.is_empty());
    assert_eq!(
        fixture.index_mode("tool.sh"),
        Some(0o100644),
        "the worktree's mode, which is HEAD's again"
    );
    assert_eq!(
        fixture.index_content("tool.sh").as_deref(),
        Some("two\n"),
        "the staged edit is not what align was aligning"
    );
    let file = &repo.refresh().unwrap().snapshot.files[0];
    assert_eq!(file.total_hunks(), 1, "only the staged edit is left");
    assert_eq!(file.stage(), FileStage::Staged);
}

#[test]
#[cfg(unix)]
fn staging_a_mode_hunk_beside_an_index_only_hunk_leaves_the_staged_content() {
    // Issue #101's forward corner through the ops: an edit staged, the
    // worktree reverted, then a chmod. The mode hunk stages on its own —
    // the staged blob is untouched, and the file reads `◐` after, the
    // index-only content hunk still `◑`.
    let fixture = RepoFixture::new();
    fixture
        .write("tool.sh", "one\n")
        .commit_all("init")
        .write("tool.sh", "two\n")
        .stage("tool.sh")
        .write("tool.sh", "one\n")
        .set_exec("tool.sh");

    let repo = Repo::discover(fixture.path()).unwrap();
    let hunk = repo.refresh().unwrap().snapshot.files[0].hunks[0].clone();
    assert!(hunk.is_mode_change());

    repo.stage_hunk("tool.sh", &hunk).unwrap();
    assert_eq!(fixture.index_mode("tool.sh"), Some(0o100755));
    assert_eq!(
        fixture.index_content("tool.sh").as_deref(),
        Some("two\n"),
        "the staged edit survives the mode write"
    );

    let file = &repo.refresh().unwrap().snapshot.files[0];
    assert_eq!(file.stage(), FileStage::PartiallyStaged);
    assert_eq!(file.hunks[0].stage, HunkStage::Staged);
    assert_eq!(file.hunks[1].stage, HunkStage::StagedStale);
}

#[test]
#[cfg(unix)]
fn staging_a_type_change_writes_the_symlink_to_the_index() {
    // A file↔symlink swap is zero-hunk (ADR 0017), so it routes through
    // the whole-file index write — the op a symlink swap needs anyway,
    // since there is no line content to apply. Issue #100 owns the rest
    // of the type-change treatment; this pins that staging one works.
    let fixture = RepoFixture::new();
    fixture
        .write("thing", "content\n")
        .write("target.txt", "elsewhere\n")
        .commit_all("init")
        .remove("thing")
        .symlink("thing", "target.txt");

    let repo = Repo::discover(fixture.path()).unwrap();
    repo.stage_owned_hunks("thing", None).unwrap();
    assert_eq!(
        fixture.index_mode("thing"),
        Some(0o120000),
        "staged as a symlink"
    );
    assert_eq!(
        fixture.index_bytes("thing").unwrap(),
        b"target.txt".to_vec()
    );

    repo.unstage_owned_hunks("thing", None).unwrap();
    assert_eq!(
        fixture.index_mode("thing"),
        Some(0o100644),
        "HEAD's entry back"
    );
    assert_eq!(fixture.index_bytes("thing").unwrap(), b"content\n".to_vec());
}

#[test]
#[cfg(unix)]
fn staging_a_symlink_to_file_swap_writes_the_file_to_the_index() {
    // The reverse direction (#100): stage writes the worktree file's
    // content and mode, unstage restores HEAD's symlink entry — the
    // target string at link mode.
    let fixture = RepoFixture::new();
    fixture
        .write("target.txt", "elsewhere\n")
        .commit_all("seed")
        .symlink("thing", "target.txt")
        .commit_all("link")
        .remove("thing")
        .write("thing", "content\n");

    let repo = Repo::discover(fixture.path()).unwrap();
    repo.stage_owned_hunks("thing", None).unwrap();
    assert_eq!(
        fixture.index_mode("thing"),
        Some(0o100644),
        "staged as a regular file"
    );
    assert_eq!(fixture.index_bytes("thing").unwrap(), b"content\n".to_vec());

    repo.unstage_owned_hunks("thing", None).unwrap();
    assert_eq!(
        fixture.index_mode("thing"),
        Some(0o120000),
        "HEAD's symlink entry back"
    );
    assert_eq!(
        fixture.index_bytes("thing").unwrap(),
        b"target.txt".to_vec()
    );
}

// --- the staging verbs' sweep (#145) ---------------------------------------
// `Repo::stage_sweep` is the CLI's primitive: the caller's own persisting
// refresh hands it the snapshot, so validation and application see one
// state. What lands here rather than at the binary seam is the fail-soft
// split, which needs a worktree edit *between* the snapshot and the apply —
// a mid-command race the binary seam cannot inject (ADR 0008).

#[test]
fn a_sweep_narrows_to_the_named_rows_and_is_satisfied_when_repeated() {
    let (fixture, repo) = bulk_fixture();
    let snapshot = repo.refresh().unwrap().snapshot;

    let swept = repo
        .stage_sweep(&snapshot, Some("one"), &["b.txt"])
        .unwrap();

    assert_eq!((swept.moved, swept.skipped), (1, 0));
    assert_eq!(
        swept.receipt.echo.as_deref(),
        Some("staged 1 hunk — b.txt in 'one'")
    );
    assert_eq!(
        fixture.index_content("a.txt").as_deref(),
        Some(numbered(&[]).as_str()),
        "`one`'s hunk in the row that was not named stayed out of the index"
    );

    // Satisfied, not refused: an idempotent retry is always safe (#145).
    let snapshot = repo.refresh().unwrap().snapshot;
    let swept = repo
        .stage_sweep(&snapshot, Some("one"), &["b.txt"])
        .unwrap();

    assert_eq!((swept.moved, swept.skipped), (0, 0));
    assert!(
        !swept.moved_nothing(),
        "a satisfied scope had nothing to move, which is a success — the zero \
         it shares with the wholly-stale sweep below is not the same answer"
    );
    assert_eq!(
        swept.receipt.echo.as_deref(),
        Some("nothing to stage — b.txt in 'one'")
    );
}

#[test]
fn a_sweep_over_several_rows_names_every_one_of_them() {
    let (_fixture, repo) = bulk_fixture();
    let snapshot = repo.refresh().unwrap().snapshot;

    let swept = repo
        .stage_sweep(&snapshot, Some("one"), &["a.txt", "b.txt"])
        .unwrap();

    assert_eq!(
        swept.receipt.echo.as_deref(),
        Some("staged 2 hunks — a.txt, b.txt in 'one'")
    );
}

#[test]
fn a_partly_stale_sweep_counts_its_skips_in_the_echo() {
    let (fixture, repo) = bulk_fixture();
    let snapshot = repo.refresh().unwrap().snapshot;
    // The worktree moves on after the snapshot the sweep validated
    // against: b.txt's hunk no longer exists in the live tree.
    fixture.write("b.txt", &numbered(&[(2, "a different b edit")]));

    let swept = repo.stage_sweep(&snapshot, Some("one"), &[]).unwrap();

    assert_eq!((swept.moved, swept.skipped), (1, 1));
    assert!(
        !swept.moved_nothing(),
        "one hunk landed, so the CLI's split answers success with the skips counted"
    );
    assert_eq!(
        swept.receipt.echo.as_deref(),
        Some("staged 1 of 2 hunks (1 skipped as stale) — 'one'"),
        "the count is on stdout so a harness that drops stderr still re-reads"
    );
    assert_eq!(
        swept.receipt.advisories,
        vec![Advisory::StaleHunk {
            path: "b.txt".into(),
            new_start: hunks(&snapshot, "b.txt")[0].new_start,
        }]
    );
    assert_eq!(
        fixture.index_content("a.txt").as_deref(),
        Some(numbered(&[(2, "edit near top")]).as_str()),
        "the hunk that was still there went in"
    );
    assert_eq!(
        fixture.index_content("b.txt").as_deref(),
        Some(numbered(&[]).as_str()),
        "the stale one applied nothing at all"
    );
}

#[test]
fn a_wholly_stale_sweep_moves_nothing_and_says_which_of_the_two_it_was() {
    let (fixture, repo) = bulk_fixture();
    let snapshot = repo.refresh().unwrap().snapshot;
    fixture
        .write(
            "a.txt",
            &numbered(&[(2, "a different top edit"), (18, "edit near bottom")]),
        )
        .write("b.txt", &numbered(&[(2, "a different b edit")]));

    let swept = repo.stage_sweep(&snapshot, Some("one"), &[]).unwrap();

    // Zero moved with skips is what the CLI turns into a refusal; nothing
    // moved with nothing skipped is the satisfied case above. The counts
    // are what separates them — the echo's prose is a display.
    assert_eq!((swept.moved, swept.skipped), (0, 2));
    assert!(
        swept.moved_nothing(),
        "nothing the caller asked for moved, which is the refusing half of \
         the fail-soft split (#145)"
    );
    assert_eq!(swept.receipt.advisories.len(), 2);
    assert_eq!(
        fixture.index_content("a.txt").as_deref(),
        Some(numbered(&[]).as_str())
    );
}
