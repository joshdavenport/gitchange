//! The commit guard's breadth (issue #57, ADR 0007): core maps every
//! in-progress git operation libgit2 recognises onto a `GitOperation`,
//! and commit is globally guarded while any of them holds. Merge is
//! proven in `conflicts.rs`, where it sits with the quarantine
//! assertions; this file drives the remaining arms of that mapping —
//! all three rebase backends, cherry-pick and revert in both single and
//! sequence form, and `git am` — each against a real repo left genuinely
//! mid-operation (ADR 0008).
//!
//! Every case asserts the same three properties, plus the
//! `RepositoryState` its fixture reached, so a builder that quietly
//! failed to start its operation can't make the guard look guarded.
//!
//! The last test is the mirror image: a detached HEAD, which ADR 0007
//! pins but deliberately does not guard. The guard's edge is only pinned
//! from both sides.

mod support;

use git2::RepositoryState;
use gitchange_core::{CommitOptions, CommitOutcome, Error, GitOperation, Head, Repo};
use support::RepoFixture;

/// The clean, unconflicted file every fixture commits and every guard
/// assertion stages — the "staging is never guarded" half needs a path
/// the operation isn't touching.
const BYSTANDER: &str = "b.txt";

/// `feature` and `main` editing the same line of `a.txt` differently, so
/// replaying either onto the other conflicts however it is replayed.
/// HEAD ends on `main`.
fn diverged() -> RepoFixture {
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", "base\n")
        .write(BYSTANDER, "bystander base\n")
        .commit_all("init");
    fixture
        .branch("feature")
        .checkout("feature")
        .write("a.txt", "feature side\n")
        .commit_all("feature edit");
    fixture
        .checkout("main")
        .write("a.txt", "main side\n")
        .commit_all("main edit");
    fixture
}

/// `diverged()` with a second, independent commit on `feature`, so a
/// two-commit replay stops on the first and leaves the second in the
/// sequencer's todo — the sequence states are a different
/// `RepositoryState` arm than their single-commit forms.
fn diverged_pair() -> RepoFixture {
    let fixture = diverged();
    fixture
        .checkout("feature")
        .write("c.txt", "a file the replay never touches\n")
        .commit_all("feature addition");
    fixture.checkout("main");
    fixture
}

/// Three commits rewriting the same line of `a.txt` in turn, so
/// reverting the middle one conflicts with the newest.
fn stacked_edits() -> RepoFixture {
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", "v1\n")
        .write(BYSTANDER, "bystander base\n")
        .commit_all("first");
    fixture.write("a.txt", "v2\n").commit_all("second");
    fixture.write("a.txt", "v3\n").commit_all("third");
    fixture
}

/// The three properties ADR 0007 promises while `expected` is in
/// progress: the snapshot reports it, `Repo::commit` refuses with it,
/// and staging is unaffected — the guard is on commit alone, and the
/// only paths staging refuses are the unmerged ones.
fn assert_guard_holds(fixture: &RepoFixture, expected: GitOperation) {
    let repo = Repo::discover(fixture.path()).unwrap();

    let snapshot = repo.refresh().unwrap();
    assert_eq!(snapshot.operation, Some(expected));

    // Nothing staged, no changelist: the guard fires ahead of both, so
    // this can never be mistaken for a `NothingStaged` refusal.
    let result = repo.commit(None, "msg", &CommitOptions::default(), None);
    assert!(
        matches!(
            &result,
            Err(Error::OperationInProgress { operation }) if *operation == expected
        ),
        "commit mid-{} must refuse with that operation: {result:?}",
        expected.label()
    );

    // The bystander is clean and unconflicted, and stages by hunk
    // exactly as it would with no operation running.
    fixture.write(BYSTANDER, "bystander edited\n");
    let snapshot = repo.refresh().unwrap();
    let hunk = snapshot
        .files
        .iter()
        .find(|file| file.path == BYSTANDER)
        .expect("the bystander's edit is in the universe")
        .hunks
        .first()
        .expect("one hunk for the one-line edit")
        .clone();
    let advisories = repo.stage_hunk(BYSTANDER, &hunk).unwrap().advisories;

    assert_eq!(advisories, vec![]);
    assert_eq!(
        fixture.index_content(BYSTANDER).as_deref(),
        Some("bystander edited\n"),
        "hunk staging is never operation-guarded"
    );

    // Whole-file staging is the other unguarded op: a further edit, then
    // the index catches up to the whole worktree file.
    fixture.write(BYSTANDER, "bystander edited\nand again\n");
    repo.refresh().unwrap();
    repo.stage_file(BYSTANDER).unwrap();

    assert_eq!(
        fixture.index_content(BYSTANDER).as_deref(),
        Some("bystander edited\nand again\n"),
        "file staging is never operation-guarded"
    );
}

/// Each rebase backend with the `RepositoryState` arms its on-disk shape
/// can reach. The apply backend writes `.git/rebase-apply/rebasing` and
/// has an arm to itself; both others write `.git/rebase-merge`, which
/// libgit2 names `RebaseInteractive` or `RebaseMerge` depending on
/// whether git left an `interactive` marker beside it — a git-version
/// detail (2.49 writes one for `--merge` too), so those two are pinned
/// as a pair rather than individually. `RebaseMerge` may therefore not
/// be reachable at all on a given git; `--apply` versus the rest is what
/// proves the collapse spans genuinely distinct states.
const REBASE_BACKENDS: [(&str, &[RepositoryState]); 3] = [
    ("--apply", &[RepositoryState::Rebase]),
    (
        "--merge",
        &[
            RepositoryState::RebaseInteractive,
            RepositoryState::RebaseMerge,
        ],
    ),
    (
        "--interactive",
        &[
            RepositoryState::RebaseInteractive,
            RepositoryState::RebaseMerge,
        ],
    ),
];

