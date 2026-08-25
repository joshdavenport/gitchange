//! End-to-end tests of `diff`'s scope resolution and its two faces
//! (#158/#159): which files a scope selects, how an address is validated,
//! the teaching fragments the annotated patch carries, and the envelope the
//! machine face delivers. Its own module because `diff`'s fixtures are its
//! own: ownership records, degenerate hunks, and identical hunks that need
//! telling apart.
//!
//! The dialect's shapes are pinned at core's serialiser seam (core's `wire`
//! suite, ADR 0018); what these assert about the JSON is what only the
//! binary can answer — the flags wired, the delivery, and the two faces
//! agreeing.
//!
//! The builders below start from `support::initialised_repo` rather than
//! `support::committed_repo` — these fixtures choose their own first
//! commit's contents, so the shared stem stops at the identity.

use std::path::Path;

use crate::support::{
    git, git_command, gitchange, initialised_repo, long_file, seed_state_raw, state_path,
};

fn write(dir: &Path, path: &str, contents: &str) {
    let file = dir.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, contents).unwrap();
}

fn commit(dir: &Path, message: &str) {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "--no-verify", "-m", message]);
}

/// The fixture most of these tests read: three changed files across two
/// changelists and unassigned, plus a clean file and a subdirectory.
///
/// - `a.txt` — two hunks, the first claimed by `'feature'` by record, the
///   second recordless and so unassigned: one file, two owners, which is
///   what makes "whole file objects, foreign hunks included" observable.
/// - `b.txt` — one hunk, claimed by `'docs'`.
/// - `sub/c.txt` — one hunk, unassigned; the subdirectory a directory
///   argument and a cwd-relative path both need.
/// - `keep.txt` — committed and untouched: a valid-but-empty scope.
fn owned_repo() -> tempfile::TempDir {
    let dir = initialised_repo();
    let repo = dir.path();
    write(repo, "a.txt", &long_file("first", "last"));
    write(repo, "b.txt", "one\n");
    write(repo, "sub/c.txt", "one\n");
    write(repo, "keep.txt", "unchanged\n");
    commit(repo, "init");
    write(repo, "a.txt", &long_file("first edited", "last edited"));
    write(repo, "b.txt", "two\n");
    write(repo, "sub/c.txt", "two\n");
    seed_state_raw(
        repo,
        r#"{
  "version": 1, "active": null,
  "changelists": [{ "name": "feature" }, { "name": "docs" }],
  "records": [
    {
      "path": "a.txt", "old_start": 1, "old_lines": 4,
      "new_start": 1, "new_lines": 4, "changelist": "feature",
      "anchor": ["-first\n", "+first edited\n"], "dormant_since": null
    },
    {
      "path": "b.txt", "old_start": 1, "old_lines": 1,
      "new_start": 1, "new_lines": 1, "changelist": "docs",
      "anchor": ["-one\n", "+two\n"], "dormant_since": null
    }
  ]
}"#,
    );
    dir
}

/// The command's stdout, the run asserted to have succeeded.
fn diff(dir: &Path, args: &[&str]) -> String {
    let output = gitchange(dir, &[&["diff"], args].concat());
    assert_eq!(
        output.status.code(),
        Some(0),
        "gitchange diff {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        output.stderr,
        Vec::<u8>::new(),
        "a read advises nothing (#143)"
    );
    String::from_utf8(output.stdout).unwrap()
}

/// The refusal `gitchange diff <args>` produces: exit 1, empty stdout, and
/// the stderr line, returned for the caller to read the fragments it cares
/// about (#122: refusals name the condition and the next step).
fn refusal(dir: &Path, args: &[&str]) -> String {
    let output = gitchange(dir, &[&["diff"], args].concat());
    assert_eq!(
        output.status.code(),
        Some(1),
        "gitchange diff {args:?} should refuse"
    );
    assert_eq!(
        output.stdout,
        Vec::<u8>::new(),
        "a failed command leaves stdout empty (#122)"
    );
    String::from_utf8(output.stderr).unwrap()
}

