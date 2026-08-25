//! Everything that happens before a command's own work does, and what
//! happens when it never gets there: the global `-C` (#139), repository
//! discovery, the bare invocation's terminal guard (#110), an unknown
//! subcommand, and the stub contract's promise to touch no repository
//! (#140).
//!
//! Its own module because the subject is the invocation rather than any
//! one command — the same order of glue runs whichever verb follows, and
//! the tests below pin that order by watching which diagnostic wins.
//! `grammar.rs` asserts the same edges where no repository is involved;
//! here they are asserted with one to find, which is what makes the
//! precedence observable.

use crate::support::{
    CAPTURE_PENDING_HINT, TUI_NEEDS_A_TERMINAL, dirty_repo, elsewhere, gitchange, path_str,
    seed_state, state_path,
};

// --- discovery and the stub contract ---------------------------------------

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
fn unknown_subcommand_exits_2() {
    let dir = elsewhere();
    let output = gitchange(dir.path(), &["frobnicate"]);

    assert_eq!(output.status.code(), Some(2));
}

// --- the global -C ---------------------------------------------------------

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

// --- the bare invocation and its terminal guard ----------------------------

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
