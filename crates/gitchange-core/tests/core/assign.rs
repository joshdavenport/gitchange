//! Explicit membership assignment (tickets #32/#41): `Repo::assign_hunks`
//! re-anchors records under the locked state cycle; membership is asserted
//! through the public `Repo::refresh()` on real temp repos (ADR 0008),
//! never record internals.

use crate::support::RepoFixture;
use gitchange_core::{Advisory, Error, Repo, Snapshot};

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

#[test]
fn an_explicit_assign_to_unassigned_is_sticky_across_edits() {
    let (fixture, repo, snapshot) = two_hunk_fixture();
    let hunk = snapshot.files[0].hunks[1].clone();

    repo.assign_hunks("a.txt", &[hunk], None).unwrap();
    let after = repo.refresh().unwrap();
    assert_eq!(owners(&after, "a.txt"), vec![Some("fixes".into()), None]);

    // Editing the unassigned hunk keeps the claim — it is not captured
    // back by the active changelist (delete-orphan semantics).
    fixture.write(
        "a.txt",
        &numbered(30, &[(5, "five!"), (25, "twentyfive, edited!")]),
    );
    let edited = repo.refresh().unwrap();
    assert_eq!(owners(&edited, "a.txt"), vec![Some("fixes".into()), None]);
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
