//! End-to-end tests of `unstage`'s mirror of the add sweep (#161) and the
//! addressed forms in this direction (#162): the `●`-only filter, the
//! notices that name each kept `◑`, the ungated addressed `◑`, and the
//! shared scope model asserted in the other direction. Ground truth is
//! git's own view of the index (`git diff --cached`, `git show :<path>`).
//!
//! What is *not* re-asserted here is the offender machinery `add` already
//! pins test-by-test (`add.rs`): both verbs run the same
//! `staging::resolve` against the same snapshot, so this module checks the
//! classes reach `unstage` with its own verb in the text, rather than
//! spelling every class twice.
//!
//! The fail-soft split is core's (ADR 0008): it needs a worktree edit
//! between the refresh and the apply, which no child process can inject.

use std::path::Path;

use crate::support::{address, git, gitchange, owned_repo, staged, staged_paths, write};

/// `add`'s fixture with its whole worktree staged by raw `git add` — the
/// op ADR 0003 says refresh absorbs, and the only way to reach a staged
/// index without going through the verb under test. Ownership is
/// unchanged (`support::owned_repo`), so the two directions sweep exactly
/// the same rows.
fn staged_repo() -> tempfile::TempDir {
    let dir = owned_repo();
    git(dir.path(), &["add", "-A"]);
    dir
}

/// `gitchange unstage <args>`, asserted to have succeeded: the stdout echo
/// and the stderr notices, which this verb's receipts carry as a matter of
/// course.
fn unstage(dir: &Path, args: &[&str]) -> (String, String) {
    let output = gitchange(dir, &[&["unstage"], args].concat());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "gitchange unstage {args:?}: {stderr}"
    );
    (String::from_utf8(output.stdout).unwrap(), stderr)
}

/// The refusal `gitchange unstage <args>` produces: exit 1, empty stdout,
/// and the stderr text for the caller to read the fragments it cares about.
fn refusal(dir: &Path, args: &[&str]) -> String {
    let output = gitchange(dir, &[&["unstage"], args].concat());
    assert_eq!(
        output.status.code(),
        Some(1),
        "gitchange unstage {args:?} should refuse"
    );
    assert_eq!(
        output.stdout,
        Vec::<u8>::new(),
        "a failed command leaves stdout empty (#122)"
    );
    String::from_utf8(output.stderr).unwrap()
}

/// The composed address a notice line names — everything from the path
/// through the ID, which is what a caller pastes back into a verb.
fn address_in(notice: &str, path: &str) -> String {
    let start = notice.find(path).unwrap_or_else(|| panic!("{notice}"));
    notice[start..]
        .split_whitespace()
        .next()
        .expect("the address is one word")
        .to_owned()
}

// --- the sweep --------------------------------------------------------------

#[test]
fn a_bare_changelist_unstages_every_hunk_it_owns_and_nobody_elses() {
    let dir = staged_repo();
    let repo = dir.path();

    let (echo, _) = unstage(repo, &["feature"]);

    assert!(echo.contains("unstaged 1 hunk"), "{echo}");
    let staged = staged(repo, "a.txt");
    assert!(
        !staged.contains("first edited"),
        "'feature''s hunk left the index: {staged}"
    );
    assert!(
        staged.contains("last edited"),
        "the co-owner's hunk stayed staged: {staged}"
    );
    assert_eq!(
        staged_paths(repo),
        vec!["a.txt", "b.txt", "sub/c.txt"],
        "no other file was touched"
    );
}

#[test]
fn a_path_narrows_to_the_file_row_so_a_co_owned_file_never_leaks() {
    let dir = staged_repo();
    let repo = dir.path();

    let (echo, _) = unstage(repo, &["unassigned", "a.txt"]);

    assert!(echo.contains("unstaged 1 hunk"), "{echo}");
    let staged = staged(repo, "a.txt");
    assert!(staged.contains("first edited"), "{staged}");
    assert!(!staged.contains("last edited"), "{staged}");
    assert!(
        staged_paths(repo).contains(&"sub/c.txt".to_owned()),
        "the unassigned sweep's other file was left behind by the narrowing"
    );
}

#[test]
fn unassigned_is_a_scope_that_sweeps() {
    let dir = staged_repo();
    let repo = dir.path();

    let (echo, _) = unstage(repo, &["unassigned"]);

    assert!(echo.contains("unstaged 2 hunks"), "{echo}");
    assert_eq!(staged_paths(repo), vec!["a.txt", "b.txt"]);
    assert!(staged(repo, "a.txt").contains("first edited"));
}

// --- the `●`-only filter and its notices ------------------------------------

