//! Changelist sync ops + state-file persistence (ticket 23, ADR 0002).
//! Asserted through core's public ops against real temp repos (ADR 0008).

use std::fs;

use crate::support::RepoFixture;
use gitchange_core::{Error, RESERVED_NAMES, Repo};

fn repo(fixture: &RepoFixture) -> Repo {
    Repo::discover(fixture.path()).unwrap()
}

fn names_and_active(repo: &Repo) -> (Vec<String>, Option<String>) {
    let snapshot = repo.refresh().unwrap().snapshot;
    let names = snapshot
        .changelists
        .iter()
        .map(|cl| cl.name.clone())
        .collect();
    (names, snapshot.active)
}

#[test]
fn creating_a_changelist_never_moves_the_active_marker() {
    // ADR 0015: only `switch` moves the marker. A fresh repo has
    // unassigned active, and creating changelists leaves it there —
    // otherwise a create would turn capture back on under whoever
    // switched it off.
    let fixture = RepoFixture::new();
    let repo = repo(&fixture);

    repo.create_changelist("feature").unwrap();
    repo.create_changelist("bugfix").unwrap();

    let (names, active) = names_and_active(&repo);
    assert_eq!(names, vec!["feature", "bugfix"]);
    assert_eq!(active, None, "unassigned is still active");

    repo.switch(Some("bugfix")).unwrap();
    repo.create_changelist("chore").unwrap();
    let (_, active) = names_and_active(&repo);
    assert_eq!(active.as_deref(), Some("bugfix"));
}

#[test]
fn switching_to_unassigned_turns_capture_off_and_back_on() {
    // The capture-off state (#52, ADR 0015): `unassigned` is a valid
    // switch target, and switching back restores capture.
    let fixture = RepoFixture::new();
    let repo = repo(&fixture);
    repo.create_changelist("feature").unwrap();
    repo.switch(Some("feature")).unwrap();

    repo.switch(None).unwrap();
    let (names, active) = names_and_active(&repo);
    assert_eq!(names, vec!["feature"]);
    assert_eq!(active, None);

    repo.switch(Some("feature")).unwrap();
    let (_, active) = names_and_active(&repo);
    assert_eq!(active.as_deref(), Some("feature"));
}

#[test]
fn switch_sets_active_and_round_trips_across_repo_instances() {
    let fixture = RepoFixture::new();
    {
        let repo = repo(&fixture);
        repo.create_changelist("feature").unwrap();
        repo.create_changelist("bugfix").unwrap();
        repo.switch(Some("bugfix")).unwrap();
    }

    // A fresh handle (a separate invocation) sees the persisted marker.
    let repo = repo(&fixture);
    let (_, active) = names_and_active(&repo);
    assert_eq!(active.as_deref(), Some("bugfix"));
}

#[test]
fn switch_to_unknown_name_is_an_error() {
    let fixture = RepoFixture::new();
    let repo = repo(&fixture);
    repo.create_changelist("feature").unwrap();

    let err = repo.switch(Some("nope")).unwrap_err();
    assert!(matches!(err, Error::UnknownChangelist { name } if name == "nope"));
}

#[test]
fn reserved_names_are_rejected_on_create_and_rename() {
    let fixture = RepoFixture::new();
    let repo = repo(&fixture);
    repo.create_changelist("feature").unwrap();

    // Spelled out deliberately, not read from the const: this test pins
    // the reserved set's contents, so growing or shrinking
    // `RESERVED_NAMES` must fail here rather than silently redefine what
    // is being tested. The loop below then covers every name in it.
    assert_eq!(RESERVED_NAMES, ["all", "unassigned"]);

    for reserved in RESERVED_NAMES {
        let err = repo.create_changelist(reserved).unwrap_err();
        assert!(
            matches!(&err, Error::ReservedName { name } if name == reserved),
            "create {reserved:?}: {err:?}"
        );
        let err = repo.rename_changelist("feature", reserved).unwrap_err();
        assert!(
            matches!(&err, Error::ReservedName { name } if name == reserved),
            "rename to {reserved:?}: {err:?}"
        );
    }
}

#[test]
fn empty_name_is_rejected() {
    let fixture = RepoFixture::new();
    let repo = repo(&fixture);

    let err = repo.create_changelist("").unwrap_err();
    assert!(matches!(err, Error::InvalidName { .. }));
}

#[test]
fn duplicate_names_are_rejected_on_create_and_rename() {
    let fixture = RepoFixture::new();
    let repo = repo(&fixture);
    repo.create_changelist("feature").unwrap();
    repo.create_changelist("bugfix").unwrap();

    let err = repo.create_changelist("feature").unwrap_err();
    assert!(matches!(&err, Error::ChangelistExists { name } if name == "feature"));

    let err = repo.rename_changelist("bugfix", "feature").unwrap_err();
    assert!(matches!(&err, Error::ChangelistExists { name } if name == "feature"));
}

#[test]
fn rename_carries_the_active_marker() {
    let fixture = RepoFixture::new();
    let repo = repo(&fixture);
    repo.create_changelist("feature").unwrap();
    repo.switch(Some("feature")).unwrap();

    repo.rename_changelist("feature", "feature-x").unwrap();

    let (names, active) = names_and_active(&repo);
    assert_eq!(names, vec!["feature-x"]);
    assert_eq!(active.as_deref(), Some("feature-x"));
}

