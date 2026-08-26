//! `status`'s two faces at the binary seam: the text rows a human reads
//! and the `--json` envelope a caller parses (#156/#157, ADR 0018). Both
//! render core's `Snapshot::groups`, so what they select and the order they
//! select it in is one decision asserted twice — and the read-only refusal
//! to write anything is asserted through each of them.
//!
//! Its own module because these are the assertions that pin exact output:
//! the file rows line for line, the envelope field for field. `switch`'s
//! effect on the active marker is `switch.rs`'s, and reaching `status` at
//! all — outside a repository, or through `-C` — is `invocation.rs`'s.

use std::path::Path;

use crate::support::{
    CAPTURE_PENDING_HINT, committed_repo, dirty_repo, git, gitchange, lock_path, long_file,
    merging_repo, seed_state, seed_state_raw, state_bytes, state_path, two_hunk_repo, unborn_repo,
};

/// `status --json`'s document, parsed — every assertion below reads what
/// a caller would, never gitchange's own types. Delivery is asserted here
/// once, for every test that follows: exit 0, an empty stderr (a read
/// advises nothing), and exactly one line of stdout, an envelope being one
/// document (ADR 0018).
fn status_json(dir: &Path) -> serde_json::Value {
    let output = gitchange(dir, &["status", "--json"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stderr, Vec::<u8>::new(), "a read advises nothing");
    let stdout = String::from_utf8(output.stdout).unwrap();
    // The whole of stdout, parsed as one value: anything printed beside
    // the envelope fails here, which is the promise — one document per
    // read (ADR 0018). How it is laid out inside is the serialiser's.
    serde_json::from_str(&stdout).expect("the envelope is one JSON document")
}

/// The group labels an envelope carries, in serialised order: a changelist
/// by name, the two derived groups by their `kind`.
fn group_order(envelope: &serde_json::Value) -> Vec<&str> {
    envelope["groups"]
        .as_array()
        .expect("the groups array")
        .iter()
        .map(|group| {
            group["name"]
                .as_str()
                .unwrap_or_else(|| group["kind"].as_str().expect("every group has a kind"))
        })
        .collect()
}

/// One file object out of a status envelope's groups, by group index and
/// path.
fn file_in(envelope: &serde_json::Value, group: usize, path: &str) -> serde_json::Value {
    envelope["groups"][group]["files"]
        .as_array()
        .expect("a group's files")
        .iter()
        .find(|file| file["path"] == path)
        .unwrap_or_else(|| panic!("{path} in group {group}"))
        .clone()
}

/// The text face's headers and file paths in print order, for comparing
/// selection and order against the envelope's (ADR 0018). Reads the
/// indentation the face prints: a header at column 0 behind its marker, a
/// file row four spaces in with the path as its third field. The layout
/// itself is `print_files`' in `main.rs` — this only follows it, and the
/// exact lines are pinned by the tests above. Conflicts rows are indented
/// further and shaped differently, so the one test that uses this stays on
/// a fixture without them; the conflicts group's two faces are compared
/// where the group is the subject.
fn text_face_groups(stdout: &str) -> Vec<(String, Vec<String>)> {
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    for line in stdout.lines() {
        match line.strip_prefix("    ") {
            // The capture-pending hint sits under a header too, and names
            // no file.
            Some(row) if row.starts_with("capture on") => {}
            Some(row) => {
                let path = row.split(' ').nth(2).expect("a file row's path");
                groups
                    .last_mut()
                    .expect("a file row under a header")
                    .1
                    .push(path.to_owned());
            }
            None => groups.push((line[2..].to_owned(), Vec::new())),
        }
    }
    groups
}

// --- the text face ---------------------------------------------------------

#[test]
fn status_lists_changed_files_and_exits_0() {
    // No changelists exist: the whole dirty tree is the unassigned
    // group, and unassigned holds the `*` — with nothing else to be
    // active, capture flows here (ADR 0015).
    let repo = dirty_repo();
    let output = gitchange(repo.path(), &["status"]);

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        vec![
            "* unassigned",
            "    ○ M tracked.txt 0/1",
            "    ○ ? untracked.txt 0/1",
        ]
    );
}

