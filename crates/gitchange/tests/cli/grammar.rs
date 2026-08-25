//! End-to-end tests of the clap tree (#142): every decided command parses,
//! unbuilt commands refuse as not-implemented (exit 1, empty stdout), and
//! each declarative usage rule dies as exit 2 before any repo work. Its own
//! module because nothing here needs a fixture repo: the grammar is
//! asserted from an empty directory, which is also what proves a stub does
//! no repo work.
//!
//! That empty directory is why the runner below is local rather than
//! `support::gitchange` — every case here wants a fresh nowhere to run in,
//! not a repo handed to it, so the tempdir is the helper's own business.

use std::process::{Command, Output};

fn gitchange(args: &[&str]) -> Output {
    let dir = tempfile::tempdir().unwrap();
    Command::new(env!("CARGO_BIN_EXE_gitchange"))
        .current_dir(dir.path())
        .args(args)
        .output()
        .expect("run gitchange")
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

/// The stub contract: valid usage of an unbuilt command exits 1, prints
/// nothing to stdout, and says so on stderr under the `gitchange:` prefix.
fn assert_stub(args: &[&str], command: &str) {
    let output = gitchange(args);
    assert_eq!(
        output.status.code(),
        Some(1),
        "{args:?}: {}",
        stderr(&output)
    );
    assert_eq!(stdout(&output), "", "{args:?} wrote to stdout");
    assert_eq!(
        stderr(&output),
        format!("gitchange: '{command}' is not implemented yet\n"),
        "{args:?}"
    );
}

/// A declarative usage error: clap's exit 2 with an empty stdout. The
/// prose is clap's and is not pinned.
fn assert_usage_error(args: &[&str]) {
    let output = gitchange(args);
    assert_eq!(
        output.status.code(),
        Some(2),
        "{args:?} should be a usage error: {}",
        stderr(&output)
    );
    assert_eq!(stdout(&output), "", "{args:?} wrote to stdout");
}

// --- status ----------------------------------------------------------------
// The grammar only; both faces are built (#156/#157), so what `status`
// says is asserted against fixture repos in `status.rs`.

#[test]
fn status_takes_no_arguments_and_json_takes_no_value() {
    // A changelist-shaped token is not a scope here — `status` is the
    // whole All view or nothing — and `--json` is a bare switch.
    assert_usage_error(&["status", "feature"]);
    assert_usage_error(&["status", "--json=yes"]);
}

// --- -C --------------------------------------------------------------------
// The grammar only; `-C`'s semantics — run as if launched in <dir> — are
// asserted against fixture repos in `invocation.rs`.

#[test]
fn dash_c_is_single_occurrence() {
    assert_usage_error(&["-C", "/tmp", "-C", "/tmp", "status"]);
}

#[test]
fn dash_c_needs_a_value() {
    assert_usage_error(&["-C"]);
}

// --- changelist ------------------------------------------------------------

#[test]
fn changelist_every_mode_is_a_stub() {
    assert_stub(&["changelist"], "changelist");
    assert_stub(&["changelist", "feature"], "changelist");
    assert_stub(&["changelist", "-d", "feature"], "changelist");
    assert_stub(
        &["changelist", "--delete", "feature", "bugfix"],
        "changelist",
    );
    assert_stub(&["changelist", "-d", "feature", "-f"], "changelist");
    assert_stub(&["changelist", "-d", "feature", "--force"], "changelist");
    assert_stub(&["changelist", "-D", "feature"], "changelist");
    // git's tolerance where it maps: `-f` beside `-D` is legal-redundant.
    assert_stub(&["changelist", "-D", "feature", "-f"], "changelist");
    assert_stub(&["changelist", "-m", "old", "new"], "changelist");
    assert_stub(&["changelist", "--move", "old", "new"], "changelist");
}

#[test]
fn changelist_create_conflicts_with_every_flag() {
    assert_usage_error(&["changelist", "feature", "-d", "bugfix"]);
    assert_usage_error(&["changelist", "feature", "-D", "bugfix"]);
    assert_usage_error(&["changelist", "feature", "-m", "old", "new"]);
    assert_usage_error(&["changelist", "feature", "-f"]);
}

#[test]
fn changelist_delete_flags_each_carry_their_own_list_and_do_not_combine() {
    assert_usage_error(&["changelist", "-d", "feature", "-D", "bugfix"]);
    assert_usage_error(&["changelist", "-d"]);
    assert_usage_error(&["changelist", "-D"]);
}

#[test]
fn changelist_force_requires_a_delete_mode() {
    assert_usage_error(&["changelist", "-f"]);
    assert_usage_error(&["changelist", "-m", "old", "new", "-f"]);
}

#[test]
fn changelist_move_takes_exactly_two_names_and_stands_alone() {
    // git's one-arg rename-the-active form dies as wrong arity.
    assert_usage_error(&["changelist", "-m", "new"]);
    assert_usage_error(&["changelist", "-m"]);
    assert_usage_error(&["changelist", "-m", "old", "new", "-d", "other"]);
    assert_usage_error(&["changelist", "-m", "old", "new", "-D", "other"]);
    assert_usage_error(&["changelist", "-m", "a", "b", "-m", "c", "d"]);
}

#[test]
fn changelist_has_no_machine_flags_and_no_alias() {
    assert_usage_error(&["changelist", "--json"]);
    assert_usage_error(&["changelist", "--format", "json"]);
    assert_usage_error(&["cl"]);
}

// --- assign ----------------------------------------------------------------

#[test]
fn assign_every_form_is_a_stub() {
    assert_stub(&["assign", "src/a.rs", "--to", "feature"], "assign");
    assert_stub(
        &["assign", "src/a.rs", "src/b.rs", "--to", "feature"],
        "assign",
    );
    assert_stub(
        &["assign", "src/a.rs:h1a2b3c4", "--to", "feature"],
        "assign",
    );
    assert_stub(&["assign", "src/a.rs", "--unassign"], "assign");
    assert_stub(
        &[
            "assign",
            "src/a.rs",
            "--containing",
            "let x",
            "--to",
            "feature",
        ],
        "assign",
    );
    assert_stub(
        &["assign", "src/a.rs", "--take-owned", "--to", "feature"],
        "assign",
    );
}

#[test]
fn assign_containing_accepts_a_leading_hyphen() {
    // A changed line may begin with `-`.
    assert_stub(
        &[
            "assign",
            "src/a.rs",
            "--containing",
            "-1",
            "--to",
            "feature",
        ],
        "assign",
    );
    assert_stub(
        &[
            "assign",
            "src/a.rs",
            "--containing",
            "--foo",
            "--to",
            "feature",
        ],
        "assign",
    );
}

#[test]
fn assign_needs_at_least_one_path() {
    assert_usage_error(&["assign", "--to", "feature"]);
    assert_usage_error(&["assign", "--unassign"]);
}

#[test]
fn assign_target_is_required_and_exclusive() {
    assert_usage_error(&["assign", "src/a.rs"]);
    assert_usage_error(&["assign", "src/a.rs", "--to", "feature", "--unassign"]);
    assert_usage_error(&["assign", "src/a.rs", "--to"]);
}

#[test]
fn assign_containing_is_single_occurrence_and_non_empty() {
    assert_usage_error(&[
        "assign",
        "src/a.rs",
        "--containing",
        "a",
        "--containing",
        "b",
        "--to",
        "feature",
    ]);
    assert_usage_error(&["assign", "src/a.rs", "--containing", "", "--to", "feature"]);
    assert_usage_error(&["assign", "src/a.rs", "--containing"]);
}

#[test]
fn a_hyphen_named_path_is_a_usage_error() {
    // Positional paths take no leading hyphen (#140): a typo'd flag dying
    // loudly is worth more than addressing a hyphen-named file.
    assert_usage_error(&["assign", "-foo", "--to", "feature"]);
    assert_usage_error(&["add", "feature", "-foo"]);
}

// --- add / stage / unstage -------------------------------------------------

#[test]
fn add_every_form_is_a_stub() {
    assert_stub(&["add", "feature"], "add");
    assert_stub(&["add", "feature", "src/a.rs"], "add");
    assert_stub(&["add", "feature", "src/a.rs:h1a2b3c4", "src/b.rs"], "add");
    assert_stub(&["add", "feature", "src/a.rs", "--containing", "-x"], "add");
}

#[test]
fn stage_is_add() {
    // The alias is vocabulary (git's staging verb); the stub text naming
    // `add` proves the routing.
    assert_stub(&["stage", "feature"], "add");
    assert_stub(&["stage", "feature", "src/a.rs"], "add");
}

#[test]
fn unstage_mirrors_add_without_the_alias() {
    assert_stub(&["unstage", "feature"], "unstage");
    assert_stub(&["unstage", "feature", "src/a.rs"], "unstage");
    assert_stub(
        &["unstage", "feature", "src/a.rs", "--containing", "-x"],
        "unstage",
    );
}

#[test]
fn staging_verbs_need_a_changelist() {
    assert_usage_error(&["add"]);
    assert_usage_error(&["stage"]);
    assert_usage_error(&["unstage"]);
}

#[test]
fn staging_verbs_containing_is_single_occurrence_and_non_empty() {
    assert_usage_error(&[
        "add",
        "feature",
        "src/a.rs",
        "--containing",
        "a",
        "--containing",
        "b",
    ]);
    assert_usage_error(&["add", "feature", "src/a.rs", "--containing", ""]);
    assert_usage_error(&[
        "unstage",
        "feature",
        "src/a.rs",
        "--containing",
        "a",
        "--containing",
        "b",
    ]);
    assert_usage_error(&["unstage", "feature", "src/a.rs", "--containing", ""]);
}

// --- commit ----------------------------------------------------------------

#[test]
fn commit_every_form_is_a_stub() {
    assert_stub(&["commit", "feature", "-m", "msg"], "commit");
    assert_stub(&["commit", "feature", "--message", "msg"], "commit");
    // Repeated `-m` is one source; a leading-hyphen message is git's
    // optarg behaviour, arriving with the borrowed grammar.
    assert_stub(&["commit", "feature", "-m", "one", "-m", "-two"], "commit");
    assert_stub(&["commit", "feature", "-F", "msg.txt"], "commit");
    assert_stub(&["commit", "feature", "--file", "-"], "commit");
    assert_stub(&["commit", "feature", "--amend", "--no-edit"], "commit");
    assert_stub(
        &[
            "commit",
            "feature",
            "-m",
            "msg",
            "--amend",
            "-n",
            "--allow-unassigned",
            "--allow-staged-stale",
            "--allow-foreign-head",
        ],
        "commit",
    );
    assert_stub(&["commit", "feature", "-m", "msg", "--no-verify"], "commit");
}

#[test]
fn commit_needs_a_changelist() {
    assert_usage_error(&["commit", "-m", "msg"]);
}

#[test]
fn commit_needs_exactly_one_message_source() {
    // There is no editor, so no default exists to fall back on.
    assert_usage_error(&["commit", "feature"]);
    assert_usage_error(&["commit", "feature", "--amend"]);
    assert_usage_error(&["commit", "feature", "-m", "msg", "-F", "msg.txt"]);
    assert_usage_error(&["commit", "feature", "-m", "msg", "--amend", "--no-edit"]);
    assert_usage_error(&["commit", "feature", "-F", "msg.txt", "--amend", "--no-edit"]);
    assert_usage_error(&["commit", "feature", "-m"]);
    assert_usage_error(&["commit", "feature", "-F"]);
}

#[test]
fn commit_file_takes_one_value() {
    assert_usage_error(&["commit", "feature", "-F", "a.txt", "-F", "b.txt"]);
}

#[test]
fn commit_no_edit_requires_amend() {
    assert_usage_error(&["commit", "feature", "--no-edit"]);
}

// --- diff ------------------------------------------------------------------

#[test]
fn diff_every_form_parses_and_reaches_the_repository() {
    // Both faces are built (#158/#159), so from an empty directory each
    // form gets as far as discovery and refuses there — which is what
    // proves the shape parsed rather than dying as usage. What each scope
    // selects, and what either face says, is asserted against fixture
    // repos in `diff.rs`.
    for args in [
        &["diff"][..],
        &["diff", "feature"],
        &["diff", "feature", "src/a.rs", "src/b.rs:h1a2b3c4"],
        &["diff", "--", "src/a.rs"],
        &["diff", "feature", "--", "src/a.rs", "src/b.rs"],
        &["diff", "feature", "src/a.rs", "--", "src/b.rs"],
        &["diff", "--json"],
        &["diff", "feature", "--json", "--no-content"],
    ] {
        let output = gitchange(args);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert_eq!(stdout(&output), "", "{args:?}");
        assert!(
            stderr(&output).contains("not a git repository"),
            "{args:?}: {}",
            stderr(&output)
        );
    }
}

#[test]
fn diff_no_content_requires_json() {
    assert_usage_error(&["diff", "--no-content"]);
    assert_usage_error(&["diff", "feature", "--no-content"]);
}

// --- restore ---------------------------------------------------------------

#[test]
fn restore_staged_corrects_to_unstage() {
    for args in [
        &["restore", "--staged", "src/a.rs"][..],
        &["restore", "--staged"],
        &["restore", "--staged", "--", "src/a.rs"],
        // git accepts the flag after the path too, so both orders correct.
        &["restore", "src/a.rs", "--staged"],
    ] {
        let output = gitchange(args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert_eq!(stdout(&output), "", "{args:?}");
        let stderr = stderr(&output);
        assert!(stderr.contains("unstage"), "{args:?}: {stderr}");
    }
}

#[test]
fn bare_restore_says_git_restore_stays_valid() {
    for args in [
        &["restore"][..],
        &["restore", "src/a.rs"],
        &["restore", "--source=HEAD~1", "src/a.rs"],
    ] {
        let output = gitchange(args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert_eq!(stdout(&output), "", "{args:?}");
        let stderr = stderr(&output);
        assert!(stderr.contains("git restore"), "{args:?}: {stderr}");
        assert!(!stderr.contains("unstage"), "{args:?}: {stderr}");
    }
}

// --- help ------------------------------------------------------------------

#[test]
fn top_level_help_teaches_the_tree() {
    let output = gitchange(&["--help"]);
    assert_eq!(output.status.code(), Some(0));
    let help = stdout(&output);

    // Commands in the reference order. Each is matched at line start
    // (two-space indent) so a mention inside a description cannot satisfy
    // the check.
    let commands = [
        "status",
        "switch",
        "refresh",
        "changelist",
        "assign",
        "add",
        "unstage",
        "commit",
        "diff",
    ];
    let mut positions = Vec::new();
    for command in commands {
        let needle = format!("\n  {command} ");
        let at = help.find(&needle);
        assert!(at.is_some(), "{command} missing from help:\n{help}");
        positions.push(at.unwrap());
    }
    assert!(
        positions.windows(2).all(|w| w[0] < w[1]),
        "commands out of reference order:\n{help}"
    );

    assert!(!help.contains("restore"), "restore is hidden:\n{help}");
    // The alias is asserted on `add`'s own line, since `unstage`'s line
    // would satisfy a whole-page search for "stage".
    let add_line = help
        .lines()
        .find(|line| line.starts_with("  add "))
        .expect("add's line");
    assert!(
        add_line.contains("stage"),
        "add's alias is visible: {add_line}"
    );
    assert!(help.contains("-C <dir>"), "-C is listed:\n{help}");
}

#[test]
fn every_command_help_lists_dash_c_and_exits_0() {
    for command in [
        "status",
        "switch",
        "refresh",
        "changelist",
        "assign",
        "add",
        "unstage",
        "commit",
        "diff",
    ] {
        let output = gitchange(&[command, "--help"]);
        assert_eq!(output.status.code(), Some(0), "{command} --help");
        let help = stdout(&output);
        assert!(
            help.contains("-C <dir>"),
            "{command} --help omits -C:\n{help}"
        );
    }
}

#[test]
fn version_exits_0() {
    let output = gitchange(&["--version"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(stdout(&output).starts_with("gitchange "));
}

// --- refresh ---------------------------------------------------------------

#[test]
fn refresh_is_a_stub() {
    assert_stub(&["refresh"], "refresh");
}

#[test]
fn refresh_takes_no_arguments() {
    assert_usage_error(&["refresh", "now"]);
    assert_usage_error(&["refresh", "--json"]);
}