/// The paths a patch names, in print order — the selection, read off the
/// face the way a human reads it.
fn files_in(patch: &str) -> Vec<&str> {
    patch
        .lines()
        .filter_map(|line| line.strip_prefix("diff --git a/"))
        .map(|line| line.split(' ').next().expect("the header's first path"))
        .collect()
}

/// The composed addresses a patch carries, in print order: the last token
/// of each annotated header, which is exactly the string an agent copies
/// out and pastes back into a verb.
fn addresses_in(patch: &str) -> Vec<&str> {
    patch
        .lines()
        .filter_map(|line| line.strip_suffix(']'))
        .filter_map(|line| line.rsplit_once(' '))
        .map(|(_, address)| address)
        .filter(|address| address.contains(":h"))
        .collect()
}

/// The annotated header lines — `@@` headers and the borrowed degenerate
/// spellings alike, which is every line the face annotates.
fn annotated(patch: &str) -> Vec<&str> {
    patch
        .lines()
        .filter(|line| line.ends_with(']') && line.contains(":h"))
        .collect()
}

// --- scoping ---------------------------------------------------------------

#[test]
fn bare_diff_shows_the_whole_hunk_universe_annotated() {
    // Git's unstaged-only default is not borrowed (#143): `diff` is an
    // inventory, and every hunk in it carries its owner, its stage token,
    // and its address.
    let repo = owned_repo();
    let patch = diff(repo.path(), &[]);

    assert_eq!(files_in(&patch), vec!["a.txt", "b.txt", "sub/c.txt"]);
    let headers = annotated(&patch);
    assert_eq!(
        headers.len(),
        4,
        "two hunks in a.txt, one each after: {patch}"
    );
    assert!(
        headers[0].starts_with("@@ -1,4 +1,4 @@ ['feature' ○ a.txt:h"),
        "{headers:?}"
    );
    assert!(headers[1].contains("[unassigned ○ a.txt:h"), "{headers:?}");
    assert!(headers[2].contains("['docs' ○ b.txt:h"), "{headers:?}");
    assert!(
        headers[3].contains("[unassigned ○ sub/c.txt:h"),
        "{headers:?}"
    );
}

#[test]
fn a_changelist_scope_selects_whole_files_with_foreign_hunks_included() {
    // The patch git cannot print — and the one structural rule of every
    // scope (#143): a scope chooses files, never which of a file's hunks
    // appear, so 'feature' brings a.txt's unassigned second hunk with it.
    let repo = owned_repo();
    let patch = diff(repo.path(), &["feature"]);

    assert_eq!(files_in(&patch), vec!["a.txt"]);
    let headers = annotated(&patch);
    assert_eq!(headers.len(), 2, "{patch}");
    assert!(headers[0].contains("['feature' "), "{headers:?}");
    assert!(
        headers[1].contains("[unassigned "),
        "the foreign hunk stays"
    );
}

#[test]
fn unassigned_is_a_legal_scope_and_all_is_not() {
    let repo = owned_repo();

    // Unassigned owns a.txt's second hunk and sub/c.txt's only one.
    assert_eq!(
        files_in(&diff(repo.path(), &["unassigned"])),
        vec!["a.txt", "sub/c.txt"]
    );

    // One scope, one spelling: bare `diff` is already the whole view.
    let stderr = refusal(repo.path(), &["all"]);
    assert!(stderr.contains("'all' is not a diff scope"), "{stderr}");
}