#[test]
fn status_marks_externally_staged_files() {
    // ADR 0003: a real `git add` in another terminal derives as staged
    // at the next refresh — no error path.
    let repo = dirty_repo();
    git(repo.path(), &["add", "tracked.txt"]);

    let output = gitchange(repo.path(), &["status"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        vec![
            "* unassigned",
            "    ● M tracked.txt 1/1",
            "    ○ ? untracked.txt 0/1",
        ]
    );
}

#[test]
fn a_clean_tree_still_says_where_capture_would_go() {
    // The `*` is capture-off's whole indicator (ADR 0015), so it renders
    // even with nothing to group under it: exactly one target is always
    // active, and a clean tree must not be the one view that hides which.
    let repo = committed_repo();
    seed_state(repo.path(), "feature", &["feature"]);

    let output = gitchange(repo.path(), &["status"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        stdout.lines().collect::<Vec<&str>>(),
        vec!["* feature"],
        "an empty unassigned group stays out while a changelist holds it"
    );

    gitchange(repo.path(), &["switch", "unassigned"]);
    let output = gitchange(repo.path(), &["status"]);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        stdout.lines().collect::<Vec<&str>>(),
        vec!["  feature", "* unassigned"],
        "and appears, file-less, once it holds the marker"
    );
}

#[test]
fn status_groups_files_by_changelist_with_unassigned_last() {
    // A record claims tracked.txt for 'bugfix' — record-derived ownership,
    // which a read shows. The untracked file is recordless, so it sits in
    // the unassigned group, rendered last and wearing the marker: capture
    // is off (ADR 0015), so nothing is pending and no hint is shown.
    let repo = dirty_repo();
    seed_state_raw(
        repo.path(),
        r#"{
  "version": 1, "active": null,
  "changelists": [{ "name": "feature" }, { "name": "bugfix" }],
  "records": [
    {
      "path": "tracked.txt", "old_start": 1, "old_lines": 1,
      "new_start": 1, "new_lines": 1, "changelist": "bugfix",
      "anchor": ["-one\n", "+two\n"], "dormant_since": null
    }
  ]
}"#,
    );

    let output = gitchange(repo.path(), &["status"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        vec![
            "  feature",
            "  bugfix",
            "    ○ M tracked.txt 0/1",
            "* unassigned",
            "    ○ ? untracked.txt 0/1",
        ]
    );
}

#[test]
fn the_capture_pending_hint_needs_a_real_changelist_active_and_hunks_pending() {
    // The hint's whole condition (#156): a real changelist active *and*
    // unassigned holding hunks. Both are record facts — the marker and the
    // absence of records — so a read may state them. The one fixture
    // walks the two rows a switch can move between; the third row, a
    // real changelist active over a clean tree, is
    // `a_clean_tree_still_says_where_capture_would_go`, whose exact lines
    // carry no hint (and no unassigned group to hang one on).
    let repo = dirty_repo();
    seed_state(repo.path(), "feature", &["feature"]);

    // Real changelist active, unassigned dirty: the hint, exactly.
    let output = gitchange(repo.path(), &["status"]);
    let stdout = String::from_utf8(output.stdout).unwrap();
    let hint = stdout
        .lines()
        .find(|line| line.contains("capture on"))
        .expect("the hint is present");
    assert_eq!(hint, CAPTURE_PENDING_HINT);

    // Unassigned active (capture off): nothing is pending, so no hint.
    gitchange(repo.path(), &["switch", "unassigned"]);
    let output = gitchange(repo.path(), &["status"]);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        !stdout.contains("capture on"),
        "capture-off has nothing pending: {stdout}"
    );
}

// --- a read writes nothing and decides nothing -----------------------------

#[test]
fn status_writes_nothing_and_a_recordless_hunk_reports_unassigned() {
    // The read-only refresh at the binary seam (#156; core's own form is
    // tested in gitchange-core). The fixture is the shape a persisting
    // refresh would move most — a real changelist active over a dirty
    // tree, which it would claim both hunks of and stamp the baseline
    // for. Read twice, so the second run proves the first wrote nothing.
    let repo = dirty_repo();
    seed_state(repo.path(), "feature", &["feature"]);
    let before = state_bytes(repo.path());

    for _ in 0..2 {
        let output = gitchange(repo.path(), &["status"]);
        assert_eq!(output.status.code(), Some(0));
        assert_eq!(output.stderr, Vec::<u8>::new(), "a read advises nothing");
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert_eq!(
            stdout.lines().collect::<Vec<&str>>(),
            vec![
                "* feature",
                "  unassigned",
                CAPTURE_PENDING_HINT,
                "    ○ M tracked.txt 0/1",
                "    ○ ? untracked.txt 0/1",
            ],
            "context-derived ownership never previews: recordless is unassigned"
        );
        assert_eq!(
            state_bytes(repo.path()),
            before,
            "no capture, no record write, no baseline stamp"
        );
    }
    assert!(
        !lock_path(repo.path()).exists(),
        "a read takes no lock, so none is left behind"
    );
}

