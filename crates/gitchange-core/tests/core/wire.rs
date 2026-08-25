//! The machine surface (ADR 0018): the one envelope composition both
//! `--json` reads are serialised through. What these pin is the dialect
//! — `schema_version`, snake_case names, `kind`-discriminated unions, no
//! advisories field — and the ordering promise, over real repos through
//! the public serialisers.

use crate::support::RepoFixture;
use gitchange_core::{ChangedFile, Repo, Snapshot, diff_envelope, status_envelope};
use serde_json::{Value, json};

/// The status read's envelope, parsed — every assertion below reads the
/// document a caller would, never the private wire types.
fn status_of(repo: &Repo) -> Value {
    parse(&status_envelope(&repo.read_only_refresh().unwrap()))
}

/// The diff read's envelope over a whole snapshot, parsed. Scope
/// resolution is the diff command's (#158); the serialiser takes the
/// files it is handed.
fn diff_of(snapshot: &Snapshot) -> Value {
    let files: Vec<&ChangedFile> = snapshot.files.iter().collect();
    parse(&diff_envelope(&files))
}

fn parse(document: &str) -> Value {
    serde_json::from_str(document).expect("the envelope is one JSON document")
}

/// One file object out of a status envelope's groups, by path — the
/// group it sits in is the caller's business.
fn file_in(envelope: &Value, group: usize, path: &str) -> Value {
    envelope["groups"][group]["files"]
        .as_array()
        .expect("a group's files")
        .iter()
        .find(|file| file["path"] == path)
        .unwrap_or_else(|| panic!("{path} in group {group}"))
        .clone()
}

/// The file object `path` serialises to in a diff envelope.
fn diff_file(envelope: &Value, path: &str) -> Value {
    envelope["files"]
        .as_array()
        .expect("the files array")
        .iter()
        .find(|file| file["path"] == path)
        .unwrap_or_else(|| panic!("{path} in the diff envelope"))
        .clone()
}

/// The group labels an envelope carries, in serialised order: a
/// changelist by name, the two derived groups by their `kind`.
fn group_order(envelope: &Value) -> Vec<String> {
    envelope["groups"]
        .as_array()
        .expect("the groups array")
        .iter()
        .map(|group| {
            group["name"]
                .as_str()
                .unwrap_or_else(|| group["kind"].as_str().expect("every group has a kind"))
                .to_owned()
        })
        .collect()
}

/// Whether any object anywhere in the document carries `key` — how the
/// no-advisories rule is asserted, since a field nested inside a group
/// or a hunk would breach it just as loudly as a top-level one.
fn carries_key(value: &Value, key: &str) -> bool {
    match value {
        Value::Object(fields) => {
            fields.contains_key(key) || fields.values().any(|value| carries_key(value, key))
        }
        Value::Array(values) => values.iter().any(|value| carries_key(value, key)),
        _ => false,
    }
}

/// A repo with one modified text file, on `main` with capture off.
fn one_modified_file() -> (RepoFixture, Repo) {
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", "one\ntwo\nthree\n")
        .commit_all("init")
        .write("a.txt", "one\ntwo edited\nthree\n");
    let repo = Repo::discover(fixture.path()).unwrap();
    (fixture, repo)
}

/// A file long enough that an edit at each end is two hunks, not one:
/// three lines of context each side never meet in the middle.
fn long_file(first: &str, last: &str) -> String {
    let mut lines: Vec<String> = (1..=12).map(|line| format!("line {line}\n")).collect();
    lines[0] = format!("{first}\n");
    lines[11] = format!("{last}\n");
    lines.concat()
}

/// A repo mid-merge with `a.txt` quarantined (ADR 0007) — the `conflicts`
/// module's recipe: each side commits its own edit to the same line.
fn mid_merge() -> RepoFixture {
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", "base\n")
        .write("b.txt", "b base\n")
        .commit_all("init")
        .branch("feature")
        .checkout("feature")
        .write("a.txt", "feature side\n")
        .commit_all("feature edit")
        .checkout("main")
        .write("a.txt", "main side\n")
        .commit_all("main edit")
        .merge_conflicting("feature");
    fixture
}