#[test]
fn paths_union_and_a_changelist_intersects_them_at_the_file_level() {
    let repo = owned_repo();

    assert_eq!(
        files_in(&diff(repo.path(), &["--", "b.txt", "a.txt"])),
        vec!["a.txt", "b.txt"],
        "paths union, and the result keeps the snapshot's path order"
    );
    assert_eq!(
        files_in(&diff(repo.path(), &["feature", "--", "a.txt", "b.txt"])),
        vec!["a.txt"],
        "the changelist intersects the paths at the file level"
    );
    assert_eq!(
        diff(repo.path(), &["docs", "--", "a.txt"]),
        "",
        "an intersection that selects nothing is an empty answer, not an error"
    );
}

#[test]
fn a_clean_path_is_an_answer_and_a_missing_one_is_a_wrong_question() {
    let repo = owned_repo();

    assert_eq!(diff(repo.path(), &["--", "keep.txt"]), "");

    let stderr = refusal(repo.path(), &["--", "nope.txt"]);
    assert!(stderr.contains("no such path 'nope.txt'"), "{stderr}");
}

#[test]
fn path_offenders_are_named_all_at_once() {
    // All-or-nothing (#122): one round trip teaches every mistake on the
    // command line — the directory, the escape, and the typo together.
    let repo = owned_repo();

    let stderr = refusal(repo.path(), &["--", "sub", "../outside.txt", "nope.txt"]);

    assert!(
        stderr.contains("'sub' is a directory — name the changed files under it: sub/c.txt"),
        "{stderr}"
    );
    assert!(
        stderr.contains("'../outside.txt' is outside the repository"),
        "{stderr}"
    );
    assert!(stderr.contains("no such path 'nope.txt'"), "{stderr}");
}

#[test]
fn paths_resolve_against_the_cwd_and_print_repo_relative() {
    // Git's own grammar in, gitchange's one path spelling out (#122): an
    // address printed from a subdirectory is the same string as one
    // printed from the root.
    let repo = owned_repo();
    let sub = repo.path().join("sub");

    let patch = diff(&sub, &["--", "c.txt", "../a.txt"]);

    assert_eq!(files_in(&patch), vec!["a.txt", "sub/c.txt"]);
    assert!(
        addresses_in(&patch)
            .iter()
            .all(|address| { address.starts_with("a.txt:") || address.starts_with("sub/c.txt:") }),
        "{patch}"
    );
}

/// Temporary (#181): the two spellings, read off `test (windows-latest)`.
/// Fails on purpose — a passing test's output is captured, and the values
/// are the whole point. Deleted once the mechanism is recorded on the
/// comparison it explains.
#[cfg(windows)]
#[test]
fn the_windows_spellings_of_one_directory() {
    let repo = owned_repo();
    let cwd = std::process::Command::new("cmd")
        .args(["/C", "cd"])
        .current_dir(repo.path())
        .output()
        .expect("run cmd");

    panic!(
        "temp_dir:  {:?}\ntempdir:   {:?}\ncanonical: {:?}\nchild cwd: {}\ntoplevel:  {}",
        std::env::temp_dir(),
        repo.path(),
        repo.path().canonicalize(),
        String::from_utf8_lossy(&cwd.stdout).trim(),
        git(repo.path(), &["rev-parse", "--show-toplevel"]),
    );
}

#[test]
fn an_absolute_path_resolves_and_one_outside_the_worktree_still_refuses() {
    // Either spelling in (#181): the caller's cwd and libgit2's worktree
    // root can name the same directory differently, so both sides of the
    // comparison are canonicalized — and an absolute path typed from
    // outside must still read as the escape it is.
    let repo = owned_repo();
    let inside = repo.path().join("a.txt");
    let outside = repo.path().parent().unwrap().join("outside.txt");

    assert_eq!(
        files_in(&diff(repo.path(), &["--", inside.to_str().unwrap()])),
        vec!["a.txt"]
    );

    let stderr = refusal(repo.path(), &["--", outside.to_str().unwrap()]);
    assert!(stderr.contains("is outside the repository"), "{stderr}");
}

