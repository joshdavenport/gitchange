//! End-to-end tests of the binary: output and the 0/1/2/3 exit-code
//! contract (ticket 13, grown to the full published scheme by #136).
//! Fixtures are built with the git CLI so git2 stays a
//! gitchange-core-only dependency (ADR 0006).

use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::Duration;

fn gitchange(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_gitchange"))
        .current_dir(dir)
        .args(args)
        .output()
        .expect("run gitchange")
}

/// Real git in the fixture repo, with the host's global and system config
/// cut out (a nonexistent path reads as empty config) — the same cut
/// `RepoFixture::git_output` makes, and for the same reason: a developer
/// with a global `commit.gpgsign` or `core.hooksPath` would otherwise see
/// the fixture's own commit below fail on their machine and never in CI.
///
/// The cut-out is used instead of pinning individual knobs because it is
/// strictly stronger, and because it needs nothing from
/// `gitchange-test-support` — these fixtures stay on the git CLI so git2
/// is no direct dependency of this crate (ADR 0006).
fn git(dir: &Path, args: &[&str]) -> String {
    let absent = dir.join(".git/absent-config");
    let output = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", &absent)
        .env("GIT_CONFIG_SYSTEM", &absent)
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
    // Pin the initial branch for the same reason the other two builders
    // do: plain `init` takes it from `init.defaultBranch`, which the
    // cut-out above now removes rather than inherits.
    git(dir.path(), &["init", "-q", "--initial-branch=main"]);
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

/// This worktree's state file, spelled once. Tests that seed it and
/// tests that assert on it read the same path.
fn state_path(dir: &Path) -> std::path::PathBuf {
    dir.join(".git/gitchange/state.json")
}

/// Seed a state file directly (the CLI has no create subcommand by
/// design — ticket 13 fixed the surface to `status` + `switch`). Doubles
/// as a schema-v1 stability check: this JSON must stay readable.
fn seed_state(dir: &Path, active: &str, names: &[&str]) {
    let changelists: Vec<String> = names
        .iter()
        .map(|name| format!("{{ \"name\": \"{name}\" }}"))
        .collect();
    let json = format!(
        "{{ \"version\": 1, \"active\": \"{active}\", \"changelists\": [{}] }}\n",
        changelists.join(", ")
    );
    seed_state_raw(dir, &json);
}

/// Seed a verbatim state file, for fixtures that need records.
fn seed_state_raw(dir: &Path, json: &str) {
    let path = state_path(dir);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, json).unwrap();
}

/// This worktree's state lockfile, beside the state file — the path core
/// takes and the one its dead-holder refusal names.
fn lock_path(dir: &Path) -> std::path::PathBuf {
    state_path(dir).with_file_name("state.json.lock")
}

/// A dirty repo whose lockfile is held by another writer, `switch bugfix`
/// waiting to contend with it. `contents` is the recorded PID (#135),
/// which decides whether the hold reads as live, dead, or unreadable. The
/// state file is seeded first, so this is contention over an existing
/// state rather than a first write.
fn repo_holding(contents: &str) -> tempfile::TempDir {
    let repo = dirty_repo();
    seed_state(repo.path(), "feature", &["feature", "bugfix"]);
    std::fs::write(lock_path(repo.path()), contents).unwrap();
    repo
}

/// The PID of a process that has exited and been reaped — one the OS
/// answers "gone" for, so the hold classifies as dead. (Recycling could in
/// principle hand the number out again before the binary reads it; nothing
/// portable rules that out, and the window is microseconds.)
fn reaped_pid() -> u32 {
    let mut child = Command::new("git")
        .arg("--version")
        .stdout(Stdio::null())
        .spawn()
        .expect("git is on PATH for this suite");
    let pid = child.id();
    child.wait().unwrap();
    pid
}

