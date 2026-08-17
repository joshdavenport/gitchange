//! Auto-capture into the active changelist, and the advisories that make
//! each automatic decision visible exactly once.

use crate::support::RepoFixture;
use gitchange_core::{Advisory, HunkStage};

use super::helpers::{numbered, owners, record_at, repo, state_json};

#[test]
fn new_hunks_capture_to_the_active_changelist() {
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(20, &[]))
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();
    repo.create_changelist("two").unwrap();
    repo.switch(Some("two")).unwrap();

    fixture
        .write("a.txt", &numbered(20, &[(10, "ten!")]))
        .write("new.txt", "hello\n");

    let snapshot = repo.refresh().unwrap();
    assert_eq!(owners(&snapshot, "a.txt"), vec![Some("two".into())]);
    assert_eq!(owners(&snapshot, "new.txt"), vec![Some("two".into())]);
    let in_two: Vec<&str> = snapshot
        .files_in(Some("two"))
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    assert_eq!(in_two, vec!["a.txt", "new.txt"]);
    assert!(snapshot.files_in(Some("one")).is_empty());
}

#[test]
fn an_unowned_externally_staged_hunk_captures_to_active_with_an_advisory() {
    // ADR 0003 routes unowned *staged* hunks through ADR 0001's assignment
    // rules unchanged — the same capture the tests above drive with
    // worktree-only hunks, reached the ordinary way: `git add` before
    // gitchange ever saw the hunk (a pre-launch stage, `git add -p`, a
    // hook). The capture must be as loud here as anywhere, and it must not
    // disturb the derived staged state it arrives with.
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(20, &[]))
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();
    repo.create_changelist("two").unwrap();
    repo.switch(Some("two")).unwrap();

    // Staged before any refresh could record it, so nothing owns it.
    fixture
        .write("a.txt", &numbered(20, &[(10, "ten!")]))
        .stage("a.txt");

    let snapshot = repo.refresh().unwrap();
    assert_eq!(owners(&snapshot, "a.txt"), vec![Some("two".into())]);
    assert_eq!(
        snapshot.files[0].hunks[0].stage,
        HunkStage::Staged,
        "captured as it stands: the index is untouched by the capture"
    );
    assert_eq!(
        snapshot.advisories,
        vec![Advisory::AutoCaptured {
            path: "a.txt".into(),
            new_start: 7,
            changelist: "two".into(),
        }],
        "a staged hunk's capture is announced like any other"
    );

    // And the record persisted, so the capture is a decision rather than a
    // per-refresh re-guess.
    let snapshot = repo.refresh().unwrap();
    assert_eq!(owners(&snapshot, "a.txt"), vec![Some("two".into())]);
    assert!(snapshot.advisories.is_empty());
}

#[test]
fn a_routine_auto_capture_notices_once() {
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(20, &[]))
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();
    repo.switch(Some("one")).unwrap();

    fixture.write("a.txt", &numbered(20, &[(10, "ten!")]));
    let snapshot = repo.refresh().unwrap();
    assert_eq!(
        snapshot.advisories,
        vec![Advisory::AutoCaptured {
            path: "a.txt".into(),
            new_start: 7,
            changelist: "one".into(),
        }],
        "a genuinely new hunk's capture is visible, never silent"
    );

    // The decision became a record: the next refresh is quiet.
    let snapshot = repo.refresh().unwrap();
    assert!(snapshot.advisories.is_empty());
}

#[test]
fn with_unassigned_active_nothing_notices() {
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(20, &[]))
        .commit_all("init")
        .write("a.txt", &numbered(20, &[(10, "ten!")]));

    // A changelist-less repo: unassigned is active by definition
    // (ADR 0015), so the edit falls through to it.
    let repo = repo(&fixture);
    let snapshot = repo.refresh().unwrap();
    assert_eq!(owners(&snapshot, "a.txt"), vec![None]);
    assert!(
        snapshot.advisories.is_empty(),
        "unassigned fall-through decides nothing worth spot-checking"
    );
}

#[test]
fn with_unassigned_active_new_hunks_stay_out_of_every_changelist() {
    // Capture-off (#52, ADR 0015): the changelists exist and one of them
    // was active a moment ago, but `switch unassigned` stops capture.
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(20, &[]))
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();
    repo.switch(Some("one")).unwrap();
    repo.switch(None).unwrap();

    fixture
        .write("a.txt", &numbered(20, &[(10, "ten!")]))
        .write("new.txt", "hello\n");

    let snapshot = repo.refresh().unwrap();
    assert_eq!(owners(&snapshot, "a.txt"), vec![None]);
    assert_eq!(owners(&snapshot, "new.txt"), vec![None]);
    assert!(snapshot.files_in(Some("one")).is_empty());
    assert!(
        snapshot.advisories.is_empty(),
        "nothing was decided, so nothing is advised"
    );

    // Switching back restores capture for what follows.
    repo.switch(Some("one")).unwrap();
    fixture.write("b.txt", "b\n");
    let snapshot = repo.refresh().unwrap();
    assert_eq!(owners(&snapshot, "b.txt"), vec![Some("one".into())]);
}