#[test]
fn both_read_envelopes_carry_the_same_integer_schema_version() {
    // One version for the whole dialect (ADR 0018): the two surfaces
    // share types, so they can only be versioned together.
    let (_fixture, repo) = one_modified_file();
    let snapshot = repo.read_only_refresh().unwrap();
    let status = parse(&status_envelope(&snapshot));
    let diff = diff_of(&snapshot);

    assert_eq!(status["schema_version"], json!(1));
    assert_eq!(diff["schema_version"], status["schema_version"]);
    assert!(
        status["schema_version"].is_u64(),
        "an integer, not a string: {}",
        status["schema_version"]
    );
}

#[test]
fn neither_read_envelope_carries_advisories() {
    // Structural in core — a read-only refresh returns none — and
    // stated here at the wire, where a future serialiser could invent a
    // field for them (ADR 0018).
    let (fixture, repo) = one_modified_file();
    repo.create_changelist("one").unwrap();
    fixture.write("b.txt", "new\n");
    let snapshot = repo.read_only_refresh().unwrap();

    for envelope in [parse(&status_envelope(&snapshot)), diff_of(&snapshot)] {
        assert!(
            !carries_key(&envelope, "advisories"),
            "no advisories field: {envelope}"
        );
    }
}

#[test]
fn the_status_envelope_carries_head_active_and_operation() {
    let (_fixture, repo) = one_modified_file();
    repo.create_changelist("one").unwrap();
    repo.switch(Some("one")).unwrap();

    let envelope = status_of(&repo);

    assert_eq!(envelope["head"], json!({"kind": "branch", "name": "main"}));
    assert_eq!(envelope["operation"], json!(null));
    assert_eq!(envelope["active"], json!("one"));
}

#[test]
fn unassigned_active_serialises_as_a_null_active() {
    // The active marker lives in `active` alone, so unassigned-active
    // has to be spelled by its absence of a name (#157).
    let (_fixture, repo) = one_modified_file();
    repo.switch(None).unwrap();

    assert_eq!(status_of(&repo)["active"], json!(null));
}

#[test]
fn a_detached_head_names_its_short_id() {
    let (fixture, repo) = one_modified_file();
    fixture.detach_head();

    let head = status_of(&repo)["head"].clone();

    assert_eq!(head["kind"], json!("detached"));
    assert!(
        head["short_id"].as_str().is_some_and(|id| !id.is_empty()),
        "the commit's short id: {head}"
    );
}

#[test]
fn an_unborn_head_names_the_branch_that_has_no_commits() {
    // A fresh `git init` still reads (#157), so the envelope has a shape
    // for a HEAD pointing nowhere yet.
    let fixture = RepoFixture::new();
    fixture.write("a.txt", "one\n");
    let repo = Repo::discover(fixture.path()).unwrap();

    assert_eq!(
        status_of(&repo)["head"],
        json!({"kind": "unborn", "name": "main"})
    );
}

#[test]
fn an_operation_in_progress_is_reported_by_name() {
    let fixture = mid_merge();
    let repo = Repo::discover(fixture.path()).unwrap();

    assert_eq!(status_of(&repo)["operation"], json!("merge"));
}

#[test]
fn groups_serialise_in_all_view_order() {
    // The ordering promise (ADR 0018) resolved through the user-order
    // definition (#122): conflicts first, changelists in
    // creation-append order with the empty ones kept, unassigned last.
    let fixture = mid_merge();
    fixture.write("b.txt", "b edited\n");
    let repo = Repo::discover(fixture.path()).unwrap();
    repo.create_changelist("first").unwrap();
    repo.create_changelist("second").unwrap();
    repo.switch(Some("second")).unwrap();
    // Capture is a persisting refresh's, so 'second' owns b.txt by
    // record; 'first' stays empty and still has to appear.
    repo.refresh().unwrap();

    let envelope = status_of(&repo);

    assert_eq!(
        group_order(&envelope),
        vec!["conflicts", "first", "second"],
        "{envelope}"
    );
    assert_eq!(envelope["groups"][1]["files"], json!([]), "{envelope}");
    assert_eq!(
        envelope["groups"][2]["kind"],
        json!("changelist"),
        "a named group is discriminated too, never by having a name"
    );
}

