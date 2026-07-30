//! Write-through staging ops (ticket 26, ADR 0003): stage/unstage hunk
//! and file perform a real apply on the live index — asserted through
//! public `Repo` ops against real index content per ADR 0008. Stale
//! hunks fail soft with a notice datum (ADR 0005's validate-at-apply).

mod support;

use gitchange_core::{FileStage, HunkStage, Notice, Repo};
use support::RepoFixture;

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
    let snapshot = repo.refresh().unwrap();
    let hunk = snapshot.files[0].hunks[0].clone();

    let notices = repo.stage_hunk("a.txt", &hunk).unwrap();

    assert_eq!(notices, vec![]);
    // Ground truth: the real index holds only the first edit.
    assert_eq!(
        fixture.index_content("a.txt").as_deref(),
        Some(numbered(&[(2, "edit near top")]).as_str())
    );
    let snapshot = repo.refresh().unwrap();
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
    let snapshot = repo.refresh().unwrap();
    let hunk = snapshot.files[0].hunks[0].clone();

    let notices = repo.unstage_hunk("a.txt", &hunk).unwrap();

    assert_eq!(notices, vec![]);
    assert_eq!(
        fixture.index_content("a.txt").as_deref(),
        Some(numbered(&[(18, "edit near bottom")]).as_str())
    );
    let snapshot = repo.refresh().unwrap();
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
    let snapshot = repo.refresh().unwrap();
    let hunk = snapshot.files[0].hunks[0].clone();

    // The worktree moves on after the snapshot: the hunk's content no
    // longer exists in the live tree.
    fixture.write(
        "a.txt",
        &numbered(&[(2, "a different edit"), (18, "edit near bottom")]),
    );

    let notices = repo.stage_hunk("a.txt", &hunk).unwrap();

    assert_eq!(
        notices,
        vec![Notice::StaleHunk {
            path: "a.txt".into(),
            new_start: hunk.new_start,
        }]
    );
    // Nothing half-applied: the index still matches HEAD.
    assert_eq!(
        fixture.index_content("a.txt").as_deref(),
        Some(numbered(&[]).as_str())
    );
    let snapshot = repo.refresh().unwrap();
    assert_eq!(snapshot.files[0].stage(), FileStage::Unstaged);
}

