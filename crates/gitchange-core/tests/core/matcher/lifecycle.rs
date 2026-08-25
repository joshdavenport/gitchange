//! What a changelist's deletion, rename, reassignment or absence does to
//! records — plus the two state-file properties that ride on the same
//! path: no file without changelists, no rewrite without a change.

use std::fs;

use crate::support::{RepoFixture, delete, done, offenders};

use gitchange_core::{Advisory, RecordCounts, Release, Undeletable};

use super::helpers::{numbered, owners, repo, state_json};

#[test]
fn records_naming_an_unknown_changelist_get_delete_semantics() {
    // A hand-edited state file (or a delete racing a refresh) can leave
    // records naming a changelist that no longer exists: all of them are
    // pruned, live and dormant alike (ADR 0016) — a deleted changelist
    // must never claim hunks again. The freed hunk is then recordless
    // and flows to the active changelist like any new hunk.
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
    let snapshot = repo.refresh().unwrap().snapshot;

    assert_eq!(owners(&snapshot, "a.txt"), vec![Some("feature".into())]);
    let json = state_json(&fixture);
    let records = json["records"].as_array().unwrap();
    assert_eq!(records.len(), 1, "both ghost records are pruned");
    assert_eq!(
        records[0]["changelist"], "feature",
        "the only surviving record is the freed hunk's capture"
    );
}

#[test]
fn the_records_guard_refuses_while_records_exist_and_force_releases_them() {
    // #149's guard: a delete that would release hunks refuses, because
    // released hunks do not rest — the next persisting refresh claims
    // them, possibly another actor's, under a name nobody chose. Force is
    // the override, and it counts what it released.
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(20, &[]))
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();
    repo.switch(Some("one")).unwrap();
    fixture.write("a.txt", &numbered(20, &[(10, "ten!")]));
    repo.refresh().unwrap();

    let refused = repo.delete_changelists(&["one"], Release::Guarded).unwrap();
    assert_eq!(
        offenders(refused),
        vec![Undeletable::HoldsRecords {
            name: "one".to_owned(),
            records: RecordCounts {
                live: 1,
                dormant: 0
            },
        }]
    );
    assert_eq!(
        state_json(&fixture)["records"][0]["changelist"],
        "one",
        "a refused delete released nothing"
    );

    let receipt = done(repo.delete_changelists(&["one"], Release::Forced).unwrap());
    assert_eq!(
        receipt.advisories,
        vec![
            Advisory::RecordsReleased {
                changelist: "one".to_owned(),
                records: RecordCounts {
                    live: 1,
                    dormant: 0
                },
            },
            // "one" held the marker, so both notices ride one receipt:
            // what was released, then the aftermath.
            Advisory::ActiveChangelistDeleted {
                changelist: "one".to_owned(),
            },
        ],
        "the release is counted, never silent"
    );
    assert_eq!(state_json(&fixture)["records"].as_array().unwrap().len(), 0);
}

#[test]
fn the_records_guard_counts_dormant_records_too() {
    // Dormant records claim hunks that would come back (ADR 0002), and a
    // delete prunes them with the rest (ADR 0016) — so a changelist
    // holding nothing live is still holding membership, and the guard
    // says which kind. Asserted here rather than at the binary seam,
    // which cannot see dormancy at all.
    let mut fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(20, &[]))
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();
    repo.switch(Some("one")).unwrap();
    fixture.write("a.txt", &numbered(20, &[(10, "ten!")]));
    repo.refresh().unwrap();

    // Real dormancy: the assigned hunk vanishes from the diff.
    fixture.stash();
    repo.refresh().unwrap();
    assert!(state_json(&fixture)["records"][0]["dormant_since"].is_u64());

    let dormant_only = RecordCounts {
        live: 0,
        dormant: 1,
    };
    let refused = offenders(repo.delete_changelists(&["one"], Release::Guarded).unwrap());
    assert_eq!(
        refused,
        vec![Undeletable::HoldsRecords {
            name: "one".to_owned(),
            records: dormant_only,
        }],
        "dormant-only membership is still membership"
    );
    // The stake is the revival, not a release: a dormant record's hunk is
    // not in the diff, so there is nothing there for a refresh to claim
    // and saying otherwise would be a promise about nothing. Pinned as a
    // fragment — the phrasing is a display, the distinction is not.
    let refusal = refused[0].message();
    assert!(
        refusal.contains("holds 1 dormant record") && refusal.contains("is restored if it comes"),
        "{refusal}"
    );
    assert!(
        !refusal.contains("refresh"),
        "no refresh claims what is not in the diff: {refusal}"
    );

    let receipt = done(repo.delete_changelists(&["one"], Release::Forced).unwrap());
    assert_eq!(
        receipt.advisories.first(),
        Some(&Advisory::RecordsReleased {
            changelist: "one".to_owned(),
            records: dormant_only,
        })
    );
    let notice = receipt.advisories[0].message();
    assert!(
        notice.contains("dropped 1 dormant record") && !notice.contains("refresh"),
        "the notice mirrors the refusal's stake: {notice}"
    );
}

