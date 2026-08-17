//! Explicit membership assignment (tickets #32/#41): `Repo::assign_hunks`
//! re-anchors records under the locked state cycle; an unassigned target
//! releases — deletes records, writes nothing (ADR 0016). Membership is
//! asserted through the public `Repo::refresh()` on real temp repos
//! (ADR 0008); the state file is read only to prove recordlessness.

use crate::support::RepoFixture;
use gitchange_core::{Advisory, Error, HunkStage, Repo, Snapshot};

/// `count` numbered lines, with `edits` as (1-based line, replacement).
fn numbered(count: usize, edits: &[(usize, &str)]) -> String {
    (1..=count)
        .map(|n| {
            edits
                .iter()
                .find(|(line, _)| *line == n)
                .map(|(_, text)| format!("{text}\n"))
                .unwrap_or_else(|| format!("line {n}\n"))
        })
        .collect()
}

fn repo(fixture: &RepoFixture) -> Repo {
    Repo::discover(fixture.path()).unwrap()
}

/// Each hunk's owning changelist for `path`, in file order.
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

/// A repo with `a.txt` carrying two well-separated hunks, both captured
/// by the active changelist 'fixes'; 'chores' exists as an assign target.
fn two_hunk_fixture() -> (RepoFixture, Repo, Snapshot) {
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(30, &[]))
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("fixes").unwrap();
    repo.create_changelist("chores").unwrap();
    repo.switch(Some("fixes")).unwrap();
    fixture.write("a.txt", &numbered(30, &[(5, "five!"), (25, "twentyfive!")]));
    let snapshot = repo.refresh().unwrap();
    assert_eq!(
        owners(&snapshot, "a.txt"),
        vec![Some("fixes".into()), Some("fixes".into())],
        "both fresh hunks capture to the active changelist"
    );
    (fixture, repo, snapshot)
}

#[test]
fn assigning_a_hunk_to_another_changelist_updates_membership_on_refresh() {
    let (_fixture, repo, snapshot) = two_hunk_fixture();
    let hunk = snapshot.files[0].hunks[1].clone();

    let advisories = repo
        .assign_hunks("a.txt", &[hunk], Some("chores"))
        .unwrap()
        .advisories;
    assert!(advisories.is_empty());

    let after = repo.refresh().unwrap();
    assert_eq!(
        owners(&after, "a.txt"),
        vec![Some("fixes".into()), Some("chores".into())]
    );
    // File counts follow hunk-level membership: the file now appears in
    // both changelists.
    assert_eq!(after.files_in(Some("fixes")).len(), 1);
    assert_eq!(after.files_in(Some("chores")).len(), 1);
    assert!(
        after.advisories.is_empty(),
        "a clean assign raises no advisories"
    );
}

#[test]
fn assigning_every_hunk_empties_the_source_changelist() {
    let (_fixture, repo, snapshot) = two_hunk_fixture();
    let hunks = snapshot.files[0].hunks.clone();

    let advisories = repo
        .assign_hunks("a.txt", &hunks, Some("chores"))
        .unwrap()
        .advisories;
    assert!(advisories.is_empty());

    let after = repo.refresh().unwrap();
    assert_eq!(
        owners(&after, "a.txt"),
        vec![Some("chores".into()), Some("chores".into())]
    );
    assert!(after.files_in(Some("fixes")).is_empty());
}