#[test]
fn every_rebase_backend_reports_rebase_and_guards_commit() {
    for (backend, expected_states) in REBASE_BACKENDS {
        let fixture = diverged();
        fixture
            .checkout("feature")
            .rebase_conflicting(backend, "main");

        let state = fixture.git_state();
        assert!(
            expected_states.contains(&state),
            "rebase {backend} reached {state:?}, expected one of {expected_states:?}"
        );
        assert_guard_holds(&fixture, GitOperation::Rebase);
    }
}

#[test]
fn a_cherry_pick_reports_cherry_pick_and_guards_commit() {
    let fixture = diverged();
    fixture.cherry_pick_conflicting(&["feature"]);

    assert_eq!(fixture.git_state(), RepositoryState::CherryPick);
    assert_guard_holds(&fixture, GitOperation::CherryPick);
}

#[test]
fn a_cherry_pick_sequence_reports_cherry_pick_and_guards_commit() {
    let fixture = diverged_pair();
    // Two revs, oldest first: the first conflicts and the second waits
    // in the sequencer, which is what makes this the sequence state.
    fixture.cherry_pick_conflicting(&["feature~1", "feature"]);

    assert_eq!(fixture.git_state(), RepositoryState::CherryPickSequence);
    assert_guard_holds(&fixture, GitOperation::CherryPick);
}

#[test]
fn a_revert_reports_revert_and_guards_commit() {
    let fixture = stacked_edits();
    fixture.revert_conflicting(&["HEAD~1"]);

    assert_eq!(fixture.git_state(), RepositoryState::Revert);
    assert_guard_holds(&fixture, GitOperation::Revert);
}

#[test]
fn a_revert_sequence_reports_revert_and_guards_commit() {
    let fixture = stacked_edits();
    fixture.revert_conflicting(&["HEAD~1", "HEAD~2"]);

    assert_eq!(fixture.git_state(), RepositoryState::RevertSequence);
    assert_guard_holds(&fixture, GitOperation::Revert);
}

#[test]
fn an_am_in_progress_reports_am_and_guards_commit() {
    let fixture = diverged();
    // The mailbox is `feature`'s commit, which cannot apply to `main`'s
    // conflicting line: git stops with the patch unapplied, so nothing
    // here is conflicted — the guard doesn't need conflicts, only an
    // operation.
    fixture.am_conflicting("feature");

    assert_eq!(fixture.git_state(), RepositoryState::ApplyMailbox);
    assert_guard_holds(&fixture, GitOperation::Am);
}

#[test]
fn a_detached_head_is_not_an_operation_and_leaves_commit_unguarded() {
    // The guard's one deliberate omission (ADR 0007): a detached HEAD is
    // *pinned* in the Log panel but never guarded, because committing
    // while detached is legitimate git — a spike off a tag, a fixup
    // mid-bisect. The pin and the reporting are asserted elsewhere; what
    // needs asserting here is the absence, since widening `operation()`
    // to cover detached HEAD would otherwise break no test at all.
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", "base\n")
        .write(BYSTANDER, "bystander base\n")
        .commit_all("init");
    let start = fixture.head_oid();
    fixture.detach_head();
    // The raw state every test above asserts too: detaching leaves git
    // with no operation on disk, so `None` below is core reading a clean
    // repo and not a mapping that lost one.
    assert_eq!(fixture.git_state(), RepositoryState::Clean);

    let repo = Repo::discover(fixture.path()).unwrap();
    repo.create_changelist("one").unwrap();
    fixture.write("a.txt", "detached edit\n").stage("a.txt");
    let snapshot = repo.refresh().unwrap();
    assert_eq!(
        snapshot.operation, None,
        "a detached HEAD is a location, not an operation in progress"
    );
    assert!(
        matches!(snapshot.head, Head::Detached { .. }),
        "the fixture must really be detached, got {:?}",
        snapshot.head
    );

    let outcome = repo
        .commit(
            Some("one"),
            "detached: a.txt",
            &CommitOptions::default(),
            None,
        )
        .expect("committing while detached is not guarded");
    assert!(
        matches!(outcome, CommitOutcome::Committed { .. }),
        "{outcome:?}"
    );
    assert_eq!(fixture.head_message(), "detached: a.txt\n");
    assert_eq!(
        fixture.head_bytes("a.txt"),
        Some(b"detached edit\n".to_vec())
    );
    assert_eq!(fixture.commit_count(), 2);

    // Still detached afterwards, now pinned at the commit just made: the
    // commit moved HEAD itself, not the branch it was detached from.
    let snapshot = repo.refresh().unwrap();
    match &snapshot.head {
        Head::Detached { short_id } => {
            assert!(fixture.head_oid().starts_with(short_id.as_str()));
            assert_ne!(fixture.head_oid(), start, "HEAD advanced");
        }
        other => panic!("committing must not reattach HEAD, got {other:?}"),
    }
    assert_eq!(snapshot.operation, None);
}
