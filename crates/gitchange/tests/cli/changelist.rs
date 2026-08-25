//! `changelist` at the binary seam (#149/#166): the bare listing and
//! create. Its own module because the noun command's subject is the
//! changelist roster rather than the change universe — every assertion
//! here reads the listing, which is both the feature and the way a
//! creation is observed.
//!
//! The listing is the one read that touches no diff at all (a pure state
//! read), so its write-nothing property is asserted against the state
//! file's bytes here, as `status`'s is in `status.rs`.

use crate::support::{
    committed_repo, dirty_repo, git, gitchange, repo_holding, seed_state, seed_state_raw,
    state_path,
};

/// The listing's lines, with the exit code asserted along the way — a
/// listing that refused is never a listing.
fn listing(dir: &std::path::Path) -> Vec<String> {
    let output = gitchange(dir, &["changelist"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "listing refused: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn a_repo_with_no_changelists_lists_unassigned_alone() {
    // Never empty (#149): unassigned holds the marker by default, and
    // the marker is always on something.
    let repo = committed_repo();

    assert_eq!(listing(repo.path()), vec!["* unassigned"]);
    assert!(
        !state_path(repo.path()).exists(),
        "a listing writes nothing, so it does not mint a state file"
    );
}

#[test]
fn the_listing_is_user_order_with_the_marker_on_the_active_changelist() {
    // Creation-append order (#122), the `*` where `switch` put it, and
    // no `unassigned` line: it is not a changelist, and it does not hold
    // the marker here.
    let repo = dirty_repo();
    seed_state(repo.path(), "bugfix", &["feature", "bugfix", "docs"]);

    assert_eq!(
        listing(repo.path()),
        vec!["  feature", "* bugfix", "  docs"]
    );
}

#[test]
fn unassigned_lists_last_and_only_while_it_holds_the_marker() {
    let repo = dirty_repo();
    seed_state_raw(
        repo.path(),
        r#"{ "version": 1, "active": null,
             "changelists": [{ "name": "feature" }, { "name": "docs" }] }"#,
    );

    assert_eq!(
        listing(repo.path()),
        vec!["  feature", "  docs", "* unassigned"]
    );

    // The marker moves; the line it stood on goes with it.
    assert_eq!(
        gitchange(repo.path(), &["switch", "docs"]).status.code(),
        Some(0)
    );
    assert_eq!(listing(repo.path()), vec!["  feature", "* docs"]);
}

#[test]
fn the_listing_leaves_the_state_file_byte_identical() {
    // Read-only per #122's taxonomy — and a pure state read at that: no
    // capture, no record rewrite, not even a baseline stamp.
    let repo = dirty_repo();
    seed_state(repo.path(), "feature", &["feature"]);
    let before = std::fs::read(state_path(repo.path())).unwrap();

    listing(repo.path());

    assert_eq!(std::fs::read(state_path(repo.path())).unwrap(), before);
}

#[test]
fn the_listing_answers_where_a_refresh_would_refuse() {
    // The one place the two read mechanisms differ observably, and so
    // the pin on ADR 0005's "runs neither form": a bare repository has
    // no working directory to scan, so every command that builds the
    // change universe refuses — while the roster, which derives from no
    // diff, is still there to read.
    let dir = tempfile::tempdir().unwrap();
    git(
        dir.path(),
        &["init", "-q", "--bare", "--initial-branch=main"],
    );
    let state = dir.path().join("gitchange/state.json");
    std::fs::create_dir_all(state.parent().unwrap()).unwrap();
    std::fs::write(
        &state,
        r#"{ "version": 1, "active": "feature",
             "changelists": [{ "name": "feature" }, { "name": "docs" }] }"#,
    )
    .unwrap();

    assert_eq!(
        gitchange(dir.path(), &["status"]).status.code(),
        Some(1),
        "status builds the universe, which a bare repo has none of"
    );
    assert_eq!(listing(dir.path()), vec!["* feature", "  docs"]);
}

#[test]
fn the_listing_takes_no_lock() {
    // The other half of read-only (#122): a live writer holding the
    // lockfile — a running TUI mid-write — never delays or refuses a
    // glance at the roster. The holder is this test process, the one
    // PID certain to be running.
    let repo = repo_holding(&format!("{}\n", std::process::id()));

    assert_eq!(listing(repo.path()), vec!["* feature", "  bugfix"]);
}

#[test]
fn create_appends_to_user_order_and_leaves_the_marker_alone() {
    // ADR 0015: only `switch` moves the marker, so a preparatory create
    // can never redirect capture mid-flow.
    let repo = dirty_repo();
    seed_state(repo.path(), "feature", &["feature", "docs"]);

    let output = gitchange(repo.path(), &["changelist", "bugfix"]);

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("created changelist 'bugfix'"),
        "unexpected stdout: {stdout}"
    );
    assert_eq!(
        listing(repo.path()),
        vec!["* feature", "  docs", "  bugfix"]
    );
}

#[test]
fn create_in_a_repo_with_no_state_leaves_capture_off() {
    let repo = committed_repo();

    assert_eq!(
        gitchange(repo.path(), &["changelist", "feature"])
            .status
            .code(),
        Some(0)
    );

    assert_eq!(listing(repo.path()), vec!["  feature", "* unassigned"]);
}

#[test]
fn reserved_names_refuse() {
    let repo = dirty_repo();

    for name in ["unassigned", "all"] {
        let output = gitchange(repo.path(), &["changelist", name]);
        assert_eq!(output.status.code(), Some(1), "'{name}' should refuse");
        assert_eq!(
            output.stdout,
            Vec::<u8>::new(),
            "a failed command leaves stdout empty (#122)"
        );
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(
            stderr.starts_with("gitchange: ") && stderr.contains("reserved"),
            "unexpected stderr: {stderr}"
        );
    }
    assert_eq!(listing(repo.path()), vec!["* unassigned"]);
}

#[test]
fn an_existing_name_refuses_loudly() {
    // Not satisfied (#149): a quiet "already exists" would mask two
    // actors colliding on one name in a shared tree.
    let repo = dirty_repo();
    seed_state(repo.path(), "feature", &["feature"]);

    let output = gitchange(repo.path(), &["changelist", "feature"]);

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(output.stdout, Vec::<u8>::new());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.starts_with("gitchange: ") && stderr.contains("already exists"),
        "unexpected stderr: {stderr}"
    );
    assert_eq!(listing(repo.path()), vec!["* feature"]);
}