#[test]
fn rename_unknown_name_is_an_error() {
    let fixture = RepoFixture::new();
    let repo = repo(&fixture);

    let err = repo.rename_changelist("nope", "other").unwrap_err();
    assert!(matches!(err, Error::UnknownChangelist { name } if name == "nope"));
}

#[test]
fn deleting_the_active_changelist_leaves_unassigned_active() {
    // ADR 0015: promoting a neighbour would point capture at a
    // changelist nobody named — in a shared tree, possibly another
    // actor's. Capture stops instead, where the `*` says so.
    let fixture = RepoFixture::new();
    let repo = repo(&fixture);
    repo.create_changelist("feature").unwrap();
    repo.switch(Some("feature")).unwrap();
    repo.create_changelist("bugfix").unwrap();

    repo.delete_changelist("feature").unwrap();

    let (names, active) = names_and_active(&repo);
    assert_eq!(names, vec!["bugfix"]);
    assert_eq!(active, None);
}

#[test]
fn deleting_an_inactive_changelist_leaves_the_marker_alone() {
    let fixture = RepoFixture::new();
    let repo = repo(&fixture);
    repo.create_changelist("feature").unwrap();
    repo.create_changelist("bugfix").unwrap();
    repo.switch(Some("feature")).unwrap();

    repo.delete_changelist("bugfix").unwrap();

    let (names, active) = names_and_active(&repo);
    assert_eq!(names, vec!["feature"]);
    assert_eq!(active.as_deref(), Some("feature"));
}

#[test]
fn deleting_the_last_changelist_leaves_unassigned_active() {
    let fixture = RepoFixture::new();
    let repo = repo(&fixture);
    repo.create_changelist("feature").unwrap();
    // Held by the changelist first, so the delete is what moves it.
    repo.switch(Some("feature")).unwrap();

    repo.delete_changelist("feature").unwrap();

    let (names, active) = names_and_active(&repo);
    assert!(names.is_empty());
    assert_eq!(active, None);
}

#[test]
fn delete_unknown_name_is_an_error() {
    let fixture = RepoFixture::new();
    let repo = repo(&fixture);

    let err = repo.delete_changelist("nope").unwrap_err();
    assert!(matches!(err, Error::UnknownChangelist { name } if name == "nope"));
}

#[test]
fn state_file_is_pretty_json_with_schema_version_at_the_git_path() {
    let fixture = RepoFixture::new();
    let repo = repo(&fixture);
    repo.create_changelist("feature").unwrap();
    repo.switch(Some("feature")).unwrap();

    let state_path = fixture.path().join(".git/gitchange/state.json");
    let raw = fs::read_to_string(&state_path).expect("state file exists");
    assert!(raw.contains('\n'), "expected pretty-printed JSON");
    let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(json["version"], 1);
    assert_eq!(json["active"], "feature");
    assert_eq!(json["changelists"][0]["name"], "feature");
}

#[test]
fn a_held_lockfile_fails_fast_with_lock_contention() {
    let fixture = RepoFixture::new();
    let repo = repo(&fixture);
    repo.create_changelist("feature").unwrap();

    let lock_path = fixture.path().join(".git/gitchange/state.json.lock");
    fs::write(&lock_path, "").unwrap();

    let err = repo.create_changelist("bugfix").unwrap_err();
    assert!(matches!(err, Error::LockContention { .. }), "{err:?}");

    // The held lock was not stolen or removed by the failed attempt.
    assert!(lock_path.exists());
    fs::remove_file(&lock_path).unwrap();
    repo.create_changelist("bugfix").unwrap();
}

#[test]
fn linked_worktrees_have_independent_state_files() {
    let fixture = RepoFixture::new();
    fixture.write("a.txt", "content\n").commit_all("init");
    let worktree_path = fixture.add_worktree("side");

    let main_repo = repo(&fixture);
    let side_repo = Repo::discover(&worktree_path).unwrap();

    main_repo.create_changelist("main-work").unwrap();
    main_repo.switch(Some("main-work")).unwrap();
    side_repo.create_changelist("side-work").unwrap();
    side_repo.switch(Some("side-work")).unwrap();

    let (main_names, main_active) = names_and_active(&main_repo);
    let (side_names, side_active) = names_and_active(&side_repo);
    assert_eq!(main_names, vec!["main-work"]);
    assert_eq!(main_active.as_deref(), Some("main-work"));
    assert_eq!(side_names, vec!["side-work"]);
    assert_eq!(side_active.as_deref(), Some("side-work"));

    // Per ADR 0002: the linked worktree's state lives under its private
    // git dir, not the shared one.
    assert!(
        fixture
            .path()
            .join(".git/worktrees/side/gitchange/state.json")
            .exists()
    );
}

#[test]
fn a_missing_state_file_reads_as_no_changelists() {
    let fixture = RepoFixture::new();
    let repo = repo(&fixture);

    let (names, active) = names_and_active(&repo);
    assert!(names.is_empty());
    assert_eq!(active, None);
}

#[test]
fn an_unsupported_schema_version_is_a_loud_error() {
    let fixture = RepoFixture::new();
    let dir = fixture.path().join(".git/gitchange");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("state.json"),
        r#"{ "version": 99, "active": null, "changelists": [] }"#,
    )
    .unwrap();

    let repo = repo(&fixture);
    let err = repo.refresh().unwrap_err();
    assert!(matches!(err, Error::State(_)), "{err:?}");
}
