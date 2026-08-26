//! End-to-end tests of `commit` (#170, #171): target validation, the
//! guard stack in its fixed order, `--amend`, the three message sources,
//! and the receipt. Ground truth is git's own account of what happened —
//! `git log`, `git show`, `git diff --cached` — never gitchange's.
//!
//! The runner is this module's own rather than `support::gitchange`
//! because `commit` is the one verb that shells out to git: the binary's
//! child `git commit` must read the same cut-down config the fixtures are
//! built with, or a developer with a global `commit.gpgsign` sees these
//! fail and CI never does. It also carries stdin, which `-F -` needs.
//!
//! Two things this seam cannot answer stay at core's (ADR 0008), where
//! both are already pinned: the record aftermath — which no git command
//! reports, so the amend cases below read the last-commit record only
//! through the guard's own outcome — and the mode-hunk carve-out from
//! the foreign-content guard, where a mode hunk commits no content, so
//! asserting it needs the payload rather than the commit (core's
//! `commit::temp_index`, `a_mode_only_payload_commits_past_a_split_entry`).

use std::path::Path;
use std::process::{Command, Output, Stdio};

use crate::support::{
    absent_config, address, committed_repo, git, git_command, long_file, owned_repo, path_str,
    seed_state, seed_state_raw, staged_paths, write,
};

/// Run the binary in `dir` with the host's git config cut out and
/// `stdin` on its standard input.
fn run(dir: &Path, args: &[&str], stdin: &str) -> Output {
    let absent = absent_config(dir);
    let mut child = Command::new(env!("CARGO_BIN_EXE_gitchange"))
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", &absent)
        .env("GIT_CONFIG_SYSTEM", &absent)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run gitchange");
    std::io::Write::write_all(
        child.stdin.as_mut().expect("stdin is piped"),
        stdin.as_bytes(),
    )
    .expect("write stdin");
    child.wait_with_output().expect("collect gitchange output")
}