#[test]
fn an_unassigned_group_appears_exactly_as_the_text_face_shows_it() {
    // Non-empty, or active to carry the marker — core's `groups()`
    // decides, and the envelope renders that decision rather than a
    // second rule (#157).
    let (_fixture, repo) = one_modified_file();
    repo.create_changelist("one").unwrap();
    repo.switch(Some("one")).unwrap();

    // Recordless hunks report unassigned on a read (ADR 0005), so the
    // group is here on its own merits.
    assert_eq!(group_order(&status_of(&repo)), vec!["one", "unassigned"]);

    repo.refresh().unwrap();
    assert_eq!(
        group_order(&status_of(&repo)),
        vec!["one"],
        "captured away, and 'one' holds the marker, so nothing is left to show"
    );

    repo.switch(None).unwrap();
    assert_eq!(
        group_order(&status_of(&repo)),
        vec!["one", "unassigned"],
        "empty but active: the group carries capture-off's whole surface"
    );
}

#[test]
fn a_conflicted_groups_files_carry_their_path_alone() {
    // Quarantined paths own no hunks (ADR 0007), so a stage mark or a
    // hunk count would be a fabricated fact (#157).
    let fixture = mid_merge();
    let repo = Repo::discover(fixture.path()).unwrap();

    let envelope = status_of(&repo);

    assert_eq!(envelope["groups"][0]["kind"], json!("conflicts"));
    assert_eq!(envelope["groups"][0]["files"], json!([{"path": "a.txt"}]));
}

#[test]
fn a_status_file_object_carries_whole_file_facts() {
    // Whole-file counts on every row, `●` only in `staged_hunks`: a
    // staged-stale hunk surfaces through `partially_staged` instead of
    // being counted staged (#157).
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &long_file("first", "last"))
        .commit_all("init")
        // Two hunks: the first staged clean, the second staged and then
        // edited again, so it reads `◑` and is not counted staged.
        .write("a.txt", &long_file("first edited", "last edited"))
        .stage("a.txt")
        .write("a.txt", &long_file("first edited", "last edited twice"))
        .write("new.txt", "fresh\n");
    let repo = Repo::discover(fixture.path()).unwrap();

    let envelope = status_of(&repo);

    assert_eq!(
        file_in(&envelope, 0, "a.txt"),
        json!({
            "path": "a.txt",
            "change_kind": "modified",
            "stage": "partially_staged",
            "staged_hunks": 1,
            "total_hunks": 2,
        })
    );
    assert_eq!(
        file_in(&envelope, 0, "new.txt"),
        json!({
            "path": "new.txt",
            "change_kind": "untracked",
            "stage": "unstaged",
            "staged_hunks": 0,
            "total_hunks": 1,
        })
    );
    // The other half of the ordering promise: files in path order inside
    // a group, as the text face lists them.
    assert_eq!(
        envelope["groups"][0]["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|file| file["path"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["a.txt", "new.txt"]
    );
}

#[test]
fn a_path_two_changelists_own_appears_under_both_groups() {
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &long_file("first", "last"))
        .commit_all("init")
        .write("a.txt", &long_file("first edited", "last edited"));
    let repo = Repo::discover(fixture.path()).unwrap();
    repo.create_changelist("first").unwrap();
    repo.create_changelist("second").unwrap();
    repo.switch(Some("first")).unwrap();
    let snapshot = repo.refresh().unwrap().snapshot;
    let hunks = &snapshot.files[0].hunks;
    assert_eq!(hunks.len(), 2, "two hunks to split between owners");
    repo.assign_hunks("a.txt", &hunks[1..], Some("second"))
        .unwrap();

    let envelope = status_of(&repo);

    for group in 0..2 {
        assert_eq!(
            file_in(&envelope, group, "a.txt")["total_hunks"],
            json!(2),
            "whole-file facts on each row (#157): {envelope}"
        );
    }
}

