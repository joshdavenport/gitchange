//! `changelist` at the binary seam (#149/#166/#167): the bare listing,
//! create, and delete behind the records guard. Its own module because
//! the noun command's subject is the changelist roster rather than the
//! change universe — most assertions here read the listing, which is both
//! the feature and the way a creation or a deletion is observed.
//!
//! The listing is the one read that touches no diff at all (a pure state
//! read), so its write-nothing property is asserted against the state
//! file's bytes here, as `status`'s is in `status.rs` — and so is the
//! all-or-nothing promise that a refused delete wrote nothing.
//!
//! Dormancy is absent: a dormant record's guard case cannot be built
//! through this seam (it needs a refresh with the hunk gone and another
//! with it back), so it lands at core's integration seam instead.

use std::path::Path;

use crate::support::{
    committed_repo, dirty_repo, git, gitchange, owned_repo, owners, repo_holding, seed_state,
    seed_state_raw, state_path,
};

/// `gitchange changelist <args>`, asserted to have succeeded: its stdout
/// echo and its stderr notices, for the caller to read the fragments it
/// cares about.
fn succeeds(dir: &Path, args: &[&str]) -> (String, String) {
    let output = gitchange(dir, &[&["changelist"], args].concat());
    assert_eq!(
        output.status.code(),
        Some(0),
        "gitchange changelist {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    (
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

/// The refusal `gitchange changelist <args>` produces: exit 1, empty
/// stdout, and the stderr text to read fragments off.
fn refusal(dir: &Path, args: &[&str]) -> String {
    let output = gitchange(dir, &[&["changelist"], args].concat());
    assert_eq!(
        output.status.code(),
        Some(1),
        "gitchange changelist {args:?} should refuse"
    );
    assert_eq!(
        output.stdout,
        Vec::<u8>::new(),
        "a failed command leaves stdout empty (#122)"
    );
    String::from_utf8(output.stderr).unwrap()
}

/// The `notice:` lines of a receipt, in order — the stderr half of the
/// receipt shape (#122), with the prefix asserted and stripped.
fn notices(stderr: &str) -> Vec<String> {
    stderr
        .lines()
        .map(|line| {
            line.strip_prefix("gitchange: notice: ")
                .unwrap_or_else(|| panic!("not a prefixed notice: {line}"))
                .to_owned()
        })
        .collect()
}

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

#[test]
fn an_empty_changelist_deletes_ungated_and_force_is_inert() {
    // No records, nothing to release: the guard has nothing to guard
    // (#149), so the delete is an ordinary bare write with one echo and
    // no notices — and `-D` is inert here, releasing nothing and saying
    // nothing, because force overrides a guard that never fired.
    for flags in [vec!["-d", "empty"], vec!["-D", "empty"]] {
        let repo = owned_repo();

        let (stdout, stderr) = succeeds(repo.path(), &flags);

        assert!(
            stdout.contains("deleted changelist 'empty'"),
            "{flags:?} stdout: {stdout}"
        );
        assert_eq!(stderr, "", "{flags:?} decided nothing else");
        assert_eq!(
            listing(repo.path()),
            vec!["  feature", "  docs", "* unassigned"]
        );
    }
}

#[test]
fn a_changelist_holding_records_refuses_naming_the_counts_and_the_override() {
    // The TUI's delete-confirm as a refusal with a named override
    // (ADR 0015): the counts say what is at stake, the mechanism says
    // what deletion would do, and `-D` is the way to say yes.
    let repo = owned_repo();
    let before = std::fs::read(state_path(repo.path())).unwrap();

    let stderr = refusal(repo.path(), &["-d", "feature"]);

    assert!(
        stderr.contains("'feature' holds 1 live record"),
        "the counts are named: {stderr}"
    );
    assert!(
        stderr.contains("releases its hunks recordless")
            && stderr.contains("for the next persisting refresh to claim"),
        "the mechanism is named: {stderr}"
    );
    assert!(
        stderr.contains("changelist -D"),
        "the override is named: {stderr}"
    );
    assert!(
        !stderr.contains("'docs'") && !stderr.contains("unassigned"),
        "a forecast names no destination (#122): {stderr}"
    );
    assert_eq!(
        std::fs::read(state_path(repo.path())).unwrap(),
        before,
        "a refused delete wrote nothing at all"
    );
}

#[test]
fn both_force_spellings_release_the_records_with_a_counting_notice() {
    // `-D` is sugar for `--delete --force` (git's grammar, borrowed
    // wholesale), so the two spellings are one op — and the release is
    // counted on the receipt, never silent.
    for flags in [vec!["-d", "feature", "-f"], vec!["-D", "feature"]] {
        let repo = owned_repo();

        let (stdout, stderr) = succeeds(repo.path(), &flags);

        assert!(
            stdout.contains("deleted changelist 'feature'"),
            "{flags:?} stdout: {stdout}"
        );
        let notices = notices(&stderr);
        assert_eq!(notices.len(), 1, "{flags:?} notices: {notices:?}");
        assert!(
            notices[0].contains("released 1 hunk from 'feature'")
                && notices[0].contains("for the next persisting refresh to claim"),
            "{flags:?} notice: {}",
            notices[0]
        );
        assert!(
            !notices[0].contains("'docs'") && !notices[0].contains("unassigned"),
            "the notice names the mechanism and no destination: {}",
            notices[0]
        );
        assert_eq!(
            listing(repo.path()),
            vec!["  docs", "  empty", "* unassigned"]
        );
    }
}

#[test]
fn released_hunks_are_claimed_by_the_next_persisting_refresh() {
    // What the guard exists to warn about, carried out: the release names
    // no destination because the destination is the *next* refresh's
    // context — here a `switch` between the two decides it, and that
    // refresh's own receipt reports the landing.
    let repo = owned_repo();
    succeeds(repo.path(), &["-D", "feature"]);
    assert_eq!(owners(repo.path(), "a.txt"), vec![None, None]);

    assert_eq!(
        gitchange(repo.path(), &["switch", "docs"]).status.code(),
        Some(0)
    );
    // A mutating verb, so a persisting refresh runs: it captures the
    // released hunks and says so once.
    let output = gitchange(repo.path(), &["add", "docs"]);
    assert_eq!(output.status.code(), Some(0));

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("auto-captured hunk at a.txt") && stderr.contains("'docs'"),
        "the claiming refresh reports the landing: {stderr}"
    );
    assert_eq!(
        owners(repo.path(), "a.txt"),
        vec![Some("docs".to_owned()), Some("docs".to_owned())]
    );
}

#[test]
fn released_hunks_stay_unassigned_while_capture_is_off() {
    // The other side of the flow (ADR 0015's capture-off): with
    // unassigned active nothing claims them, so a release is where the
    // hunks stop rather than a handover.
    let repo = owned_repo();
    succeeds(repo.path(), &["-D", "feature"]);

    let output = gitchange(repo.path(), &["add", "docs"]);
    assert_eq!(output.status.code(), Some(0));

    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "",
        "capture off, so the refresh decided nothing"
    );
    assert_eq!(owners(repo.path(), "a.txt"), vec![None, None]);
}