/// The persisted records' owners, in file order — read straight off the
/// state file (ADR 0002's `cat`-debuggable shape).
fn stored_owners(fixture: &RepoFixture) -> Vec<serde_json::Value> {
    let raw = std::fs::read_to_string(fixture.path().join(".git/gitchange/state.json")).unwrap();
    let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
    json["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|record| record["changelist"].clone())
        .collect()
}

#[test]
fn an_explicit_assign_to_unassigned_releases_to_the_active_changelist() {
    // ADR 0016: assigning to unassigned deletes the record and writes
    // nothing back. The hunk is then recordless, and the next refresh
    // runs the uniform rule on it: 'fixes' is active, so it captures —
    // loudly, like any auto-capture.
    let (_fixture, repo, snapshot) = two_hunk_fixture();
    let hunk = snapshot.files[0].hunks[1].clone();

    let outcome = repo.assign_hunks("a.txt", &[hunk], None).unwrap();
    assert_eq!(
        outcome.echo.as_deref(),
        Some("released 1 hunk — a.txt"),
        "the echo states a release, not an assignment"
    );

    let after = repo.refresh().unwrap();
    assert_eq!(
        owners(&after, "a.txt"),
        vec![Some("fixes".into()), Some("fixes".into())]
    );
    assert_eq!(
        after.advisories,
        vec![Advisory::AutoCaptured {
            path: "a.txt".into(),
            new_start: 22,
            changelist: "fixes".into(),
        }],
        "the released hunk's re-capture is visible, never silent"
    );
}

#[test]
fn with_unassigned_active_a_release_stays_unassigned_across_edits() {
    // The supported way to keep hunks loose (ADR 0016): capture off
    // first. The release deletes the record; with unassigned active,
    // nothing re-claims the recordless hunk — not even an edit.
    let (fixture, repo, snapshot) = two_hunk_fixture();
    let hunk = snapshot.files[0].hunks[1].clone();

    repo.switch(None).unwrap();
    repo.assign_hunks("a.txt", &[hunk], None).unwrap();

    let after = repo.refresh().unwrap();
    assert_eq!(owners(&after, "a.txt"), vec![Some("fixes".into()), None]);
    assert_eq!(
        stored_owners(&fixture),
        vec![serde_json::json!("fixes")],
        "the released hunk is recordless: only the other hunk's record remains"
    );

    fixture.write(
        "a.txt",
        &numbered(30, &[(5, "five!"), (25, "twentyfive, edited!")]),
    );
    let edited = repo.refresh().unwrap();
    assert_eq!(owners(&edited, "a.txt"), vec![Some("fixes".into()), None]);
}

#[test]
fn releasing_recordless_hunks_is_a_true_no_op() {
    // Assigning already-recordless hunks to unassigned deletes nothing
    // and writes nothing — a changelist-less repo must not even grow a
    // state file from it (ADR 0016).
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(30, &[]))
        .commit_all("init")
        .write("a.txt", &numbered(30, &[(5, "five!")]));
    let repo = repo(&fixture);
    let snapshot = repo.refresh().unwrap();
    assert_eq!(owners(&snapshot, "a.txt"), vec![None]);

    let hunk = snapshot.files[0].hunks[0].clone();
    let outcome = repo.assign_hunks("a.txt", &[hunk], None).unwrap();
    assert_eq!(outcome.echo.as_deref(), Some("released 1 hunk — a.txt"));
    assert!(
        !fixture.path().join(".git/gitchange/state.json").exists(),
        "a no-op release writes no state file"
    );
}

#[test]
fn a_stale_hunk_fails_soft_and_the_fresh_one_is_still_assigned() {
    let (fixture, repo, snapshot) = two_hunk_fixture();
    let hunks = snapshot.files[0].hunks.clone();
    let stale_start = hunks[0].new_start;

    // The first hunk's content changes under the snapshot's feet.
    fixture.write(
        "a.txt",
        &numbered(30, &[(5, "five, changed!"), (25, "twentyfive!")]),
    );

    let advisories = repo
        .assign_hunks("a.txt", &hunks, Some("chores"))
        .unwrap()
        .advisories;
    assert_eq!(
        advisories,
        vec![Advisory::StaleHunk {
            path: "a.txt".into(),
            new_start: stale_start,
        }],
        "the vanished hunk fails soft; nothing else is reported"
    );

    let after = repo.refresh().unwrap();
    assert_eq!(
        owners(&after, "a.txt"),
        vec![Some("fixes".into()), Some("chores".into())],
        "the stale hunk keeps its owner; the fresh one was assigned"
    );
}

#[test]
fn a_binary_whole_file_hunk_still_assigns_after_a_mid_flight_rewrite() {
    // Path continuity (ADR 0009) at the validate-at-apply boundary
    // (ADR 0005): the whole file *is* the hunk, so a build rewriting the
    // export between snapshot and keypress must not turn the assign into
    // the stale no-op the test above asserts for text — that would be the
    // membership loss the whole-file hunk exists to prevent.
    let fixture = RepoFixture::new();
    fixture
        .write_bytes("logo.png", &[0u8, 1, 2, 3])
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("fixes").unwrap();
    repo.switch(Some("fixes")).unwrap();
    repo.create_changelist("chores").unwrap();
    fixture.write_bytes("logo.png", &[0u8, 9, 9]);
    let snapshot = repo.refresh().unwrap();
    assert_eq!(owners(&snapshot, "logo.png"), vec![Some("fixes".into())]);
    let hunk = snapshot.files[0].hunks[0].clone();

    // The export is rebuilt under the snapshot's feet: a new blob OID, so
    // an OID-matched re-find would miss it.
    fixture.write_bytes("logo.png", &[0u8, 7, 7, 7, 7, 7]);

    let outcome = repo
        .assign_hunks("logo.png", &[hunk], Some("chores"))
        .unwrap();
    assert!(
        outcome.advisories.is_empty(),
        "the hunk is re-found at its path: no StaleHunk"
    );
    assert_eq!(
        outcome.echo.as_deref(),
        Some("assigned 1 hunk — logo.png → 'chores'")
    );

    let after = repo.refresh().unwrap();
    assert_eq!(owners(&after, "logo.png"), vec![Some("chores".into())]);
    assert!(
        after.advisories.is_empty(),
        "a clean assign raises no advisories"
    );
}

