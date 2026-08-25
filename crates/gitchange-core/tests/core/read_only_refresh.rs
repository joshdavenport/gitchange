//! The read-only refresh (ADR 0005 §Read-only refresh): the same full
//! recompute, deciding nothing. `refresh` covers the persisting form;
//! what these pin is the absence — no record, no baseline stamp, no
//! lock — and which ownership survives the filter.

use crate::support::RepoFixture;
use gitchange_core::{Error, Repo, Snapshot};

/// Each hunk's owning changelist for `path`, in file order — the one
/// reading every ownership assertion here goes through.
fn owners(snapshot: &Snapshot, path: &str) -> Vec<Option<String>> {
    let file = snapshot
        .files
        .iter()
        .find(|file| file.path == path)
        .unwrap_or_else(|| panic!("{path} not in snapshot"));
    file.hunks
        .iter()
        .map(|hunk| hunk.changelist.clone())
        .collect()
}

/// A repo whose next persisting refresh genuinely owes two writes: a
/// record for an uncaptured hunk, and a fresh baseline stamp for an
/// external HEAD move (ADR 0012). Every write-nothing assertion below
/// starts here, so none of them can pass vacuously.
fn repo_with_writes_due() -> (RepoFixture, Repo) {
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", "one\ntwo\nthree\n")
        .commit_all("init");
    let repo = Repo::discover(fixture.path()).unwrap();
    repo.create_changelist("one").unwrap();
    repo.switch(Some("one")).unwrap();

    // Prior state worth protecting: a recorded hunk in 'one'.
    fixture.write("a.txt", "one\ntwo edited\nthree\n");
    repo.refresh().unwrap();

    fixture.write("new.txt", "fresh\n");
    fixture
        .write("elsewhere.txt", "unrelated\n")
        .stage("elsewhere.txt")
        .commit_index("external");
    (fixture, repo)
}

#[test]
fn a_read_only_refresh_leaves_the_state_file_byte_identical() {
    let (fixture, repo) = repo_with_writes_due();
    let state_path = repo.state_file_path();
    let before = std::fs::read(&state_path).unwrap();

    repo.read_only_refresh().unwrap();

    assert_eq!(
        std::fs::read(&state_path).unwrap(),
        before,
        "a read writes no record and stamps no baseline"
    );
    assert_eq!(
        fixture.state_dir_entries(),
        vec!["state.json"],
        "and leaves nothing beside it — no temp file, no lock"
    );

    // Not a vacuous pass: the persisting form, from the same state,
    // writes.
    repo.refresh().unwrap();
    assert_ne!(
        std::fs::read(&state_path).unwrap(),
        before,
        "the writes were genuinely due"
    );
}

#[test]
fn a_read_only_refresh_takes_no_lock() {
    // Nothing to write means nothing to serialise against: a read
    // succeeds under a lock the persisting form refuses to take.
    let (_fixture, repo) = repo_with_writes_due();
    let state_dir = repo.state_file_path().parent().unwrap().to_path_buf();
    std::fs::write(state_dir.join("state.json.lock"), b"").unwrap();

    repo.read_only_refresh().unwrap();

    let err = repo.refresh().unwrap_err();
    assert!(
        matches!(err, Error::LockContention { .. }),
        "the same state under the same lock stops the persisting form: {err:?}"
    );
}

#[test]
fn a_read_only_refresh_on_a_stateless_repo_creates_no_state_dir() {
    // Writing nothing at its strongest: a repo that has never held
    // gitchange state must not acquire a directory just for being looked
    // at.
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", "one\ntwo\nthree\n")
        .commit_all("init")
        .write("a.txt", "one\ntwo edited\nthree\n");
    let repo = Repo::discover(fixture.path()).unwrap();

    let snapshot = repo.read_only_refresh().unwrap();

    assert_eq!(snapshot.files.len(), 1, "the recompute still ran");
    assert!(
        !repo.state_file_path().parent().unwrap().exists(),
        "no state dir, so nothing inside one either"
    );
}

#[test]
fn a_recordless_hunk_reports_unassigned() {
    // Capture is context-derived, so a read never previews it: the hunk
    // an active changelist would claim reports as unassigned until a
    // persisting refresh actually claims it.
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", "one\ntwo\nthree\n")
        .commit_all("init");
    let repo = Repo::discover(fixture.path()).unwrap();
    repo.create_changelist("one").unwrap();
    repo.switch(Some("one")).unwrap();
    fixture.write("a.txt", "one\ntwo edited\nthree\n");

    let snapshot = repo.read_only_refresh().unwrap();
    assert_eq!(owners(&snapshot, "a.txt"), vec![None]);
    assert_eq!(snapshot.files_in(None).len(), 1, "shown, under unassigned");
    assert!(snapshot.files_in(Some("one")).is_empty());
    assert_eq!(
        snapshot.active.as_deref(),
        Some("one"),
        "the marker still reads where it is — capture-off is the matcher's, not the state's"
    );

    // Not a vacuous pass: the persisting form claims exactly this hunk.
    let refreshed = repo.refresh().unwrap();
    assert_eq!(
        owners(&refreshed.snapshot, "a.txt"),
        vec![Some("one".into())]
    );
}