#[test]
fn deleting_a_changelist_prunes_its_dormant_records() {
    // Deletion prunes all of a changelist's records, dormant included
    // (ADR 0016/0002). Without this, the record would survive the
    // delete as a dormant ghost and later revive its hunk to a
    // changelist that no longer exists.
    let mut fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(20, &[]))
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();
    repo.switch(Some("one")).unwrap();

    fixture.write("a.txt", &numbered(20, &[(10, "ten!")]));
    repo.refresh().unwrap();

    // Real dormancy: the assigned hunk vanishes from the diff.
    fixture.stash();
    repo.refresh().unwrap();
    let json = state_json(&fixture);
    assert_eq!(json["records"][0]["changelist"], "one");
    assert!(json["records"][0]["dormant_since"].is_u64());

    // A survivor is active when "one" dies, as in the release test: the
    // prune must not depend on the delete promoting a new active.
    repo.create_changelist("two").unwrap();
    repo.switch(Some("two")).unwrap();
    delete(&repo, "one");

    let json = state_json(&fixture);
    assert_eq!(
        json["records"].as_array().unwrap().len(),
        0,
        "the deleted changelist's dormant record is pruned"
    );
}

#[test]
fn deleting_a_changelist_releases_its_hunks_to_the_active_changelist() {
    // ADR 0016: deletion prunes all of the changelist's records. Its
    // hunks are then recordless, and the uniform rule captures them into
    // whatever is active on the next refresh — loudly.
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(20, &[]))
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();
    repo.switch(Some("one")).unwrap();

    fixture.write("a.txt", &numbered(20, &[(10, "ten!")]));
    repo.refresh().unwrap();

    repo.create_changelist("two").unwrap();
    repo.switch(Some("two")).unwrap();
    delete(&repo, "one");

    let refreshed = repo.refresh().unwrap();
    let snapshot = &refreshed.snapshot;
    assert_eq!(owners(snapshot, "a.txt"), vec![Some("two".into())]);
    assert_eq!(
        refreshed.advisories,
        vec![gitchange_core::Advisory::AutoCaptured {
            path: "a.txt".into(),
            new_start: 7,
            changelist: "two".into(),
        }],
        "the released hunk's capture is visible, never silent"
    );
    assert_eq!(
        state_json(&fixture)["records"][0]["changelist"],
        "two",
        "none of the deleted changelist's records survive"
    );
}

#[test]
fn deleting_a_changelist_with_unassigned_active_leaves_its_hunks_unassigned() {
    // The capture-off case (ADR 0015/0016): with unassigned active,
    // nothing captures the released hunks. They stay unassigned and
    // recordless — across refreshes and edits.
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(20, &[]))
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();
    repo.create_changelist("keep").unwrap();
    repo.switch(Some("one")).unwrap();

    fixture.write("a.txt", &numbered(20, &[(10, "ten!")]));
    repo.refresh().unwrap();

    repo.switch(None).unwrap();
    delete(&repo, "one");

    let refreshed = repo.refresh().unwrap();
    let snapshot = &refreshed.snapshot;
    assert_eq!(owners(snapshot, "a.txt"), vec![None]);
    assert!(refreshed.advisories.is_empty(), "nothing was decided");
    assert_eq!(
        state_json(&fixture)["records"].as_array().unwrap().len(),
        0,
        "the released hunk is recordless"
    );

    fixture.write("a.txt", &numbered(20, &[(10, "ten!!"), (11, "eleven!")]));
    let snapshot = repo.refresh().unwrap().snapshot;
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
    repo.switch(Some("one")).unwrap();
    fixture.write("a.txt", &numbered(20, &[(10, "ten!")]));
    repo.refresh().unwrap();

    repo.rename_changelist("one", "uno").unwrap();

    let snapshot = repo.refresh().unwrap().snapshot;
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
    let snapshot = repo.refresh().unwrap().snapshot;

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
    let refreshed = repo.refresh().unwrap();
    let snapshot = &refreshed.snapshot;

    let file = snapshot
        .files
        .iter()
        .find(|file| file.path == "logo.png")
        .expect("logo.png in snapshot");
    assert!(file.binary, "fixture must produce a binary diff");
    assert_eq!(owners(snapshot, "logo.png"), vec![None]);
    assert!(refreshed.advisories.is_empty());
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
    repo.switch(Some("art")).unwrap();
    repo.create_changelist("other").unwrap();
    fixture.write_bytes("logo.png", &[0u8, 9, 9]);
    let snapshot = repo.refresh().unwrap().snapshot;
    assert_eq!(owners(&snapshot, "logo.png"), vec![Some("art".into())]);

    let hunk = snapshot.files[0].hunks[0].clone();
    let advisories = repo
        .assign_hunks("logo.png", &[hunk], Some("other"))
        .unwrap()
        .advisories;
    assert!(advisories.is_empty());

    let snapshot = repo.refresh().unwrap().snapshot;
    assert_eq!(owners(&snapshot, "logo.png"), vec![Some("other".into())]);
}
