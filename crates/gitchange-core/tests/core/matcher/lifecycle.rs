//! What a changelist's deletion, rename, reassignment or absence does to
//! records — plus the two state-file properties that ride on the same
//! path: no file without changelists, no rewrite without a change.

use std::fs;

use crate::support::RepoFixture;

use super::helpers::{numbered, owners, repo, state_json};

#[test]
fn records_naming_an_unknown_changelist_get_delete_semantics() {
    // A hand-edited state file (or a delete racing a refresh) can leave
    // records naming a changelist that no longer exists: live ones
    // orphan to unassigned — never captured by a survivor — and dormant
    // ones are pruned.
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(20, &[]))
        .commit_all("init")
        .write("a.txt", &numbered(20, &[(10, "ten!")]));
    let dir = fixture.path().join(".git/gitchange");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("state.json"),
        r#"{
  "version": 1,
  "active": "feature",
  "changelists": [{ "name": "feature" }],
  "records": [
    {
      "path": "a.txt", "old_start": 7, "old_lines": 7,
      "new_start": 7, "new_lines": 7, "changelist": "ghost",
      "anchor": ["-stale\n"], "dormant_since": null
    },
    {
      "path": "b.txt", "old_start": 1, "old_lines": 1,
      "new_start": 1, "new_lines": 1, "changelist": "ghost",
      "anchor": ["-stale\n"], "dormant_since": 1
    }
  ]
}"#,
    )
    .unwrap();

    let repo = repo(&fixture);
    let snapshot = repo.refresh().unwrap();

    assert_eq!(owners(&snapshot, "a.txt"), vec![None]);
    let json = state_json(&fixture);
    let records = json["records"].as_array().unwrap();
    assert_eq!(records.len(), 1, "the dormant ghost record is pruned");
    assert_eq!(records[0]["changelist"], serde_json::Value::Null);
}

#[test]
fn deleting_a_changelist_prunes_its_dormant_records() {
    // The delete-time half of the ADR 0002 dormant prune. Without it,
    // the record would survive the delete as an unassigned dormant
    // ghost and later revive its hunk to unassigned instead of the
    // active changelist capturing it.
    let mut fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(20, &[]))
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();

    fixture.write("a.txt", &numbered(20, &[(10, "ten!")]));
    repo.refresh().unwrap();

    // Real dormancy: the assigned hunk vanishes from the diff.
    fixture.stash();
    repo.refresh().unwrap();
    let json = state_json(&fixture);
    assert_eq!(json["records"][0]["changelist"], "one");
    assert!(json["records"][0]["dormant_since"].is_u64());

    // A survivor is active when "one" dies, as in the orphan test: the
    // prune must not depend on the delete promoting a new active.
    repo.create_changelist("two").unwrap();
    repo.switch("two").unwrap();
    repo.delete_changelist("one").unwrap();

    // Pruned outright — not orphaned to unassigned like a live record.
    let json = state_json(&fixture);
    assert_eq!(
        json["records"].as_array().unwrap().len(),
        0,
        "the deleted changelist's dormant record is pruned"
    );
}

#[test]
fn deleting_a_changelist_orphans_its_hunks_to_unassigned() {
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(20, &[]))
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();

    fixture.write("a.txt", &numbered(20, &[(10, "ten!")]));
    repo.refresh().unwrap();

    // Another changelist is active when "one" dies: its hunks must land
    // in unassigned, never be captured by the survivor.
    repo.create_changelist("two").unwrap();
    repo.switch("two").unwrap();
    repo.delete_changelist("one").unwrap();

    let snapshot = repo.refresh().unwrap();
    assert_eq!(owners(&snapshot, "a.txt"), vec![None]);
    let unassigned: Vec<&str> = snapshot
        .files_in(None)
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    assert_eq!(unassigned, vec!["a.txt"]);

    // The orphan claim is sticky: editing the hunk keeps it unassigned.
    fixture.write("a.txt", &numbered(20, &[(10, "ten!!"), (11, "eleven!")]));
    let snapshot = repo.refresh().unwrap();
    assert_eq!(owners(&snapshot, "a.txt"), vec![None]);
}