#[test]
fn a_deleted_path_named_as_an_argument_resolves_through_the_snapshot() {
    // The tail of a path argument stays lexical (#181): a deleted file has
    // no on-disk path to canonicalize, and canonicalizing it would make
    // `diff` unable to name the deletion it prints.
    let repo = initialised_repo();
    write(repo.path(), "gone.txt", "one\n");
    commit(repo.path(), "init");
    std::fs::remove_file(repo.path().join("gone.txt")).unwrap();

    let patch = diff(repo.path(), &["--", "gone.txt"]);

    assert!(patch.contains("--- a/gone.txt\n+++ /dev/null\n"), "{patch}");
}

// --- token resolution ------------------------------------------------------

#[test]
fn a_token_that_is_both_a_changelist_and_a_path_refuses_naming_the_cure() {
    // Git's rule set (#143): with no `--` to settle it, gitchange picks
    // neither reading — and `--` cures it in both directions.
    let repo = owned_repo();
    seed_state_raw(
        repo.path(),
        r#"{ "version": 1, "active": null, "changelists": [{ "name": "a.txt" }] }"#,
    );

    let stderr = refusal(repo.path(), &["a.txt"]);
    assert!(
        stderr.contains("'a.txt' is both a changelist and a path"),
        "{stderr}"
    );
    assert!(stderr.contains("gitchange diff a.txt --"), "{stderr}");
    assert!(stderr.contains("gitchange diff -- a.txt"), "{stderr}");

    assert_eq!(
        diff(repo.path(), &["a.txt", "--"]),
        "",
        "the changelist reading owns nothing"
    );
    assert_eq!(
        files_in(&diff(repo.path(), &["--", "a.txt"])),
        vec!["a.txt"],
        "the path reading is the file"
    );
}

#[test]
fn a_second_changelist_name_reads_as_a_path_and_fails_loud() {
    // Gitchange has no range grammar, so there is nothing for a second
    // name to mean (#143): everything after the first positional is a
    // path, and a changelist name is not one.
    let repo = owned_repo();

    let stderr = refusal(repo.path(), &["feature", "docs"]);

    assert!(stderr.contains("no such path 'docs'"), "{stderr}");
}

#[test]
fn dash_c_is_where_paths_resolve_from() {
    // `-C` is "run as if launched in <dir>" (#139), so a path argument
    // resolves against it without `diff` reading the flag at all.
    let repo = owned_repo();
    let elsewhere = tempfile::tempdir().unwrap();
    let repo_path = repo.path().to_str().unwrap();

    let output = gitchange(elsewhere.path(), &["-C", repo_path, "diff", "--", "b.txt"]);

    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        files_in(&String::from_utf8(output.stdout).unwrap()),
        vec!["b.txt"]
    );
}

#[test]
fn a_token_that_is_neither_refuses_naming_both_readings_and_the_candidates() {
    let repo = owned_repo();

    let stderr = refusal(repo.path(), &["featur"]);

    assert!(
        stderr.contains("'featur' is neither a changelist nor a path"),
        "{stderr}"
    );
    assert!(
        stderr.contains("unassigned") && stderr.contains("'feature'") && stderr.contains("'docs'"),
        "the valid scopes are listed: {stderr}"
    );
}

// --- hunk addresses --------------------------------------------------------

#[test]
fn an_address_selects_the_whole_file_and_a_seven_character_prefix_resolves() {
    // A `<path>:<hunk-id>` scope is a selector with validation, never hunk
    // narrowing (#143): it answers "show me the file, and fail loud if my
    // address has aged".
    let repo = owned_repo();
    let patch = diff(repo.path(), &["--", "a.txt"]);
    let second = addresses_in(&patch)[1];
    let (path, id) = second.split_once(':').unwrap();
    let short = format!("{path}:{}", &id[..8]);

    let scoped = diff(repo.path(), &[&short]);

    assert_eq!(files_in(&scoped), vec!["a.txt"]);
    assert_eq!(
        annotated(&scoped).len(),
        2,
        "the whole file, the addressed hunk's siblings included"
    );
    assert!(
        id.trim_start_matches('h').len() >= 7,
        "the abbreviation the face prints clears the minimum it must be pasteable at: {id}"
    );
}