#[test]
fn switch_then_status_round_trip_the_active_marker() {
    let repo = dirty_repo();
    seed_state(repo.path(), "feature", &["feature", "bugfix"]);

    let output = gitchange(repo.path(), &["switch", "bugfix"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    // Core's echo, printed verbatim: the receipt's stdout line is
    // composed beside the write (ADR 0006/0007), never here (#138).
    assert_eq!(stdout, "switched to 'bugfix'\n");

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
    assert_eq!(
        output.stdout,
        Vec::<u8>::new(),
        "a failed command leaves stdout empty (#122)"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("no changelist named 'nope'"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn switching_to_the_already_active_changelist_says_nothing() {
    // Nothing decided, nothing printed (#122): git's "Already on 'x'"
    // comfort text is not borrowed, so stdout carries decisions only.
    let repo = dirty_repo();
    seed_state(repo.path(), "feature", &["feature"]);

    let output = gitchange(repo.path(), &["switch", "feature"]);

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, Vec::<u8>::new());
    assert_eq!(output.stderr, Vec::<u8>::new());
}

#[test]
fn switch_without_a_name_exits_2() {
    let repo = dirty_repo();
    let output = gitchange(repo.path(), &["switch"]);

    assert_eq!(output.status.code(), Some(2));
}

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
fn switch_unassigned_turns_capture_off_and_marks_the_group() {
    // #52 / ADR 0015: `unassigned` is a switch target, and the existing
    // `*` on its group is the whole indicator.
    let repo = dirty_repo();
    seed_state(repo.path(), "feature", &["feature"]);

    let output = gitchange(repo.path(), &["switch", "unassigned"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "switched to 'unassigned'\n"
    );

    let output = gitchange(repo.path(), &["status"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        vec![
            "  feature",
            "* unassigned",
            "    ○ M tracked.txt 0/1",
            "    ○ ? untracked.txt 0/1",
        ],
        "the dirty tree stays out of 'feature': capture is off"
    );

    // And back: the changelist reclaims the marker and captures again.
    let output = gitchange(repo.path(), &["switch", "feature"]);
    assert_eq!(output.status.code(), Some(0));
    let output = gitchange(repo.path(), &["status"]);
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        vec![
            "* feature",
            "    ○ M tracked.txt 0/1",
            "    ○ ? untracked.txt 0/1",
        ]
    );
}

#[test]
fn status_groups_files_by_changelist_with_unassigned_last() {
    // A record claims tracked.txt for 'bugfix'; capture is off
    // (unassigned active, ADR 0015), so the untracked file stays loose
    // and the unassigned group renders last, wearing the marker.
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
fn a_stub_refuses_identically_inside_a_repo_and_does_no_repo_work() {
    // The stub contract (#140): no discovery, no lock, no state read — so
    // inside a repository the refusal is byte-for-byte the one `grammar.rs`
    // asserts from an empty directory, and a dirty tree that would capture
    // under a persisting refresh is left without a state file.
    let repo = dirty_repo();
    let output = gitchange(repo.path(), &["refresh"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, Vec::<u8>::new());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "gitchange: 'refresh' is not implemented yet\n"
    );
    assert!(
        !state_path(repo.path()).exists(),
        "the stub touched the repository"
    );
}

#[test]
fn a_hold_released_within_the_budget_never_surfaces() {
    // The routine concurrency the retry exists for (#122): a live TUI
    // holds the lock across one write, and the CLI absorbs it. The
    // holder is this test process, the only PID certain to be running.
    let repo = repo_holding(&format!("{}\n", std::process::id()));

    let child = Command::new(env!("CARGO_BIN_EXE_gitchange"))
        .current_dir(repo.path())
        .args(["switch", "bugfix"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run gitchange");
    // Long enough that the child is past its first contended attempt
    // even on a cold debug binary — that attempt is what makes the
    // release load-bearing rather than incidental. An overshoot on a
    // loaded runner is safe by a wide margin: the child's budget only
    // starts once it has discovered the repo and contended, so a release
    // near a second late still lands well inside it.
    std::thread::sleep(Duration::from_millis(700));
    std::fs::remove_file(lock_path(repo.path())).unwrap();

    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "switched to 'bugfix'\n"
    );
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "",
        "absorbed contention is not an error, so nothing is said about it"
    );
}

#[test]
fn a_live_hold_outlasting_the_budget_exits_3() {
    // Budget spent against a holder still running: transient, so the
    // answer is the retry code and never removal advice — following that
    // advice would break the live holder's own load-mutate-save cycle.
    let pid = std::process::id();
    let repo = repo_holding(&format!("{pid}\n"));
    let state_before = std::fs::read(state_path(repo.path())).unwrap();

    let output = gitchange(repo.path(), &["switch", "bugfix"]);

    assert_eq!(output.status.code(), Some(3));
    assert_eq!(
        output.stdout,
        Vec::<u8>::new(),
        "a failed command leaves stdout empty"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains(&pid.to_string()) && stderr.contains("run the same command again"),
        "the live holder is named and the retry is the same command: {stderr}"
    );
    assert!(
        !stderr.contains("remove"),
        "removing a live holder's lock breaks its session: {stderr}"
    );
    assert!(
        stderr.lines().all(|line| line.starts_with("gitchange: ")),
        "every diagnostic carries the prefix: {stderr}"
    );
    assert_eq!(
        std::fs::read(state_path(repo.path())).unwrap(),
        state_before,
        "the refused switch wrote nothing"
    );
    assert!(
        lock_path(repo.path()).exists(),
        "gitchange never breaks a lock it did not take"
    );
}

#[test]
fn an_unreadable_hold_outlasting_the_budget_exits_3() {
    // No PID to read — a hold whose write was cut short, or a lockfile
    // gitchange did not write. Assumed running (the fail-safe direction),
    // so it takes the live holder's exit code and its silence about
    // removal.
    let repo = repo_holding("");

    let output = gitchange(repo.path(), &["switch", "bugfix"]);

    assert_eq!(output.status.code(), Some(3));
    assert_eq!(output.stdout, Vec::<u8>::new());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        !stderr.contains("remove"),
        "assumed-alive means retry, never remove: {stderr}"
    );
    assert!(
        stderr.contains("run the same command again"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn a_dead_hold_outlasting_the_budget_exits_1_naming_the_lockfile() {
    // A holder proven gone will never release, so this is no longer
    // transient: an ordinary refusal, carrying the one resolution that is
    // accurate here. The file is named rather than removed — gitchange
    // never breaks a lock (ADR 0002).
    let repo = repo_holding(&format!("{}\n", reaped_pid()));

    let output = gitchange(repo.path(), &["switch", "bugfix"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, Vec::<u8>::new());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        // The file is matched by name, not by full path: git hands core a
        // forward-slashed git dir even on Windows, so the printed path
        // differs from a hand-spelled one by separator alone (#178).
        stderr.contains("remove") && stderr.contains("state.json.lock"),
        "the refusal names the file to remove: {stderr}"
    );
    assert!(
        !stderr.contains("run the same command again"),
        "retrying into a stale lock is not the resolution: {stderr}"
    );
    assert!(lock_path(repo.path()).exists());

    // The advice is genuinely the resolution.
    std::fs::remove_file(lock_path(repo.path())).unwrap();
    let output = gitchange(repo.path(), &["switch", "bugfix"]);
    assert_eq!(output.status.code(), Some(0));
}

#[test]
fn unknown_subcommand_exits_2() {
    let dir = tempfile::tempdir().unwrap();
    let output = gitchange(dir.path(), &["frobnicate"]);

    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn bare_invocation_outside_a_repo_exits_1() {
    // Discovery runs before the terminal guard (#110), so outside a
    // repository this message wins over the refusal — the diagnostic
    // names the condition the user can act on. The launched TUI itself
    // is smoke-tested in gitchange-tui.
    let dir = tempfile::tempdir().unwrap();
    let output = gitchange(dir.path(), &[]);

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("not a git repository"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn bare_invocation_without_a_terminal_refuses_before_the_tui() {
    // `Command::output()` gives the child pipes, not a terminal — the
    // condition the terminal guard refuses on. Inside a repo, so
    // discovery succeeds and the refusal is what speaks (#110).
    let repo = dirty_repo();
    let output = gitchange(repo.path(), &[]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        output.stdout,
        Vec::<u8>::new(),
        "a refusal renders nothing: not a frame, not an escape byte"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(
        stderr,
        "gitchange: the TUI needs a terminal on stdin and stdout; \
         run 'gitchange --help' for the command-line surface\n"
    );
}

#[test]
fn the_terminal_refusal_decides_nothing() {
    // The terminal guard runs before the Engine, so a refused run persists
    // nothing (ADR 0005): no capture, no records, no baseline stamp.
    // The fixture is the shape that would move most — a real changelist
    // active over a dirty tree, so a single persisting refresh would
    // claim both hunks and stamp the baseline.
    let repo = dirty_repo();
    seed_state(repo.path(), "feature", &["feature"]);
    let state = state_path(repo.path());
    let before = std::fs::read(&state).unwrap();

    let output = gitchange(repo.path(), &[]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        std::fs::read(&state).unwrap(),
        before,
        "the refused run rewrote the state file"
    );
}

#[test]
fn the_terminal_refusal_writes_no_state_file() {
    // The same property from the other side: a repo that has never run
    // gitchange still has none after the refusal.
    let repo = dirty_repo();
    let output = gitchange(repo.path(), &[]);

    assert_eq!(output.status.code(), Some(1));
    assert!(
        !state_path(repo.path()).exists(),
        "the refused run created a state file"
    );
}
