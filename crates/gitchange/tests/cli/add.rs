//! End-to-end tests of `add`'s ownership-scoped sweep (#160) and its
//! addressed forms (#162): which hunks a scope reaches, the all-or-nothing
//! refusals, and the receipt. Ground truth is git's own view of the index
//! (`git diff --cached`, `git show :<path>`) — never gitchange's account
//! of it.
//!
//! The fixture is `support::owned_repo`, which `unstage.rs` stages and
//! sweeps in the other direction: ownership records that split one file
//! between two changelists, a changelist that owns nothing, and a clean
//! path — the shapes every offender class needs.
//!
//! The fail-soft split (some hunks stale, all hunks stale) is not here: it
//! needs a worktree edit between the refresh and the apply, which no child
//! process can inject, so it lands at core's integration seam (ADR 0008,
//! core's `staging` suite).

use std::path::Path;

use crate::support::{
    address, git, gitchange, initialised_repo, merging_repo, owned_repo, seed_state, staged,
    staged_paths, write,
};

fn commit(dir: &Path, message: &str) {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "--no-verify", "-m", message]);
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

// --- the addressed forms (#162) ---------------------------------------------
// The three non-declarative exit-`2` checks are in `grammar.rs`, where the
// empty directory proves they answer before any repository is opened.

#[test]
fn an_explicit_id_stages_exactly_the_hunk_it_names() {
    let dir = owned_repo();
    let repo = dir.path();
    let address = address(repo, "a.txt", 0);

    let echo = add(repo, &["feature", &address]);

    assert!(
        echo.contains(&format!("staged 1 hunk — {address}")),
        "{echo}"
    );
    let staged = staged(repo, "a.txt");
    assert!(staged.contains("first edited"), "{staged}");
    assert!(
        !staged.contains("last edited"),
        "the address narrowed the file row to one hunk: {staged}"
    );
}

#[test]
fn the_consistency_guard_names_the_actual_owner() {
    // a.txt's second hunk is unassigned, and 'feature' does own hunks in
    // that file — so this is the cross-ownership stage the guard exists to
    // refuse, and what it prints is the same command with one word changed.
    let dir = owned_repo();
    let repo = dir.path();
    let address = address(repo, "a.txt", 1);

    let refusal = refusal(repo, &["feature", &address]);

    assert!(
        refusal.contains(&format!(
            "hunk '{address}' belongs to unassigned, not 'feature'"
        )),
        "{refusal}"
    );
    assert!(
        refusal.contains(&format!("gitchange add unassigned {address}")),
        "the one-step exact retry: {refusal}"
    );
    assert!(staged_paths(repo).is_empty());
}

#[test]
fn unassigned_addresses_its_own_hunks() {
    let dir = owned_repo();
    let repo = dir.path();
    let address = address(repo, "a.txt", 1);

    let echo = add(repo, &["unassigned", &address]);

    assert!(echo.contains("staged 1 hunk"), "{echo}");
    let staged = staged(repo, "a.txt");
    assert!(staged.contains("last edited"), "{staged}");
    assert!(!staged.contains("first edited"), "{staged}");
}

#[test]
fn an_address_mixes_freely_with_a_whole_path() {
    let dir = owned_repo();
    let repo = dir.path();
    let address = address(repo, "a.txt", 1);

    let echo = add(repo, &["unassigned", "sub/c.txt", &address]);

    assert!(echo.contains("staged 2 hunks"), "{echo}");
    assert!(
        echo.contains(&format!("sub/c.txt, {address}")),
        "the echo names both narrowings, in argument order: {echo}"
    );
    assert_eq!(staged_paths(repo), vec!["a.txt", "sub/c.txt"]);
    assert!(!staged(repo, "a.txt").contains("first edited"));
}

#[test]
fn one_path_may_repeat_with_different_ids() {
    // a.txt's two hunks under their two owners, addressed one at a time:
    // the repeat is two narrowings of one file, not a duplicate.
    let dir = owned_repo();
    let repo = dir.path();
    let first = address(repo, "a.txt", 0);
    let last = address(repo, "a.txt", 1);

    add(repo, &["feature", &first]);
    let echo = add(repo, &["unassigned", &last]);

    assert!(echo.contains("staged 1 hunk"), "{echo}");
    let staged = staged(repo, "a.txt");
    assert!(staged.contains("first edited"), "{staged}");
    assert!(staged.contains("last edited"), "{staged}");
}

#[test]
fn one_hunk_named_twice_in_one_command_moves_once() {
    let dir = owned_repo();
    let repo = dir.path();
    let address = address(repo, "a.txt", 0);

    let echo = add(repo, &["feature", &address, &address]);

    assert!(
        echo.contains("staged 1 hunk"),
        "one hunk named twice is one narrowing: {echo}"
    );
}

#[test]
fn an_addressed_offender_joins_the_all_or_nothing_set() {
    // An address's own refusal is an offender like any other (#162), so it
    // is named beside the path offender rather than bailing ahead of it —
    // and nothing moved.
    let dir = owned_repo();
    let repo = dir.path();

    let refusal = refusal(repo, &["feature", "a.txt:h1a2b3c4", "keep.txt"]);

    assert!(
        refusal.contains("no hunk 'h1a2b3c4' in 'a.txt'"),
        "{refusal}"
    );
    assert!(refusal.contains("'keep.txt' has no changes"), "{refusal}");
    assert!(staged_paths(repo).is_empty());
}

#[test]
fn a_stale_explicit_id_refuses_at_validation() {
    // Mint an address, move the hunk on, replay it: an explicit ID refuses
    // rather than resolving to whatever now sits at that position (#122),
    // so a stale address never acts.
    let dir = owned_repo();
    let repo = dir.path();
    let address = address(repo, "b.txt", 0);
    write(repo, "b.txt", "three\n");

    let refusal = refusal(repo, &["docs", &address]);

    assert!(refusal.contains("addresses one snapshot"), "{refusal}");
    assert!(staged_paths(repo).is_empty());
}