#[test]
fn a_short_or_unknown_or_stale_address_refuses() {
    let repo = owned_repo();
    let address = addresses_in(&diff(repo.path(), &["--", "a.txt"]))[0].to_owned();

    let stderr = refusal(repo.path(), &["--", "a.txt:h123"]);
    assert!(stderr.contains("at least 7 characters"), "{stderr}");
    assert_eq!(
        refusal(repo.path(), &["a.txt:h123"]),
        stderr,
        "an address is an address in the scope slot too — one token, one refusal"
    );

    let stderr = refusal(repo.path(), &["--", "a.txt:hdeadbeef"]);
    assert!(
        stderr.contains("no hunk 'hdeadbeef' in 'a.txt'"),
        "{stderr}"
    );

    // The staleness tripwire: the hunk the address named is edited away,
    // so the same string now resolves to nothing rather than to whatever
    // took its place.
    write(
        repo.path(),
        "a.txt",
        &long_file("first edited again", "last edited"),
    );
    let stderr = refusal(repo.path(), &[&address]);
    assert!(stderr.contains("no hunk"), "{stderr}");
    assert!(
        stderr.contains("gitchange diff a.txt"),
        "the refusal names the re-read: {stderr}"
    );
}

#[test]
fn an_address_rooted_at_the_wrong_path_refuses_naming_the_right_one() {
    // The path prefix is a consistency guard (#122), and an ID is
    // repo-unique — so a mis-rooted address is a caller mistake gitchange
    // can point straight at.
    let repo = owned_repo();
    let b = addresses_in(&diff(repo.path(), &["--", "b.txt"]))[0].to_owned();
    let (_, id) = b.split_once(':').unwrap();

    let stderr = refusal(repo.path(), &["--", &format!("a.txt:{id}")]);

    assert!(
        stderr.contains(&format!("hunk '{id}' is in 'b.txt', not 'a.txt'")),
        "{stderr}"
    );
}

/// Seven distinct lines, repeated: the same edit in two non-adjacent
/// blocks is two hunks with byte-identical anchors, so they share a base
/// ID and need the ordinal to tell them apart.
const BLOCK: &str = "alpha\nbeta\ngamma\ndelta\nepsilon\nzeta\neta\n";

#[test]
fn an_ambiguous_address_refuses_listing_candidates_that_resolve() {
    let repo = initialised_repo();
    write(repo.path(), "twin.txt", &BLOCK.repeat(3));
    commit(repo.path(), "init");
    write(
        repo.path(),
        "twin.txt",
        &[
            BLOCK.replace("delta", "DELTA"),
            BLOCK.to_owned(),
            BLOCK.replace("delta", "DELTA"),
        ]
        .concat(),
    );
    let patch = diff(repo.path(), &[]);
    let printed = addresses_in(&patch);
    assert_eq!(printed.len(), 2);
    let (path, id) = printed[0].split_once(':').unwrap();
    let base = id.split('/').next().unwrap().to_owned();

    let stderr = refusal(repo.path(), &["--", &format!("{path}:{base}")]);

    assert!(stderr.contains("is ambiguous"), "{stderr}");
    for candidate in [&printed[0], &printed[1]] {
        assert!(
            stderr.contains(candidate),
            "{candidate} listed in: {stderr}"
        );
    }
    // And each listed candidate is an address that resolves, ordinal and
    // all — the refusal's list is the retry.
    assert_eq!(
        files_in(&diff(repo.path(), &[printed[1]])),
        vec!["twin.txt"]
    );
}

// --- the text face ---------------------------------------------------------