/// `gitchange <args>`, asserted to have succeeded, its stdout.
fn succeeds(dir: &Path, args: &[&str]) -> String {
    let output = run(dir, args, "");
    assert_eq!(
        output.status.code(),
        Some(0),
        "gitchange {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

/// The refusal `gitchange commit <args>` produces: exit 1, empty stdout —
/// the receipt contract (#122) asserted at every rung — and the stderr
/// text for the caller to read the fragments it cares about.
fn refusal(dir: &Path, args: &[&str]) -> String {
    let output = run(dir, &[&["commit"], args].concat(), "");
    assert_eq!(
        output.status.code(),
        Some(1),
        "gitchange commit {args:?} should refuse: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        output.stdout,
        Vec::<u8>::new(),
        "a failed command leaves stdout empty (#122)"
    );
    String::from_utf8(output.stderr).unwrap()
}

/// `gitchange commit <args>`, asserted to have committed, its echo.
fn commit(dir: &Path, args: &[&str]) -> String {
    succeeds(dir, &[&["commit"], args].concat())
}

fn head_short(dir: &Path) -> String {
    git(dir, &["rev-parse", "--short", "HEAD"])
}

fn head_message(dir: &Path) -> String {
    git(dir, &["log", "-1", "--format=%B"])
}

/// The paths the tip commit touched, in git's order.
fn head_paths(dir: &Path) -> Vec<String> {
    git(dir, &["show", "--format=", "--name-only", "HEAD"])
        .lines()
        .map(str::to_owned)
        .collect()
}

fn commit_count(dir: &Path) -> usize {
    git(dir, &["rev-list", "--count", "HEAD"])
        .parse()
        .expect("a commit count")
}

// --- the happy path ---------------------------------------------------------

#[test]
fn the_new_tip_carries_exactly_the_changelists_staged_hunks() {
    let dir = owned_repo();
    let repo = dir.path();
    // Two changelists with staged work; only one of them is committed.
    succeeds(repo, &["add", "feature"]);
    succeeds(repo, &["add", "docs"]);
    let before = head_short(repo);

    let echo = commit(repo, &["feature", "-m", "the first half"]);

    let short = head_short(repo);
    assert_ne!(short, before, "a commit was made");
    assert_eq!(echo.lines().count(), 1, "one line on stdout: {echo}");
    assert!(echo.contains(&short), "the echo carries the handle: {echo}");
    assert!(echo.contains("'feature'"), "{echo}");
    assert!(echo.contains("1 hunk"), "{echo}");

    assert_eq!(head_paths(repo), vec!["a.txt"]);
    let landed = git(repo, &["show", "HEAD", "--", "a.txt"]);
    assert!(landed.contains("+first edited"), "{landed}");
    assert!(
        !landed.contains("last edited"),
        "the co-owner's hunk stayed out: {landed}"
    );
    assert_eq!(head_message(repo), "the first half");
}

#[test]
fn another_changelists_staged_work_survives_in_the_live_index() {
    let dir = owned_repo();
    let repo = dir.path();
    succeeds(repo, &["add", "feature"]);
    succeeds(repo, &["add", "docs"]);

    commit(repo, &["feature", "-m", "the first half"]);

    assert_eq!(
        staged_paths(repo),
        vec!["b.txt"],
        "'docs' still has its hunk staged, needing no restoring"
    );
    assert_eq!(git(repo, &["show", ":b.txt"]), "two");
}

/// An entry's *unstaged* hunks sit outside its staged content, so a file
/// two changelists share never trips the foreign-content guard on their
/// account — `a.txt`'s second hunk is unassigned and unstaged throughout
/// the happy path above.
#[test]
fn an_entrys_unstaged_hunks_never_trip_the_foreign_content_guard() {
    let dir = owned_repo();
    let repo = dir.path();
    succeeds(repo, &["add", "feature"]);

    let echo = commit(repo, &["feature", "-m", "half a file"]);

    assert!(echo.contains("committed"), "{echo}");
    let worktree = std::fs::read_to_string(repo.join("a.txt")).unwrap();
    assert!(
        worktree.contains("last edited"),
        "the unstaged hunk is still in the worktree: {worktree}"
    );
}

// --- target validation, ahead of every guard --------------------------------

#[test]
fn all_is_refused_as_a_name_no_changelist_has() {
    let dir = owned_repo();

    let stderr = refusal(dir.path(), &["all", "-m", "everything"]);

    assert!(stderr.contains("'all' is not a changelist"), "{stderr}");
    assert!(
        stderr.contains("commit one changelist by name"),
        "the resolution is named: {stderr}"
    );
    // Never the view/scope vocabulary: there is no second commit meaning
    // for `all` to be refused as.
    assert!(!stderr.contains("view"), "{stderr}");
    assert!(!stderr.contains("scope"), "{stderr}");
}

#[test]
fn an_unrecognised_target_lists_the_candidates() {
    let dir = owned_repo();

    let stderr = refusal(dir.path(), &["featrue", "-m", "typo"]);

    assert!(stderr.contains("no changelist named 'featrue'"), "{stderr}");
    assert!(stderr.contains("the changelist scopes are:"), "{stderr}");
    assert!(stderr.contains("unassigned"), "{stderr}");
    assert!(stderr.contains("'feature'"), "{stderr}");
}

#[test]
fn target_validation_speaks_even_mid_operation() {
    let dir = merging_repo_with_a_changelist();
    let repo = dir.path();

    let all = refusal(repo, &["all", "-m", "everything"]);
    assert!(all.contains("'all' is not a changelist"), "{all}");
    assert!(!all.contains("in progress"), "{all}");

    let unknown = refusal(repo, &["nope", "-m", "typo"]);
    assert!(unknown.contains("no changelist named 'nope'"), "{unknown}");
    assert!(!unknown.contains("in progress"), "{unknown}");
}

// --- rung 1: the operation guard --------------------------------------------

/// `support::merging_repo` with a changelist to name, so the operation
/// guard can be reached with a valid target.
fn merging_repo_with_a_changelist() -> tempfile::TempDir {
    let dir = crate::support::merging_repo();
    seed_state(dir.path(), "feature", &["feature"]);
    dir
}

#[test]
fn an_operation_in_progress_refuses_before_the_payloads_own_rungs() {
    let dir = merging_repo_with_a_changelist();

    // Rung 6 would otherwise speak: 'feature' has nothing staged.
    let stderr = refusal(dir.path(), &["feature", "-m", "mid-merge"]);

    assert!(stderr.contains("merge in progress"), "{stderr}");
    assert!(stderr.contains("conclude or abort it first"), "{stderr}");
    assert!(!stderr.contains("gitchange add"), "{stderr}");
}

#[test]
fn an_operation_in_progress_refuses_before_the_unassigned_gate() {
    let dir = merging_repo_with_a_changelist();

    let stderr = refusal(dir.path(), &["unassigned", "-m", "mid-merge"]);

    assert!(stderr.contains("merge in progress"), "{stderr}");
    assert!(!stderr.contains("--allow-unassigned"), "{stderr}");
}

// --- rung 2: foreign content ------------------------------------------------

fn numbered(lines: usize) -> String {
    (1..=lines).map(|n| format!("line {n}\n")).collect()
}

/// A repo whose `notes.txt` is one index entry two changelists hold
/// content in, presented as a whole-file hunk (ADR 0004's foreign-content
/// condition, ADR 0009's indivisible entry): two staged text hunks
/// assigned apart, then a binary rewrite that leaves the entry with no
/// smaller committable unit.
///
/// `mid_operation` stops a conflicting merge on `tracked.txt` before the
/// split is built, so rung 1 and rung 2 co-hold and their order is
/// observable.
fn split_entry_repo(mid_operation: bool) -> tempfile::TempDir {
    let dir = committed_repo();
    let repo = dir.path();
    write(repo, "notes.txt", &numbered(30));
    git(repo, &["add", "notes.txt"]);
    git(repo, &["commit", "-q", "--no-verify", "-m", "notes"]);
    if mid_operation {
        conflict_on_tracked(repo);
    }
    let mut lines: Vec<String> = numbered(30)
        .lines()
        .map(|line| format!("{line}\n"))
        .collect();
    lines[4] = "line five, edited\n".into();
    lines[24] = "line twenty-five, edited\n".into();
    write(repo, "notes.txt", &lines.concat());
    git(repo, &["add", "notes.txt"]);
    // Capture off, so assigning one hunk away cannot claim the other back
    // (ADR 0016).
    seed_state_raw(
        repo,
        r#"{
  "version": 1, "active": null,
  "changelists": [{ "name": "art" }, { "name": "other" }]
}"#,
    );
    let first = address(repo, "notes.txt", 0);
    let second = address(repo, "notes.txt", 1);
    succeeds(repo, &["assign", &first, "--to", "art"]);
    succeeds(repo, &["assign", &second, "--to", "other"]);
    // The worktree turns binary: one entry, a whole-file hunk over
    // content two holders own.
    std::fs::write(repo.join("notes.txt"), [0u8, 1, 2, 3]).unwrap();
    dir
}

/// Stop a merge on `tracked.txt`, `support::merging_repo`'s own shape,
/// applied to a repo that already has its other files committed.
fn conflict_on_tracked(repo: &Path) {
    git(repo, &["checkout", "-q", "-b", "sideline"]);
    write(repo, "tracked.txt", "sideline\n");
    git(
        repo,
        &["commit", "-q", "--no-verify", "-am", "sideline edit"],
    );
    git(repo, &["checkout", "-q", "main"]);
    write(repo, "tracked.txt", "mainline\n");
    git(
        repo,
        &["commit", "-q", "--no-verify", "-am", "mainline edit"],
    );
    let merge = git_command(repo)
        .args(["merge", "--no-edit", "sideline"])
        .output()
        .expect("run git");
    assert!(!merge.status.success(), "the merge is meant to conflict");
}

#[test]
fn a_split_entry_refuses_either_side_naming_the_holders() {
    let dir = split_entry_repo(false);
    let repo = dir.path();

    // Under capture-off the newcomer whole-file hunk is unassigned, so
    // each side has two other holders — and every one of them is named:
    // the refusal is complete within its rung.
    for (committing, other) in [("art", "'other'"), ("other", "'art'")] {
        let stderr = refusal(repo, &[committing, "-m", "half an entry"]);
        assert!(stderr.contains("cannot commit notes.txt"), "{stderr}");
        let holders = stderr
            .split_once("content held by ")
            .expect("the refusal names who else is in the entry")
            .1;
        assert!(holders.starts_with("unassigned, "), "{stderr}");
        assert!(
            holders.contains(other),
            "the other holder is named: {stderr}"
        );
        assert!(
            !holders.contains(&format!("'{committing}'")),
            "the payload's own holder is not among them: {stderr}"
        );
        assert!(
            stderr.contains("assign the file's hunks to one changelist"),
            "the one-op resolution is named: {stderr}"
        );
        assert!(stderr.contains("nothing was committed"), "{stderr}");
    }
    assert_eq!(commit_count(repo), 2, "nothing was committed");
}

#[test]
fn the_operation_guard_speaks_before_foreign_content() {
    let dir = split_entry_repo(true);

    let stderr = refusal(dir.path(), &["art", "-m", "half an entry"]);

    assert!(stderr.contains("merge in progress"), "{stderr}");
    assert!(!stderr.contains("cannot commit notes.txt"), "{stderr}");
}

// --- rung 3: --amend and the foreign head -----------------------------------

/// The rung-3 refusal, asserted whole: the wording is one string in
/// `commit::refuse_foreign_head`, so it is one assertion here rather than
/// a fragment repeated per case.
fn assert_foreign_head(stderr: &str, target: &str) {
    assert!(
        stderr.contains(&format!(
            "HEAD is not the commit gitchange last made for {target}"
        )),
        "{stderr}"
    );
}

/// `owned_repo` carried to the state an amend needs: `'feature'`'s own
/// gitchange commit at HEAD, a second hunk of its own staged for the
/// amend to fold in, and `'docs'` staged work standing by in the live
/// index.
///
/// `a.txt`'s other hunk survives the first commit unassigned — capture is
/// off in this fixture — so it is assigned before it is staged.
fn amendable_repo() -> tempfile::TempDir {
    let dir = owned_repo();
    let repo = dir.path();
    succeeds(repo, &["add", "feature"]);
    commit(repo, &["feature", "-m", "the first half"]);
    succeeds(repo, &["assign", "a.txt", "--to", "feature"]);
    succeeds(repo, &["add", "feature"]);
    succeeds(repo, &["add", "docs"]);
    dir
}

#[test]
fn the_amended_tip_is_heads_content_plus_the_payload() {
    let dir = amendable_repo();
    let repo = dir.path();
    let before = commit_count(repo);

    let echo = commit(repo, &["feature", "--amend", "--no-edit"]);

    assert_eq!(
        commit_count(repo),
        before,
        "the tip was replaced, not added to"
    );
    assert!(echo.contains(&head_short(repo)), "{echo}");
    let landed = git(repo, &["show", "HEAD", "--", "a.txt"]);
    assert!(
        landed.contains("+first edited"),
        "HEAD's own content is still there: {landed}"
    );
    assert!(
        landed.contains("+last edited"),
        "the payload joined it: {landed}"
    );
    assert_eq!(head_paths(repo), vec!["a.txt"]);
    assert_eq!(
        staged_paths(repo),
        vec!["b.txt"],
        "'docs' still has its hunk staged, needing no restoring"
    );
}

/// The loop ADR 0004 §Amend leaves unguarded: an amend re-records, so
/// HEAD stays the changelist's own last commit however many times it
/// moves.
#[test]
fn amend_after_amend_passes() {
    let dir = amendable_repo();
    let repo = dir.path();
    commit(repo, &["feature", "--amend", "--no-edit"]);

    write(repo, "a.txt", "rewritten\n");
    succeeds(repo, &["assign", "a.txt", "--to", "feature"]);
    succeeds(repo, &["add", "feature"]);
    let echo = commit(repo, &["feature", "--amend", "-m", "the whole of it"]);

    assert!(echo.contains("committed"), "{echo}");
    assert_eq!(head_message(repo), "the whole of it");
    assert_eq!(git(repo, &["show", "HEAD:a.txt"]), "rewritten");
}

#[test]
fn no_record_yet_refuses() {
    // A repo that has never committed through gitchange: HEAD is
    // somebody's, and nothing says whose.
    let dir = one_hunk_repo();

    let stderr = refusal(dir.path(), &["feature", "--amend", "--no-edit"]);

    assert_foreign_head(&stderr, "'feature'");
    assert!(stderr.contains("--allow-foreign-head"), "{stderr}");
    assert!(stderr.contains("commit without '--amend'"), "{stderr}");
    assert_eq!(commit_count(dir.path()), 1, "nothing was committed");
}

#[test]
fn another_changelists_commit_refuses() {
    let dir = amendable_repo();
    let repo = dir.path();
    // 'docs' commits last, so HEAD is its commit and the record is its
    // name — amending 'feature' now would fold 'feature''s payload in.
    commit(repo, &["docs", "-m", "the other half"]);

    let stderr = refusal(repo, &["feature", "--amend", "--no-edit"]);

    assert_foreign_head(&stderr, "'feature'");
    assert_eq!(head_message(repo), "the other half", "nothing was amended");
}

#[test]
fn a_commit_made_outside_gitchange_refuses() {
    let dir = amendable_repo();
    let repo = dir.path();
    write(repo, "outside.txt", "elsewhere\n");
    git(repo, &["add", "outside.txt"]);
    git(repo, &["commit", "-q", "--no-verify", "-m", "outside"]);

    let stderr = refusal(repo, &["feature", "--amend", "--no-edit"]);

    assert_foreign_head(&stderr, "'feature'");
    assert_eq!(head_message(repo), "outside", "nothing was amended");
}

/// A deleted name can be recreated, and what comes back is not the
/// changelist that made the commit — so the delete clears the record
/// (ADR 0004 §Aftermath) and the guard sees the stranger's commit it is.
#[test]
fn a_deleted_then_recreated_name_refuses() {
    let dir = amendable_repo();
    let repo = dir.path();
    succeeds(repo, &["changelist", "-D", "feature"]);
    succeeds(repo, &["changelist", "feature"]);
    succeeds(repo, &["assign", "a.txt", "--to", "feature"]);
    succeeds(repo, &["add", "feature"]);

    let stderr = refusal(repo, &["feature", "--amend", "--no-edit"]);

    assert_foreign_head(&stderr, "'feature'");
}

#[test]
fn the_foreign_head_flag_amends_head_as_it_stands() {
    let dir = one_hunk_repo();
    let repo = dir.path();

    let echo = commit(
        repo,
        &["feature", "--amend", "--no-edit", "--allow-foreign-head"],
    );

    assert!(echo.contains("committed"), "{echo}");
    assert_eq!(commit_count(repo), 1, "the tip was replaced");
    assert_eq!(head_message(repo), "init", "--no-edit kept what HEAD had");
    assert_eq!(git(repo, &["show", "HEAD:tracked.txt"]), "two");
}

#[test]
fn the_foreign_head_flag_is_inert_where_the_guard_would_not_fire() {
    let dir = amendable_repo();
    let repo = dir.path();

    // On an own-record amend, and on a commit that is no amend at all.
    let amended = commit(
        repo,
        &["feature", "--amend", "--no-edit", "--allow-foreign-head"],
    );
    assert!(amended.contains("committed"), "{amended}");
    let committed = commit(
        repo,
        &["docs", "-m", "the other half", "--allow-foreign-head"],
    );
    assert!(committed.contains("'docs'"), "{committed}");
    assert_eq!(commit_count(repo), 3);
}

// --- rung 3's place in the stack --------------------------------------------

#[test]
fn the_foreign_head_guard_speaks_before_the_unassigned_gate() {
    let dir = unassigned_payload_repo();

    let stderr = refusal(
        dir.path(),
        &["unassigned", "--amend", "--no-edit", "--allow-unassigned"],
    );

    assert_foreign_head(&stderr, "unassigned");

    // And without the gate's override too: rung 3 speaks either way.
    let bare = refusal(dir.path(), &["unassigned", "--amend", "--no-edit"]);
    assert_foreign_head(&bare, "unassigned");
    assert!(!bare.contains("skips the assign step"), "{bare}");
}

#[test]
fn the_foreign_head_guard_speaks_before_staged_stale() {
    let dir = stale_repo();
    let repo = dir.path();
    let stale = address(repo, "tracked.txt", 0);

    let stderr = refusal(repo, &["feature", "--amend", "--no-edit"]);

    assert_foreign_head(&stderr, "'feature'");
    assert!(!stderr.contains(&stale), "{stderr}");
}

#[test]
fn the_foreign_head_guard_speaks_before_the_empty_payload() {
    let dir = owned_repo();

    let stderr = refusal(dir.path(), &["feature", "--amend", "--no-edit"]);

    assert_foreign_head(&stderr, "'feature'");
    assert!(!stderr.contains("no staged hunks"), "{stderr}");
}

/// The other direction of the same order (#151: the first in order
/// speaks). Rungs 1 and 2 are core's conditions and rung 3 the CLI's, so
/// nothing but this holds them in one sequence.
#[test]
fn the_operation_guard_speaks_before_the_foreign_head() {
    // Mid-merge and no record at all: both conditions hold.
    let dir = merging_repo_with_a_changelist();

    let stderr = refusal(dir.path(), &["feature", "--amend", "--no-edit"]);

    assert!(stderr.contains("merge in progress"), "{stderr}");
    assert!(!stderr.contains("gitchange last made"), "{stderr}");
}

#[test]
fn foreign_content_speaks_before_the_foreign_head() {
    let dir = split_entry_repo(false);

    let stderr = refusal(dir.path(), &["art", "--amend", "--no-edit"]);

    assert!(stderr.contains("cannot commit notes.txt"), "{stderr}");
    assert!(!stderr.contains("gitchange last made"), "{stderr}");
}

// --- rung 6 under --amend: reword stays git's job ---------------------------

#[test]
fn an_empty_amend_payload_names_add_and_raw_git() {
    let dir = amendable_repo();
    let repo = dir.path();
    // The amend consumes the payload, so the next one has none — which is
    // what a reword looks like from here.
    commit(repo, &["feature", "--amend", "--no-edit"]);

    let stderr = refusal(repo, &["feature", "--amend", "--no-edit"]);

    assert!(stderr.contains("no staged hunks"), "{stderr}");
    assert!(stderr.contains("gitchange add feature"), "{stderr}");
    assert!(stderr.contains("git commit --amend"), "{stderr}");
    assert_eq!(commit_count(repo), 2, "nothing was committed");
}

/// The plain refusal stays plain: raw git is amend's resolution, and a
/// commit that is no amend has no reword to be mistaken for.
#[test]
fn an_empty_payload_without_amend_names_git_nothing() {
    let dir = owned_repo();

    let stderr = refusal(dir.path(), &["feature", "-m", "nothing yet"]);

    assert!(!stderr.contains("git commit --amend"), "{stderr}");
}

// --- the overrides composed -------------------------------------------------

#[test]
fn an_unassigned_amend_composes_both_overrides_and_re_records() {
    let dir = unassigned_payload_repo();
    let repo = dir.path();
    commit(
        repo,
        &["unassigned", "-m", "the pool", "--allow-unassigned"],
    );
    // A second unassigned hunk, in an entry nobody else holds.
    write(repo, "keep.txt", "changed\n");
    git(repo, &["add", "keep.txt"]);

    let echo = commit(
        repo,
        &[
            "unassigned",
            "--amend",
            "--no-edit",
            "--allow-unassigned",
            "--allow-foreign-head",
        ],
    );

    assert!(echo.contains("unassigned"), "{echo}");
    assert_eq!(head_paths(repo), vec!["keep.txt", "sub/c.txt"]);
    // The amend re-recorded under `unassigned`, which is what lets the
    // next one pass the guard with no override at all — the record is
    // not readable at this seam, but its effect is.
    write(repo, "extra.txt", "more\n");
    git(repo, &["add", "extra.txt"]);
    let again = commit(
        repo,
        &["unassigned", "--amend", "--no-edit", "--allow-unassigned"],
    );
    assert!(again.contains("committed"), "{again}");
}

// --- rung 4: the unassigned gate --------------------------------------------

/// `owned_repo` with the unassigned pool staged: `sub/c.txt` and
/// `a.txt`'s second hunk belong to nobody, and raw `git add` puts the
/// whole of `sub/c.txt` in the index (ADR 0003 — refresh absorbs it).
fn unassigned_payload_repo() -> tempfile::TempDir {
    let dir = owned_repo();
    git(dir.path(), &["add", "sub/c.txt"]);
    dir
}

#[test]
fn bare_commit_unassigned_refuses_naming_both_resolutions() {
    let dir = unassigned_payload_repo();

    let stderr = refusal(dir.path(), &["unassigned", "-m", "the pool"]);

    assert!(stderr.contains("skips the assign step"), "{stderr}");
    assert!(stderr.contains("gitchange assign"), "{stderr}");
    assert!(stderr.contains("--allow-unassigned"), "{stderr}");
    assert_eq!(commit_count(dir.path()), 1, "nothing was committed");
}

#[test]
fn the_unassigned_flag_commits_the_pool_under_the_full_stack() {
    let dir = unassigned_payload_repo();
    let repo = dir.path();

    let echo = commit(
        repo,
        &["unassigned", "-m", "the pool", "--allow-unassigned"],
    );

    assert!(echo.contains("unassigned"), "{echo}");
    assert_eq!(head_paths(repo), vec!["sub/c.txt"]);
}

#[test]
fn the_unassigned_flag_is_inert_with_a_named_changelist() {
    let dir = owned_repo();
    let repo = dir.path();
    succeeds(repo, &["add", "feature"]);

    let echo = commit(
        repo,
        &["feature", "-m", "the first half", "--allow-unassigned"],
    );

    assert!(echo.contains("'feature'"), "{echo}");
    assert_eq!(head_paths(repo), vec!["a.txt"]);
}

/// The gate is rung 4 and the empty payload rung 6, so a scope that is
/// both unassigned and empty speaks the gate.
#[test]
fn the_unassigned_gate_speaks_before_the_empty_payload() {
    let dir = owned_repo();

    let stderr = refusal(dir.path(), &["unassigned", "-m", "nothing at all"]);

    assert!(stderr.contains("skips the assign step"), "{stderr}");
    assert!(!stderr.contains("no staged hunks"), "{stderr}");
}

// --- rung 5: staged-stale ---------------------------------------------------

/// A repo where `'feature'` owns one hunk the index and the worktree
/// disagree about: captured and staged, then edited again.
fn stale_repo() -> tempfile::TempDir {
    let dir = committed_repo();
    let repo = dir.path();
    write(repo, "tracked.txt", "two\n");
    seed_state(repo, "feature", &["feature"]);
    succeeds(repo, &["add", "feature"]);
    write(repo, "tracked.txt", "three\n");
    dir
}

#[test]
fn a_stale_hunk_refuses_naming_its_address_and_both_resolutions() {
    let dir = stale_repo();
    let repo = dir.path();
    let stale = address(repo, "tracked.txt", 0);

    let stderr = refusal(repo, &["feature", "-m", "which version?"]);

    assert!(stderr.contains(&stale), "the address is named: {stderr}");
    assert!(stderr.contains("gitchange add feature"), "{stderr}");
    assert!(stderr.contains("--allow-staged-stale"), "{stderr}");
    assert_eq!(commit_count(repo), 1, "nothing was committed");
}

#[test]
fn the_staged_stale_flag_ships_the_version_the_index_holds() {
    let dir = stale_repo();
    let repo = dir.path();

    commit(
        repo,
        &["feature", "-m", "the checkpoint", "--allow-staged-stale"],
    );

    assert_eq!(
        git(repo, &["show", "HEAD:tracked.txt"]),
        "two",
        "the index's version shipped, not the worktree's"
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("tracked.txt")).unwrap(),
        "three\n",
        "the worktree is untouched"
    );
}