#[test]
fn assigning_to_an_unknown_changelist_is_an_error_and_changes_nothing() {
    let (_fixture, repo, snapshot) = two_hunk_fixture();
    let hunk = snapshot.files[0].hunks[1].clone();

    let err = repo
        .assign_hunks("a.txt", &[hunk], Some("nope"))
        .unwrap_err();
    assert!(matches!(err, Error::UnknownChangelist { name } if name == "nope"));

    let after = repo.refresh().unwrap();
    assert_eq!(
        owners(&after, "a.txt"),
        vec![Some("fixes".into()), Some("fixes".into())]
    );
}

#[test]
fn an_assigned_hunk_keeps_its_new_owner_when_edited() {
    let (fixture, repo, snapshot) = two_hunk_fixture();
    let hunk = snapshot.files[0].hunks[1].clone();
    repo.assign_hunks("a.txt", &[hunk], Some("chores")).unwrap();
    repo.refresh().unwrap();

    // The old owner's record must not linger and re-claim the hunk via
    // overlap once the anchor changes.
    fixture.write(
        "a.txt",
        &numbered(30, &[(5, "five!"), (25, "twentyfive, edited!")]),
    );
    let after = repo.refresh().unwrap();
    assert_eq!(
        owners(&after, "a.txt"),
        vec![Some("fixes".into()), Some("chores".into())]
    );
}

/// The #106 content shape: HEAD holds text, the index holds a staged text
/// edit, and the worktree holds binary bytes — one index entry presenting
/// a whole-file hunk and an index-only content hunk. Both arrive together,
/// so the active changelist captures the unit whole; 'chores' waits as an
/// assign target.
fn shared_entry_fixture() -> (RepoFixture, Repo, Snapshot) {
    let fixture = RepoFixture::new();
    fixture.write("notes.txt", "original\n").commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("fixes").unwrap();
    repo.create_changelist("chores").unwrap();
    repo.switch(Some("fixes")).unwrap();
    fixture
        .write("notes.txt", "staged text\n")
        .stage("notes.txt");
    fixture.write_bytes("notes.txt", &[0u8, 1, 2, 3]);
    let snapshot = repo.refresh().unwrap();
    assert!(
        snapshot.files[0].hunks[0].is_whole_file(),
        "the worktree's binary rewrite, as a whole-file hunk"
    );
    assert_eq!(
        owners(&snapshot, "notes.txt"),
        vec![Some("fixes".into()), Some("fixes".into())],
        "precondition: one owner across the entry"
    );
    (fixture, repo, snapshot)
}

#[test]
fn assigning_a_whole_file_hunk_moves_the_content_sharing_its_index_entry() {
    // ADR 0009: the entry holds one blob and a whole-file payload commits
    // it verbatim, so the whole-file hunk and the content hunks beside it
    // are one assignable unit — pointing at either moves both, and the
    // echo counts what actually moved.
    let (_fixture, repo, snapshot) = shared_entry_fixture();
    let whole_file = snapshot.files[0].hunks[0].clone();

    let outcome = repo
        .assign_hunks("notes.txt", &[whole_file], Some("chores"))
        .unwrap();
    assert_eq!(
        outcome.echo.as_deref(),
        Some("assigned 2 hunks — notes.txt → 'chores'"),
        "the unit's other hunk is in the payload, so the echo says two"
    );

    let after = repo.refresh().unwrap();
    assert_eq!(
        owners(&after, "notes.txt"),
        vec![Some("chores".into()), Some("chores".into())]
    );
}

#[test]
fn assigning_the_entrys_content_hunk_moves_the_whole_file_hunk_with_it() {
    // The other direction of the same unit: the index-only content hunk
    // cannot leave the entry on its own either.
    let (_fixture, repo, snapshot) = shared_entry_fixture();
    let content = snapshot.files[0].hunks[1].clone();

    let outcome = repo
        .assign_hunks("notes.txt", &[content], Some("chores"))
        .unwrap();
    assert_eq!(
        outcome.echo.as_deref(),
        Some("assigned 2 hunks — notes.txt → 'chores'")
    );

    let after = repo.refresh().unwrap();
    assert_eq!(
        owners(&after, "notes.txt"),
        vec![Some("chores".into()), Some("chores".into())]
    );
}