#[test]
#[cfg(unix)]
fn degenerate_hunks_borrow_gits_spellings_and_carry_the_suffix() {
    // A chmod'd binary: two degenerate hunks, distinct addresses, and
    // neither has coordinates to frame — so git's own words for the change
    // stand as the hunk header, annotation and all (#143).
    let repo = initialised_repo();
    std::fs::write(repo.path().join("blob.bin"), [0u8, 1, 2, 3]).unwrap();
    commit(repo.path(), "init");
    std::fs::write(repo.path().join("blob.bin"), [0u8, 9, 9, 9, 9]).unwrap();
    git(repo.path(), &["update-index", "--chmod=+x", "blob.bin"]);

    let patch = diff(repo.path(), &[]);
    let headers = annotated(&patch);

    assert_eq!(headers.len(), 2, "{patch}");
    assert!(
        headers[0].starts_with("old mode 100644 [unassigned "),
        "{headers:?}"
    );
    assert!(patch.contains("\nnew mode 100755\n"), "{patch}");
    assert!(
        headers[1].starts_with("Binary files a/blob.bin and b/blob.bin differ [unassigned "),
        "{headers:?}"
    );
    let addresses = addresses_in(&patch);
    assert_ne!(addresses[0], addresses[1], "one path, two addresses");
}

#[test]
fn a_conflicted_file_states_its_quarantine_and_no_changelist_claims_it() {
    // ADR 0007: an unmerged path owns no hunks, so it is stated with one
    // line and never diffed — and it can only appear in the views that do
    // not ask about ownership.
    let repo = initialised_repo();
    write(repo.path(), "tracked.txt", "one\n");
    commit(repo.path(), "init");
    git(repo.path(), &["checkout", "-q", "-b", "feature"]);
    write(repo.path(), "tracked.txt", "feature side\n");
    commit(repo.path(), "feature edit");
    git(repo.path(), &["checkout", "-q", "main"]);
    write(repo.path(), "tracked.txt", "main side\n");
    commit(repo.path(), "main edit");
    let merge = git_command(repo.path())
        .args(["merge", "--no-edit", "feature"])
        .output()
        .expect("run git");
    assert!(!merge.status.success(), "the merge is meant to conflict");

    let patch = diff(repo.path(), &[]);

    assert_eq!(
        patch.lines().collect::<Vec<&str>>(),
        vec![
            "diff --git a/tracked.txt b/tracked.txt",
            "tracked.txt is conflicted — resolve outside gitchange",
        ],
        "the header, one quarantine line, and no content"
    );
    assert_eq!(
        diff(repo.path(), &["unassigned"]),
        "",
        "a quarantined path owns nothing, so no changelist scope selects it"
    );
    assert_eq!(
        files_in(&diff(repo.path(), &["--", "tracked.txt"])),
        vec!["tracked.txt"],
        "a path scope still states it"
    );
}

#[test]
#[cfg(unix)]
fn a_type_change_is_never_spelled_as_a_chmod() {
    // ADR 0017 keeps the two flavours apart, and git spells them apart
    // too: `old mode`/`new mode` is its chmod pair, while a symlink swap
    // is a deleted file and a new one. Borrowing the chmod pair here would
    // call the swap a permission flip.
    let repo = initialised_repo();
    std::os::unix::fs::symlink("target", repo.path().join("link")).unwrap();
    commit(repo.path(), "init");
    std::fs::remove_file(repo.path().join("link")).unwrap();
    write(repo.path(), "link", "now a regular file\n");

    let patch = diff(repo.path(), &[]);

    assert!(
        annotated(&patch)[0].starts_with("deleted file mode 120000 [unassigned "),
        "{patch}"
    );
    assert!(patch.contains("\nnew file mode 100644\n"), "{patch}");
    assert!(!patch.contains("old mode"), "not a chmod: {patch}");
}