#[test]
fn with_unassigned_active_an_ambiguous_overlap_lands_unassigned_and_notices() {
    // The routing that the ambiguity rule can't infer: with no real
    // changelist active, the overlap resolves to unassigned — and says
    // so, because two changelists' claims were dropped on the way.
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(40, &[]))
        .commit_all("init");
    let repo = repo(&fixture);

    repo.create_changelist("one").unwrap();
    repo.switch(Some("one")).unwrap();
    fixture.write("a.txt", &numbered(40, &[(10, "ten!")]));
    repo.refresh().unwrap();

    repo.create_changelist("two").unwrap();
    repo.switch(Some("two")).unwrap();
    fixture.write("a.txt", &numbered(40, &[(10, "ten!"), (20, "twenty!")]));
    repo.refresh().unwrap();

    repo.switch(None).unwrap();
    // Bridge the gap: one fresh hunk now overlaps both records.
    let edits: Vec<(usize, String)> = (10..=20).map(|n| (n, format!("bridge {n}"))).collect();
    let edits: Vec<(usize, &str)> = edits.iter().map(|(n, s)| (*n, s.as_str())).collect();
    fixture.write("a.txt", &numbered(40, &edits));

    let snapshot = repo.refresh().unwrap();
    assert_eq!(owners(&snapshot, "a.txt"), vec![None]);
    assert_eq!(
        snapshot.advisories,
        vec![Advisory::AmbiguousOverlap {
            path: "a.txt".into(),
            new_start: 7,
            candidates: vec!["one".into(), "two".into()],
            assigned_to: None,
        }]
    );
    assert_eq!(
        state_json(&fixture)["records"].as_array().unwrap().len(),
        0,
        "the dropped claims leave nothing behind: unassigned is recordless (ADR 0016)"
    );

    // Recordless and capture-off: the next refresh decides nothing.
    let snapshot = repo.refresh().unwrap();
    assert_eq!(owners(&snapshot, "a.txt"), vec![None]);
    assert!(snapshot.advisories.is_empty());
}

#[test]
fn with_unassigned_active_a_changed_return_hunk_stays_unassigned() {
    // The dormant-revival edge: exact-match revival is unaffected by the
    // marker, but a *changed* return is a fresh capture — which
    // capture-off routes to unassigned, silently, leaving the dormant
    // record dormant rather than reviving it into its old changelist.
    let mut fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(20, &[]))
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();
    repo.switch(Some("one")).unwrap();

    fixture.write("a.txt", &numbered(20, &[(10, "ten-stashed")]));
    repo.refresh().unwrap();
    fixture.stash();
    repo.refresh().unwrap();
    assert!(
        record_at(&state_json(&fixture), "a.txt")["dormant_since"].is_u64(),
        "precondition: the record is dormant"
    );

    repo.switch(None).unwrap();
    fixture.write("a.txt", &numbered(20, &[(10, "ten-different")]));

    let snapshot = repo.refresh().unwrap();
    assert_eq!(owners(&snapshot, "a.txt"), vec![None]);
    assert!(snapshot.advisories.is_empty());
    assert!(
        record_at(&state_json(&fixture), "a.txt")["dormant_since"].is_u64(),
        "'one' keeps its dormant record, unrevived"
    );

    // The exact hunk returning still revives it, marker or no marker.
    fixture.write("a.txt", &numbered(20, &[(10, "ten-stashed")]));
    let snapshot = repo.refresh().unwrap();
    assert_eq!(owners(&snapshot, "a.txt"), vec![Some("one".into())]);
}

#[test]
fn dormant_revival_notices_with_a_per_changelist_count() {
    let mut fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(30, &[]))
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();
    repo.switch(Some("one")).unwrap();

    // Two separate hunks, both owned by "one".
    fixture.write("a.txt", &numbered(30, &[(5, "five!"), (20, "twenty!")]));
    repo.refresh().unwrap();

    fixture.stash();
    repo.refresh().unwrap();

    fixture.stash_pop();
    let snapshot = repo.refresh().unwrap();
    assert_eq!(
        owners(&snapshot, "a.txt"),
        vec![Some("one".into()), Some("one".into())]
    );
    assert_eq!(
        snapshot.advisories,
        vec![Advisory::DormantRevival {
            path: "a.txt".into(),
            changelist: "one".into(),
            hunks: 2,
        }],
        "exact-match revival is an automatic decision: one notice, counted"
    );
}

