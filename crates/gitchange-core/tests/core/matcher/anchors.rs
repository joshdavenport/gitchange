//! ADR 0001's two matching tiers: exact anchor match and, when the anchor
//! breaks, HEAD-side overlap with a stored record. Includes the anchor
//! shape itself, and ADR 0009's whole-file flavours of both tiers.

use crate::support::RepoFixture;
use gitchange_core::Advisory;

use super::helpers::{numbered, owners, record_at, repo, state_json};

/// The stored anchor for `path`, in the shape the matcher's tier 1
/// compares — read off the persisted record, like [`record_at`].
fn stored_anchor(json: &serde_json::Value, path: &str) -> Vec<String> {
    serde_json::from_value(record_at(json, path)["anchor"].clone()).unwrap()
}

/// What `numbered(20, &[(10, "ten!")])` anchors to at libgit2's default
/// context width: the change plus three verbatim context lines each side.
const ANCHOR_AT_CONTEXT_THREE: [&str; 8] = [
    " line 7\n",
    " line 8\n",
    " line 9\n",
    "-line 10\n",
    "+ten!\n",
    " line 11\n",
    " line 12\n",
    " line 13\n",
];

#[test]
fn a_worktree_hunks_stored_anchor_carries_three_context_lines() {
    // ADR 0001, "The anchor's context width": three lines each side,
    // taken from libgit2's default — `Git2Backend::diffs` sets none. Pin
    // it where the matcher reads it, the persisted record. This half
    // covers the HEAD↔worktree diff.
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(20, &[]))
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();

    fixture.write("a.txt", &numbered(20, &[(10, "ten!")]));
    repo.refresh().unwrap();

    assert_eq!(
        stored_anchor(&state_json(&fixture), "a.txt"),
        ANCHOR_AT_CONTEXT_THREE
    );
}

#[test]
fn an_index_only_hunks_stored_anchor_carries_three_context_lines() {
    // `diffs()`'s other half: an index-only hunk (staged, then reverted
    // in the worktree) never reaches the worktree diff, so its anchor
    // comes from HEAD↔index. Narrowing that diff alone would leave the
    // sibling above passing.
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(20, &[]))
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();

    // Staged and reverted before the first refresh, so no worktree-side
    // record exists for tier 1 to carry forward: the anchor under test
    // can only have come from the index diff.
    fixture
        .write("a.txt", &numbered(20, &[(10, "ten!")]))
        .stage("a.txt")
        .write("a.txt", &numbered(20, &[]));

    let snapshot = repo.refresh().unwrap();
    assert!(
        snapshot.files[0].hunks[0].index_only,
        "the index-only path is the one under test"
    );
    assert_eq!(
        stored_anchor(&state_json(&fixture), "a.txt"),
        ANCHOR_AT_CONTEXT_THREE
    );
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
        snapshot.advisories,
        vec![Advisory::AutoCaptured {
            path: "a.txt".into(),
            new_start: 1,
            changelist: "two".into(),
        }],
        "only the capture advisories; the exact-match survivor is quiet"
    );
}

#[test]
fn a_reworked_hunk_below_a_fresh_insertion_keeps_membership_via_overlap() {
    // ADR 0001 tier 2, both halves in one refresh: fresh lines above the
    // owned hunk (a nonzero line delta) *and* a rework of the owned hunk
    // itself (anchor broken, so tier 1 cannot rescue it). Both stored
    // records and fresh hunks address HEAD-side coordinates, so the
    // insertion must not disturb the overlap; a change of coordinate
    // basis would break exactly this scenario.
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(30, &[]))
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();

    fixture.write("a.txt", &numbered(30, &[(20, "twenty-a")]));
    repo.refresh().unwrap();

    // Another changelist is active when both edits land together.
    repo.create_changelist("two").unwrap();
    repo.switch("two").unwrap();
    let reworked = format!(
        "alpha\nbeta\ngamma\n{}",
        numbered(30, &[(20, "twenty-b"), (21, "twentyone-b")])
    );
    fixture.write("a.txt", &reworked);

    let snapshot = repo.refresh().unwrap();
    assert_eq!(
        owners(&snapshot, "a.txt"),
        vec![Some("two".into()), Some("one".into())],
        "top insertion captures to active; the reworked hunk keeps its owner"
    );
    assert_eq!(
        snapshot.advisories,
        vec![Advisory::AutoCaptured {
            path: "a.txt".into(),
            new_start: 1,
            changelist: "two".into(),
        }],
        "only the capture advisories; the tier-2 survivor is quiet"
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
    assert!(snapshot.advisories.is_empty());
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
    assert!(snapshot.advisories.is_empty());
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
        snapshot.advisories,
        vec![Advisory::AmbiguousOverlap {
            path: "a.txt".into(),
            new_start: 7,
            candidates: vec!["one".into(), "two".into()],
            assigned_to: Some("two".into()),
        }]
    );

    // The decision became a record: the next refresh is quiet.
    let snapshot = repo.refresh().unwrap();
    assert!(snapshot.advisories.is_empty());
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
        snapshot.advisories,
        vec![Advisory::AutoCaptured {
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
        snapshot.advisories.is_empty(),
        "an exact match is not a capture"
    );
}
