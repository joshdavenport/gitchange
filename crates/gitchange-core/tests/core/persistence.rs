//! The persistence properties ADR 0002 states about the file rather than
//! about an operation (issue #60): changelists are global to the working
//! tree and ride across a branch switch, the atomic write-then-rename
//! leaves no temp sibling, and nothing gitchange does writes a git
//! object. Asserted through core's public ops on real temp repos
//! (ADR 0008) — the operation-shaped half of ADR 0002 lives in
//! `tests/core/changelists.rs`, and dormancy in
//! `tests/core/matcher/dormancy.rs`.

use std::fs;

use crate::support::RepoFixture;
use gitchange_core::{ChangedFile, CommitOptions, Head, Repo, Snapshot};

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

fn file_of<'a>(snapshot: &'a Snapshot, path: &str) -> &'a ChangedFile {
    snapshot
        .files
        .iter()
        .find(|file| file.path == path)
        .unwrap_or_else(|| panic!("{path} not in snapshot"))
}

/// Each hunk's owning changelist for `path`, in file order.
fn owners(snapshot: &Snapshot, path: &str) -> Vec<Option<String>> {
    file_of(snapshot, path)
        .hunks
        .iter()
        .map(|hunk| hunk.changelist.clone())
        .collect()
}

/// Each hunk's worktree-side start line for `path`, in file order — the
/// coordinates a re-derivation must land back on.
fn starts(snapshot: &Snapshot, path: &str) -> Vec<u32> {
    file_of(snapshot, path)
        .hunks
        .iter()
        .map(|hunk| hunk.new_start)
        .collect()
}

#[test]
fn owned_dirty_hunks_ride_across_a_branch_switch() {
    // ADR 0002: changelists are global to the working tree. Git carries
    // dirty hunks across `git switch` untouched, so membership rides along
    // — which holds only because records name no branch, so there is
    // nothing for the switch to strand them against.
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(20, &[]))
        .commit_all("init");
    // `other` gets a commit of its own, so the switch genuinely moves HEAD
    // and rewrites the tree rather than re-pointing at the same commit. It
    // touches a different file because it has to: git refuses a switch that
    // would clobber local modifications, so a carried path is always
    // identical in both trees.
    fixture
        .branch("other")
        .switch_branch("other")
        .write("b.txt", "other branch\n")
        .commit_all("other: b.txt")
        .switch_branch("main");

    let repo = repo(&fixture);
    repo.create_changelist("feature").unwrap();
    repo.switch(Some("feature")).unwrap();
    fixture.write("a.txt", &numbered(20, &[(5, "five!"), (15, "fifteen!")]));
    let before = repo.refresh().unwrap().snapshot;
    assert_eq!(
        owners(&before, "a.txt"),
        vec![Some("feature".into()), Some("feature".into())]
    );

    // A second changelist takes the active marker, so survival can't be
    // confused with re-capture: hunks that lost their records would land on
    // 'idle' below, not back on 'feature'.
    repo.create_changelist("idle").unwrap();
    repo.switch(Some("idle")).unwrap();

    fixture.switch_branch("other");

    let refreshed = repo.refresh().unwrap();
    let after = &refreshed.snapshot;
    // Fixture integrity first: the switch landed, so what follows is
    // asserted on the other branch rather than over an unmoved HEAD.
    assert!(
        matches!(&after.head, Head::Branch { name } if name == "other"),
        "expected HEAD on other, got {:?}",
        after.head
    );
    assert_eq!(
        owners(after, "a.txt"),
        vec![Some("feature".into()), Some("feature".into())],
        "the same changelist owns the same hunks on the other branch"
    );
    assert_eq!(
        starts(after, "a.txt"),
        starts(&before, "a.txt"),
        "and at the same coordinates — the hunks themselves rode across"
    );
    let names: Vec<&str> = after
        .changelists
        .iter()
        .map(|cl| cl.name.as_str())
        .collect();
    assert_eq!(names, vec!["feature", "idle"]);
    assert_eq!(after.active.as_deref(), Some("idle"));
    assert!(
        refreshed.advisories.is_empty(),
        "a switch that carries hunks untouched is not a membership event"
    );

    // The other half of the property: nothing branch-shaped is persisted,
    // by field or by value. Read as raw text rather than as fields, so a
    // branch smuggled in under any new key fails here too.
    let raw = fs::read_to_string(fixture.path().join(".git/gitchange/state.json"))
        .expect("state file exists");
    for branch in ["main", "other", "refs/heads", "branch"] {
        assert!(
            !raw.contains(branch),
            "the state file names {branch:?}:\n{raw}"
        );
    }
    // The text record's full field set (a whole-file record adds
    // `oid_anchor`, ADR 0009; a mode record `mode_change`, ADR 0017):
    // what matters is that the list is closed and holds nothing naming a
    // branch.
    let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
    for record in json["records"].as_array().unwrap() {
        let mut keys: Vec<&str> = record
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "anchor",
                "changelist",
                "dormant_since",
                "new_lines",
                "new_start",
                "old_lines",
                "old_start",
                "path",
            ],
            "record fields are path, coordinates, owner and anchor — no branch"
        );
    }
}