#[test]
fn renaming_a_changelist_carries_its_records() {
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(20, &[]))
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();
    fixture.write("a.txt", &numbered(20, &[(10, "ten!")]));
    repo.refresh().unwrap();

    repo.rename_changelist("one", "uno").unwrap();

    let snapshot = repo.refresh().unwrap();
    assert_eq!(owners(&snapshot, "a.txt"), vec![Some("uno".into())]);
}

#[test]
fn with_no_changelists_hunks_are_unassigned_and_no_state_file_is_written() {
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(20, &[]))
        .commit_all("init")
        .write("a.txt", &numbered(20, &[(10, "ten!")]));

    let repo = repo(&fixture);
    let snapshot = repo.refresh().unwrap();

    assert_eq!(owners(&snapshot, "a.txt"), vec![None]);
    assert!(
        !fixture.path().join(".git/gitchange").exists(),
        "a changelist-less refresh must not grow a state file"
    );
}

#[test]
fn with_no_changelists_a_binary_hunk_is_unassigned_too() {
    // ADR 0009: the unassigned-with-no-changelists rule holds for
    // whole-file (binary) hunks. The rule is uniform in code — the
    // matcher iterates hunks of either flavour — so this test is a
    // regression pin against a later binary-specific early return.
    let fixture = RepoFixture::new();
    fixture
        .write_bytes("logo.png", &[0u8, 1, 2, 3])
        .commit_all("init")
        .write_bytes("logo.png", &[0u8, 9, 9]);

    let repo = repo(&fixture);
    let snapshot = repo.refresh().unwrap();

    let file = snapshot
        .files
        .iter()
        .find(|file| file.path == "logo.png")
        .expect("logo.png in snapshot");
    assert!(file.binary, "fixture must produce a binary diff");
    assert_eq!(owners(&snapshot, "logo.png"), vec![None]);
    assert!(snapshot.advisories.is_empty());
    assert!(
        !fixture.path().join(".git/gitchange").exists(),
        "a changelist-less refresh must not grow a state file"
    );
}

#[test]
fn refresh_does_not_rewrite_the_state_file_when_records_are_unchanged() {
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(20, &[]))
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();
    fixture.write("a.txt", &numbered(20, &[(10, "ten!")]));
    repo.refresh().unwrap();

    // Backdate the file; an unchanged refresh must leave it untouched
    // (ADR 0005's belt-and-braces for the watcher self-loop filter).
    let state_path = fixture.path().join(".git/gitchange/state.json");
    let file = fs::File::options().write(true).open(&state_path).unwrap();
    file.set_times(fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH))
        .unwrap();
    drop(file);

    repo.refresh().unwrap();
    let modified = fs::metadata(&state_path).unwrap().modified().unwrap();
    assert_eq!(modified, std::time::UNIX_EPOCH);
}

#[test]
fn a_binary_whole_file_hunk_is_assignable_like_any_other() {
    // The whole-file hunk flows through the explicit assign op like any
    // hunk (ADR 0009: assignable between changelists).
    let fixture = RepoFixture::new();
    fixture
        .write_bytes("logo.png", &[0u8, 1, 2, 3])
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("art").unwrap();
    repo.create_changelist("other").unwrap();
    fixture.write_bytes("logo.png", &[0u8, 9, 9]);
    let snapshot = repo.refresh().unwrap();
    assert_eq!(owners(&snapshot, "logo.png"), vec![Some("art".into())]);

    let hunk = snapshot.files[0].hunks[0].clone();
    let advisories = repo
        .assign_hunks("logo.png", &[hunk], Some("other"))
        .unwrap()
        .advisories;
    assert!(advisories.is_empty());

    let snapshot = repo.refresh().unwrap();
    assert_eq!(owners(&snapshot, "logo.png"), vec![Some("other".into())]);
}