#[test]
fn deleting_the_active_changelist_notices_that_unassigned_is_active() {
    // Capture turning off is a decision the caller did not ask for, so it
    // is announced (#149) — and it is a notice, not a refusal: an empty
    // active changelist's deletion endangers nothing but the default.
    let repo = dirty_repo();
    seed_state(repo.path(), "feature", &["feature", "docs"]);

    let (stdout, stderr) = succeeds(repo.path(), &["-d", "feature"]);

    assert!(stdout.contains("deleted changelist 'feature'"));
    let notices = notices(&stderr);
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(
        notices[0].contains("unassigned is active now"),
        "the notice: {}",
        notices[0]
    );
    assert_eq!(listing(repo.path()), vec!["  docs", "* unassigned"]);
}

#[test]
fn a_forced_delete_of_the_active_changelist_carries_both_notices() {
    let repo = owned_repo();
    assert_eq!(
        gitchange(repo.path(), &["switch", "feature"]).status.code(),
        Some(0)
    );

    let (_, stderr) = succeeds(repo.path(), &["-D", "feature"]);

    let notices = notices(&stderr);
    assert_eq!(notices.len(), 2, "{notices:?}");
    assert!(notices[0].contains("released 1 hunk from 'feature'"));
    assert!(notices[1].contains("unassigned is active now"));
}

#[test]
fn delete_is_all_or_nothing_and_names_every_offender() {
    // One refusal, every offender, candidates listed (#122's gh shape):
    // the retry is this command corrected, and it costs one round trip.
    // A reserved name is simply unrecognised here — no mode of this
    // command has a meaning for one.
    let repo = owned_repo();
    let before = std::fs::read(state_path(repo.path())).unwrap();

    let stderr = refusal(repo.path(), &["-d", "empty", "nope", "unassigned"]);

    assert!(
        stderr.contains("no changelist named 'nope'")
            && stderr.contains("no changelist named 'unassigned'"),
        "every offender is named: {stderr}"
    );
    assert!(
        stderr.contains("the changelists are: 'feature', 'docs', 'empty'"),
        "the candidates are listed: {stderr}"
    );
    assert_eq!(
        std::fs::read(state_path(repo.path())).unwrap(),
        before,
        "the deletable name in the list was not deleted either"
    );
    assert_eq!(
        listing(repo.path()),
        vec!["  feature", "  docs", "  empty", "* unassigned"]
    );
}

#[test]
fn force_does_not_excuse_an_unrecognised_name() {
    // Force overrides the records guard only (#149): a typo is still a
    // typo, and it still takes the whole command down with it.
    let repo = owned_repo();

    let stderr = refusal(repo.path(), &["-D", "feature", "nope"]);

    assert!(
        stderr.contains("no changelist named 'nope'"),
        "unexpected stderr: {stderr}"
    );
    assert_eq!(
        owners(repo.path(), "a.txt"),
        vec![Some("feature".to_owned()), None],
        "'feature' still holds its record"
    );
}
