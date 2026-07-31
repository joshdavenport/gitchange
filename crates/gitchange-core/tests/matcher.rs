//! Matcher behaviour (ticket 25, ADRs 0001/0002/0005): membership
//! records, assignment rules, dormancy — asserted through the public
//! `Repo::refresh()` on real temp repos (ADR 0008), never matcher
//! internals. Determinism makes these table-shaped: same records + same
//! diffs → same membership.

mod support;

use std::fs;

use gitchange_core::{Notice, Repo, Snapshot};
use support::RepoFixture;

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

fn state_json(fixture: &RepoFixture) -> serde_json::Value {
    let raw = fs::read_to_string(fixture.path().join(".git/gitchange/state.json"))
        .expect("state file exists");
    serde_json::from_str(&raw).unwrap()
}

#[test]
fn a_moved_hunk_keeps_membership_via_exact_anchor_match() {
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(30, &[]))
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();

    fixture.write("a.txt", &numbered(30, &[(20, "twenty!")]));
    repo.refresh().unwrap();

    // New edits now belong elsewhere; the old hunk moves down the file.
    repo.create_changelist("two").unwrap();
    repo.switch("two").unwrap();
    let moved = format!("alpha\nbeta\ngamma\n{}", numbered(30, &[(20, "twenty!")]));
    fixture.write("a.txt", &moved);

    let snapshot = repo.refresh().unwrap();
    assert_eq!(
        owners(&snapshot, "a.txt"),
        vec![Some("two".into()), Some("one".into())],
        "top insertion captures to active; the moved hunk keeps its owner"
    );
    assert_eq!(
        snapshot.notices,
        vec![Notice::AutoCaptured {
            path: "a.txt".into(),
            new_start: 1,
            changelist: "two".into(),
        }],
        "only the capture notices; the exact-match survivor is quiet"
    );
}

#[test]
fn editing_your_own_hunk_keeps_membership_via_overlap() {
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(20, &[]))
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();

    fixture.write("a.txt", &numbered(20, &[(10, "ten-a")]));
    repo.refresh().unwrap();

    // Rework the same hunk while another changelist is active: the
    // anchor no longer matches, but the overlap does.
    repo.create_changelist("two").unwrap();
    repo.switch("two").unwrap();
    fixture.write("a.txt", &numbered(20, &[(10, "ten-b"), (11, "eleven-b")]));

    let snapshot = repo.refresh().unwrap();
    assert_eq!(owners(&snapshot, "a.txt"), vec![Some("one".into())]);
    assert!(snapshot.notices.is_empty());
}

#[test]
fn a_split_hunk_inherits_the_parents_changelist() {
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(40, &[]))
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();

    // One block edit spanning lines 10..=25.
    let edits: Vec<(usize, String)> = (10..=25).map(|n| (n, format!("mod {n}"))).collect();
    let edits: Vec<(usize, &str)> = edits.iter().map(|(n, s)| (*n, s.as_str())).collect();
    fixture.write("a.txt", &numbered(40, &edits));
    repo.refresh().unwrap();

    // Revert the middle back to HEAD: the hunk splits in two.
    repo.create_changelist("two").unwrap();
    repo.switch("two").unwrap();
    let split: Vec<(usize, &str)> = edits
        .iter()
        .copied()
        .filter(|(n, _)| !(14..=21).contains(n))
        .collect();
    fixture.write("a.txt", &numbered(40, &split));

    let snapshot = repo.refresh().unwrap();
    assert_eq!(
        owners(&snapshot, "a.txt"),
        vec![Some("one".into()), Some("one".into())],
        "both fragments inherit the parent record's changelist"
    );
    assert!(snapshot.notices.is_empty());
}

#[test]
fn a_hunk_overlapping_two_changelists_captures_to_active_with_a_notice() {
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(40, &[]))
        .commit_all("init");
    let repo = repo(&fixture);

    repo.create_changelist("one").unwrap();
    fixture.write("a.txt", &numbered(40, &[(10, "ten!")]));
    repo.refresh().unwrap();

    repo.create_changelist("two").unwrap();
    repo.switch("two").unwrap();
    fixture.write("a.txt", &numbered(40, &[(10, "ten!"), (20, "twenty!")]));
    repo.refresh().unwrap();

    // Bridge the gap: one fresh hunk now overlaps both records.
    let edits: Vec<(usize, String)> = (10..=20).map(|n| (n, format!("bridge {n}"))).collect();
    let edits: Vec<(usize, &str)> = edits.iter().map(|(n, s)| (*n, s.as_str())).collect();
    fixture.write("a.txt", &numbered(40, &edits));

    let snapshot = repo.refresh().unwrap();
    assert_eq!(owners(&snapshot, "a.txt"), vec![Some("two".into())]);
    assert_eq!(
        snapshot.notices,
        vec![Notice::AmbiguousOverlap {
            path: "a.txt".into(),
            new_start: 7,
            candidates: vec!["one".into(), "two".into()],
            assigned_to: Some("two".into()),
        }]
    );

    // The decision became a record: the next refresh is quiet.
    let snapshot = repo.refresh().unwrap();
    assert!(snapshot.notices.is_empty());
}