#[test]
fn a_changed_return_hunk_notices_its_auto_capture() {
    // ADR 0007: a hunk returning *changed* at a dormant record's lines
    // doesn't revive (exact-match only) — it auto-captures to active,
    // and that capture emits its own notice.
    let mut fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(20, &[]))
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();
    repo.switch(Some("one")).unwrap();

    fixture.write("a.txt", &numbered(20, &[(10, "ten-stashed")]));
    repo.refresh().unwrap();
    fixture.stash();
    repo.refresh().unwrap();
    assert!(
        record_at(&state_json(&fixture), "a.txt")["dormant_since"].is_u64(),
        "precondition: the record is dormant"
    );

    // A different edit at the dormant record's lines, under a second
    // active changelist.
    repo.create_changelist("two").unwrap();
    repo.switch(Some("two")).unwrap();
    fixture.write("a.txt", &numbered(20, &[(10, "ten-different")]));

    let snapshot = repo.refresh().unwrap();
    assert_eq!(
        owners(&snapshot, "a.txt"),
        vec![Some("two".into())],
        "changed-return captures to active, never to the dormant owner"
    );
    assert_eq!(
        snapshot.advisories,
        vec![Advisory::AutoCaptured {
            path: "a.txt".into(),
            new_start: 7,
            changelist: "two".into(),
        }],
        "the changed-return capture is visible, never silent"
    );

    // The decision became a record: the next refresh is quiet.
    let snapshot = repo.refresh().unwrap();
    assert!(snapshot.advisories.is_empty());
}

#[test]
fn a_changed_return_binary_notices_its_auto_capture() {
    // The binary flavour of the same contract: different bytes at a
    // path with a dormant whole-file record are a fresh capture (ADR
    // 0009 revival needs the exact changed-side OID), with its notice.
    let fixture = RepoFixture::new();
    fixture
        .write_bytes("logo.png", &[0u8, 1, 2, 3])
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("art").unwrap();
    repo.switch(Some("art")).unwrap();
    fixture.write_bytes("logo.png", &[0u8, 9, 9]);
    repo.refresh().unwrap();

    // Revert on disk: the diff vanishes, the record goes dormant.
    fixture.write_bytes("logo.png", &[0u8, 1, 2, 3]);
    repo.refresh().unwrap();
    assert!(
        record_at(&state_json(&fixture), "logo.png")["dormant_since"].is_u64(),
        "precondition: the record is dormant"
    );

    // Different content at the path, under a second active changelist.
    repo.create_changelist("other").unwrap();
    repo.switch(Some("other")).unwrap();
    fixture.write_bytes("logo.png", &[0u8, 5, 5, 5]);

    let snapshot = repo.refresh().unwrap();
    assert_eq!(
        owners(&snapshot, "logo.png"),
        vec![Some("other".into())],
        "changed-return captures to active, never to the dormant owner"
    );
    assert_eq!(
        snapshot.advisories,
        vec![Advisory::AutoCaptured {
            path: "logo.png".into(),
            new_start: 0,
            changelist: "other".into(),
        }],
        "the changed-return capture is visible, never silent"
    );

    // The decision became a record: the next refresh is quiet.
    let snapshot = repo.refresh().unwrap();
    assert!(snapshot.advisories.is_empty());
}

#[test]
#[cfg(unix)]
fn a_zero_hunk_change_captures_and_is_assignable_like_any_other() {
    // ADR 0017: a mode-only change is one mode hunk, so it flows through
    // capture and the explicit assign op like any hunk — including the
    // release back to unassigned (ADR 0016).
    let fixture = RepoFixture::new();
    fixture.write("tool.sh", "#!/bin/sh\n").commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("scripts").unwrap();
    repo.switch(Some("scripts")).unwrap();
    repo.create_changelist("other").unwrap();

    fixture.set_exec("tool.sh");
    let snapshot = repo.refresh().unwrap();
    assert_eq!(owners(&snapshot, "tool.sh"), vec![Some("scripts".into())]);
    let in_scripts: Vec<&str> = snapshot
        .files_in(Some("scripts"))
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    assert_eq!(in_scripts, vec!["tool.sh"]);

    let hunk = snapshot.files[0].hunks[0].clone();
    let outcome = repo
        .assign_hunks("tool.sh", std::slice::from_ref(&hunk), Some("other"))
        .unwrap();
    assert!(outcome.advisories.is_empty());
    let snapshot = repo.refresh().unwrap();
    assert_eq!(owners(&snapshot, "tool.sh"), vec![Some("other".into())]);

    // Released under capture-off, so nothing re-claims it (ADR 0015/0016).
    repo.switch(None).unwrap();
    repo.assign_hunks("tool.sh", &[hunk], None).unwrap();
    let snapshot = repo.refresh().unwrap();
    assert_eq!(owners(&snapshot, "tool.sh"), vec![None]);
    assert_eq!(snapshot.files_in(None).len(), 1, "released to unassigned");
}