#[test]
fn status_creates_no_state_file() {
    // The same property from the other side (#156): a repository that
    // has never run gitchange still has no state file after being read.
    let repo = dirty_repo();
    let output = gitchange(repo.path(), &["status"]);

    assert_eq!(output.status.code(), Some(0));
    assert!(
        !state_path(repo.path()).exists(),
        "the read created a state file"
    );
}

#[test]
fn status_never_advises_even_where_the_persisting_form_would() {
    // Records from two changelists overlap the fresh hunk without an
    // exact anchor match — the ambiguous-overlap shape (ADR 0001), which a
    // persisting refresh resolves by capturing to active and advising.
    // A read resolves nothing (#156): the routing is context-derived, so
    // the hunk reports unassigned, and the advisory the recompute still
    // produces is discarded as a preview — a read commits no decision,
    // and delivering it would repeat on every glance (ADR 0005).
    let repo = dirty_repo();
    seed_state_raw(
        repo.path(),
        r#"{
  "version": 1, "active": "feature",
  "changelists": [{ "name": "feature" }, { "name": "bugfix" }],
  "records": [
    {
      "path": "tracked.txt", "old_start": 1, "old_lines": 1,
      "new_start": 1, "new_lines": 1, "changelist": "feature",
      "anchor": ["-stale\n"], "dormant_since": null
    },
    {
      "path": "tracked.txt", "old_start": 1, "old_lines": 1,
      "new_start": 1, "new_lines": 1, "changelist": "bugfix",
      "anchor": ["-also stale\n"], "dormant_since": null
    }
  ]
}"#,
    );
    let before = state_bytes(repo.path());

    let output = gitchange(repo.path(), &["status"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        output.stderr,
        Vec::<u8>::new(),
        "no notice line on a read, ever"
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        stdout.lines().collect::<Vec<&str>>(),
        vec![
            "* feature",
            "  bugfix",
            "  unassigned",
            CAPTURE_PENDING_HINT,
            "    ○ M tracked.txt 0/1",
            "    ○ ? untracked.txt 0/1",
        ],
        "the ambiguous hunk is not routed, so it stays unassigned"
    );
    assert_eq!(
        state_bytes(repo.path()),
        before,
        "the stale records stand as they were"
    );
}

#[test]
fn status_leaves_a_moved_head_unstamped_and_stranded_records_unclaimed() {
    // ADR 0012's guard, on a read (#156): the fixture is the guard's
    // minimal shape — a stored baseline behind HEAD, the moved path
    // carrying a live record whose anchor can't match, and a hunk on it.
    // A persisting refresh would go dormant, capture, advise twice, and
    // stamp the new baseline; a read does none of it, and the state file
    // — baseline included — is byte-identical afterwards. Grown from
    // `committed_repo` so the fixture has exactly the guard's shape.
    let dir = committed_repo();
    let repo = dir.path();
    // The baseline the state file will claim the records address.
    let baseline = git(repo, &["rev-parse", "HEAD"]);
    // The external HEAD move: a commit touching tracked.txt, so
    // diff(baseline↔HEAD) names it and tier 2 is disabled there.
    std::fs::write(repo.join("tracked.txt"), "two\n").unwrap();
    git(repo, &["add", "."]);
    git(repo, &["commit", "-q", "--no-verify", "-m", "external"]);
    // A worktree hunk whose anchor matches no record: tier 1 can't
    // rescue 'bugfix', so its record is stranded and the hunk captures.
    std::fs::write(repo.join("tracked.txt"), "three\n").unwrap();
    seed_state_raw(
        repo,
        &format!(
            r#"{{
  "version": 1, "active": "feature",
  "baseline_head": "{baseline}",
  "changelists": [{{ "name": "feature" }}, {{ "name": "bugfix" }}],
  "records": [
    {{
      "path": "tracked.txt", "old_start": 1, "old_lines": 1,
      "new_start": 1, "new_lines": 1, "changelist": "bugfix",
      "anchor": ["-stale\n"], "dormant_since": null
    }}
  ]
}}"#
        ),
    );

    let before = state_bytes(repo);

    let output = gitchange(repo, &["status"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        output.stderr,
        Vec::<u8>::new(),
        "neither the guarded capture nor the dormancy is advised on a read"
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        stdout.lines().collect::<Vec<&str>>(),
        vec![
            "* feature",
            "  bugfix",
            "  unassigned",
            CAPTURE_PENDING_HINT,
            "    ○ M tracked.txt 0/1",
        ],
        "the stranded record's hunk is not captured, so it reports unassigned"
    );
    assert_eq!(
        state_bytes(repo),
        before,
        "no baseline stamp, no dormancy write"
    );
}