#[test]
fn the_staged_stale_flag_is_inert_with_no_stale_hunk() {
    let dir = owned_repo();
    let repo = dir.path();
    succeeds(repo, &["add", "feature"]);

    let echo = commit(
        repo,
        &["feature", "-m", "the first half", "--allow-staged-stale"],
    );

    assert!(echo.contains("committed"), "{echo}");
    assert_eq!(head_paths(repo), vec!["a.txt"]);
}

// --- rung 6: the empty payload ----------------------------------------------

#[test]
fn an_empty_payload_refuses_naming_add() {
    let dir = owned_repo();

    let stderr = refusal(dir.path(), &["feature", "-m", "nothing yet"]);

    assert!(stderr.contains("'feature'"), "{stderr}");
    assert!(stderr.contains("no staged hunks"), "{stderr}");
    assert!(stderr.contains("gitchange add feature"), "{stderr}");
    // No stage-all flag, and no `-a` borrow.
    assert!(!stderr.contains("--all"), "{stderr}");
}

// --- the message sources ----------------------------------------------------

/// A repo with one staged hunk owned by `'feature'` — the smallest thing
/// a message can be attached to.
fn one_hunk_repo() -> tempfile::TempDir {
    let dir = committed_repo();
    let repo = dir.path();
    write(repo, "tracked.txt", "two\n");
    seed_state(repo, "feature", &["feature"]);
    succeeds(repo, &["add", "feature"]);
    dir
}