#[test]
fn every_state_file_write_leaves_no_temp_sibling() {
    // ADR 0002's write-then-rename: the temp file exists only between the
    // write and the rename, so by the time an op returns the state dir
    // holds nothing but `state.json`. A leaked `state.json.tmp` is
    // invisible to every other assertion in the suite — it would still
    // load, still lock, still round-trip — so this is the one that catches
    // it.
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(20, &[]))
        .commit_all("init");
    let repo = repo(&fixture);
    let only_the_state_file = |label: &str| {
        assert_eq!(
            fixture.state_dir_entries(),
            vec!["state.json"],
            "after {label}"
        );
    };

    repo.create_changelist("feature").unwrap();
    repo.switch(Some("feature")).unwrap();
    only_the_state_file("create_changelist");

    // A refresh that persists records: a fresh hunk captures to the active
    // changelist, so this write goes through the refresh path rather than
    // the changelist-op one. Refresh only saves when records change
    // (ADR 0005's self-loop filter), so the capture is asserted — without
    // it a regression that persisted nothing would leave this step
    // asserting that a write-free op writes no temp file.
    fixture.write("a.txt", &numbered(20, &[(5, "five!")]));
    let snapshot = repo.refresh().unwrap().snapshot;
    assert_eq!(owners(&snapshot, "a.txt"), vec![Some("feature".into())]);
    only_the_state_file("the record-writing refresh");

    repo.create_changelist("chores").unwrap();
    let hunks = &snapshot.files[0].hunks;
    assert_eq!(hunks.len(), 1, "the assign below must have a hunk to move");
    repo.assign_hunks("a.txt", hunks, Some("chores")).unwrap();
    only_the_state_file("assign_hunks");

    repo.rename_changelist("chores", "errands").unwrap();
    only_the_state_file("rename_changelist");

    repo.switch(Some("errands")).unwrap();
    only_the_state_file("switch");

    // The commit path writes the state file too — its own locked update for
    // the ADR 0012 aftermath — and does it alongside the temp index and
    // message file, the two other things that must not outlive the op.
    repo.stage_all(Some("errands")).unwrap();
    repo.commit(
        Some("errands"),
        "errands: five",
        &CommitOptions::default(),
        None,
    )
    .unwrap();
    only_the_state_file("commit");

    repo.delete_changelist("errands").unwrap();
    only_the_state_file("delete_changelist");
}

#[test]
fn no_membership_op_writes_a_git_object() {
    // ADR 0002: the sidecar writes no git objects. That is the property
    // the JSON file was chosen for over a refs-based backend, whose
    // per-refresh record rewrite would synthesize commit+tree+blob on a
    // hot path — so the assertion belongs on the refresh and the
    // membership ops, which is the whole set below.
    //
    // Staging and committing write objects legitimately (a real apply
    // computes postimage blobs; a commit writes a tree and a commit), so
    // they are excluded from the sweep rather than the sweep being
    // weakened to a bound.
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(20, &[]))
        .commit_all("init");
    let repo = repo(&fixture);

    let baseline = fixture.odb_object_count();
    assert!(
        baseline > 0,
        "the fixture's own commit wrote objects — a count stuck at zero \
         would pass this test over anything"
    );
    let unchanged = |label: &str| {
        assert_eq!(
            fixture.odb_object_count(),
            baseline,
            "{label} wrote a git object"
        );
    };

    repo.create_changelist("feature").unwrap();
    repo.switch(Some("feature")).unwrap();
    repo.create_changelist("chores").unwrap();
    unchanged("create_changelist");

    // The refresh does the most work of any of these: two diffs, the hunk
    // universe, the matcher, and a record write.
    fixture.write("a.txt", &numbered(20, &[(5, "five!"), (15, "fifteen!")]));
    let snapshot = repo.refresh().unwrap().snapshot;
    assert_eq!(
        owners(&snapshot, "a.txt"),
        vec![Some("feature".into()), Some("feature".into())],
        "the refresh under test captured and persisted records"
    );
    unchanged("refresh");

    repo.assign_hunks("a.txt", &snapshot.files[0].hunks[1..], Some("chores"))
        .unwrap();
    unchanged("assign_hunks");

    repo.rename_changelist("chores", "errands").unwrap();
    unchanged("rename_changelist");

    repo.switch(Some("errands")).unwrap();
    unchanged("switch");

    repo.delete_changelist("errands").unwrap();
    unchanged("delete_changelist");

    // The dormancy the delete leaves behind is re-derived on the next
    // refresh, which rewrites records again — still no objects.
    repo.refresh().unwrap();
    unchanged("the refresh after the delete");
}
