//! End-to-end tests of the binary: output and the 0/1/2 exit-code
//! contract (ticket 13). Fixtures are built with the git CLI so git2
//! stays a gitchange-core-only dependency (ADR 0006).

use std::path::Path;
use std::process::{Command, Output};

fn gitchange(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_gitchange"))
        .current_dir(dir)
        .args(args)
        .output()
        .expect("run gitchange")
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

/// An initialised repo with a committing identity and one committed
/// file, worktree clean — the stem both fixtures below grow from.
fn committed_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q"]);
    git(dir.path(), &["config", "user.name", "gitchange-tests"]);
    git(
        dir.path(),
        &["config", "user.email", "tests@gitchange.invalid"],
    );
    std::fs::write(dir.path().join("tracked.txt"), "one\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-q", "--no-verify", "-m", "init"]);
    dir
}

fn dirty_repo() -> tempfile::TempDir {
    let dir = committed_repo();
    std::fs::write(dir.path().join("tracked.txt"), "two\n").unwrap();
    std::fs::write(dir.path().join("untracked.txt"), "hello\n").unwrap();
    dir
}

/// Seed a state file directly (the CLI has no create subcommand by
/// design — ticket 13 fixed the surface to `status` + `switch`). Doubles
/// as a schema-v1 stability check: this JSON must stay readable.
fn seed_state(dir: &Path, active: &str, names: &[&str]) {
    let gitchange_dir = dir.join(".git/gitchange");
    std::fs::create_dir_all(&gitchange_dir).unwrap();
    let changelists: Vec<String> = names
        .iter()
        .map(|name| format!("{{ \"name\": \"{name}\" }}"))
        .collect();
    let json = format!(
        "{{ \"version\": 1, \"active\": \"{active}\", \"changelists\": [{}] }}\n",
        changelists.join(", ")
    );
    std::fs::write(gitchange_dir.join("state.json"), json).unwrap();
}

/// Seed a verbatim state file, for fixtures that need records.
fn seed_state_raw(dir: &Path, json: &str) {
    let gitchange_dir = dir.join(".git/gitchange");
    std::fs::create_dir_all(&gitchange_dir).unwrap();
    std::fs::write(gitchange_dir.join("state.json"), json).unwrap();
}

#[test]
fn switch_then_status_round_trip_the_active_marker() {
    let repo = dirty_repo();
    seed_state(repo.path(), "feature", &["feature", "bugfix"]);

    let output = gitchange(repo.path(), &["switch", "bugfix"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(stdout, "Switched to changelist 'bugfix'\n");

    // A separate invocation sees the persisted marker; the dirty files
    // auto-capture to the newly active changelist.
    let output = gitchange(repo.path(), &["status"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        vec![
            "  feature",
            "* bugfix",
            "    ○ M tracked.txt 0/1",
            "    ○ ? untracked.txt 0/1",
        ]
    );
}

#[test]
fn switch_to_unknown_name_exits_1_with_message_on_stderr() {
    let repo = dirty_repo();
    seed_state(repo.path(), "feature", &["feature"]);

    let output = gitchange(repo.path(), &["switch", "nope"]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("no changelist named 'nope'"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn switch_without_a_name_exits_2() {
    let repo = dirty_repo();
    let output = gitchange(repo.path(), &["switch"]);

    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn status_lists_changed_files_and_exits_0() {
    // No changelists exist: the whole dirty tree is the unassigned group.
    let repo = dirty_repo();
    let output = gitchange(repo.path(), &["status"]);

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        vec![
            "  unassigned",
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
            "  unassigned",
            "    ● M tracked.txt 1/1",
            "    ○ ? untracked.txt 0/1",
        ]
    );
}

#[test]
fn status_groups_files_by_changelist_with_unassigned_last() {
    // A null-owner record claims tracked.txt for unassigned (an orphan
    // of a deleted changelist); the untracked file captures to active.
    let repo = dirty_repo();
    seed_state_raw(
        repo.path(),
        r#"{
  "version": 1, "active": "feature",
  "changelists": [{ "name": "feature" }, { "name": "bugfix" }],
  "records": [
    {
      "path": "tracked.txt", "old_start": 1, "old_lines": 1,
      "new_start": 1, "new_lines": 1, "changelist": null,
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
            "* feature",
            "    ○ ? untracked.txt 0/1",
            "  bugfix",
            "  unassigned",
            "    ○ M tracked.txt 0/1",
        ]
    );
}

#[test]
fn status_prints_ambiguous_overlap_advisories_on_stderr() {
    // Records from two changelists overlap the fresh hunk without an
    // exact anchor match: active captures it, with a notice (ADR 0001).
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

    let output = gitchange(repo.path(), &["status"]);
    assert_eq!(output.status.code(), Some(0));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(
        stderr,
        // The untracked file's hunk is genuinely new, so its capture
        // notices too (ticket #34: auto-capture is never silent).
        // Phrasing is core's canonical `Advisory::message` (ADR 0006).
        "gitchange: notice: auto-captured hunk at tracked.txt:1 → 'feature' \
         (ambiguous overlap: 'bugfix', 'feature')\n\
         gitchange: notice: auto-captured hunk at untracked.txt:1 → 'feature'\n"
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("* feature\n    ○ M tracked.txt 0/1"),
        "the hunk lands in the active changelist: {stdout}"
    );
}

#[test]
fn status_prints_the_head_move_dormancy_advisory_on_stderr() {
    // ADR 0012's advisory reaches stderr through the same loop as the
    // overlap one above — same `notice:` dressing, same core phrasing.
    // The fixture is the guard's minimal shape: a stored baseline behind
    // HEAD, the moved path carrying a live record whose anchor can't
    // match, and a hunk on it to capture. Grown from `committed_repo`
    // rather than `dirty_repo` so that, without the untracked file, the
    // two advisories on stderr are exactly the guard's own.
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

    let output = gitchange(repo, &["status"]);
    assert_eq!(output.status.code(), Some(0));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(
        stderr,
        // Phrasing is core's canonical `Advisory::message` (ADR 0006);
        // the CLI adds only the prefix. The guarded capture notices
        // alongside the dormancy — both are the guard's doing.
        "gitchange: notice: auto-captured hunk at tracked.txt:1 → 'feature'\n\
         gitchange: notice: external HEAD move changed tracked.txt — records \
         in 'bugfix' went dormant; affected hunks captured to active\n"
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("* feature\n    ○ M tracked.txt 0/1"),
        "the stranded record's hunk lands in the active changelist: {stdout}"
    );
}

#[test]
fn status_outside_a_repo_exits_1_with_message_on_stderr() {
    let dir = tempfile::tempdir().unwrap();
    let output = gitchange(dir.path(), &["status"]);

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("not a git repository"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn unknown_subcommand_exits_2() {
    let dir = tempfile::tempdir().unwrap();
    let output = gitchange(dir.path(), &["frobnicate"]);

    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn bare_invocation_outside_a_repo_exits_1() {
    // Bare `gitchange` launches the TUI, which needs a terminal — the
    // testable slice is the pre-terminal failure path: repository
    // discovery fails loudly with an operational exit code. The
    // launched TUI itself is smoke-tested in gitchange-tui.
    let dir = tempfile::tempdir().unwrap();
    let output = gitchange(dir.path(), &[]);

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("not a git repository"),
        "unexpected stderr: {stderr}"
    );
}
