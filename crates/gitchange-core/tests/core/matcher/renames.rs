//! ADR 0011: records match by path, so a rename presents as a deletion
//! plus a fresh addition and membership never follows the file.

use crate::support::RepoFixture;
use gitchange_core::Advisory;

use super::helpers::{numbered, owners, record_at, repo, state_json};

#[test]
fn a_text_rename_leaves_membership_at_the_old_path_and_captures_the_new_one() {
    // ADR 0011: records match by path, so membership never follows a
    // rename. The old path still diffs as a deletion, so its record
    // re-anchors there by overlap — neither moved nor dropped — while the
    // new path is fresh content for the active changelist.
    let fixture = RepoFixture::new();
    fixture
        .write("old.txt", &numbered(20, &[]))
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();
    repo.switch(Some("one")).unwrap();
    fixture.write("old.txt", &numbered(20, &[(10, "ten!")]));
    repo.refresh().unwrap();

    // A second changelist is active when the rename lands, so a silent
    // transfer to the new path would be visible as "one" owning it.
    repo.create_changelist("two").unwrap();
    repo.switch(Some("two")).unwrap();
    fixture.rename("old.txt", "new.txt");

    let refreshed = repo.refresh().unwrap();
    let snapshot = &refreshed.snapshot;
    assert_eq!(
        owners(snapshot, "old.txt"),
        vec![Some("one".into())],
        "the deletion half inherits the record by overlap: membership stays put"
    );
    assert_eq!(
        owners(snapshot, "new.txt"),
        vec![Some("two".into())],
        "the new path's content is a fresh change, not the old record's"
    );
    assert_eq!(
        refreshed.advisories,
        vec![Advisory::AutoCaptured {
            path: "new.txt".into(),
            new_start: 1,
            changelist: "two".into(),
        }],
        "the capture is announced — visible, never a silent transfer"
    );
    let json = state_json(&fixture);
    assert!(
        record_at(&json, "old.txt")["dormant_since"].is_null(),
        "the old path still diffs, so its record is live on the deletion"
    );
    assert_eq!(record_at(&json, "old.txt")["changelist"], "one");
    assert_eq!(record_at(&json, "new.txt")["changelist"], "two");

    // The decision became a record: the next refresh is quiet.
    let refreshed = repo.refresh().unwrap();
    assert!(refreshed.advisories.is_empty());
}

#[test]
fn a_binary_rename_leaves_membership_at_the_old_path_and_captures_the_new_one() {
    // The same for a whole-file, OID-anchored record (ADR 0009): path
    // continuity is *path* continuity, so it holds across the old path's
    // deletion but not across the rename, even though the identical bytes
    // land at the new path.
    let fixture = RepoFixture::new();
    fixture
        .write_bytes("logo.png", &[0u8, 1, 2, 3])
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("art").unwrap();
    repo.switch(Some("art")).unwrap();
    fixture.write_bytes("logo.png", &[0u8, 9, 9]);
    repo.refresh().unwrap();
    let anchored_oid =
        record_at(&state_json(&fixture), "logo.png")["oid_anchor"]["changed"].clone();
    assert!(anchored_oid.is_string(), "the record anchors on a blob OID");

    repo.create_changelist("other").unwrap();
    repo.switch(Some("other")).unwrap();
    fixture.rename("logo.png", "brand.png");

    let refreshed = repo.refresh().unwrap();
    let snapshot = &refreshed.snapshot;
    assert_eq!(
        owners(snapshot, "logo.png"),
        vec![Some("art".into())],
        "the deletion half keeps the whole-file record by path continuity"
    );
    assert_eq!(
        owners(snapshot, "brand.png"),
        vec![Some("other".into())],
        "the same bytes at a new path are a fresh change: active captures them"
    );
    assert_eq!(
        refreshed.advisories,
        vec![Advisory::AutoCaptured {
            path: "brand.png".into(),
            new_start: 0,
            changelist: "other".into(),
        }],
        "a whole-file capture is announced like any other"
    );

    let json = state_json(&fixture);
    assert_eq!(
        record_at(&json, "brand.png")["oid_anchor"]["changed"],
        anchored_oid,
        "identical content, so tier 1 would have matched on OID alone — \
         only the path differs, and that is what stops it"
    );
    let old = record_at(&json, "logo.png");
    assert!(
        old["dormant_since"].is_null(),
        "the old path still diffs as a deletion, so its record is live"
    );
    assert_eq!(old["changelist"], "art");
    assert!(
        old["oid_anchor"]["changed"].is_null(),
        "the record follows the deletion: no changed side any more"
    );

    let refreshed = repo.refresh().unwrap();
    assert!(refreshed.advisories.is_empty());
}

