//! End-to-end tests of `add`'s ownership-scoped sweep (#160): which hunks
//! a scope reaches, the all-or-nothing refusals, and the receipt. Ground
//! truth is git's own view of the index (`git diff --cached`, `git show
//! :<path>`) — never gitchange's account of it.
//!
//! Its own module because the fixtures are its own: ownership records that
//! split one file between two changelists, a changelist that owns nothing,
//! and a clean path — the shapes every offender class needs.
//!
//! The fail-soft split (some hunks stale, all hunks stale) is not here: it
//! needs a worktree edit between the refresh and the apply, which no child
//! process can inject, so it lands at core's integration seam (ADR 0008,
//! core's `staging` suite).

use std::path::Path;

use crate::support::{
    git, gitchange, initialised_repo, long_file, merging_repo, seed_state, seed_state_raw,
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

/// The fixture most of these tests sweep, capture off (`active: null`) so
/// ownership is exactly what the records say:
///
/// - `a.txt` — two hunks, the first claimed by `'feature'` by record, the
///   second recordless and so unassigned: one file, two owners, which is
///   what makes path narrowing observable.
/// - `b.txt` — one hunk, claimed by `'docs'`.
/// - `sub/c.txt` — one hunk, unassigned; also the subdirectory a directory
///   argument needs.
/// - `keep.txt` — committed and untouched: the clean-path offender.
/// - `'empty'` — a changelist owning no hunks at all.
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
  "changelists": [{ "name": "feature" }, { "name": "docs" }, { "name": "empty" }],
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

/// `gitchange add <args>`, asserted to have succeeded, its stdout echo.
fn add(dir: &Path, args: &[&str]) -> String {
    let output = gitchange(dir, &[&["add"], args].concat());
    assert_eq!(
        output.status.code(),
        Some(0),
        "gitchange add {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// The refusal `gitchange add <args>` produces: exit 1, empty stdout, and
/// the stderr text for the caller to read the fragments it cares about.
fn refusal(dir: &Path, args: &[&str]) -> String {
    let output = gitchange(dir, &[&["add"], args].concat());
    assert_eq!(
        output.status.code(),
        Some(1),
        "gitchange add {args:?} should refuse"
    );
    assert_eq!(
        output.stdout,
        Vec::<u8>::new(),
        "a failed command leaves stdout empty (#122)"
    );
    String::from_utf8(output.stderr).unwrap()
}

/// The paths the index holds a change for, in git's order — the ground
/// truth for what a sweep reached.
fn staged_paths(dir: &Path) -> Vec<String> {
    let staged = git(dir, &["diff", "--cached", "--name-only"]);
    staged.lines().map(str::to_owned).collect()
}

/// The index's content for one path, as git resolves it.
fn staged(dir: &Path, path: &str) -> String {
    git(dir, &["show", &format!(":{path}")])
}

// --- the sweep --------------------------------------------------------------

#[test]
fn a_bare_changelist_stages_every_hunk_it_owns_and_nobody_elses() {
    let dir = owned_repo();
    let repo = dir.path();

    let echo = add(repo, &["feature"]);

    assert!(echo.contains("staged 1 hunk"), "{echo}");
    assert_eq!(staged_paths(repo), vec!["a.txt"]);
    let staged = staged(repo, "a.txt");
    assert!(staged.contains("first edited"), "{staged}");
    assert!(
        !staged.contains("last edited"),
        "the co-owner's hunk stayed out of the index: {staged}"
    );
}

#[test]
fn a_path_narrows_to_the_file_row_so_a_co_owned_file_never_leaks() {
    let dir = owned_repo();
    let repo = dir.path();

    // Same file, the other owner: only that owner's hunk moves, and the
    // unassigned sweep's other file is left behind by the narrowing.
    let echo = add(repo, &["unassigned", "a.txt"]);

    assert!(echo.contains("staged 1 hunk"), "{echo}");
    assert_eq!(staged_paths(repo), vec!["a.txt"]);
    let staged = staged(repo, "a.txt");
    assert!(staged.contains("last edited"), "{staged}");
    assert!(!staged.contains("first edited"), "{staged}");
}

#[test]
fn unassigned_is_a_scope_that_sweeps() {
    let dir = owned_repo();
    let repo = dir.path();

    let echo = add(repo, &["unassigned"]);

    assert!(echo.contains("staged 2 hunks"), "{echo}");
    assert_eq!(staged_paths(repo), vec!["a.txt", "sub/c.txt"]);
    assert!(!staged(repo, "a.txt").contains("first edited"));
}

#[test]
fn a_sweep_takes_a_staged_stale_hunk_too() {
    // `add` states that the worktree version is the one meant — `git
    // add`'s own meaning on a re-modified file (#145).
    let dir = owned_repo();
    let repo = dir.path();
    git(repo, &["add", "sub/c.txt"]);
    write(repo, "sub/c.txt", "three\n");

    let echo = add(repo, &["unassigned", "sub/c.txt"]);

    assert!(echo.contains("staged 1 hunk"), "{echo}");
    assert_eq!(staged(repo, "sub/c.txt"), "three");
}

#[test]
fn repeating_an_add_over_a_fully_staged_scope_is_satisfied() {
    let dir = owned_repo();
    let repo = dir.path();
    add(repo, &["feature"]);

    let echo = add(repo, &["feature"]);

    assert!(echo.contains("nothing to stage"), "{echo}");
    assert_eq!(staged_paths(repo), vec!["a.txt"]);
}

#[test]
fn the_stage_alias_reaches_the_same_behaviour() {
    // Routing is the skeleton's test (`grammar.rs`); this is the semantic
    // spot-check that the alias is the same verb.
    let dir = owned_repo();
    let repo = dir.path();

    let output = gitchange(repo, &["stage", "docs"]);

    assert_eq!(output.status.code(), Some(0));
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("staged 1 hunk")
    );
    assert_eq!(staged_paths(repo), vec!["b.txt"]);
}

// --- the refusals -----------------------------------------------------------

#[test]
fn all_is_a_view_not_a_scope() {
    let dir = owned_repo();

    let refusal = refusal(dir.path(), &["all"]);

    assert!(refusal.contains("'all' is a view"), "{refusal}");
    assert!(staged_paths(dir.path()).is_empty());
}

#[test]
fn an_unrecognised_changelist_lists_the_candidates() {
    let dir = owned_repo();

    let refusal = refusal(dir.path(), &["ghost"]);

    assert!(refusal.contains("no changelist named 'ghost'"), "{refusal}");
    assert!(
        refusal.contains("unassigned, 'feature', 'docs', 'empty'"),
        "{refusal}"
    );
}

#[test]
fn a_git_habit_is_taught_the_changelist_first_grammar() {
    // `gitchange add a.txt` parses its path as the changelist; the refusal
    // has to teach the grammar, because "no changelist named 'a.txt'"
    // alone reads as a lie about a file that plainly exists (#145).
    let dir = owned_repo();

    let refusal = refusal(dir.path(), &["a.txt"]);

    assert!(refusal.contains("no changelist named 'a.txt'"), "{refusal}");
    assert!(refusal.contains("names the changelist first"), "{refusal}");
    assert!(staged_paths(dir.path()).is_empty());
}

#[test]
fn a_changelist_owning_no_hunks_has_nothing_to_stage() {
    let dir = owned_repo();

    let refusal = refusal(dir.path(), &["empty"]);

    assert!(refusal.contains("'empty' owns no hunks"), "{refusal}");
}

#[test]
fn a_path_whose_hunks_are_owned_elsewhere_names_the_actual_owners() {
    let dir = owned_repo();

    let refusal = refusal(dir.path(), &["docs", "a.txt"]);

    assert!(
        refusal.contains("no hunks of 'a.txt' belong to 'docs'"),
        "{refusal}"
    );
    assert!(refusal.contains("'feature', unassigned"), "{refusal}");
}

#[test]
fn a_clean_path_refuses_rather_than_sweeping_nothing() {
    let dir = owned_repo();

    let refusal = refusal(dir.path(), &["feature", "keep.txt"]);

    assert!(refusal.contains("'keep.txt' has no changes"), "{refusal}");
}

#[test]
fn a_directory_names_the_changed_files_under_it() {
    let dir = owned_repo();

    let refusal = refusal(dir.path(), &["unassigned", "sub"]);

    assert!(refusal.contains("'sub' is a directory"), "{refusal}");
    assert!(refusal.contains("sub/c.txt"), "{refusal}");
}

#[test]
fn a_repo_escaping_path_refuses() {
    // The shared grammar's own refusal (#122/#158), asserted here because
    // it is one of the offender classes `add`'s validation collects.
    let dir = owned_repo();

    let refusal = refusal(dir.path(), &["feature", "../outside.txt"]);

    assert!(refusal.contains("is outside the repository"), "{refusal}");
}

#[test]
fn a_conflicted_path_is_quarantined_rather_than_swept() {
    let dir = merging_repo();

    let refusal = refusal(dir.path(), &["unassigned", "tracked.txt"]);

    assert!(refusal.contains("tracked.txt is conflicted"), "{refusal}");
}

#[test]
fn one_offender_among_valid_arguments_refuses_the_whole_command() {
    let dir = owned_repo();
    let repo = dir.path();

    // Two offenders of different classes beside a path that would have
    // swept fine: every offender is named, and nothing moved.
    let refusal = refusal(repo, &["feature", "a.txt", "keep.txt", "gone.txt"]);

    assert!(refusal.contains("'keep.txt' has no changes"), "{refusal}");
    assert!(refusal.contains("no such path 'gone.txt'"), "{refusal}");
    assert_eq!(
        git(repo, &["diff", "--cached"]),
        "",
        "a refused command leaves the index byte-identical"
    );
}

#[test]
fn an_unrecognised_changelist_still_reports_the_path_offenders_beside_it() {
    let dir = owned_repo();

    let refusal = refusal(dir.path(), &["ghost", "gone.txt"]);

    assert!(refusal.contains("no changelist named 'ghost'"), "{refusal}");
    assert!(refusal.contains("no such path 'gone.txt'"), "{refusal}");
}

#[test]
fn the_grammar_note_sits_after_the_offender_list_not_inside_it() {
    // The git habit with a second offender behind it: the offenders stay
    // one `; `-joined line, and the note about what to type instead comes
    // after them — where a reader looks for the next step.
    let dir = owned_repo();

    let refusal = refusal(dir.path(), &["a.txt", "gone.txt"]);

    let offenders = refusal
        .lines()
        .find(|line| line.contains("no changelist named 'a.txt'"))
        .expect("the offender list");
    assert!(
        offenders.contains("no such path 'gone.txt'"),
        "both offenders on one line: {refusal}"
    );
    let note = refusal
        .lines()
        .find(|line| line.contains("names the changelist first"))
        .expect("the grammar note");
    assert!(
        !note.contains("no such path"),
        "the note is its own line: {refusal}"
    );
}

// --- the addressed forms, still to build (#162) ----------------------------
// Refused rather than quietly swept: either would act on a wider scope than
// the caller named. #162 replaces both guards with the real resolution.

#[test]
fn the_addressed_forms_refuse_as_unbuilt() {
    let dir = owned_repo();
    let repo = dir.path();

    for args in [
        &["feature", "a.txt:h1a2b3c4"][..],
        &["feature", "a.txt", "--containing", "first edited"],
    ] {
        let refusal = refusal(repo, args);
        assert!(
            refusal.contains("is not implemented yet"),
            "{args:?}: {refusal}"
        );
    }
    assert_eq!(git(repo, &["diff", "--cached"]), "");
}

// --- the receipt ------------------------------------------------------------

#[test]
fn add_stages_the_hunks_its_own_refresh_just_captured() {
    // The deferred-capture flow (#122): `switch` writes the marker and
    // nothing else, so the hunk edited after it is claimed by this
    // invocation's own persisting refresh — and staged by the sweep that
    // follows, with the capture reported once on the receipt.
    let dir = initialised_repo();
    let repo = dir.path();
    write(repo, "a.txt", "one\n");
    commit(repo, "init");
    seed_state(repo, "feature", &["feature"]);
    write(repo, "a.txt", "two\n");

    let output = gitchange(repo, &["add", "feature"]);

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("staged 1 hunk"),
        "{stderr}"
    );
    assert!(
        stderr.contains("gitchange: notice: auto-captured hunk at a.txt"),
        "{stderr}"
    );
    assert_eq!(staged(repo, "a.txt"), "two");
}