#[test]
fn releasing_a_shared_entry_releases_the_whole_unit() {
    // The unit holds for a release too (ADR 0016): the records of both
    // hunks are deleted, so neither is left behind claimed while the other
    // is loose. Capture-off first, or the uniform rule would re-claim them
    // on the next refresh.
    let (_fixture, repo, snapshot) = shared_entry_fixture();
    let whole_file = snapshot.files[0].hunks[0].clone();

    repo.switch(None).unwrap();
    let outcome = repo.assign_hunks("notes.txt", &[whole_file], None).unwrap();
    assert_eq!(
        outcome.echo.as_deref(),
        Some("released 2 hunks — notes.txt")
    );

    let after = repo.refresh().unwrap();
    assert_eq!(owners(&after, "notes.txt"), vec![None, None]);
}

#[test]
#[cfg(unix)]
fn a_mode_hunk_stays_outside_the_index_entry_unit() {
    // ADR 0017's boundary, which the unit rule leaves alone: a mode hunk
    // has its own index write and no content, so it neither moves with the
    // entry nor drags the entry with it.
    let fixture = RepoFixture::new();
    fixture.write("notes.txt", "original\n").commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("fixes").unwrap();
    repo.create_changelist("chores").unwrap();
    repo.switch(Some("fixes")).unwrap();
    fixture
        .write("notes.txt", "staged text\n")
        .stage("notes.txt");
    fixture.write_bytes("notes.txt", &[0u8, 1, 2, 3]);
    fixture.set_exec("notes.txt");
    let snapshot = repo.refresh().unwrap();
    let hunks = &snapshot.files[0].hunks;
    assert!(hunks[0].is_mode_change(), "the mode hunk sits first");
    assert!(hunks[1].is_whole_file());

    // The entry's unit moves without the mode hunk.
    let outcome = repo
        .assign_hunks("notes.txt", &[hunks[1].clone()], Some("chores"))
        .unwrap();
    assert_eq!(
        outcome.echo.as_deref(),
        Some("assigned 2 hunks — notes.txt → 'chores'")
    );
    let after = repo.refresh().unwrap();
    assert_eq!(
        owners(&after, "notes.txt"),
        vec![
            Some("fixes".into()),
            Some("chores".into()),
            Some("chores".into())
        ],
        "the mode hunk stays where it was"
    );

    // And the mode hunk moves without the entry.
    let mode = after.files[0].hunks[0].clone();
    let outcome = repo
        .assign_hunks("notes.txt", &[mode], Some("fixes"))
        .unwrap();
    assert_eq!(
        outcome.echo.as_deref(),
        Some("assigned 1 hunk — notes.txt → 'fixes'"),
        "a mode hunk is its own unit"
    );
    let after = repo.refresh().unwrap();
    assert_eq!(
        owners(&after, "notes.txt"),
        vec![
            Some("fixes".into()),
            Some("chores".into()),
            Some("chores".into())
        ]
    );
}

#[test]
fn the_entry_unit_holds_across_unstaged_content() {
    // ADR 0009: unit membership is independent of staging. Here the index
    // holds the binary rewrite and the worktree has been edited back to
    // text, so the entry's whole-file hunk is index-only and the content
    // hunk beside it is unstaged. They still assign as one unit — were the
    // unit only what the index holds, staging that content hunk would slide
    // it into the entry under a second owner, which is the split the rule
    // exists to prevent.
    let fixture = RepoFixture::new();
    fixture.write("notes.txt", "original\n").commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("fixes").unwrap();
    repo.create_changelist("chores").unwrap();
    repo.switch(Some("fixes")).unwrap();
    fixture
        .write_bytes("notes.txt", &[0u8, 1, 2, 3])
        .stage("notes.txt");
    fixture.write("notes.txt", "edited text\n");
    let snapshot = repo.refresh().unwrap();
    let hunks = &snapshot.files[0].hunks;
    assert!(hunks[0].is_whole_file(), "the staged binary");
    assert_eq!(hunks[1].stage, HunkStage::Unstaged, "the worktree's text");

    let outcome = repo
        .assign_hunks("notes.txt", &[hunks[0].clone()], Some("chores"))
        .unwrap();
    assert_eq!(
        outcome.echo.as_deref(),
        Some("assigned 2 hunks — notes.txt → 'chores'"),
        "the unstaged content hunk is in the unit too"
    );
    let after = repo.refresh().unwrap();
    assert_eq!(
        owners(&after, "notes.txt"),
        vec![Some("chores".into()), Some("chores".into())]
    );
}