#[test]
fn a_stale_hunk_fails_soft_on_unstage_too() {
    let fixture = two_hunk_fixture();
    fixture.stage("a.txt");
    let repo = Repo::discover(fixture.path()).unwrap();
    let snapshot = repo.refresh().unwrap();
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

    let notices = repo.unstage_hunk("a.txt", &hunk).unwrap();

    assert_eq!(
        notices,
        vec![Notice::StaleHunk {
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
    let snapshot = repo.refresh().unwrap();
    let hunk = snapshot.files[0].hunks[0].clone();
    assert_eq!(hunk.stage, HunkStage::StagedStale);

    repo.stage_hunk("a.txt", &hunk).unwrap();

    assert_eq!(
        fixture.index_content("a.txt").as_deref(),
        Some(numbered(&[(2, "second version")]).as_str())
    );
    let snapshot = repo.refresh().unwrap();
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
    let snapshot = repo.refresh().unwrap();
    let hunk = snapshot.files[0].hunks[0].clone();
    assert_eq!(hunk.stage, HunkStage::StagedStale);

    repo.stage_hunk("a.txt", &hunk).unwrap();

    assert_eq!(
        fixture.index_content("a.txt").as_deref(),
        Some(numbered(&[]).as_str())
    );
    let snapshot = repo.refresh().unwrap();
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
    let snapshot = repo.refresh().unwrap();
    let hunk = snapshot.files[0].hunks[0].clone();

    repo.unstage_hunk("a.txt", &hunk).unwrap();

    assert_eq!(
        fixture.index_content("a.txt").as_deref(),
        Some(numbered(&[]).as_str())
    );
    let snapshot = repo.refresh().unwrap();
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
    let snapshot = repo.refresh().unwrap();
    let hunk = snapshot.files[0].hunks[0].clone();

    let shifted = format!("inserted line\n{}", numbered(&[(18, "edit near bottom")]));
    fixture.write("a.txt", &shifted);

    let notices = repo.stage_hunk("a.txt", &hunk).unwrap();

    assert_eq!(notices, vec![]);
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
    let snapshot = repo.refresh().unwrap();
    let hunk = snapshot.files[0].hunks[0].clone();

    let notices = repo.stage_hunk("a.txt", &hunk).unwrap();

    assert_eq!(notices, vec![]);
    assert_eq!(
        fixture.index_content("a.txt").as_deref(),
        Some(numbered(&[(2, "edit near top"), (18, "edit near bottom")]).as_str())
    );
}

#[test]
fn unstaging_an_unstaged_hunk_is_a_no_op() {
    let fixture = two_hunk_fixture();
    let repo = Repo::discover(fixture.path()).unwrap();
    let snapshot = repo.refresh().unwrap();
    let hunk = snapshot.files[0].hunks[0].clone();

    let notices = repo.unstage_hunk("a.txt", &hunk).unwrap();

    assert_eq!(notices, vec![]);
    assert_eq!(
        fixture.index_content("a.txt").as_deref(),
        Some(numbered(&[]).as_str())
    );
}

#[test]
fn stage_file_stages_every_hunk() {
    let fixture = two_hunk_fixture();
    let repo = Repo::discover(fixture.path()).unwrap();
    repo.refresh().unwrap();

    repo.stage_file("a.txt").unwrap();

    assert_eq!(
        fixture.index_content("a.txt").as_deref(),
        Some(numbered(&[(2, "edit near top"), (18, "edit near bottom")]).as_str())
    );
    let snapshot = repo.refresh().unwrap();
    let file = &snapshot.files[0];
    assert_eq!(file.stage(), FileStage::Staged);
    assert_eq!((file.staged_hunks(), file.total_hunks()), (2, 2));
}

#[test]
fn unstage_file_resets_every_hunk() {
    let fixture = two_hunk_fixture();
    fixture.stage("a.txt");
    let repo = Repo::discover(fixture.path()).unwrap();
    repo.refresh().unwrap();

    repo.unstage_file("a.txt").unwrap();

    assert_eq!(
        fixture.index_content("a.txt").as_deref(),
        Some(numbered(&[]).as_str())
    );
    let snapshot = repo.refresh().unwrap();
    assert_eq!(snapshot.files[0].stage(), FileStage::Unstaged);
}

#[test]
fn stage_file_stages_an_untracked_file() {
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", "one\n")
        .commit_all("init")
        .write("new.txt", "alpha\nbeta\n");
    let repo = Repo::discover(fixture.path()).unwrap();

    repo.stage_file("new.txt").unwrap();

    assert_eq!(
        fixture.index_content("new.txt").as_deref(),
        Some("alpha\nbeta\n")
    );
    let snapshot = repo.refresh().unwrap();
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
    let snapshot = repo.refresh().unwrap();
    let hunk = snapshot.files[0].hunks[0].clone();

    let notices = repo.stage_hunk("new.txt", &hunk).unwrap();

    assert_eq!(notices, vec![]);
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
    let snapshot = repo.refresh().unwrap();
    let hunk = snapshot.files[0].hunks[0].clone();

    let notices = repo.unstage_hunk("new.txt", &hunk).unwrap();

    assert_eq!(notices, vec![]);
    assert_eq!(fixture.index_content("new.txt"), None);
    let snapshot = repo.refresh().unwrap();
    assert_eq!(snapshot.files[0].stage(), FileStage::Unstaged);
}

#[test]
fn stage_file_stages_a_deletion() {
    let fixture = RepoFixture::new();
    fixture.write("doomed.txt", "gone\n").commit_all("init");
    std::fs::remove_file(fixture.path().join("doomed.txt")).unwrap();
    let repo = Repo::discover(fixture.path()).unwrap();

    repo.stage_file("doomed.txt").unwrap();

    assert_eq!(fixture.index_content("doomed.txt"), None);
    let snapshot = repo.refresh().unwrap();
    assert_eq!(snapshot.files[0].stage(), FileStage::Staged);
}

#[test]
fn unstage_file_restores_a_staged_deletion() {
    let fixture = RepoFixture::new();
    fixture.write("doomed.txt", "gone\n").commit_all("init");
    std::fs::remove_file(fixture.path().join("doomed.txt")).unwrap();
    let repo = Repo::discover(fixture.path()).unwrap();
    repo.stage_file("doomed.txt").unwrap();

    repo.unstage_file("doomed.txt").unwrap();

    assert_eq!(
        fixture.index_content("doomed.txt").as_deref(),
        Some("gone\n")
    );
    let snapshot = repo.refresh().unwrap();
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
    let snapshot = repo.refresh().unwrap();
    let hunk = snapshot.files[0].hunks[0].clone();

    let notices = repo.stage_hunk("doomed.txt", &hunk).unwrap();

    assert_eq!(notices, vec![]);
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
    let snapshot = repo.refresh().unwrap();
    let hunk = snapshot.files[0].hunks[0].clone();

    let notices = repo.stage_hunk("latin1.txt", &hunk).unwrap();

    assert_eq!(notices, vec![]);
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
    let snapshot = repo.refresh().unwrap();
    let hunks = snapshot.files[0].hunks.clone();
    assert_eq!(hunks.len(), 2);
    assert_eq!(hunks[0].lines, hunks[1].lines);

    let notices = repo.stage_hunk("a.txt", &hunks[1]).unwrap();

    assert_eq!(notices, vec![]);
    assert_eq!(
        fixture.index_content("a.txt").as_deref(),
        Some(file("delta", "EDIT").as_str())
    );
}