#[test]
fn renaming_an_untracked_file_leaves_its_record_dormant() {
    // The other half of ADR 0011: with no deletion to hold it, the old
    // path leaves the universe entirely, so its record goes dormant —
    // retained, never dropped — while the new path captures to active.
    let fixture = RepoFixture::new();
    fixture.write("keep.txt", "keep\n").commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();
    repo.switch(Some("one")).unwrap();
    fixture.write("draft.txt", "draft\n");
    repo.refresh().unwrap();

    repo.create_changelist("two").unwrap();
    repo.switch(Some("two")).unwrap();
    fixture.rename("draft.txt", "final.txt");

    let refreshed = repo.refresh().unwrap();
    let snapshot = &refreshed.snapshot;
    assert!(
        !snapshot.files.iter().any(|file| file.path == "draft.txt"),
        "the old path has no diff left at all"
    );
    assert_eq!(owners(snapshot, "final.txt"), vec![Some("two".into())]);
    assert_eq!(
        refreshed.advisories,
        vec![Advisory::AutoCaptured {
            path: "final.txt".into(),
            new_start: 1,
            changelist: "two".into(),
        }]
    );

    let json = state_json(&fixture);
    let left_behind = record_at(&json, "draft.txt");
    assert!(
        left_behind["dormant_since"].is_u64(),
        "an unmatched record is retained dormant, not dropped"
    );
    assert_eq!(left_behind["changelist"], "one");
    assert_eq!(record_at(&json, "final.txt")["changelist"], "two");

    let refreshed = repo.refresh().unwrap();
    assert!(refreshed.advisories.is_empty());
}

#[test]
fn renaming_an_untracked_binary_leaves_its_whole_file_record_dormant() {
    // The whole-file flavour, where membership is keyed on path alone: the
    // dormant record and the fresh one carry the *same* changed-side OID.
    let fixture = RepoFixture::new();
    fixture.write("keep.txt", "keep\n").commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("art").unwrap();
    repo.switch(Some("art")).unwrap();
    fixture.write_bytes("draft.png", &[0u8, 9, 9]);
    repo.refresh().unwrap();

    repo.create_changelist("other").unwrap();
    repo.switch(Some("other")).unwrap();
    fixture.rename("draft.png", "final.png");

    let refreshed = repo.refresh().unwrap();
    let snapshot = &refreshed.snapshot;
    assert!(!snapshot.files.iter().any(|file| file.path == "draft.png"));
    assert_eq!(owners(snapshot, "final.png"), vec![Some("other".into())]);
    assert_eq!(
        refreshed.advisories,
        vec![Advisory::AutoCaptured {
            path: "final.png".into(),
            new_start: 0,
            changelist: "other".into(),
        }]
    );

    let json = state_json(&fixture);
    let left_behind = record_at(&json, "draft.png");
    assert!(left_behind["dormant_since"].is_u64());
    assert_eq!(left_behind["changelist"], "art");
    assert_eq!(
        record_at(&json, "final.png")["oid_anchor"]["changed"],
        left_behind["oid_anchor"]["changed"],
        "same blob, different path: revival is path-scoped too"
    );

    let refreshed = repo.refresh().unwrap();
    assert!(refreshed.advisories.is_empty());
}