#[test]
fn a_staged_stale_hunk_carries_its_own_token() {
    // `◑` is per-hunk alone (`CONTEXT.md` §Staging set): the index holds a
    // different version of the hunk the worktree now shows.
    let repo = owned_repo();
    git(repo.path(), &["add", "b.txt"]);
    write(repo.path(), "b.txt", "three\n");

    let patch = diff(repo.path(), &["--", "b.txt"]);

    assert!(
        annotated(&patch)[0].contains("['docs' ◑ b.txt:h"),
        "{patch}"
    );
}

#[test]
fn a_deletion_and_an_addition_name_their_missing_side() {
    let repo = initialised_repo();
    write(repo.path(), "gone.txt", "one\n");
    commit(repo.path(), "init");
    std::fs::remove_file(repo.path().join("gone.txt")).unwrap();
    write(repo.path(), "new.txt", "one\n");

    let patch = diff(repo.path(), &[]);

    assert!(patch.contains("--- a/gone.txt\n+++ /dev/null\n"), "{patch}");
    assert!(patch.contains("--- /dev/null\n+++ b/new.txt\n"), "{patch}");
}

#[test]
fn a_staged_hunk_carries_the_staging_sets_token() {
    // The annotation's middle field is the staging set, per hunk: one
    // `git add` and the same hunk reads `●` (ADR 0003).
    let repo = owned_repo();
    git(repo.path(), &["add", "b.txt"]);

    let patch = diff(repo.path(), &["--", "b.txt"]);

    assert!(
        annotated(&patch)[0].contains("['docs' ● b.txt:h"),
        "{patch}"
    );
}

// --- the JSON face ---------------------------------------------------------

/// `diff --json`'s document, parsed — every assertion below reads what a
/// caller would. Delivery is asserted by the shared `diff` helper (exit 0,
/// empty stderr); what this adds is the one-document promise (ADR 0018).
fn diff_json(dir: &Path, args: &[&str]) -> serde_json::Value {
    let stdout = diff(dir, &[&["--json"], args].concat());
    serde_json::from_str(&stdout).expect("the envelope is one JSON document")
}

/// The paths a JSON envelope names, in serialised order — the selection,
/// read off the machine face.
fn json_files(envelope: &serde_json::Value) -> Vec<&str> {
    envelope["files"]
        .as_array()
        .expect("the files array")
        .iter()
        .map(|file| file["path"].as_str().expect("every file has a path"))
        .collect()
}

#[test]
fn the_json_face_arms_a_caller_with_owners_stages_and_full_addresses() {
    // The read that arms `assign`, `add`, and `unstage` (#159): one hunk
    // object per hunk, its owner named, its address carried whole — the
    // sigil and all 64 hex, never the face's abbreviation.
    let repo = owned_repo();

    let envelope = diff_json(repo.path(), &["--", "a.txt"]);

    assert_eq!(envelope["schema_version"], serde_json::json!(1));
    assert_eq!(json_files(&envelope), vec!["a.txt"]);
    let hunks = envelope["files"][0]["hunks"].as_array().unwrap().clone();
    assert_eq!(hunks.len(), 2, "the whole file object: {envelope}");
    assert_eq!(hunks[0]["changelist"], serde_json::json!("feature"));
    assert_eq!(
        hunks[1]["changelist"],
        serde_json::json!(null),
        "unassigned is spelled null, and the foreign hunk stays"
    );
    for hunk in &hunks {
        let id = hunk["id"].as_str().expect("id is a string");
        assert_eq!(
            id.strip_prefix('h').map(str::len),
            Some(64),
            "the sigil travels and the wire never abbreviates: {id}"
        );
        assert_eq!(hunk["stage"], serde_json::json!("unstaged"), "{hunk}");
        assert_eq!(hunk["kind"], serde_json::json!("text"), "{hunk}");
    }
}