#[test]
fn a_text_hunk_carries_its_coordinates_and_verbatim_lines() {
    let (_fixture, repo) = one_modified_file();

    let snapshot = repo.read_only_refresh().unwrap();

    assert_eq!(
        diff_file(&diff_of(&snapshot), "a.txt"),
        json!({
            "path": "a.txt",
            "change_kind": "modified",
            "binary": false,
            "sides": null,
            "hunks": [{
                "kind": "text",
                "changelist": null,
                "stage": "unstaged",
                "index_only": false,
                "old_start": 1,
                "old_lines": 3,
                "new_start": 1,
                "new_lines": 3,
                "lines": [
                    {"origin": " ", "content": "one\n"},
                    {"origin": "-", "content": "two\n"},
                    {"origin": "+", "content": "two edited\n"},
                    {"origin": " ", "content": "three\n"},
                ],
            }],
        })
    );
}

#[test]
fn a_hunks_owner_is_named_and_unassigned_is_null() {
    let (_fixture, repo) = one_modified_file();
    repo.create_changelist("one").unwrap();
    repo.switch(Some("one")).unwrap();
    repo.refresh().unwrap();

    let snapshot = repo.read_only_refresh().unwrap();

    assert_eq!(
        diff_file(&diff_of(&snapshot), "a.txt")["hunks"][0]["changelist"],
        json!("one")
    );
}

#[test]
fn the_staged_stale_flavours_are_told_apart_by_index_only() {
    let fixture = RepoFixture::new();
    fixture
        .write("edited.txt", "one\n")
        .write("reverted.txt", "one\n")
        .commit_all("init")
        .write("edited.txt", "one staged\n")
        .stage("edited.txt")
        .write("edited.txt", "one staged then edited\n")
        .write("reverted.txt", "one staged\n")
        .stage("reverted.txt")
        .write("reverted.txt", "one\n");
    let repo = Repo::discover(fixture.path()).unwrap();

    let snapshot = repo.read_only_refresh().unwrap();
    let envelope = diff_of(&snapshot);

    let edited = &diff_file(&envelope, "edited.txt")["hunks"][0];
    assert_eq!(edited["stage"], json!("staged_stale"));
    assert_eq!(edited["index_only"], json!(false));

    let reverted = &diff_file(&envelope, "reverted.txt")["hunks"][0];
    assert_eq!(reverted["stage"], json!("staged_stale"));
    assert_eq!(reverted["index_only"], json!(true), "{reverted}");
}

#[test]
fn a_changed_binary_carries_its_sides_and_no_lines() {
    let fixture = RepoFixture::new();
    fixture
        .write_bytes("blob.bin", &[0u8, 1, 2, 3])
        .commit_all("init")
        .write_bytes("blob.bin", &[0u8, 9, 9, 9, 9]);
    let repo = Repo::discover(fixture.path()).unwrap();

    let snapshot = repo.read_only_refresh().unwrap();
    let file = diff_file(&diff_of(&snapshot), "blob.bin");

    assert_eq!(file["binary"], json!(true));
    assert_eq!(file["sides"]["head"]["size"], json!(4));
    assert_eq!(file["sides"]["changed"]["size"], json!(5));
    assert!(
        file["sides"]["head"]["oid"]
            .as_str()
            .is_some_and(|oid| !oid.is_empty()),
        "the blob OID each side names: {file}"
    );
    assert_eq!(
        file["hunks"],
        json!([{
            "kind": "whole_file",
            "changelist": null,
            "stage": "unstaged",
            "index_only": false,
            "mode_delta": null,
        }]),
        "no lines and no coordinates on a degenerate hunk, and no type delta to name"
    );
}

