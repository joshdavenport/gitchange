//! End-to-end tests of `assign` (#147): what a path sweep takes, what an
//! address narrows to, the ownership guard and its named override, the
//! unit-subset refusal that keeps an address from widening, the
//! all-or-nothing refusals, and the receipt. Membership is asserted through
//! gitchange's own read — `diff --json`'s per-hunk owner — because membership
//! is a gitchange fact with no git command to check it against, unlike the
//! index the staging suites hold `git diff --cached` up to.
//!
//! The main fixture is `support::owned_repo`, capture off so ownership is
//! exactly what the records say: one file split between a changelist and
//! unassigned, one file another changelist owns, an unassigned file, a clean
//! path, and a changelist owning nothing. The index-entry-unit cases need a
//! file whose entry holds two hunks at once, so they build their own below.
//!
//! The fail-soft split (some hunks stale, all hunks stale) is not here: it
//! needs a worktree edit between the refresh and the apply, which no child
//! process can inject, so it lands at core's integration seam (ADR 0008,
//! core's `assign` suite).

use std::path::Path;

use crate::support::{
    address, git, gitchange, initialised_repo, merging_repo, owned_repo, seed_state,
    seed_state_raw, write,
};

/// Each hunk's owner for `path`, in file order — read back through
/// `diff --json`, the surface a caller has for the same question.
fn owners(dir: &Path, path: &str) -> Vec<Option<String>> {
    let output = gitchange(dir, &["diff", "--json", "--no-content", path]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "diff --json {path}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("the envelope is one JSON document");
    envelope["files"]
        .as_array()
        .expect("files")
        .iter()
        .find(|file| file["path"] == serde_json::json!(path))
        .unwrap_or_else(|| panic!("no '{path}' in {envelope}"))["hunks"]
        .as_array()
        .expect("hunks")
        .iter()
        .map(|hunk| hunk["changelist"].as_str().map(str::to_owned))
        .collect()
}

/// `gitchange assign <args>`, asserted to have succeeded, its stdout echo.
fn assign(dir: &Path, args: &[&str]) -> String {
    let output = gitchange(dir, &[&["assign"], args].concat());
    assert_eq!(
        output.status.code(),
        Some(0),
        "gitchange assign {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// The refusal `gitchange assign <args>` produces: exit 1, empty stdout, and
/// the stderr text for the caller to read the fragments it cares about.
fn refusal(dir: &Path, args: &[&str]) -> String {
    let output = gitchange(dir, &[&["assign"], args].concat());
    assert_eq!(
        output.status.code(),
        Some(1),
        "gitchange assign {args:?} should refuse"
    );
    assert_eq!(
        output.stdout,
        Vec::<u8>::new(),
        "a failed command leaves stdout empty (#122)"
    );
    String::from_utf8(output.stderr).unwrap()
}

fn changelist(name: &str) -> Option<String> {
    Some(name.to_owned())
}

// --- the sweep --------------------------------------------------------------

#[test]
fn a_whole_path_sweep_takes_every_hunk_of_the_path_staged_and_unstaged() {
    // Membership and staging are separate axes (ADR 0003), so the staged
    // file's hunk moves exactly like the unstaged one — and a.txt's hunk
    // that 'feature' already holds is satisfied rather than counted again.
    let dir = owned_repo();
    let repo = dir.path();
    git(repo, &["add", "sub/c.txt"]);

    let echo = assign(repo, &["sub/c.txt", "a.txt", "--to", "feature"]);

    assert!(echo.contains("assigned 2 hunks"), "{echo}");
    assert!(
        echo.contains("'feature'"),
        "the echo names the target: {echo}"
    );
    assert_eq!(
        owners(repo, "a.txt"),
        vec![changelist("feature"), changelist("feature")]
    );
    assert_eq!(owners(repo, "sub/c.txt"), vec![changelist("feature")]);
}

#[test]
fn unassign_and_to_unassigned_release_identically() {
    for target in [vec!["--unassign"], vec!["--to", "unassigned"]] {
        let dir = owned_repo();
        let repo = dir.path();

        let echo = assign(
            repo,
            &[&["b.txt", "--take-owned"], target.as_slice()].concat(),
        );

        assert!(echo.contains("released 1 hunk"), "{target:?}: {echo}");
        assert!(
            echo.contains("unassigned"),
            "the echo names the target: {echo}"
        );
        assert_eq!(owners(repo, "b.txt"), vec![None], "{target:?}");
    }
}

#[test]
fn a_released_hunk_is_claimed_by_the_next_refresh_with_a_changelist_active() {
    // ADR 0016: a release is recordless, not a parking spot. With capture
    // on, the *next* persisting refresh claims the hunk back — here the one
    // the following mutation runs — and says so.
    let dir = initialised_repo();
    let repo = dir.path();
    write(repo, "a.txt", "one\n");
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-q", "--no-verify", "-m", "init"]);
    seed_state(repo, "feature", &["feature"]);
    write(repo, "a.txt", "two\n");
    // This invocation's own refresh captures the hunk into 'feature'; the
    // release then deletes the record it just wrote.
    assign(repo, &["a.txt", "--unassign", "--take-owned"]);
    assert_eq!(owners(repo, "a.txt"), vec![None], "released, for now");

    let recaptured = gitchange(repo, &["add", "feature"]);

    assert_eq!(recaptured.status.code(), Some(0));
    assert!(
        String::from_utf8(recaptured.stderr)
            .unwrap()
            .contains("gitchange: notice: auto-captured hunk at a.txt"),
        "the claim is loud, never silent"
    );
    assert_eq!(owners(repo, "a.txt"), vec![changelist("feature")]);
}

#[test]
fn a_released_hunk_stays_unassigned_with_capture_off() {
    // The supported way to keep hunks loose: unassigned active. The fixture
    // is capture-off already, so nothing re-claims the released hunk — not
    // even the next mutation's refresh.
    let dir = owned_repo();
    let repo = dir.path();
    assign(repo, &["b.txt", "--take-owned", "--unassign"]);

    let staged = gitchange(repo, &["add", "unassigned", "b.txt"]);

    assert_eq!(staged.status.code(), Some(0));
    assert_eq!(owners(repo, "b.txt"), vec![None]);
}

// --- ownership --------------------------------------------------------------

#[test]
fn a_path_holding_another_changelists_hunks_refuses_naming_the_owner() {
    let dir = owned_repo();
    let repo = dir.path();

    let refusal = refusal(repo, &["a.txt", "--to", "docs"]);

    assert!(
        refusal.contains("'a.txt' holds hunks owned by 'feature'"),
        "{refusal}"
    );
    assert!(refusal.contains("--take-owned"), "{refusal}");
    assert_eq!(
        owners(repo, "a.txt"),
        vec![changelist("feature"), None],
        "a refused command assigned nothing"
    );
}

#[test]
fn unassigned_hunks_are_not_owned_and_pass_unguarded() {
    let dir = owned_repo();
    let repo = dir.path();

    let echo = assign(repo, &["sub/c.txt", "--to", "docs"]);

    assert!(echo.contains("assigned 1 hunk"), "{echo}");
    assert_eq!(owners(repo, "sub/c.txt"), vec![changelist("docs")]);
}

#[test]
fn take_owned_takes_the_other_changelists_hunks_too() {
    let dir = owned_repo();
    let repo = dir.path();

    let echo = assign(repo, &["a.txt", "--to", "docs", "--take-owned"]);

    assert!(echo.contains("assigned 2 hunks"), "{echo}");
    assert_eq!(
        owners(repo, "a.txt"),
        vec![changelist("docs"), changelist("docs")]
    );
}

// --- the refusals -----------------------------------------------------------

#[test]
fn an_unrecognised_target_lists_the_candidates_and_creates_nothing() {
    let dir = owned_repo();
    let repo = dir.path();

    let refusal = refusal(repo, &["sub/c.txt", "--to", "ghost"]);

    assert!(refusal.contains("no changelist named 'ghost'"), "{refusal}");
    assert!(
        refusal.contains("unassigned, 'feature', 'docs', 'empty'"),
        "{refusal}"
    );
    let status = gitchange(repo, &["status"]);
    assert!(
        !String::from_utf8(status.stdout).unwrap().contains("ghost"),
        "assign never creates a changelist"
    );
}

#[test]
fn a_clean_path_refuses_rather_than_assigning_nothing() {
    let dir = owned_repo();

    let refusal = refusal(dir.path(), &["keep.txt", "--to", "feature"]);

    assert!(refusal.contains("'keep.txt' has no changes"), "{refusal}");
}

#[test]
fn a_nonexistent_path_refuses() {
    let dir = owned_repo();

    let refusal = refusal(dir.path(), &["gone.txt", "--to", "feature"]);

    assert!(refusal.contains("no such path 'gone.txt'"), "{refusal}");
}

#[test]
fn a_directory_names_the_changed_files_under_it() {
    let dir = owned_repo();

    let refusal = refusal(dir.path(), &["sub", "--to", "feature"]);

    assert!(refusal.contains("'sub' is a directory"), "{refusal}");
    assert!(refusal.contains("sub/c.txt"), "{refusal}");
}

#[test]
fn a_repo_escaping_path_refuses() {
    let dir = owned_repo();

    let refusal = refusal(dir.path(), &["../outside.txt", "--to", "feature"]);

    assert!(refusal.contains("is outside the repository"), "{refusal}");
}

#[test]
fn a_conflicted_path_is_quarantined_rather_than_swept() {
    let dir = merging_repo();

    let refusal = refusal(dir.path(), &["tracked.txt", "--unassign"]);

    assert!(refusal.contains("tracked.txt is conflicted"), "{refusal}");
}

#[test]
fn one_offender_among_valid_arguments_refuses_the_whole_command() {
    let dir = owned_repo();
    let repo = dir.path();
    let before = membership(repo);

    // Three offender classes beside a path that would have swept fine.
    let refusal = refusal(
        repo,
        &["sub/c.txt", "keep.txt", "gone.txt", "a.txt", "--to", "docs"],
    );

    assert!(refusal.contains("'keep.txt' has no changes"), "{refusal}");
    assert!(refusal.contains("no such path 'gone.txt'"), "{refusal}");
    assert!(
        refusal.contains("'a.txt' holds hunks owned by 'feature'"),
        "{refusal}"
    );
    assert_eq!(
        membership(repo),
        before,
        "a refused command assigned nothing — not even the arguments that were valid"
    );
}

/// Every changed file's per-hunk ownership: what "a refused command assigned
/// nothing" is asserted against.
///
/// Membership rather than the state file's bytes, because those move for a
/// reason of their own: the invocation's persisting refresh restamps the file
/// on its way to the refusal (ADR 0005), which is a decision about baselines,
/// not about who owns what.
fn membership(dir: &Path) -> Vec<(String, Vec<Option<String>>)> {
    ["a.txt", "b.txt", "sub/c.txt"]
        .iter()
        .map(|path| ((*path).to_owned(), owners(dir, path)))
        .collect()
}

#[test]
fn an_unrecognised_target_still_reports_the_path_offenders_beside_it() {
    let dir = owned_repo();

    let refusal = refusal(dir.path(), &["gone.txt", "--to", "ghost"]);

    assert!(refusal.contains("no changelist named 'ghost'"), "{refusal}");
    assert!(refusal.contains("no such path 'gone.txt'"), "{refusal}");
}

// --- idempotency ------------------------------------------------------------

#[test]
fn repeating_an_assign_over_target_owned_hunks_is_satisfied() {
    let dir = owned_repo();
    let repo = dir.path();
    assign(repo, &["sub/c.txt", "--to", "docs"]);

    let echo = assign(repo, &["sub/c.txt", "--to", "docs"]);

    assert!(echo.contains("nothing to assign"), "{echo}");
    assert_eq!(owners(repo, "sub/c.txt"), vec![changelist("docs")]);
}

#[test]
fn repeating_a_release_over_unassigned_hunks_is_satisfied() {
    let dir = owned_repo();
    let repo = dir.path();

    let echo = assign(repo, &["sub/c.txt", "--unassign"]);

    assert!(echo.contains("nothing to release"), "{echo}");
    assert_eq!(owners(repo, "sub/c.txt"), vec![None]);
}

#[test]
fn one_path_named_twice_sweeps_once() {
    let dir = owned_repo();
    let repo = dir.path();

    let echo = assign(repo, &["sub/c.txt", "sub/c.txt", "--to", "docs"]);

    assert!(echo.contains("assigned 1 hunk"), "{echo}");
}

// --- deferred capture, two-sided --------------------------------------------

/// A repo with `a.txt` edited under an active changelist and no records: the
/// hunk this invocation's own refresh will capture on its way to validation.
fn capturing_repo() -> tempfile::TempDir {
    let dir = initialised_repo();
    let repo = dir.path();
    write(repo, "a.txt", "one\n");
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-q", "--no-verify", "-m", "init"]);
    seed_state(repo, "active", &["active", "other"]);
    write(repo, "a.txt", "two\n");
    dir
}

#[test]
fn assigning_a_just_captured_hunk_to_the_active_changelist_is_satisfied() {
    let dir = capturing_repo();
    let repo = dir.path();

    let output = gitchange(repo, &["assign", "a.txt", "--to", "active"]);

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("nothing to assign"),
        "this command's own refresh already placed it there"
    );
    assert!(
        stderr.contains("gitchange: notice: auto-captured hunk at a.txt"),
        "the capture rides this receipt, once: {stderr}"
    );
    assert_eq!(owners(repo, "a.txt"), vec![changelist("active")]);
}

#[test]
fn assigning_a_just_captured_hunk_elsewhere_trips_the_ownership_guard() {
    // The other side of the same fact: the capture happened before
    // validation, so the active changelist is the owner the guard names —
    // and the caller learns both in one round trip.
    let dir = capturing_repo();
    let repo = dir.path();

    let output = gitchange(repo, &["assign", "a.txt", "--to", "other"]);

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(output.stdout.is_empty());
    assert!(
        stderr.contains("gitchange: notice: auto-captured hunk at a.txt"),
        "{stderr}"
    );
    assert!(
        stderr.contains("'a.txt' holds hunks owned by 'active'"),
        "{stderr}"
    );
    assert_eq!(owners(repo, "a.txt"), vec![changelist("active")]);
}

// --- the addressed forms (#164) ---------------------------------------------
// The three non-declarative exit-`2` checks are in `grammar.rs`, where the
// empty directory proves they answer before any repository is opened.

/// The `<hunk-id>` half of a composed address — what a caller retypes under
/// another path when the consistency guard is what is being tested.
fn hunk_id(address: &str) -> &str {
    address
        .rsplit(':')
        .next()
        .expect("an address is path-rooted")
}

#[test]
fn an_explicit_id_assigns_exactly_the_hunk_it_names() {
    // a.txt's second hunk alone, while 'feature' still holds the first: an
    // address narrows the ownership guard to the hunk it names, so a foreign
    // hunk elsewhere in the file is irrelevant to an exact address (#147).
    let dir = owned_repo();
    let repo = dir.path();
    let last = address(repo, "a.txt", 1);

    let echo = assign(repo, &[&last, "--to", "docs"]);

    assert!(echo.contains("assigned 1 hunk"), "{echo}");
    assert_eq!(
        owners(repo, "a.txt"),
        vec![changelist("feature"), changelist("docs")]
    );
}

#[test]
fn an_id_resolving_to_another_file_refuses() {
    // The path prefix is the consistency guard (#122): an ID is repo-unique,
    // so naming it under the wrong path is a caller mistake worth naming —
    // never a silent misassign.
    let dir = owned_repo();
    let repo = dir.path();
    let elsewhere = address(repo, "b.txt", 0);

    let refusal = refusal(
        repo,
        &[&format!("a.txt:{}", hunk_id(&elsewhere)), "--to", "docs"],
    );

    assert!(refusal.contains("is in 'b.txt', not 'a.txt'"), "{refusal}");
    assert_eq!(owners(repo, "b.txt"), vec![changelist("docs")]);
}

#[test]
fn an_addressed_foreign_hunk_refuses_naming_the_owner() {
    let dir = owned_repo();
    let repo = dir.path();
    let first = address(repo, "a.txt", 0);

    let refusal = refusal(repo, &[&first, "--to", "docs"]);

    assert!(
        refusal.contains(&format!("hunk '{first}' is owned by 'feature'")),
        "{refusal}"
    );
    assert!(refusal.contains("--take-owned"), "{refusal}");
    assert_eq!(owners(repo, "a.txt"), vec![changelist("feature"), None]);
}

#[test]
fn take_owned_composes_with_an_address() {
    let dir = owned_repo();
    let repo = dir.path();
    let first = address(repo, "a.txt", 0);

    let echo = assign(repo, &[&first, "--to", "docs", "--take-owned"]);

    assert!(echo.contains("assigned 1 hunk"), "{echo}");
    assert_eq!(
        owners(repo, "a.txt"),
        vec![changelist("docs"), None],
        "the addressed hunk moved, and only it"
    );
}

#[test]
fn a_stale_explicit_id_refuses_at_validation() {
    // Mint an address, move the hunk on, replay it: an explicit ID refuses
    // rather than resolving to whatever now sits at that position (#122), so
    // a stale address never acts.
    let dir = owned_repo();
    let repo = dir.path();
    let one_hunk = address(repo, "sub/c.txt", 0);
    write(repo, "sub/c.txt", "three\n");

    let refusal = refusal(repo, &[&one_hunk, "--to", "docs"]);

    assert!(refusal.contains("addresses one snapshot"), "{refusal}");
    assert_eq!(owners(repo, "sub/c.txt"), vec![None]);
}

#[test]
fn a_misparse_dies_as_not_found() {
    // A suffix that is not ID-shaped leaves the whole token a path (#122),
    // which then lives or dies as one — never as a silent misassign.
    let dir = owned_repo();

    let refusal = refusal(dir.path(), &["a.txt:notanid", "--to", "docs"]);

    assert!(
        refusal.contains("no such path 'a.txt:notanid'"),
        "{refusal}"
    );
}

#[test]
fn an_addressed_offender_joins_the_all_or_nothing_set() {
    let dir = owned_repo();
    let repo = dir.path();
    let before = membership(repo);

    let refusal = refusal(repo, &["a.txt:h1a2b3c4", "keep.txt", "--to", "docs"]);

    assert!(
        refusal.contains("no hunk 'h1a2b3c4' in 'a.txt'"),
        "{refusal}"
    );
    assert!(refusal.contains("'keep.txt' has no changes"), "{refusal}");
    assert_eq!(
        membership(repo),
        before,
        "a refused command assigned nothing"
    );
}

#[test]
fn an_address_mixes_freely_with_a_whole_path() {
    let dir = owned_repo();
    let repo = dir.path();
    let last = address(repo, "a.txt", 1);

    let echo = assign(repo, &["sub/c.txt", &last, "--to", "docs"]);

    assert!(echo.contains("assigned 2 hunks"), "{echo}");
    assert_eq!(
        owners(repo, "a.txt"),
        vec![changelist("feature"), changelist("docs")],
        "the whole-path argument beside it never widened the address"
    );
    assert_eq!(owners(repo, "sub/c.txt"), vec![changelist("docs")]);
}

#[test]
fn one_path_may_repeat_with_different_ids() {
    let dir = owned_repo();
    let repo = dir.path();
    let first = address(repo, "a.txt", 0);
    let last = address(repo, "a.txt", 1);

    let echo = assign(repo, &[&first, &last, "--to", "docs", "--take-owned"]);

    assert!(echo.contains("assigned 2 hunks"), "{echo}");
    assert_eq!(
        owners(repo, "a.txt"),
        vec![changelist("docs"), changelist("docs")]
    );
}

#[test]
fn one_hunk_named_twice_in_one_command_moves_once() {
    let dir = owned_repo();
    let repo = dir.path();
    let last = address(repo, "a.txt", 1);

    let echo = assign(repo, &[&last, &last, "--to", "docs"]);

    assert!(echo.contains("assigned 1 hunk"), "{echo}");
}

#[test]
fn a_swept_path_subsumes_an_address_into_it() {
    // Both spellings of the same file in one command line: the sweep already
    // takes the addressed hunk, so it moves once and is counted once.
    let dir = owned_repo();
    let repo = dir.path();
    let last = address(repo, "a.txt", 1);

    let echo = assign(repo, &["a.txt", &last, "--to", "docs", "--take-owned"]);

    assert!(echo.contains("assigned 2 hunks"), "{echo}");
    assert_eq!(
        owners(repo, "a.txt"),
        vec![changelist("docs"), changelist("docs")]
    );
}

#[test]
fn an_addressed_hunk_the_target_already_owns_is_satisfied() {
    let dir = owned_repo();
    let repo = dir.path();
    let first = address(repo, "a.txt", 0);

    let echo = assign(repo, &[&first, "--to", "feature"]);

    assert!(echo.contains("nothing to assign"), "{echo}");
    assert_eq!(owners(repo, "a.txt"), vec![changelist("feature"), None]);
}

// --- --containing -----------------------------------------------------------

#[test]
fn containing_assigns_the_hunk_holding_the_line() {
    let dir = owned_repo();
    let repo = dir.path();

    let echo = assign(
        repo,
        &["a.txt", "--containing", "last edited", "--to", "docs"],
    );

    assert!(echo.contains("assigned 1 hunk"), "{echo}");
    assert_eq!(
        owners(repo, "a.txt"),
        vec![changelist("feature"), changelist("docs")]
    );
}

#[test]
fn containing_refuses_zero_and_several_matches_with_candidate_ids() {
    let dir = owned_repo();
    let repo = dir.path();
    let candidates = [address(repo, "a.txt", 0), address(repo, "a.txt", 1)];
    let before = membership(repo);

    // Zero: `line 2` is context in the first hunk, and matching is over
    // changed lines only — nothing the caller did not write can match.
    let zero = refusal(repo, &["a.txt", "--containing", "line 2", "--to", "docs"]);
    // Several: `edited` is in both hunks, so the value has to be narrowed
    // rather than broadened.
    let several = refusal(repo, &["a.txt", "--containing", "edited", "--to", "docs"]);

    assert!(
        zero.contains("no changed line of 'a.txt' contains 'line 2'"),
        "{zero}"
    );
    assert!(several.contains("is in 2 hunks of 'a.txt'"), "{several}");
    for candidate in &candidates {
        assert!(zero.contains(candidate), "{candidate} not in: {zero}");
        assert!(several.contains(candidate), "{candidate} not in: {several}");
    }
    assert_eq!(
        membership(repo),
        before,
        "a refused command assigned nothing"
    );
}

#[test]
fn a_candidate_from_a_refusal_is_the_retry_verbatim() {
    // What a candidate list is for (#147): the addresses it prints are the
    // argument the corrected command takes, pasted with nothing edited.
    let dir = owned_repo();
    let repo = dir.path();
    let several = refusal(repo, &["a.txt", "--containing", "edited", "--to", "docs"]);
    let candidate = several
        .split_whitespace()
        .find(|word| word.starts_with("a.txt:h"))
        .expect("the refusal lists candidates")
        .trim_end_matches(',')
        .to_owned();

    let echo = assign(repo, &[&candidate, "--to", "docs", "--take-owned"]);

    assert!(echo.contains("assigned 1 hunk"), "{echo}");
}

#[test]
fn containing_resolves_over_the_universe_before_the_ownership_guard() {
    // The value matches a hunk 'docs' does not own: the answer is the
    // ownership guard naming who does, not "no match". Scope-filtered
    // matching would make one value quietly mean different hunks under
    // different verbs (#122).
    let dir = owned_repo();
    let repo = dir.path();
    let first = address(repo, "a.txt", 0);

    let refusal = refusal(
        repo,
        &["a.txt", "--containing", "first edited", "--to", "docs"],
    );

    assert!(
        refusal.contains(&format!("hunk '{first}' is owned by 'feature'")),
        "{refusal}"
    );
    assert_eq!(owners(repo, "a.txt"), vec![changelist("feature"), None]);
}

// --- degenerate hunks -------------------------------------------------------

/// A file whose index entry holds two hunks at once — ADR 0009's entry unit:
/// a text edit staged, then the worktree copy replaced with binary content,
/// so the universe presents a whole-file hunk beside the index-only text hunk
/// it shares an index entry with. Both unassigned.
fn entry_unit_repo() -> tempfile::TempDir {
    let dir = initialised_repo();
    let repo = dir.path();
    write(repo, "f.txt", "one\ntwo\nthree\n");
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-q", "--no-verify", "-m", "init"]);
    // Capture off (`active: null`), as `owned_repo` is: the hunks below stay
    // unassigned, so what the sweep moves is the sweep's own doing.
    seed_state_raw(
        repo,
        r#"{ "version": 1, "active": null, "changelists": [{ "name": "feature" }] }"#,
    );
    write(repo, "f.txt", "one\nEDIT\nthree\n");
    git(repo, &["add", "f.txt"]);
    std::fs::write(repo.join("f.txt"), b"bin\x00\x01\x02data\n").unwrap();
    dir
}

#[test]
fn a_sweep_over_an_entry_unit_assigns_it_whole() {
    // Degenerate hunks are hunks (#147): the whole-file hunk of a binary
    // moves like any other, and a sweep names everything, so the ADR 0009
    // widening it would trip is a no-op here — the unit-subset refusal is
    // the addressed forms' alone, never a sweep's.
    let dir = entry_unit_repo();
    let repo = dir.path();

    let echo = assign(repo, &["f.txt", "--to", "feature"]);

    assert!(echo.contains("assigned 2 hunks"), "{echo}");
    assert_eq!(
        owners(repo, "f.txt"),
        vec![changelist("feature"), changelist("feature")]
    );
}

#[test]
fn a_degenerate_hunk_is_a_candidate_but_never_a_match() {
    // f.txt's whole-file hunk is the binary one: no changed lines, so a
    // `--containing` value can only ever zero-match it — and its ID is in the
    // candidate list all the same, which is where an agent learns it exists
    // (#147).
    let dir = entry_unit_repo();
    let repo = dir.path();

    let refusal = refusal(
        repo,
        &["f.txt", "--containing", "absent", "--to", "feature"],
    );

    assert!(
        refusal.contains("no changed line of 'f.txt' contains 'absent'"),
        "{refusal}"
    );
    assert!(
        refusal.contains(&address(repo, "f.txt", 0)),
        "the degenerate hunk is a candidate: {refusal}"
    );
}

// --- the unit-subset refusal ------------------------------------------------

/// `entry_unit_repo` with the index-only text hunk claimed by `'feature'`,
/// so the unit is split between a changelist and unassigned — the shape whose
/// full-membership retry has to meet the ownership guard.
fn split_entry_unit_repo() -> tempfile::TempDir {
    let dir = initialised_repo();
    let repo = dir.path();
    write(repo, "f.txt", "one\ntwo\nthree\n");
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-q", "--no-verify", "-m", "init"]);
    seed_state_raw(
        repo,
        r#"{
  "version": 1, "active": null,
  "changelists": [{ "name": "feature" }, { "name": "docs" }],
  "records": [
    {
      "path": "f.txt", "old_start": 1, "old_lines": 3,
      "new_start": 1, "new_lines": 3, "changelist": "feature",
      "anchor": ["-two\n", "+EDIT\n"], "dormant_since": null
    }
  ]
}"#,
    );
    write(repo, "f.txt", "one\nEDIT\nthree\n");
    git(repo, &["add", "f.txt"]);
    std::fs::write(repo.join("f.txt"), b"bin\x00\x01\x02data\n").unwrap();
    dir
}

/// A file whose whole change is one whole-file hunk: a committed text file
/// replaced with binary content, nothing staged. Its index-entry unit has a
/// single member, which is the shape that must never trip the refusal.
fn lone_member_repo() -> tempfile::TempDir {
    let dir = initialised_repo();
    let repo = dir.path();
    write(repo, "f.txt", "one\ntwo\nthree\n");
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-q", "--no-verify", "-m", "init"]);
    seed_state_raw(
        repo,
        r#"{ "version": 1, "active": null, "changelists": [{ "name": "feature" }] }"#,
    );
    std::fs::write(repo.join("f.txt"), b"bin\x00\x01\x02data\n").unwrap();
    dir
}

#[test]
fn addressing_one_member_of_a_unit_refuses_listing_every_member() {
    // Core widens every assign payload to the whole index-entry unit
    // (ADR 0009), so on the CLI an address naming one member would be a
    // silent broaden. It refuses instead, and the list is the retry (#147).
    let dir = entry_unit_repo();
    let repo = dir.path();
    let first = address(repo, "f.txt", 0);
    let second = address(repo, "f.txt", 1);

    let refusal = refusal(repo, &[&second, "--to", "feature"]);

    assert!(refusal.contains(&first), "every member is named: {refusal}");
    assert!(refusal.contains(&second), "{refusal}");
    assert!(
        refusal.contains("sweep the path: 'f.txt'"),
        "both resolutions are named: {refusal}"
    );
    assert_eq!(
        owners(repo, "f.txt"),
        vec![None, None],
        "a refused command assigned nothing"
    );
}

#[test]
fn the_full_membership_retry_assigns_the_unit() {
    // The retry is an ordinary multi-ID assign under the standing rules — no
    // new flag, no unit-scope variant (#147).
    let dir = entry_unit_repo();
    let repo = dir.path();
    let first = address(repo, "f.txt", 0);
    let second = address(repo, "f.txt", 1);

    let echo = assign(repo, &[&first, &second, "--to", "feature"]);

    assert!(echo.contains("assigned 2 hunks"), "{echo}");
    assert_eq!(
        owners(repo, "f.txt"),
        vec![changelist("feature"), changelist("feature")]
    );
}

#[test]
fn a_split_units_full_membership_retry_meets_the_ownership_guard() {
    // In a split unit the retry the refusal asked for runs into the members
    // owned elsewhere, and `--take-owned` is the deliberate statement the
    // guard exists to extract (#147).
    let dir = split_entry_unit_repo();
    let repo = dir.path();
    let first = address(repo, "f.txt", 0);
    let second = address(repo, "f.txt", 1);
    assert_eq!(
        owners(repo, "f.txt"),
        vec![None, changelist("feature")],
        "the fixture's unit is split"
    );

    let refusal = refusal(repo, &[&first, &second, "--to", "docs"]);
    assert!(
        refusal.contains(&format!("hunk '{second}' is owned by 'feature'")),
        "{refusal}"
    );

    let echo = assign(repo, &[&first, &second, "--to", "docs", "--take-owned"]);

    assert!(echo.contains("assigned 2 hunks"), "{echo}");
    assert_eq!(
        owners(repo, "f.txt"),
        vec![changelist("docs"), changelist("docs")]
    );
}

#[test]
fn a_release_of_part_of_a_unit_refuses_too() {
    // ADR 0009's rule is explicitly "in a release too", so the refusal is
    // the target's business only insofar as every direction has it.
    let dir = entry_unit_repo();
    let repo = dir.path();
    let member = address(repo, "f.txt", 1);
    assign(repo, &["f.txt", "--to", "feature"]);

    let refusal = refusal(repo, &[&member, "--unassign"]);

    assert!(refusal.contains("sweep the path: 'f.txt'"), "{refusal}");
    assert_eq!(
        owners(repo, "f.txt"),
        vec![changelist("feature"), changelist("feature")],
        "a refused release released nothing"
    );
}

#[test]
fn a_swept_path_satisfies_the_unit_beside_an_address_of_its_own() {
    // A sweep names everything, so an address of the same path beside it
    // cannot be naming only part of the unit.
    let dir = entry_unit_repo();
    let repo = dir.path();
    let member = address(repo, "f.txt", 1);

    let echo = assign(repo, &["f.txt", &member, "--to", "feature"]);

    assert!(echo.contains("assigned 2 hunks"), "{echo}");
    assert_eq!(
        owners(repo, "f.txt"),
        vec![changelist("feature"), changelist("feature")]
    );
}

#[test]
fn a_single_member_unit_never_trips_the_refusal() {
    // A plain binary: the unit is the one hunk addressed, so naming it is
    // naming all of it and core's widening has nothing to widen.
    let dir = lone_member_repo();
    let repo = dir.path();
    let only = address(repo, "f.txt", 0);

    let echo = assign(repo, &[&only, "--to", "feature"]);

    assert!(echo.contains("assigned 1 hunk"), "{echo}");
    assert_eq!(owners(repo, "f.txt"), vec![changelist("feature")]);
}

#[test]
fn containing_landing_on_a_unit_member_trips_the_refusal() {
    // `--containing`'s unique match is the addressed set, so it is a proper
    // subset exactly as an explicit ID would be (#147).
    let dir = entry_unit_repo();
    let repo = dir.path();
    let first = address(repo, "f.txt", 0);
    let second = address(repo, "f.txt", 1);

    let refusal = refusal(repo, &["f.txt", "--containing", "EDIT", "--to", "feature"]);

    assert!(refusal.contains(&first), "{refusal}");
    assert!(refusal.contains(&second), "{refusal}");
    assert_eq!(owners(repo, "f.txt"), vec![None, None]);
}

#[test]
fn the_unit_subset_refusal_joins_the_all_or_nothing_set() {
    let dir = entry_unit_repo();
    let repo = dir.path();
    let member = address(repo, "f.txt", 1);

    let refusal = refusal(repo, &[&member, "gone.txt", "--to", "feature"]);

    assert!(refusal.contains("no such path 'gone.txt'"), "{refusal}");
    assert!(
        refusal.contains("sweep the path: 'f.txt'"),
        "the unit offender is named beside it: {refusal}"
    );
    assert_eq!(owners(repo, "f.txt"), vec![None, None]);
}