#[test]
fn overlap_inheritance_still_shows() {
    // Record-derived ownership is what the records say, so it survives
    // the filter: an edited hunk still reads as its owner's.
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", "one\ntwo\nthree\n")
        .commit_all("init");
    let repo = Repo::discover(fixture.path()).unwrap();
    repo.create_changelist("one").unwrap();
    repo.switch(Some("one")).unwrap();
    fixture.write("a.txt", "one\ntwo edited\nthree\n");
    repo.refresh().unwrap();

    // Re-edited: no exact anchor match left, so ownership can only come
    // from tier 2's overlap claim on the stored record.
    fixture.write("a.txt", "one\ntwo edited again\nthree\n");

    let snapshot = repo.read_only_refresh().unwrap();
    assert_eq!(owners(&snapshot, "a.txt"), vec![Some("one".into())]);
}

#[test]
fn a_dormant_records_revival_still_shows() {
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", "one\ntwo\nthree\n")
        .commit_all("init");
    let repo = Repo::discover(fixture.path()).unwrap();
    repo.create_changelist("one").unwrap();
    repo.switch(Some("one")).unwrap();
    fixture.write("a.txt", "one\ntwo edited\nthree\n");
    repo.refresh().unwrap();

    // The hunk goes away and its record goes dormant, then the exact
    // content comes back.
    fixture.write("a.txt", "one\ntwo\nthree\n");
    repo.refresh().unwrap();
    fixture.write("a.txt", "one\ntwo edited\nthree\n");

    let state_path = repo.state_file_path();
    let before = std::fs::read(&state_path).unwrap();
    let snapshot = repo.read_only_refresh().unwrap();

    assert_eq!(
        owners(&snapshot, "a.txt"),
        vec![Some("one".into())],
        "an exact anchor match reads its record's owner, dormant or not"
    );
    assert_eq!(
        std::fs::read(&state_path).unwrap(),
        before,
        "the revival is reported, never committed — the record stays dormant"
    );
}

#[test]
fn a_head_move_guarded_hunk_reports_unassigned() {
    // The one place a read shows less than a changelist's records do.
    // Under ADR 0012's guard the stranded record proves nothing about
    // this hunk — the persisting form calls that a capture, and a
    // capture is exactly what a read never previews.
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", "one\ntwo\nthree\nfour\nfive\n")
        .commit_all("init");
    let repo = Repo::discover(fixture.path()).unwrap();
    repo.create_changelist("one").unwrap();
    repo.switch(Some("one")).unwrap();
    fixture.write("a.txt", "one\ntwo edited\nthree\nfour\nfive\n");
    repo.refresh().unwrap();

    // A second changelist is active, so the control below can't land on
    // 'one' by luck.
    repo.create_changelist("two").unwrap();
    repo.switch(Some("two")).unwrap();

    // An external commit touching a.txt, and the recorded hunk reworked:
    // its anchor no longer matches tier 1, and tier 2 is disabled for the
    // path the HEAD move changed.
    fixture
        .write("a.txt", "one\ntwo\nthree\nfour committed\nfive\n")
        .stage("a.txt")
        .write("a.txt", "one\ntwo reworked\nthree\nfour committed\nfive\n")
        .commit_index("external");

    let snapshot = repo.read_only_refresh().unwrap();
    assert_eq!(owners(&snapshot, "a.txt"), vec![None]);

    // Not a vacuous pass: the persisting form does decide, and its
    // decision is a capture to the active changelist.
    let refreshed = repo.refresh().unwrap();
    assert_eq!(
        owners(&refreshed.snapshot, "a.txt"),
        vec![Some("two".into())],
        "the guarded tier captures to active — the preview a read withholds"
    );
}

#[test]
fn an_entry_unit_join_never_previews() {
    // ADR 0009's join is context-derived too (it reads the entry the
    // newcomer landed in), so a read shows the split the records hold:
    // the recorded hunk owned, the newcomer unassigned.
    let fixture = RepoFixture::new();
    fixture.write("notes.txt", "original\n").commit_all("init");
    let repo = Repo::discover(fixture.path()).unwrap();
    repo.create_changelist("work").unwrap();
    repo.switch(Some("work")).unwrap();
    fixture
        .write("notes.txt", "staged text\n")
        .stage("notes.txt");
    repo.refresh().unwrap();

    // The worktree turns binary under a second active changelist: a
    // whole-file hunk arrives in the entry 'work' already owns.
    repo.create_changelist("other").unwrap();
    repo.switch(Some("other")).unwrap();
    fixture.write_bytes("notes.txt", &[0u8, 1, 2, 3]);

    let snapshot = repo.read_only_refresh().unwrap();
    let owned: Vec<Option<String>> = owners(&snapshot, "notes.txt");
    assert_eq!(
        owned.iter().filter(|owner| owner.is_none()).count(),
        1,
        "the newcomer is recordless, so it reports unassigned: {owned:?}"
    );
    assert_eq!(
        owned
            .iter()
            .filter(|owner| owner.as_deref() == Some("work"))
            .count(),
        1,
        "and the recorded hunk keeps its owner: {owned:?}"
    );

    // Not a vacuous pass: the persisting form joins the newcomer to
    // 'work' rather than to the active changelist.
    let refreshed = repo.refresh().unwrap();
    assert_eq!(
        owners(&refreshed.snapshot, "notes.txt"),
        vec![Some("work".into()), Some("work".into())]
    );
}
