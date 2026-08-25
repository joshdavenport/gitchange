//! The binary suite's fixture shim — the one surface every module of this
//! test binary shares (ADR 0008): the process runner, the git CLI wrapper
//! the fixtures are built with, the fixture builders themselves, the state
//! seeding those fixtures need, and the output constants pinned once.
//!
//! Fixtures are built with the git CLI rather than `gitchange-test-support`
//! so git2 stays a gitchange-core-only dependency (ADR 0006). This crate's
//! tests only ever see the binary boundary, so a repo they can build with
//! `git` is a repo they can build without linking libgit2 at all.
//!
//! A helper only one module calls stays in that module — what lives here is
//! what two or more of them share.

use std::path::Path;
use std::process::{Command, Output, Stdio};

/// Run the binary in `dir`, capturing its output. Every assertion in this
/// suite goes through this seam: a real process, real argv, real exit code.
pub fn gitchange(dir: &Path, args: &[&str]) -> Output {
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
/// the fixtures' commits below fail on their machine and never in CI.
///
/// The cut-out is used instead of pinning individual knobs because it is
/// strictly stronger, and because it needs nothing from
/// `gitchange-test-support` — these fixtures stay on the git CLI so git2
/// is no direct dependency of this crate (ADR 0006).
pub fn git_command(dir: &Path) -> Command {
    let absent = dir.join(".git/absent-config");
    let mut command = Command::new("git");
    command
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", &absent)
        .env("GIT_CONFIG_SYSTEM", &absent);
    command
}

/// A git command that must succeed, its trimmed stdout.
pub fn git(dir: &Path, args: &[&str]) -> String {
    let output = git_command(dir).args(args).output().expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

/// An initialised repo with a committing identity and no commits — the
/// stem `committed_repo` grows from, and the one the `diff` fixtures start
/// from directly so they can choose their own first commit's contents.
pub fn initialised_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    // Pin the initial branch for the same reason `unborn_repo` does: plain
    // `init` takes it from `init.defaultBranch`, which the cut-out above
    // now removes rather than inherits.
    git(dir.path(), &["init", "-q", "--initial-branch=main"]);
    git(dir.path(), &["config", "user.name", "gitchange-tests"]);
    git(
        dir.path(),
        &["config", "user.email", "tests@gitchange.invalid"],
    );
    dir
}

/// An initialised repo with one committed file, worktree clean — the stem
/// the fixtures below grow from.
pub fn committed_repo() -> tempfile::TempDir {
    let dir = initialised_repo();
    std::fs::write(dir.path().join("tracked.txt"), "one\n").unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-q", "--no-verify", "-m", "init"]);
    dir
}

pub fn dirty_repo() -> tempfile::TempDir {
    let dir = committed_repo();
    std::fs::write(dir.path().join("tracked.txt"), "two\n").unwrap();
    std::fs::write(dir.path().join("untracked.txt"), "hello\n").unwrap();
    dir
}

/// A repo stopped mid-merge with `tracked.txt` unmerged (ADR 0007): each
/// side commits its own edit to the same line, so the merge conflicts.
pub fn merging_repo() -> tempfile::TempDir {
    let dir = committed_repo();
    let repo = dir.path();
    git(repo, &["checkout", "-q", "-b", "feature"]);
    std::fs::write(repo.join("tracked.txt"), "feature side\n").unwrap();
    git(
        repo,
        &["commit", "-q", "--no-verify", "-am", "feature edit"],
    );
    git(repo, &["checkout", "-q", "main"]);
    std::fs::write(repo.join("tracked.txt"), "main side\n").unwrap();
    git(repo, &["commit", "-q", "--no-verify", "-am", "main edit"]);
    let merge = git_command(repo)
        .args(["merge", "--no-edit", "feature"])
        .output()
        .expect("run git");
    assert!(!merge.status.success(), "the merge is meant to conflict");
    dir
}

/// A repo with no commits at all — a fresh `git init` with one untracked
/// file. HEAD names a branch that does not exist yet, and the reads still
/// work (#157).
pub fn unborn_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "--initial-branch=main"]);
    std::fs::write(dir.path().join("untracked.txt"), "hello\n").unwrap();
    dir
}

/// A file long enough that an edit at each end is two hunks, not one:
/// three lines of context each side never meet in the middle.
pub fn long_file(first: &str, last: &str) -> String {
    let mut lines: Vec<String> = (1..=12).map(|line| format!("line {line}\n")).collect();
    lines[0] = format!("{first}\n");
    lines[11] = format!("{last}\n");
    lines.concat()
}

/// A repo whose `long.txt` carries two hunks — an edit at each end of a
/// long file — committed clean and then edited.
pub fn two_hunk_repo() -> tempfile::TempDir {
    let dir = committed_repo();
    std::fs::write(dir.path().join("long.txt"), long_file("first", "last")).unwrap();
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-q", "--no-verify", "-m", "long"]);
    std::fs::write(
        dir.path().join("long.txt"),
        long_file("first edited", "last edited"),
    )
    .unwrap();
    dir
}

/// Write a file under `dir`, creating its parent directories — the
/// fixtures' own edit, for the subdirectory cases `std::fs::write` alone
/// cannot reach.
pub fn write(dir: &Path, path: &str, contents: &str) {
    let file = dir.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, contents).unwrap();
}