// --- the JSON envelope -----------------------------------------------------

#[test]
fn status_json_delivers_the_context_envelope() {
    // The whole document, pinned literally (#157): the closed field list,
    // the group order, and — because the comparison is exact — the fields
    // that must *not* be there, the repeated active marker among them. The
    // fixture is the capture-pending state, whose hint the text face prints
    // and whose envelope carries no field for it: `active` non-null beside
    // a non-empty unassigned group is the same derivation.
    let repo = dirty_repo();
    seed_state_raw(
        repo.path(),
        r#"{
  "version": 1, "active": "feature",
  "changelists": [{ "name": "feature" }, { "name": "docs" }],
  "records": [
    {
      "path": "tracked.txt", "old_start": 1, "old_lines": 1,
      "new_start": 1, "new_lines": 1, "changelist": "feature",
      "anchor": ["-one\n", "+two\n"], "dormant_since": null
    }
  ]
}"#,
    );

    assert_eq!(
        status_json(repo.path()),
        serde_json::json!({
            "schema_version": 1,
            "head": { "kind": "branch", "name": "main" },
            "operation": null,
            "active": "feature",
            "groups": [
                {
                    "kind": "changelist",
                    "name": "feature",
                    "files": [{
                        "path": "tracked.txt",
                        "change_kind": "modified",
                        "stage": "unstaged",
                        "staged_hunks": 0,
                        "total_hunks": 1,
                    }],
                },
                // Empty, and still listed: this is the machine's
                // changelist enumeration.
                { "kind": "changelist", "name": "docs", "files": [] },
                {
                    "kind": "unassigned",
                    "files": [{
                        "path": "untracked.txt",
                        "change_kind": "untracked",
                        "stage": "unstaged",
                        "staged_hunks": 0,
                        "total_hunks": 1,
                    }],
                },
            ],
        })
    );
}

#[test]
fn the_capture_pending_state_adds_no_json_field() {
    // The text face gained a line for this state (#156); the envelope
    // gains nothing (#157) — `active` non-null plus a non-empty unassigned
    // group carries the same two record facts.
    let repo = dirty_repo();
    seed_state(repo.path(), "feature", &["feature"]);

    let stdout = String::from_utf8(gitchange(repo.path(), &["status"]).stdout).unwrap();
    assert!(stdout.contains("capture on"), "the text face hints");

    let envelope = status_json(repo.path());
    assert_eq!(envelope["active"], serde_json::json!("feature"));
    assert!(
        !envelope["groups"][1]["files"]
            .as_array()
            .expect("the unassigned group's files")
            .is_empty(),
        "the derivation's other half: {envelope}"
    );
    assert!(
        !envelope.to_string().contains("capture"),
        "no field, no hint, nowhere in the document: {envelope}"
    );
}