#[test]
fn a_misparse_dies_as_not_found() {
    // A suffix that is not ID-shaped leaves the whole token a path (#122),
    // which then lives or dies as one — never as a silent misaddress.
    let dir = owned_repo();

    let refusal = refusal(dir.path(), &["feature", "a.txt:notanid"]);

    assert!(
        refusal.contains("no such path 'a.txt:notanid'"),
        "{refusal}"
    );
}

#[test]
fn containing_stages_the_hunk_holding_the_line() {
    let dir = owned_repo();
    let repo = dir.path();

    let echo = add(repo, &["feature", "a.txt", "--containing", "first edited"]);

    assert!(echo.contains("staged 1 hunk"), "{echo}");
    assert!(
        echo.contains(&address(repo, "a.txt", 0)),
        "the echo names the address the value resolved to: {echo}"
    );
    let staged = staged(repo, "a.txt");
    assert!(staged.contains("first edited"), "{staged}");
    assert!(!staged.contains("last edited"), "{staged}");
}

#[test]
fn containing_refuses_zero_and_several_matches_with_candidate_ids() {
    let dir = owned_repo();
    let repo = dir.path();
    let candidates = [address(repo, "a.txt", 0), address(repo, "a.txt", 1)];

    // Zero: `line 2` is context in the first hunk, and matching is over
    // changed lines only — nothing the caller did not write can match.
    let zero = refusal(repo, &["feature", "a.txt", "--containing", "line 2"]);
    // Several: `edited` is in both hunks, so the value has to be narrowed
    // rather than broadened.
    let several = refusal(repo, &["feature", "a.txt", "--containing", "edited"]);

    assert!(
        zero.contains("no changed line of 'a.txt' contains 'line 2'"),
        "{zero}"
    );
    assert!(several.contains("is in 2 hunks of 'a.txt'"), "{several}");
    for candidate in &candidates {
        assert!(zero.contains(candidate), "{candidate} not in: {zero}");
        assert!(several.contains(candidate), "{candidate} not in: {several}");
    }
    assert!(staged_paths(repo).is_empty());
}

#[test]
fn containing_resolves_over_the_universe_before_the_verbs_scope() {
    // The value matches a hunk 'docs' does not own: the answer is the
    // consistency guard naming who does, not "no match". Scope-filtered
    // matching would make one value quietly mean different hunks under
    // different scopes (#145).
    let dir = owned_repo();
    let repo = dir.path();

    let refusal = refusal(repo, &["docs", "a.txt", "--containing", "first edited"]);

    assert!(
        refusal.contains("belongs to 'feature', not 'docs'"),
        "{refusal}"
    );
    assert!(staged_paths(repo).is_empty());
}

/// A file whose index entry holds two hunks at once — ADR 0009's entry
/// unit: a text edit staged, then the worktree copy replaced with binary
/// content, so the universe presents a whole-file hunk beside the
/// index-only text hunk it shares an index entry with. Both unassigned.
fn entry_unit_repo() -> tempfile::TempDir {
    let dir = initialised_repo();
    let repo = dir.path();
    write(repo, "f.txt", "one\ntwo\nthree\n");
    commit(repo, "init");
    write(repo, "f.txt", "one\nEDIT\nthree\n");
    git(repo, &["add", "f.txt"]);
    std::fs::write(repo.join("f.txt"), b"bin\x00\x01\x02data\n").unwrap();
    dir
}

#[test]
fn a_degenerate_hunk_is_a_candidate_but_never_a_match() {
    // f.txt's whole-file hunk is the binary one: no changed lines, so a
    // `--containing` value can only ever zero-match it — and its ID is in
    // the candidate list all the same (#122).
    let dir = entry_unit_repo();
    let repo = dir.path();

    let refusal = refusal(repo, &["unassigned", "f.txt", "--containing", "absent"]);

    assert!(
        refusal.contains("no changed line of 'f.txt' contains 'absent'"),
        "{refusal}"
    );
    assert!(
        refusal.contains(&address(repo, "f.txt", 0)),
        "the degenerate hunk is a candidate: {refusal}"
    );
}

#[test]
fn a_degenerate_hunk_addresses_like_any_other() {
    // The whole-file (binary) hunk staged by its own explicit ID: a
    // degenerate hunk is a hunk, and it moves by the whole-file index
    // write its flavour has (ADR 0017).
    let dir = entry_unit_repo();
    let repo = dir.path();
    let whole_file = address(repo, "f.txt", 0);

    let echo = add(repo, &["unassigned", &whole_file]);

    assert!(echo.contains("staged 1 hunk"), "{echo}");
    assert_eq!(
        git(repo, &["show", ":f.txt"]),
        "bin\0\u{1}\u{2}data",
        "the binary worktree content reached the index"
    );
}

#[test]
fn an_addressed_entry_unit_member_moves_alone() {
    // Membership moves an entry unit together (ADR 0009), but staging never
    // widens (#145): the addressed hunk moves and its unit-mate stays put.
    // The unit-subset refusal is `assign`'s own, and these verbs inherit
    // nothing from it.
    let dir = entry_unit_repo();
    let repo = dir.path();
    let text = address(repo, "f.txt", 1);

    let echo = add(repo, &["unassigned", &text]);

    assert!(echo.contains("staged 1 hunk"), "{echo}");
    assert_eq!(
        staged(repo, "f.txt"),
        "one\ntwo\nthree",
        "the index-only hunk was discarded, and the binary whole-file hunk \
         beside it — the other member of the same index entry — stayed out"
    );
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