#[test]
#[cfg(unix)]
fn a_chmod_serialises_as_a_mode_hunk_carrying_the_flip() {
    // Mode facts are hunk-attributed (#112), and the mode hunk sits
    // first — git's pseudo-hunk #0 position (ADR 0017).
    let fixture = RepoFixture::new();
    fixture
        .write_bytes("blob.bin", &[0u8, 1, 2, 3])
        .commit_all("init")
        .write_bytes("blob.bin", &[0u8, 9, 9, 9, 9])
        .set_exec("blob.bin");
    let repo = Repo::discover(fixture.path()).unwrap();

    let snapshot = repo.read_only_refresh().unwrap();
    let hunks = diff_file(&diff_of(&snapshot), "blob.bin")["hunks"].clone();

    assert_eq!(hunks[0]["kind"], json!("mode"), "{hunks}");
    assert_eq!(
        hunks[0]["mode_delta"],
        json!({"before": "100644", "after": "100755"}),
        "octal, exactly as git prints modes"
    );
    assert_eq!(hunks[1]["kind"], json!("whole_file"));
    assert_eq!(
        hunks[1]["mode_delta"],
        json!(null),
        "a permission flip is the mode hunk's fact, never the content hunk's"
    );
}

#[test]
#[cfg(unix)]
fn a_type_change_carries_its_delta_on_the_whole_file_hunk() {
    let fixture = RepoFixture::new();
    fixture
        .write("thing", "content\n")
        .write("target.txt", "elsewhere\n")
        .commit_all("init")
        .remove("thing")
        .symlink("thing", "target.txt");
    let repo = Repo::discover(fixture.path()).unwrap();

    let snapshot = repo.read_only_refresh().unwrap();
    let file = diff_file(&diff_of(&snapshot), "thing");

    assert_eq!(file["change_kind"], json!("type_changed"));
    assert_eq!(file["hunks"][0]["kind"], json!("whole_file"));
    assert_eq!(
        file["hunks"][0]["mode_delta"],
        json!({"before": "100644", "after": "120000"}),
        "non-null exactly for a type change: {file}"
    );
}

#[test]
fn a_conflicted_file_states_its_quarantine() {
    let fixture = mid_merge();
    let repo = Repo::discover(fixture.path()).unwrap();

    let snapshot = repo.read_only_refresh().unwrap();
    let file = diff_file(&diff_of(&snapshot), "a.txt");

    assert_eq!(file["change_kind"], json!("conflicted"));
    assert_eq!(file["hunks"], json!([]));
    assert_eq!(file["sides"], json!(null));
    assert_eq!(file["binary"], json!(false));
}

#[test]
fn diff_files_serialise_in_path_order_and_hunks_in_file_order() {
    let fixture = RepoFixture::new();
    fixture
        .write("b.txt", "one\n")
        .write("a.txt", &long_file("first", "last"))
        .write("nested/c.txt", "one\n")
        .commit_all("init")
        .write("b.txt", "edited\n")
        .write("a.txt", &long_file("first edited", "last edited"))
        .write("nested/c.txt", "edited\n");
    let repo = Repo::discover(fixture.path()).unwrap();

    let snapshot = repo.read_only_refresh().unwrap();
    let envelope = diff_of(&snapshot);
    let paths: Vec<&str> = envelope["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file["path"].as_str().unwrap())
        .collect();

    assert_eq!(paths, vec!["a.txt", "b.txt", "nested/c.txt"]);

    // `a.txt`'s two hunks in file order, which for text hunks is their
    // coordinate order — the same order the text face prints them in.
    let starts: Vec<u64> = diff_file(&envelope, "a.txt")["hunks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|hunk| hunk["old_start"].as_u64().unwrap())
        .collect();

    assert_eq!(starts, vec![1, 9], "{envelope}");
}