#[test]
fn status_json_and_the_text_face_agree_on_selection_and_order() {
    // Both faces render core's `Snapshot::groups`, so they cannot disagree
    // (ADR 0018) — asserted rather than assumed, since only one of them is
    // a parsing contract and only the other is read by eye.
    let repo = dirty_repo();
    seed_state_raw(
        repo.path(),
        r#"{
  "version": 1, "active": null,
  "changelists": [{ "name": "feature" }, { "name": "docs" }],
  "records": [
    {
      "path": "tracked.txt", "old_start": 1, "old_lines": 1,
      "new_start": 1, "new_lines": 1, "changelist": "docs",
      "anchor": ["-one\n", "+two\n"], "dormant_since": null
    }
  ]
}"#,
    );

    let stdout = String::from_utf8(gitchange(repo.path(), &["status"]).stdout).unwrap();
    let envelope = status_json(repo.path());

    let from_json: Vec<(String, Vec<String>)> = group_order(&envelope)
        .into_iter()
        .zip(envelope["groups"].as_array().unwrap())
        .map(|(label, group)| {
            let paths = group["files"]
                .as_array()
                .unwrap()
                .iter()
                .map(|file| file["path"].as_str().unwrap().to_owned())
                .collect();
            (label.to_owned(), paths)
        })
        .collect();

    assert_eq!(text_face_groups(&stdout), from_json, "{stdout}");
}

#[test]
fn status_json_names_where_head_points_in_every_flavour() {
    let repo = dirty_repo();
    assert_eq!(
        status_json(repo.path())["head"],
        serde_json::json!({ "kind": "branch", "name": "main" })
    );

    git(repo.path(), &["checkout", "-q", "--detach"]);
    let head = status_json(repo.path())["head"].clone();
    assert_eq!(head["kind"], serde_json::json!("detached"));
    assert!(
        head["short_id"].as_str().is_some_and(|id| !id.is_empty()),
        "the commit's short id: {head}"
    );

    // A fresh `git init` has no commit for HEAD to name, and the read
    // still answers.
    let unborn = unborn_repo();
    assert_eq!(
        status_json(unborn.path())["head"],
        serde_json::json!({ "kind": "unborn", "name": "main" })
    );
}

#[test]
fn status_json_reports_an_operation_and_states_the_quarantine() {
    // Conflicts first, and its files carry a path alone: a quarantined
    // path owns no hunks (ADR 0007), so a stage mark or a count would be a
    // fabricated fact. The operation is reporting only — the guard that
    // acts on it is commit's.
    let repo = merging_repo();

    let envelope = status_json(repo.path());

    assert_eq!(envelope["operation"], serde_json::json!("merge"));
    assert_eq!(group_order(&envelope), vec!["conflicts", "unassigned"]);
    assert_eq!(
        envelope["groups"][0]["files"],
        serde_json::json!([{ "path": "tracked.txt" }])
    );

    // The two faces select the same path under the same group — the one
    // group where they say it most differently, the text face carrying
    // core's resolve-outside note the envelope's `kind` states instead.
    let stdout = String::from_utf8(gitchange(repo.path(), &["status"]).stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines[0], "  conflicts", "{stdout}");
    assert!(
        lines[1].starts_with("      U tracked.txt ("),
        "sigil and path, no stage token and no counts: {stdout}"
    );
}

#[test]
fn the_unassigned_group_reaches_the_envelope_exactly_as_the_text_rule_has_it() {
    // Present when it holds files or holds the marker, absent otherwise —
    // core's `groups()` decides once and both faces render that decision
    // (#157). The clean tree is the shape that tells the two halves of the
    // rule apart: nothing to hold, so only the marker can put it there.
    let repo = committed_repo();
    seed_state(repo.path(), "feature", &["feature"]);

    let envelope = status_json(repo.path());
    assert_eq!(group_order(&envelope), vec!["feature"]);
    assert_eq!(envelope["active"], serde_json::json!("feature"));

    gitchange(repo.path(), &["switch", "unassigned"]);
    let envelope = status_json(repo.path());
    assert_eq!(
        group_order(&envelope),
        vec!["feature", "unassigned"],
        "empty but active: the group carries capture-off's whole surface"
    );
    assert_eq!(envelope["groups"][1]["files"], serde_json::json!([]));
    assert_eq!(
        envelope["active"],
        serde_json::json!(null),
        "unassigned-active is spelled by the absence of a name, the marker \
         living in `active` alone"
    );
}