#[test]
fn a_sweep_keeps_a_staged_stale_hunk_and_names_it_with_both_resolutions() {
    let dir = staged_repo();
    let repo = dir.path();
    // b.txt is staged and then edited again: `◑`, the one hunk 'docs'
    // owns, so the sweep has nothing left to take.
    write(repo, "b.txt", "three\n");

    let (echo, notices) = unstage(repo, &["docs"]);

    assert!(
        echo.contains("nothing to unstage"),
        "a ●-less scope is satisfied, not refused: {echo}"
    );
    let notice = notices
        .lines()
        .find(|line| line.contains("kept staged-stale hunk"))
        .unwrap_or_else(|| panic!("{notices}"));
    assert!(notice.starts_with("gitchange: notice:"), "{notice}");
    let address = address_in(notice, "b.txt:");
    assert!(
        notice.contains(&format!("gitchange unstage docs {address}")),
        "the addressed resolution: {notice}"
    );
    assert!(
        notice.contains(&format!("gitchange add docs {address}")),
        "the add-then-sweep resolution: {notice}"
    );
    assert!(
        notice.contains("sweep again"),
        "and its second step, which is what makes it a resolution: {notice}"
    );
    assert_eq!(
        staged(repo, "b.txt"),
        "two",
        "the staged version is still in the index — a sweep never discards one"
    );
}

#[test]
fn the_address_a_notice_names_is_the_one_the_read_surface_prints() {
    // The notice is only useful if it is pasteable, and what makes it
    // pasteable is that `diff` mints the same address for the same hunk
    // (#155) — asserted across the two commands rather than assumed.
    let dir = staged_repo();
    let repo = dir.path();
    write(repo, "b.txt", "three\n");

    let (_, notices) = unstage(repo, &["docs"]);

    let address = address_in(&notices, "b.txt:");
    let diff = gitchange(repo, &["diff", "docs"]);
    let diff = String::from_utf8(diff.stdout).unwrap();
    assert!(diff.contains(&address), "{address} not in:\n{diff}");
}

#[test]
fn a_sweep_takes_the_staged_hunks_beside_a_kept_one() {
    // The mixed scope: unassigned owns a staged hunk in a.txt and a `◑`
    // one in sub/c.txt. One moves, the other is kept and named.
    let dir = staged_repo();
    let repo = dir.path();
    write(repo, "sub/c.txt", "three\n");

    let (echo, notices) = unstage(repo, &["unassigned"]);

    assert!(echo.contains("unstaged 1 hunk"), "{echo}");
    assert!(!staged(repo, "a.txt").contains("last edited"));
    assert_eq!(staged(repo, "sub/c.txt"), "two");
    assert_eq!(
        notices
            .lines()
            .filter(|line| line.contains("kept staged-stale hunk"))
            .count(),
        1,
        "one notice, for the one hunk kept: {notices}"
    );
    assert!(
        notices.contains("gitchange unstage unassigned sub/c.txt:"),
        "the resolutions name the scope that swept: {notices}"
    );
}

#[test]
fn repeating_a_sweep_over_a_kept_hunk_is_satisfied_and_repeats_the_notice() {
    // Necessarily satisfied (#145): were a `●`-less scope a refusal, the
    // second run of the same command would flip outcome on residue the
    // first run deliberately left behind.
    let dir = staged_repo();
    let repo = dir.path();
    write(repo, "sub/c.txt", "three\n");
    unstage(repo, &["unassigned"]);

    let (echo, notices) = unstage(repo, &["unassigned"]);

    assert!(echo.contains("nothing to unstage"), "{echo}");
    assert!(notices.contains("kept staged-stale hunk"), "{notices}");
}

#[test]
fn repeating_an_unstage_over_an_unstaged_scope_is_satisfied() {
    let dir = staged_repo();
    let repo = dir.path();
    unstage(repo, &["feature"]);

    let (echo, notices) = unstage(repo, &["feature"]);

    assert!(echo.contains("nothing to unstage"), "{echo}");
    assert_eq!(notices, "", "nothing was kept, so nothing is named");
}

// --- the refusals -----------------------------------------------------------

#[test]
fn all_is_a_view_not_a_scope() {
    let dir = staged_repo();

    let refusal = refusal(dir.path(), &["all"]);

    assert!(
        refusal.contains("'all' is a view, not a scope for 'unstage'"),
        "{refusal}"
    );
    assert_eq!(
        staged_paths(dir.path()),
        vec!["a.txt", "b.txt", "sub/c.txt"],
        "a refused command leaves the index as it was"
    );
}

#[test]
fn a_git_habit_is_taught_the_changelist_first_grammar_in_this_verbs_words() {
    let dir = staged_repo();

    let refusal = refusal(dir.path(), &["a.txt"]);

    assert!(refusal.contains("no changelist named 'a.txt'"), "{refusal}");
    assert!(
        refusal.contains("'unstage' names the changelist first"),
        "the note teaches the verb the caller typed: {refusal}"
    );
}

#[test]
fn a_changelist_owning_no_hunks_has_nothing_to_unstage() {
    let dir = staged_repo();

    let refusal = refusal(dir.path(), &["empty"]);

    assert!(
        refusal.contains("'empty' owns no hunks — nothing to unstage"),
        "{refusal}"
    );
}

