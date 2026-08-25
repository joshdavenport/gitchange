//! Changelist sync ops + state-file persistence (ticket 23, ADR 0002).
//! Asserted through core's public ops against real temp repos (ADR 0008).

use std::fs;

use crate::support::{RepoFixture, delete, done, offenders};
use gitchange_core::{Error, RESERVED_NAMES, Release, Repo, Undeletable};

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

    delete(&repo, "feature");

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

    delete(&repo, "bugfix");

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

    delete(&repo, "feature");

    let (names, active) = names_and_active(&repo);
    assert!(names.is_empty());
    assert_eq!(active, None);
}

#[test]
fn delete_refuses_an_unrecognised_name_with_the_candidates() {
    // Not an error but an offender (#149): a delete of several names has
    // to report every one that failed, and an error carries one.
    // Reserved names are unrecognised here too — neither is a changelist
    // a delete could act on (ADR 0016).
    let fixture = RepoFixture::new();
    let repo = repo(&fixture);
    repo.create_changelist("feature").unwrap();

    for name in ["nope", "unassigned", "all"] {
        let refused = repo.delete_changelists(&[name], Release::Guarded).unwrap();
        assert_eq!(
            offenders(refused),
            vec![Undeletable::Unrecognised {
                name: name.to_owned(),
                candidates: vec!["feature".to_owned()],
            }],
            "'{name}' is not a changelist"
        );
    }
    assert_eq!(names_and_active(&repo).0, vec!["feature"]);
}

#[test]
fn delete_validates_every_name_before_deleting_any() {
    // All-or-nothing against one locked read (#149): one bad name among
    // good ones deletes nothing, so the retry is the same command
    // corrected — and every offender is named, so it takes one round trip.
    let fixture = RepoFixture::new();
    let repo = repo(&fixture);
    repo.create_changelist("feature").unwrap();
    repo.create_changelist("docs").unwrap();

    let refused = repo
        .delete_changelists(&["feature", "nope", "docs", "typo"], Release::Guarded)
        .unwrap();

    let names: Vec<String> = offenders(refused)
        .into_iter()
        .map(|offender| match offender {
            Undeletable::Unrecognised { name, .. } => name,
            other => panic!("unexpected offender: {other:?}"),
        })
        .collect();
    assert_eq!(names, vec!["nope", "typo"], "in argument order");
    assert_eq!(
        names_and_active(&repo).0,
        vec!["feature", "docs"],
        "a refused delete deleted nothing"
    );
}

#[test]
fn deleting_several_changelists_is_one_op_and_one_echo() {
    // One invocation is one op (#122), so it says one line however many
    // names it carried — and a name given twice is one delete, absorbed
    // here rather than left for every caller to dedupe.
    let fixture = RepoFixture::new();
    let repo = repo(&fixture);
    repo.create_changelist("feature").unwrap();
    repo.create_changelist("docs").unwrap();
    repo.create_changelist("keep").unwrap();

    let deleted = done(
        repo.delete_changelists(&["docs", "feature", "docs"], Release::Guarded)
            .unwrap(),
    );

    assert_eq!(
        deleted.echo.as_deref(),
        Some("deleted changelists 'docs', 'feature'")
    );
    assert_eq!(names_and_active(&repo).0, vec!["keep"]);
}

#[test]
fn the_bare_write_ops_echo_the_decision_they_made() {
    // ADR 0006/0007: the echo is composed here, beside the write, so no
    // frontend spells one of these lines itself and drifts.
    let fixture = RepoFixture::new();
    let repo = repo(&fixture);

    let created = repo.create_changelist("feature").unwrap();
    assert_eq!(
        created.echo.as_deref(),
        Some("created changelist 'feature'")
    );

    let switched = repo.switch(Some("feature")).unwrap();
    assert_eq!(switched.echo.as_deref(), Some("switched to 'feature'"));

    let renamed = repo.rename_changelist("feature", "feature-x").unwrap();
    assert_eq!(
        renamed.echo.as_deref(),
        Some("renamed changelist 'feature' → 'feature-x'")
    );

    // Unassigned is a switch target like any other (ADR 0015), and the
    // echo names it as the target rather than as a changelist.
    let switched = repo.switch(None).unwrap();
    assert_eq!(switched.echo.as_deref(), Some("switched to 'unassigned'"));

    let deleted = delete(&repo, "feature-x");
    assert_eq!(
        deleted.echo.as_deref(),
        Some("deleted changelist 'feature-x'")
    );
    assert!(deleted.advisories.is_empty(), "the marker was elsewhere");
}

#[test]
fn an_op_that_decides_nothing_echoes_nothing() {
    // #122: silence, not a comfort line — git's "Already on 'x'" is not
    // borrowed, so a caller reading stdout sees a decision or nothing.
    let fixture = RepoFixture::new();
    let repo = repo(&fixture);
    repo.create_changelist("feature").unwrap();
    repo.switch(Some("feature")).unwrap();

    let switched = repo.switch(Some("feature")).unwrap();
    assert_eq!(switched.echo, None, "already the active changelist");

    let renamed = repo.rename_changelist("feature", "feature").unwrap();
    assert_eq!(renamed.echo, None, "renamed to the name it already has");

    // Switching to unassigned from unassigned is the same nothing, on
    // the target that has no changelist behind it.
    repo.switch(None).unwrap();
    assert_eq!(repo.switch(None).unwrap().echo, None);
}

#[test]
fn deleting_the_active_changelist_notices_that_the_marker_moved() {
    // The marker moved without being asked to (ADR 0015), so the delete's
    // receipt says so — one canonical message, composed here (ADR 0006).
    let fixture = RepoFixture::new();
    let repo = repo(&fixture);
    repo.create_changelist("feature").unwrap();
    repo.switch(Some("feature")).unwrap();

    let deleted = delete(&repo, "feature");

    assert_eq!(
        deleted.echo.as_deref(),
        Some("deleted changelist 'feature'")
    );
    let messages: Vec<String> = deleted
        .advisories
        .iter()
        .map(|advisory| advisory.message())
        .collect();
    assert_eq!(
        messages,
        vec!["'feature' was the active changelist — unassigned is active now"]
    );
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