/// The fixture the staging verbs sweep, capture off (`active: null`) so
/// ownership is exactly what the records say. Its worktree is edited and
/// its index untouched; `unstage`'s own fixture stages it with raw `git
/// add` (ADR 0003 — refresh absorbs that), which is how one shape serves
/// both directions:
///
/// - `a.txt` — two hunks, the first claimed by `'feature'` by record, the
///   second recordless and so unassigned: one file, two owners, which is
///   what makes path narrowing observable.
/// - `b.txt` — one hunk, claimed by `'docs'`.
/// - `sub/c.txt` — one hunk, unassigned; also the subdirectory a directory
///   argument needs.
/// - `keep.txt` — committed and untouched: the clean-path offender.
/// - `'empty'` — a changelist owning no hunks at all.
pub fn owned_repo() -> tempfile::TempDir {
    let dir = initialised_repo();
    let repo = dir.path();
    write(repo, "a.txt", &long_file("first", "last"));
    write(repo, "b.txt", "one\n");
    write(repo, "sub/c.txt", "one\n");
    write(repo, "keep.txt", "unchanged\n");
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-q", "--no-verify", "-m", "init"]);
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

/// One hunk's composed address, read off the read surface — `diff`'s
/// annotation, which is where a caller gets one (#155). `index` counts the
/// file's hunks in patch order from `0`.
///
/// Minted through the binary rather than computed here on purpose: an
/// address is only useful if the string a read prints is the string a verb
/// takes, so every addressed test pastes `diff`'s own output back.
pub fn address(dir: &Path, path: &str, index: usize) -> String {
    let output = gitchange(dir, &["diff", path]);
    let diff = String::from_utf8(output.stdout).unwrap();
    let marker = format!("{path}:h");
    diff.match_indices(&marker)
        .map(|(at, _)| {
            diff[at..]
                .split(|character: char| character.is_whitespace() || character == ']')
                .next()
                .expect("an address is one word")
                .to_owned()
        })
        .nth(index)
        .unwrap_or_else(|| panic!("no hunk {index} of '{path}' in:\n{diff}"))
}

/// The paths the index holds a change for, in git's order — the ground
/// truth for what a sweep reached.
pub fn staged_paths(dir: &Path) -> Vec<String> {
    let staged = git(dir, &["diff", "--cached", "--name-only"]);
    staged.lines().map(str::to_owned).collect()
}

/// The index's content for one path, as git resolves it.
pub fn staged(dir: &Path, path: &str) -> String {
    git(dir, &["show", &format!(":{path}")])
}

/// A directory that is not a repository and holds nothing: the cwd for a
/// run that must find no repository, and the foreign cwd the `-C` tests
/// launch from — so that anything those commands find, they can only have
/// found through `-C`.
pub fn elsewhere() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

pub fn path_str(path: &Path) -> &str {
    path.to_str().expect("tempdir paths are UTF-8")
}

/// The terminal guard's refusal (#110), pinned once: the bare invocation
/// reaching it is how a test proves discovery succeeded.
pub const TUI_NEEDS_A_TERMINAL: &str = "gitchange: the TUI needs a terminal on stdin and stdout; \
                                        run 'gitchange --help' for the command-line surface\n";

/// The capture-pending hint (#156), as the unassigned group carries it:
/// the line a read prints when a real changelist is active and unassigned
/// holds hunks, indented to the file rows it speaks for. Pinned exactly
/// because what it omits is the point: it names the mechanism — the next
/// persisting refresh claims — and never where (#122 §Forecasts), so a
/// destination interpolated into it later fails every assertion below.
pub const CAPTURE_PENDING_HINT: &str = "    capture on: run 'gitchange refresh' to claim these, \
                                        or they're claimed at your next action";

/// This worktree's state file, spelled once. Tests that seed it and
/// tests that assert on it read the same path.
pub fn state_path(dir: &Path) -> std::path::PathBuf {
    dir.join(".git/gitchange/state.json")
}

/// Seed a state file directly: a fixture arrives with its changelists
/// and its marker already placed, rather than paying an invocation per
/// changelist through `changelist <name>` and a `switch` (#166) — and a
/// fixture built that way would assert the verbs under test with the
/// verbs under test. Doubles as a schema-v1 stability check: this JSON
/// must stay readable.
pub fn seed_state(dir: &Path, active: &str, names: &[&str]) {
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
pub fn seed_state_raw(dir: &Path, json: &str) {
    let path = state_path(dir);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, json).unwrap();
}

/// This worktree's state lockfile, beside the state file — the path core
/// takes and the one its dead-holder refusal names.
pub fn lock_path(dir: &Path) -> std::path::PathBuf {
    state_path(dir).with_file_name("state.json.lock")
}

/// A dirty repo whose lockfile is held by another writer, `switch bugfix`
/// waiting to contend with it. `contents` is the recorded PID (#135),
/// which decides whether the hold reads as live, dead, or unreadable. The
/// state file is seeded first, so this is contention over an existing
/// state rather than a first write.
pub fn repo_holding(contents: &str) -> tempfile::TempDir {
    let repo = dirty_repo();
    seed_state(repo.path(), "feature", &["feature", "bugfix"]);
    std::fs::write(lock_path(repo.path()), contents).unwrap();
    repo
}

/// The PID of a process that has exited and been reaped — one the OS
/// answers "gone" for, so the hold classifies as dead. (Recycling could in
/// principle hand the number out again before the binary reads it; nothing
/// portable rules that out, and the window is microseconds.)
pub fn reaped_pid() -> u32 {
    let mut child = Command::new("git")
        .arg("--version")
        .stdout(Stdio::null())
        .spawn()
        .expect("git is on PATH for this suite");
    let pid = child.id();
    child.wait().unwrap();
    pid
}
