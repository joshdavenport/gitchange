//! Dormancy: an unmatched record is retained rather than dropped, revives
//! only on an exact match, and is pruned after fourteen days — for text
//! records and ADR 0009's whole-file ones alike.

use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::support::RepoFixture;

use super::helpers::{numbered, owners, record_at, repo, state_json};

#[test]
fn stash_then_pop_round_trips_membership_through_dormancy() {
    let mut fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(20, &[]))
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();
    repo.switch(Some("one")).unwrap();

    fixture.write("a.txt", &numbered(20, &[(10, "ten!")]));
    repo.refresh().unwrap();

    fixture.stash();
    let snapshot = repo.refresh().unwrap();
    assert!(snapshot.files.is_empty());
    let json = state_json(&fixture);
    assert_eq!(json["records"][0]["changelist"], "one");
    assert!(
        json["records"][0]["dormant_since"].is_u64(),
        "an unmatched record is retained dormant, not dropped"
    );

    fixture.stash_pop();
    let snapshot = repo.refresh().unwrap();
    assert_eq!(owners(&snapshot, "a.txt"), vec![Some("one".into())]);
    let json = state_json(&fixture);
    assert!(
        json["records"][0]["dormant_since"].is_null(),
        "exact anchor match revives the record"
    );
}

#[test]
fn dormant_records_never_revive_via_overlap() {
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

    // A different edit at the same lines: overlaps the dormant record's
    // region but is not an exact anchor match.
    repo.create_changelist("two").unwrap();
    repo.switch(Some("two")).unwrap();
    fixture.write("a.txt", &numbered(20, &[(10, "ten-different")]));

    let snapshot = repo.refresh().unwrap();
    assert_eq!(
        owners(&snapshot, "a.txt"),
        vec![Some("two".into())],
        "a stale record must not mis-claim unrelated future edits"
    );
    let json = state_json(&fixture);
    let dormant: Vec<&serde_json::Value> = json["records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|record| record["dormant_since"].is_u64())
        .collect();
    assert_eq!(dormant.len(), 1);
    assert_eq!(dormant[0]["changelist"], "one");
}

#[test]
fn dormant_records_prune_after_fourteen_days() {
    let fixture = RepoFixture::new();
    fixture.write("a.txt", &numbered(5, &[])).commit_all("init");
    let dir = fixture.path().join(".git/gitchange");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("state.json"),
        r#"{
  "version": 1,
  "active": "one",
  "changelists": [{ "name": "one" }],
  "records": [
    {
      "path": "a.txt", "old_start": 1, "old_lines": 1,
      "new_start": 1, "new_lines": 1, "changelist": "one",
      "anchor": ["-line 1\n", "+gone\n"], "dormant_since": 1
    }
  ]
}"#,
    )
    .unwrap();

    let repo = repo(&fixture);
    repo.refresh().unwrap();

    let json = state_json(&fixture);
    assert_eq!(json["records"].as_array().unwrap().len(), 0);
    // The changelist itself is untouched by the prune.
    assert_eq!(json["changelists"][0]["name"], "one");
}

#[test]
fn whole_file_dormant_records_share_the_fourteen_day_prune() {
    // ADR 0009: dormancy and the 14-day prune apply to whole-file
    // records unchanged. The expired record must drop and the fresh one
    // must survive with its OID anchor intact — without the survivor,
    // the prune assertion could pass by dropping every dormant record.
    let fixture = RepoFixture::new();
    fixture.write("a.txt", &numbered(5, &[])).commit_all("init");
    // One day dormant: safely inside the 14-day TTL. The prune reads the
    // real clock, so the survivor needs a genuinely recent timestamp.
    let one_day = 24 * 60 * 60;
    let recent = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        - one_day;
    let dir = fixture.path().join(".git/gitchange");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("state.json"),
        r#"{
  "version": 1,
  "active": "one",
  "changelists": [{ "name": "one" }],
  "records": [
    {
      "path": "old.png", "old_start": 0, "old_lines": 0,
      "new_start": 0, "new_lines": 0, "changelist": "one",
      "anchor": [],
      "oid_anchor": { "head": "aaaa1111", "changed": "bbbb2222" },
      "dormant_since": 1
    },
    {
      "path": "new.png", "old_start": 0, "old_lines": 0,
      "new_start": 0, "new_lines": 0, "changelist": "one",
      "anchor": [],
      "oid_anchor": { "head": "cccc3333", "changed": "dddd4444" },
      "dormant_since": RECENT
    }
  ]
}"#
        .replace("RECENT", &recent.to_string()),
    )
    .unwrap();

    let repo = repo(&fixture);
    repo.refresh().unwrap();

    let json = state_json(&fixture);
    assert_eq!(
        json["records"].as_array().unwrap().len(),
        1,
        "the expired whole-file record is pruned"
    );
    let survivor = record_at(&json, "new.png");
    assert_eq!(survivor["dormant_since"], recent, "still dormant, same age");
    assert_eq!(survivor["oid_anchor"]["head"], "cccc3333");
    assert_eq!(survivor["oid_anchor"]["changed"], "dddd4444");
    // The changelist itself is untouched by the prune.
    assert_eq!(json["changelists"][0]["name"], "one");
}

#[test]
fn binary_dormant_revival_is_exact_only() {
    // ADR 0009: revival needs path *and* changed-side OID. A different
    // binary change at a path with a dormant record is a fresh change,
    // captured to active — never inheritance from dormancy.
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
        state_json(&fixture)["records"][0]["dormant_since"].is_u64(),
        "vanished binary change goes dormant"
    );

    // Different content at the path: fresh capture, not a revival.
    repo.create_changelist("other").unwrap();
    repo.switch(Some("other")).unwrap();
    fixture.write_bytes("logo.png", &[0u8, 5, 5, 5]);
    let snapshot = repo.refresh().unwrap();
    assert_eq!(owners(&snapshot, "logo.png"), vec![Some("other".into())]);

    // The exact bytes back: tier-1 revival to the dormant owner.
    fixture.write_bytes("logo.png", &[0u8, 9, 9]);
    let snapshot = repo.refresh().unwrap();
    assert_eq!(owners(&snapshot, "logo.png"), vec![Some("art".into())]);
}