#[test]
fn a_multiline_message_arrives_verbatim() {
    let dir = one_hunk_repo();
    let repo = dir.path();

    commit(repo, &["feature", "-m", "a subject\n\na body line"]);

    assert_eq!(head_message(repo), "a subject\n\na body line");
}

#[test]
fn repeated_m_lands_as_paragraphs() {
    let dir = one_hunk_repo();
    let repo = dir.path();

    commit(repo, &["feature", "-m", "a subject", "-m", "a body"]);

    assert_eq!(head_message(repo), "a subject\n\na body");
}

#[test]
fn dash_f_reads_the_named_file() {
    let dir = one_hunk_repo();
    let repo = dir.path();
    // Outside the worktree, so writing it cannot change the payload.
    let elsewhere = tempfile::tempdir().unwrap();
    let message = elsewhere.path().join("message.txt");
    std::fs::write(&message, "from a file\n\nwith a body\n").unwrap();

    commit(repo, &["feature", "-F", path_str(&message)]);

    assert_eq!(head_message(repo), "from a file\n\nwith a body");
}

#[test]
fn dash_f_dash_reads_stdin() {
    let dir = one_hunk_repo();
    let repo = dir.path();

    let output = run(
        repo,
        &["commit", "feature", "-F", "-"],
        "from stdin\n\nwith a body\n",
    );

    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(head_message(repo), "from stdin\n\nwith a body");
}