#[test]
fn stash_then_pop_round_trips_membership_through_dormancy() {
    let mut fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(20, &[]))
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();

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

    fixture.write("a.txt", &numbered(20, &[(10, "ten-stashed")]));
    repo.refresh().unwrap();
    fixture.stash();
    repo.refresh().unwrap();

    // A different edit at the same lines: overlaps the dormant record's
    // region but is not an exact anchor match.
    repo.create_changelist("two").unwrap();
    repo.switch("two").unwrap();
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
fn new_hunks_capture_to_the_active_changelist() {
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(20, &[]))
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();
    repo.create_changelist("two").unwrap();
    repo.switch("two").unwrap();

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
fn a_routine_auto_capture_notices_once() {
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(20, &[]))
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();

    fixture.write("a.txt", &numbered(20, &[(10, "ten!")]));
    let snapshot = repo.refresh().unwrap();
    assert_eq!(
        snapshot.notices,
        vec![Notice::AutoCaptured {
            path: "a.txt".into(),
            new_start: 7,
            changelist: "one".into(),
        }],
        "a genuinely new hunk's capture is visible, never silent"
    );

    // The decision became a record: the next refresh is quiet.
    let snapshot = repo.refresh().unwrap();
    assert!(snapshot.notices.is_empty());
}

#[test]
fn with_no_active_changelist_nothing_notices() {
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(20, &[]))
        .commit_all("init")
        .write("a.txt", &numbered(20, &[(10, "ten!")]));

    let repo = repo(&fixture);
    let snapshot = repo.refresh().unwrap();
    assert_eq!(owners(&snapshot, "a.txt"), vec![None]);
    assert!(
        snapshot.notices.is_empty(),
        "unassigned fall-through decides nothing worth spot-checking"
    );
}

#[test]
fn dormant_revival_notices_with_a_per_changelist_count() {
    let mut fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(30, &[]))
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();

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
        snapshot.notices,
        vec![Notice::DormantRevival {
            path: "a.txt".into(),
            changelist: Some("one".into()),
            hunks: 2,
        }],
        "exact-match revival is an automatic decision: one notice, counted"
    );
}

#[test]
fn a_reexported_binary_keeps_its_changelist_via_path_continuity() {
    // ADR 0009 tier 2: same path, still binary-changed, different
    // content — membership holds and the anchor updates. The whole file
    // is the hunk, so a re-export is an edit of your own hunk.
    let fixture = RepoFixture::new();
    fixture
        .write_bytes("logo.png", &[0u8, 1, 2, 3])
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("art").unwrap();

    fixture.write_bytes("logo.png", &[0u8, 9, 9]);
    let snapshot = repo.refresh().unwrap();
    assert_eq!(
        snapshot.notices,
        vec![Notice::AutoCaptured {
            path: "logo.png".into(),
            new_start: 0,
            changelist: "art".into(),
        }],
        "a binary's first whole-file hunk captures with a notice too"
    );
    let first_anchor = state_json(&fixture)["records"][0]["oid_anchor"].clone();

    // A second changelist becomes active: drift must not recapture.
    repo.create_changelist("other").unwrap();
    repo.switch("other").unwrap();
    fixture.write_bytes("logo.png", &[0u8, 5, 5, 5, 5]);

    let snapshot = repo.refresh().unwrap();
    assert_eq!(owners(&snapshot, "logo.png"), vec![Some("art".into())]);
    let json = state_json(&fixture);
    assert_eq!(json["records"][0]["path"], "logo.png");
    assert_ne!(
        json["records"][0]["oid_anchor"], first_anchor,
        "the anchor follows the re-exported content"
    );
    assert!(
        json["records"][0]["oid_anchor"]["changed"].is_string(),
        "binary records store OIDs, cat-debuggable"
    );
}

#[test]
fn an_unchanged_binary_matches_exactly_after_a_move() {
    // Tier 1: the changed-side OID alone pins identity, so an untouched
    // binary change survives refreshes without drifting to active.
    let fixture = RepoFixture::new();
    fixture
        .write_bytes("logo.png", &[0u8, 1, 2, 3])
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("art").unwrap();
    fixture.write_bytes("logo.png", &[0u8, 9, 9]);
    repo.refresh().unwrap();

    repo.create_changelist("other").unwrap();
    repo.switch("other").unwrap();

    let snapshot = repo.refresh().unwrap();
    assert_eq!(owners(&snapshot, "logo.png"), vec![Some("art".into())]);
    assert!(
        snapshot.notices.is_empty(),
        "an exact match is not a capture"
    );
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
    repo.switch("other").unwrap();
    fixture.write_bytes("logo.png", &[0u8, 5, 5, 5]);
    let snapshot = repo.refresh().unwrap();
    assert_eq!(owners(&snapshot, "logo.png"), vec![Some("other".into())]);

    // The exact bytes back: tier-1 revival to the dormant owner.
    fixture.write_bytes("logo.png", &[0u8, 9, 9]);
    let snapshot = repo.refresh().unwrap();
    assert_eq!(owners(&snapshot, "logo.png"), vec![Some("art".into())]);
}

#[test]
fn a_binary_whole_file_hunk_moves_between_changelists() {
    // The whole-file hunk flows through the explicit move op like any
    // hunk (ADR 0009: movable between changelists).
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
    let notices = repo.move_hunks("logo.png", &[hunk], Some("other")).unwrap();
    assert!(notices.is_empty());

    let snapshot = repo.refresh().unwrap();
    assert_eq!(owners(&snapshot, "logo.png"), vec![Some("other".into())]);
}
