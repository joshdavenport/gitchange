//! End-to-end tests of the binary: output, the 0/1/2/3 exit-code contract
//! (ticket 13, grown to the full published scheme by #136), and the global
//! `-C` asserted from a foreign cwd (#139). Fixtures are built with the
//! git CLI so git2 stays a gitchange-core-only dependency (ADR 0006).

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
fn git_command(dir: &Path) -> Command {
    let absent = dir.join(".git/absent-config");
    let mut command = Command::new("git");
    command
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", &absent)
        .env("GIT_CONFIG_SYSTEM", &absent);
    command
}

/// A git command that must succeed, its trimmed stdout.
fn git(dir: &Path, args: &[&str]) -> String {
    let output = git_command(dir).args(args).output().expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

/// An initialised repo with a committing identity and one committed
/// file, worktree clean — the stem the fixtures below grow from.
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

/// A repo stopped mid-merge with `tracked.txt` unmerged (ADR 0007): each
/// side commits its own edit to the same line, so the merge conflicts.
fn merging_repo() -> tempfile::TempDir {
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
fn unborn_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "--initial-branch=main"]);
    std::fs::write(dir.path().join("untracked.txt"), "hello\n").unwrap();
    dir
}

/// A file long enough that an edit at each end is two hunks, not one:
/// three lines of context each side never meet in the middle.
fn long_file(first: &str, last: &str) -> String {
    let mut lines: Vec<String> = (1..=12).map(|line| format!("line {line}\n")).collect();
    lines[0] = format!("{first}\n");
    lines[11] = format!("{last}\n");
    lines.concat()
}