/// `--no-edit` is the only source that carries no message: core omits
/// `-F` and forwards the flag, so git keeps what HEAD already had.
#[test]
fn no_edit_keeps_heads_message_on_amend() {
    let dir = amendable_repo();
    let repo = dir.path();

    commit(repo, &["feature", "--amend", "--no-edit"]);

    assert_eq!(head_message(repo), "the first half");
}

#[test]
fn dash_m_and_dash_f_replace_heads_message_on_amend() {
    let dir = amendable_repo();
    let repo = dir.path();

    commit(repo, &["feature", "--amend", "-m", "the whole of it"]);
    assert_eq!(head_message(repo), "the whole of it");

    let elsewhere = tempfile::tempdir().unwrap();
    let message = elsewhere.path().join("message.txt");
    std::fs::write(&message, "from a file\n").unwrap();
    write(repo, "extra.txt", "more\n");
    git(repo, &["add", "extra.txt"]);
    succeeds(repo, &["assign", "extra.txt", "--to", "feature"]);
    commit(repo, &["feature", "--amend", "-F", path_str(&message)]);
    assert_eq!(head_message(repo), "from a file");
}

// --- hooks ------------------------------------------------------------------

/// Install `hook` as `.git/hooks/<name>`, executable.
#[cfg(unix)]
fn install_hook(repo: &Path, name: &str, body: &str) {
    use std::os::unix::fs::PermissionsExt as _;
    let path = repo.join(".git/hooks").join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Hooks run natively against the temp index, so `git diff --cached`
/// inside one reports the commit's true content — the partial payload,
/// not the live index (ADR 0004).
#[cfg(unix)]
#[test]
fn a_hook_sees_the_commits_true_content() {
    let dir = owned_repo();
    let repo = dir.path();
    succeeds(repo, &["add", "feature"]);
    succeeds(repo, &["add", "docs"]);
    // Written under .git so the hook's own output is not a worktree change.
    install_hook(
        repo,
        "pre-commit",
        "git diff --cached --name-only > \"$(git rev-parse --git-dir)/hook-saw.txt\"",
    );

    commit(repo, &["feature", "-m", "the first half"]);

    let saw = std::fs::read_to_string(repo.join(".git/hook-saw.txt")).unwrap();
    assert_eq!(
        saw.lines().collect::<Vec<&str>>(),
        vec!["a.txt"],
        "the hook saw the payload, not the live index"
    );
}

#[cfg(unix)]
#[test]
fn a_rejecting_hook_exits_1_carrying_gits_stderr() {
    let dir = owned_repo();
    let repo = dir.path();
    succeeds(repo, &["add", "feature"]);
    succeeds(repo, &["add", "docs"]);
    install_hook(repo, "pre-commit", "echo 'the hook says no' >&2\nexit 1");

    let stderr = refusal(repo, &["feature", "-m", "the first half"]);

    assert!(
        stderr.contains("the hook says no"),
        "git's stderr comes through as git produced it: {stderr}"
    );
    assert!(
        stderr.starts_with("gitchange:"),
        "gitchange's own diagnostic keeps its prefix: {stderr}"
    );
    assert_eq!(commit_count(repo), 1, "nothing was committed");
    assert_eq!(
        staged_paths(repo),
        vec!["a.txt", "b.txt"],
        "the live index is untouched"
    );
}

#[cfg(unix)]
#[test]
fn no_verify_bypasses_the_hook() {
    let dir = owned_repo();
    let repo = dir.path();
    succeeds(repo, &["add", "feature"]);
    install_hook(repo, "pre-commit", "echo 'the hook says no' >&2\nexit 1");

    let echo = commit(repo, &["feature", "-m", "the first half", "-n"]);

    assert!(echo.contains("committed"), "{echo}");
    assert_eq!(head_paths(repo), vec!["a.txt"]);
}

// --- receipts ---------------------------------------------------------------

#[test]
fn a_capture_commits_refresh_made_is_reported_as_a_notice_exactly_once() {
    let dir = committed_repo();
    let repo = dir.path();
    seed_state(repo, "feature", &["feature"]);
    // Staged with raw git and recorded by nobody: commit's own refresh is
    // what captures it into the active changelist (ADR 0005).
    write(repo, "tracked.txt", "two\n");
    write(repo, "extra.txt", &long_file("first", "last"));
    git(repo, &["add", "-A"]);

    let output = run(repo, &["commit", "feature", "-m", "captured"], "");

    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    let notices: Vec<&str> = stderr
        .lines()
        .filter(|line| line.contains("auto-captured"))
        .collect();
    assert_eq!(
        notices.len(),
        2,
        "one notice per captured hunk, delivered once: {stderr}"
    );
    assert!(
        notices
            .iter()
            .all(|line| line.starts_with("gitchange: notice: ")),
        "{stderr}"
    );
    assert_eq!(head_paths(repo), vec!["extra.txt", "tracked.txt"]);
}

/// The refresh's decisions reach the caller even when a guard then
/// refuses: the capture is already written, so swallowing it would lose
/// the only report of it.
#[test]
fn a_refusal_still_reports_the_captures_its_refresh_made() {
    let dir = committed_repo();
    let repo = dir.path();
    seed_state(repo, "feature", &["feature"]);
    write(repo, "tracked.txt", "two\n");

    // Nothing is staged, so rung 6 refuses — after the refresh captured.
    let stderr = refusal(repo, &["feature", "-m", "captured then refused"]);

    assert!(stderr.contains("auto-captured"), "{stderr}");
    assert!(stderr.contains("no staged hunks"), "{stderr}");
}