#[test]
fn the_two_faces_never_disagree_about_selection_or_order() {
    // ADR 0018's ordering promise at the binary seam: one refresh, one
    // scope resolution, the face an argument to it — so a scope that
    // selects two files selects the same two, in the same order, whichever
    // face is asked.
    let repo = owned_repo();

    for args in [&[][..], &["unassigned"], &["--", "sub/c.txt", "a.txt"]] {
        let text = diff(repo.path(), args);
        assert_eq!(
            json_files(&diff_json(repo.path(), args)),
            files_in(&text),
            "gitchange diff {args:?}"
        );
    }
}

#[test]
fn an_empty_scope_is_an_empty_files_array() {
    // A valid-but-empty scope is an answer, not an error (#143) — and the
    // envelope says so in the dialect rather than by printing nothing.
    let repo = owned_repo();

    assert_eq!(
        diff_json(repo.path(), &["docs", "--", "a.txt"]),
        serde_json::json!({ "schema_version": 1, "files": [] })
    );
}

#[test]
fn no_content_reaches_the_serialiser_and_leaves_the_addresses_untouched() {
    // The assign-as-you-go hot loop's read (#159): the flag is wired
    // through to the envelope, and what an agent addresses by is unchanged
    // — so switching it on never changes which hunk a later command means.
    // That the rest of the document is identical is pinned at core's
    // serialiser seam, where the dialect lives.
    let repo = owned_repo();

    let full = diff_json(repo.path(), &[]);
    let lean = diff_json(repo.path(), &["--no-content"]);

    assert!(full.to_string().contains("\"lines\":"), "{full}");
    assert!(
        !lean.to_string().contains("\"lines\":"),
        "the content is gone, omitted rather than nulled: {lean}"
    );
    assert_eq!(
        addresses_of(&lean),
        addresses_of(&full),
        "same IDs, same offsets"
    );
}

/// Every hunk's address in an envelope, in serialised order: the `id` and
/// `offset` pair a caller pastes back into a verb.
fn addresses_of(envelope: &serde_json::Value) -> Vec<(&str, &serde_json::Value)> {
    envelope["files"]
        .as_array()
        .expect("the files array")
        .iter()
        .flat_map(|file| file["hunks"].as_array().expect("the hunks array"))
        .map(|hunk| (hunk["id"].as_str().expect("an id"), &hunk["offset"]))
        .collect()
}

// --- a read writes nothing -------------------------------------------------

#[test]
fn diff_writes_nothing_and_takes_no_lock() {
    // The read-only refresh at the binary seam (#122/#143): the fixture is
    // the shape a persisting refresh would move most — recordless hunks
    // beside a live changelist — read twice, so the second run proves the
    // first wrote nothing.
    let repo = owned_repo();
    seed_state_raw(
        repo.path(),
        r#"{
  "version": 1, "active": "feature",
  "changelists": [{ "name": "feature" }]
}"#,
    );
    let before = std::fs::read(state_path(repo.path())).unwrap();

    for _ in 0..2 {
        let patch = diff(repo.path(), &[]);
        assert!(
            addresses_in(&patch).len() == 4 && !patch.contains("'feature'"),
            "context-derived ownership never previews: recordless is unassigned\n{patch}"
        );
        // Both faces sit on the same read-only refresh, so both have to
        // leave the state file alone — and report the same ownership.
        let envelope = diff_json(repo.path(), &[]);
        assert!(
            !envelope.to_string().contains("feature"),
            "recordless is unassigned on the wire too: {envelope}"
        );
        assert_eq!(
            std::fs::read(state_path(repo.path())).unwrap(),
            before,
            "no capture, no record write, no baseline stamp"
        );
    }
    assert!(
        !state_path(repo.path())
            .with_file_name("state.json.lock")
            .exists(),
        "a read takes no lock, so none is left behind"
    );
}

#[test]
fn diff_outside_a_repo_exits_1_with_message_on_stderr() {
    let elsewhere = tempfile::tempdir().unwrap();

    let stderr = refusal(elsewhere.path(), &[]);

    assert!(stderr.starts_with("gitchange: "), "{stderr}");
}