#[test]
fn one_offender_among_valid_arguments_refuses_the_whole_command() {
    let dir = staged_repo();
    let repo = dir.path();
    let before = git(repo, &["diff", "--cached"]);

    // Three offender classes beside a path that would have swept fine:
    // every offender is named, and nothing moved.
    let refusal = refusal(repo, &["feature", "a.txt", "keep.txt", "gone.txt", "sub"]);

    assert!(refusal.contains("'keep.txt' has no changes"), "{refusal}");
    assert!(refusal.contains("no such path 'gone.txt'"), "{refusal}");
    assert!(refusal.contains("'sub' is a directory"), "{refusal}");
    assert_eq!(
        git(repo, &["diff", "--cached"]),
        before,
        "a refused command leaves the index byte-identical"
    );
}

#[test]
fn a_path_whose_hunks_are_owned_elsewhere_names_the_actual_owners() {
    let dir = staged_repo();

    let refusal = refusal(dir.path(), &["docs", "a.txt"]);

    assert!(
        refusal.contains("no hunks of 'a.txt' belong to 'docs'"),
        "{refusal}"
    );
    assert!(refusal.contains("'feature', unassigned"), "{refusal}");
}

// --- the addressed forms (#162) ---------------------------------------------
// The classes `add.rs` pins test-by-test — the ID's own refusals, the
// candidate lists, universe-first resolution — reach both verbs through the
// same `staging::resolve`. What is here is what only this direction can
// show: the ungated `◑`, and the guard speaking this verb's words.

#[test]
fn an_explicit_id_unstages_exactly_the_hunk_it_names() {
    let dir = staged_repo();
    let repo = dir.path();
    let address = address(repo, "a.txt", 0);

    let (echo, _) = unstage(repo, &["feature", &address]);

    assert!(
        echo.contains(&format!("unstaged 1 hunk — {address}")),
        "{echo}"
    );
    let staged = staged(repo, "a.txt");
    assert!(!staged.contains("first edited"), "{staged}");
    assert!(staged.contains("last edited"), "{staged}");
}

#[test]
fn an_addressed_staged_stale_hunk_unstages_ungated() {
    // The gate is deliberately absent (#145): `space` and `add` make this
    // identical index write unguarded, so guarding one direction of it
    // would teach a false asymmetry. The staged version is discarded.
    let dir = staged_repo();
    let repo = dir.path();
    write(repo, "b.txt", "three\n");
    let address = address(repo, "b.txt", 0);

    let (echo, notices) = unstage(repo, &["docs", &address]);

    assert!(echo.contains("unstaged 1 hunk"), "{echo}");
    assert_eq!(
        staged_paths(repo),
        vec!["a.txt", "sub/c.txt"],
        "b.txt's staged version left the index"
    );
    assert!(
        !notices.contains("kept staged-stale hunk"),
        "the hunk the notice would name is the one that moved: {notices}"
    );
}

#[test]
fn an_addressed_index_only_hunk_discards_content_no_other_copy_holds() {
    // sub/c.txt staged and then reverted in the worktree: the hunk lives in
    // the index alone, so unstaging it by address is an irrecoverable
    // discard — ungated by the same rule, with the skill's staleness
    // family as the mitigation (#145).
    let dir = staged_repo();
    let repo = dir.path();
    write(repo, "sub/c.txt", "one\n");
    let address = address(repo, "sub/c.txt", 0);

    let (echo, _) = unstage(repo, &["unassigned", &address]);

    assert!(echo.contains("unstaged 1 hunk"), "{echo}");
    assert_eq!(
        staged_paths(repo),
        vec!["a.txt", "b.txt"],
        "the index-only content is gone"
    );
}

#[test]
fn a_hunk_the_address_moved_is_not_also_named_as_kept() {
    // The mixed command: the file row and one of its `◑` hunks, addressed.
    // The address unstaged it, so the notice — which exists to say what
    // stayed — must not name it.
    let dir = staged_repo();
    let repo = dir.path();
    write(repo, "b.txt", "three\n");
    let address = address(repo, "b.txt", 0);

    let (echo, notices) = unstage(repo, &["docs", "b.txt", &address]);

    assert!(echo.contains("unstaged 1 hunk"), "{echo}");
    assert!(
        !notices.contains("kept staged-stale hunk"),
        "nothing stayed: {notices}"
    );
    assert_eq!(staged_paths(repo), vec!["a.txt", "sub/c.txt"]);
}

#[test]
fn containing_unstages_the_hunk_holding_the_line() {
    let dir = staged_repo();
    let repo = dir.path();

    let (echo, _) = unstage(
        repo,
        &["unassigned", "a.txt", "--containing", "last edited"],
    );

    assert!(echo.contains("unstaged 1 hunk"), "{echo}");
    let staged = staged(repo, "a.txt");
    assert!(staged.contains("first edited"), "{staged}");
    assert!(!staged.contains("last edited"), "{staged}");
}

#[test]
fn the_consistency_guard_teaches_this_verbs_retry() {
    let dir = staged_repo();
    let repo = dir.path();
    let address = address(repo, "a.txt", 1);

    let refusal = refusal(repo, &["feature", &address]);

    assert!(
        refusal.contains("belongs to unassigned, not 'feature'"),
        "{refusal}"
    );
    assert!(
        refusal.contains(&format!("gitchange unstage unassigned {address}")),
        "the retry names the verb the caller typed: {refusal}"
    );
}