/// A repo whose `long.txt` carries two hunks — an edit at each end of a
/// long file — committed clean and then edited.
fn two_hunk_repo() -> tempfile::TempDir {
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

/// A directory that is not a repository and holds nothing: the cwd for a
/// run that must find no repository, and the foreign cwd the `-C` tests
/// launch from — so that anything those commands find, they can only have
/// found through `-C`.
fn elsewhere() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn path_str(path: &Path) -> &str {
    path.to_str().expect("tempdir paths are UTF-8")
}

/// The terminal guard's refusal (#110), pinned once: the bare invocation
/// reaching it is how a test proves discovery succeeded.
const TUI_NEEDS_A_TERMINAL: &str = "gitchange: the TUI needs a terminal on stdin and stdout; \
                                    run 'gitchange --help' for the command-line surface\n";

/// The capture-pending hint (#156), as the unassigned group carries it:
/// the line a read prints when a real changelist is active and unassigned
/// holds hunks, indented to the file rows it speaks for. Pinned exactly
/// because what it omits is the point: it names the mechanism — the next
/// persisting refresh claims — and never where (#122 §Forecasts), so a
/// destination interpolated into it later fails every assertion below.
const CAPTURE_PENDING_HINT: &str = "    capture on: run 'gitchange refresh' to claim these, \
                                    or they're claimed at your next action";

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

    // A separate invocation sees the persisted marker. The dirty files
    // stay unassigned: a read claims nothing (#156), so the newly active
    // changelist is where capture *will* flow, not where it has.
    let output = gitchange(repo.path(), &["status"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        vec![
            "  feature",
            "* bugfix",
            "  unassigned",
            CAPTURE_PENDING_HINT,
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
        "capture is off: the dirty tree is unassigned with nothing pending, so no hint"
    );

    // And back: the changelist reclaims the marker, and capture is on
    // again — pending, since the read that shows it never performs it.
    let output = gitchange(repo.path(), &["switch", "feature"]);
    assert_eq!(output.status.code(), Some(0));
    let output = gitchange(repo.path(), &["status"]);
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        vec![
            "* feature",
            "  unassigned",
            CAPTURE_PENDING_HINT,
            "    ○ M tracked.txt 0/1",
            "    ○ ? untracked.txt 0/1",
        ]
    );
}

#[test]
fn status_writes_nothing_and_a_recordless_hunk_reports_unassigned() {
    // The read-only refresh at the binary seam (#156; core's own form is
    // tested in gitchange-core). The fixture is the shape a persisting
    // refresh would move most — a real changelist active over a dirty
    // tree, which it would claim both hunks of and stamp the baseline
    // for. Read twice, so the second run proves the first wrote nothing.
    let repo = dirty_repo();
    seed_state(repo.path(), "feature", &["feature"]);
    let before = std::fs::read(state_path(repo.path())).unwrap();

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
            std::fs::read(state_path(repo.path())).unwrap(),
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
    let before = std::fs::read(state_path(repo.path())).unwrap();

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
        std::fs::read(state_path(repo.path())).unwrap(),
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

    let before = std::fs::read(state_path(repo)).unwrap();

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
        std::fs::read(state_path(repo)).unwrap(),
        before,
        "no baseline stamp, no dormancy write"
    );
}

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
    let before = std::fs::read(state_path(repo.path())).unwrap();

    for _ in 0..2 {
        // `status_json` asserts exit 0 and the empty stderr — no notice
        // line on a read, ever.
        let envelope = status_json(repo.path());
        assert_eq!(envelope["active"], serde_json::json!("feature"));
        assert_eq!(
            std::fs::read(state_path(repo.path())).unwrap(),
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

#[test]
fn status_outside_a_repo_exits_1_with_message_on_stderr() {
    // Both faces: `--json` refuses through the same operational path, and
    // never answers with a JSON error object — one schema to version, so
    // errors stay plain text on stderr (ADR 0018).
    let dir = elsewhere();
    for args in [&["status"][..], &["status", "--json"]] {
        let output = gitchange(dir.path(), args);

        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert_eq!(output.stdout, Vec::<u8>::new(), "{args:?}");
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(
            stderr.contains("not a git repository"),
            "unexpected stderr: {stderr}"
        );
    }
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
fn dash_c_runs_the_command_as_if_launched_there() {
    // Git's `-C` (#122/#139): repo discovery starts in <dir>, whatever
    // the cwd. A mutation proves the write lands in <dir>'s repository;
    // the read that follows, with `-C` in the other position clap allows a
    // global, proves the same repository is what it saw.
    let repo = dirty_repo();
    seed_state(repo.path(), "feature", &["feature", "bugfix"]);
    let cwd = elsewhere();

    let output = gitchange(
        cwd.path(),
        &["-C", path_str(repo.path()), "switch", "bugfix"],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "switched to 'bugfix'\n"
    );

    let output = gitchange(cwd.path(), &["status", "-C", path_str(repo.path())]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        stdout.lines().collect::<Vec<&str>>(),
        vec![
            "  feature",
            "* bugfix",
            "  unassigned",
            CAPTURE_PENDING_HINT,
            "    ○ M tracked.txt 0/1",
            "    ○ ? untracked.txt 0/1",
        ]
    );
}

#[test]
fn dash_c_resolves_against_the_callers_cwd() {
    // A relative <dir> is relative to where the caller stands, as git's
    // is: launched from the repository's parent, its bare name reaches it.
    let repo = dirty_repo();
    let parent = repo.path().parent().unwrap();
    let name = repo.path().file_name().unwrap().to_str().unwrap();

    let output = gitchange(parent, &["-C", name, "status"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("tracked.txt"),
        "the relative -C reached the repository"
    );
}

#[test]
fn dash_c_aims_the_bare_invocation_at_that_worktree() {
    // From a non-repository cwd the bare invocation fails on discovery;
    // with `-C` it gets as far as the terminal guard, which is the TUI
    // having found the repository it was aimed at (#110 fixes the order:
    // discovery first, then the guard).
    let repo = dirty_repo();
    let cwd = elsewhere();

    let output = gitchange(cwd.path(), &[]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("not a git repository")
    );

    let output = gitchange(cwd.path(), &["-C", path_str(repo.path())]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        TUI_NEEDS_A_TERMINAL
    );
}

#[test]
fn dash_c_to_a_nonexistent_dir_exits_1_naming_it() {
    // There is no such place to run from, so this is an operational
    // refusal (exit 1): stdout empty, the dir named on stderr. A file is
    // no more a place to run from than nothing is. The refusal runs before
    // the command does — a stub that would otherwise refuse as
    // not-implemented never gets the chance — but after the parse: a usage
    // error is still clap's exit 2, whatever `-C` names.
    let cwd = elsewhere();
    let missing = cwd.path().join("nowhere");
    let missing = path_str(&missing);
    let file = cwd.path().join("a-file");
    std::fs::write(&file, "").unwrap();
    let file = path_str(&file);

    for (dir, command) in [(missing, "status"), (missing, "refresh"), (file, "status")] {
        let args = &["-C", dir, command];
        let output = gitchange(cwd.path(), args);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert_eq!(output.stdout, Vec::<u8>::new(), "{args:?}");
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(
            stderr.starts_with("gitchange: cannot change to ") && stderr.contains(dir),
            "{args:?}: {stderr}"
        );
        assert!(
            !stderr.contains("not implemented"),
            "{args:?}: the dir refused before the command ran: {stderr}"
        );
    }

    for args in [&["-C", missing, "switch"][..], &["-C", missing, "restore"]] {
        let output = gitchange(cwd.path(), args);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{args:?}: a usage error precedes the dir: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn unknown_subcommand_exits_2() {
    let dir = elsewhere();
    let output = gitchange(dir.path(), &["frobnicate"]);

    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn bare_invocation_outside_a_repo_exits_1() {
    // Discovery runs before the terminal guard (#110), so outside a
    // repository this message wins over the refusal — the diagnostic
    // names the condition the user can act on. The launched TUI itself
    // is smoke-tested in gitchange-tui.
    let dir = elsewhere();
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
    assert_eq!(stderr, TUI_NEEDS_A_TERMINAL);
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