#[test]
fn a_status_file_object_spells_every_change_kind_and_stage_it_can_carry() {
    // The shared enums (#157), on the group rows that can carry them: an
    // added path staged whole, a deletion, and — the pair the other tests
    // already cover — a modification and an untracked file. `conflicted`
    // is unreachable here by design (a conflicts group's files are
    // path-only) and `type_changed` is asserted through the same
    // `ChangeKindWire` in core's wire tests.
    let repo = dirty_repo();
    std::fs::write(repo.path().join("added.txt"), "fresh\n").unwrap();
    git(repo.path(), &["add", "added.txt"]);
    std::fs::remove_file(repo.path().join("tracked.txt")).unwrap();

    let envelope = status_json(repo.path());

    assert_eq!(
        file_in(&envelope, 0, "added.txt"),
        serde_json::json!({
            "path": "added.txt",
            "change_kind": "added",
            "stage": "staged",
            "staged_hunks": 1,
            "total_hunks": 1,
        })
    );
    assert_eq!(
        file_in(&envelope, 0, "tracked.txt")["change_kind"],
        serde_json::json!("deleted")
    );
}

#[test]
fn a_status_file_object_counts_only_cleanly_staged_hunks() {
    // `staged_hunks` counts `●` alone: the second hunk is staged and then
    // edited again, so it is staged-stale and surfaces through the file's
    // `partially_staged` rather than being counted staged (#157).
    let repo = two_hunk_repo();
    git(repo.path(), &["add", "long.txt"]);
    std::fs::write(
        repo.path().join("long.txt"),
        long_file("first edited", "last edited twice"),
    )
    .unwrap();

    assert_eq!(
        file_in(&status_json(repo.path()), 0, "long.txt"),
        serde_json::json!({
            "path": "long.txt",
            "change_kind": "modified",
            "stage": "partially_staged",
            "staged_hunks": 1,
            "total_hunks": 2,
        })
    );
}

#[test]
fn a_path_two_changelists_own_appears_under_both_groups() {
    // One record per hunk, to different owners: the path is two rows, and
    // each carries the same whole-file facts — per-group narrowing is the
    // TUI Files panel's, and group-scoped hunk detail is `diff --json`'s
    // (CONTEXT.md §File row).
    let repo = two_hunk_repo();
    seed_state_raw(
        repo.path(),
        r#"{
  "version": 1, "active": null,
  "changelists": [{ "name": "first" }, { "name": "second" }],
  "records": [
    {
      "path": "long.txt", "old_start": 1, "old_lines": 4,
      "new_start": 1, "new_lines": 4, "changelist": "first",
      "anchor": ["-first\n", "+first edited\n"], "dormant_since": null
    },
    {
      "path": "long.txt", "old_start": 9, "old_lines": 4,
      "new_start": 9, "new_lines": 4, "changelist": "second",
      "anchor": ["-last\n", "+last edited\n"], "dormant_since": null
    }
  ]
}"#,
    );

    let envelope = status_json(repo.path());

    assert_eq!(
        group_order(&envelope),
        vec!["first", "second", "unassigned"],
        "unassigned is empty here, and present because it holds the marker"
    );
    let row = serde_json::json!({
        "path": "long.txt",
        "change_kind": "modified",
        "stage": "unstaged",
        "staged_hunks": 0,
        "total_hunks": 2,
    });
    assert_eq!(file_in(&envelope, 0, "long.txt"), row);
    assert_eq!(file_in(&envelope, 1, "long.txt"), row);
    assert_eq!(envelope["groups"][2]["files"], serde_json::json!([]));
}

#[test]
fn status_json_writes_nothing_either() {
    // The read-only refresh under the JSON face too (#157): the fixture is
    // the shape a persisting refresh would move most, read twice, and the
    // state file is byte-identical throughout.
    let repo = dirty_repo();
    seed_state(repo.path(), "feature", &["feature"]);
    let before = state_bytes(repo.path());

    for _ in 0..2 {
        // `status_json` asserts exit 0 and the empty stderr — no notice
        // line on a read, ever.
        let envelope = status_json(repo.path());
        assert_eq!(envelope["active"], serde_json::json!("feature"));
        assert_eq!(
            state_bytes(repo.path()),
            before,
            "no capture, no record write, no baseline stamp"
        );
    }
    assert!(
        !lock_path(repo.path()).exists(),
        "a read takes no lock, so none is left behind"
    );

    // And the same from the other side: a repository that has never run
    // gitchange still has no state file after being read.
    let fresh = dirty_repo();
    status_json(fresh.path());
    assert!(
        !state_path(fresh.path()).exists(),
        "the read created a state file"
    );
}
