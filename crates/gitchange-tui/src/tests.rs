//! Run-loop wiring (issue #66, ADR 0008's "real executors, recorded
//! requests, one smoke test"). Three kinds of test live here:
//!
//! - **Executor tests** run [`run_op`] and [`run_commit_step`] against a
//!   real repo built by `gitchange-test-support`, through core's public
//!   sync ops. Nothing git-touching is faked.
//! - **Loop-arm tests** drive [`event_loop`] over a `TestBackend` with
//!   injected channels and a recording refresh closure. They contain no
//!   sleep and no timeout: the engine channel stays connected and silent
//!   so `select!` only ever has input ready, and dropping the input
//!   sender after the script is the loop's ordinary end-of-input exit.
//! - **One smoke test** drives a real `Engine` end to end.
//!
//! In-crate (`#[cfg(test)]`) on purpose: the executors and the loop stay
//! private, so covering them adds nothing to the `#[doc(hidden)] pub`
//! surface the render tests already forced (ADR 0006).

use std::cell::Cell;
use std::time::{Duration, Instant};

use gitchange_core::{CommitPayload, Snapshot};
use gitchange_test_support::RepoFixture;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;

use super::*;
use app::{INDICATOR_DELAY, LogEntry, Overlay, Panel};
use theme::Theme;

/// Wide enough that no panel renders degenerate — the same shape the
/// render tests draw at.
const WIDTH: u16 = 140;
const HEIGHT: u16 = 40;

/// A repo with one commit, plus the sync handle the loop mutates through.
fn repo_with_commit() -> (RepoFixture, Repo) {
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", "line 1\nline 2\nline 3\n")
        .commit_all("init");
    let repo = Repo::discover(fixture.path()).unwrap();
    (fixture, repo)
}

fn char_key(c: char) -> Event {
    Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
}

fn code_key(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

/// Type a name into an open text overlay, one key per character.
fn typed(name: &str) -> Vec<Event> {
    name.chars().map(char_key).collect()
}

/// A confirmed commit dialog's draft, as the loop would hand it to the
/// commit executor.
fn draft(changelist: Option<&str>, payload: CommitPayload, message: &str) -> CommitDraft {
    CommitDraft {
        changelist: changelist.map(str::to_owned),
        payload,
        message: message.to_owned(),
        body: String::new(),
        body_focus: false,
        no_verify: false,
        amend: false,
    }
}

fn entries(app: &App, severity: Severity) -> Vec<&str> {
    app.log
        .iter()
        .filter(|entry| entry.severity == severity)
        .map(|entry| entry.text.as_str())
        .collect()
}

/// Run [`event_loop`] over the two channels the caller supplies, counting
/// the refresh requests it emits — the outbound half of every arm under
/// test (ADR 0005's debounce bypass; the self-loop filter makes a dropped
/// request *no* refresh, not a late one).
///
/// Exactly one of the two channels must be disconnected, and the other
/// held open and empty: the disconnected one is the arm under test and
/// the loop's exit, and the open one can never be ready, so `select!`
/// has one choice at every step and the run is deterministic without a
/// sleep or a timeout anywhere.
fn run_loop(
    repo: &Repo,
    app: &mut App,
    engine_events: &Receiver<EngineEvent>,
    input: &Receiver<Event>,
) -> (Result<(), Error>, usize) {
    let refreshes = Cell::new(0usize);
    let mut terminal = ratatui::Terminal::new(TestBackend::new(WIDTH, HEIGHT)).unwrap();
    let result = event_loop(
        &mut terminal,
        engine_events,
        input,
        || refreshes.set(refreshes.get() + 1),
        repo,
        app,
    );
    (result, refreshes.get())
}

/// [`run_loop`] over a scripted input stream: the script is delivered,
/// the input sender is dropped, and the end-of-input disconnect is the
/// loop's ordinary exit.
fn drive(repo: &Repo, app: &mut App, script: &[Event]) -> (Result<(), Error>, usize) {
    let (_engine_tx, engine_rx) = unbounded::<EngineEvent>();
    let (input_tx, input_rx) = unbounded();
    for event in script {
        input_tx.send(event.clone()).unwrap();
    }
    drop(input_tx);
    run_loop(repo, app, &engine_rx, &input_rx)
}

/// [`run_loop`] over a scripted engine stream, the mirror of [`drive`]:
/// input stays open and empty, and the engine disconnect after the
/// script is what ends the run — the `EngineDied` arm.
fn drive_engine(
    repo: &Repo,
    app: &mut App,
    script: Vec<EngineEvent>,
) -> (Result<(), Error>, usize) {
    let (engine_tx, engine_rx) = unbounded();
    let (_input_tx, input_rx) = unbounded();
    for event in script {
        engine_tx.send(event).unwrap();
    }
    drop(engine_tx);
    run_loop(repo, app, &engine_rx, &input_rx)
}

// ── executors: op ───────────────────────────────────────────────────

#[test]
fn a_successful_op_echoes_cores_line_to_the_log_at_info() {
    let (fixture, repo) = repo_with_commit();
    fixture.write("a.txt", "line 1\nedited\nline 3\n");
    let mut app = App::new("repo");

    run_op(
        &repo,
        &mut app,
        Op::StageFile {
            path: "a.txt".into(),
        },
    );

    // Core composed the line; this frontend's whole contribution is the
    // channel it lands on (ADR 0006/0007). Pinned as the whole string
    // rather than a substring, because that is the claim: a frontend
    // that composed its own wording mentioning the path would pass a
    // looser assertion. Core rewording `stage_file`'s echo is a one-line
    // update here, and should be.
    assert_eq!(
        app.log,
        vec![LogEntry {
            severity: Severity::Info,
            text: "staged file — a.txt".into(),
        }]
    );
    assert!(app.error_modal.is_none());
    // …and the op really ran: the index holds the worktree's bytes.
    assert_eq!(
        fixture.index_content("a.txt").as_deref(),
        Some("line 1\nedited\nline 3\n")
    );
}

#[test]
fn a_hard_op_failure_takes_the_error_modal_and_logs_at_error() {
    let (_fixture, repo) = repo_with_commit();
    let mut app = App::new("repo");

    run_op(
        &repo,
        &mut app,
        Op::DeleteChangelist {
            name: "never-existed".into(),
        },
    );

    assert_eq!(
        app.error_modal.as_ref().map(|modal| modal.title.as_str()),
        Some("Delete changelist failed")
    );
    assert!(entries(&app, Severity::Info).is_empty());
    assert_eq!(entries(&app, Severity::Error).len(), 1);
}

#[test]
fn a_fail_soft_op_logs_its_advisories_at_notice_with_no_echo() {
    let (fixture, repo) = repo_with_commit();
    fixture.write("a.txt", "line 1\nedited\nline 3\n");
    let stale = repo.refresh().unwrap().files[0].hunks[0].clone();
    // The hunk leaves the universe before the op lands, so nothing
    // applies and the whole outcome is one advisory.
    fixture.write("a.txt", "line 1\nline 2\nline 3\n");
    let mut app = App::new("repo");

    run_op(
        &repo,
        &mut app,
        Op::StageHunk {
            path: "a.txt".into(),
            hunk: stale,
        },
    );

    assert!(app.error_modal.is_none(), "fail-soft, not a modal");
    assert!(entries(&app, Severity::Info).is_empty(), "nothing applied");
    assert_eq!(entries(&app, Severity::Notice).len(), 1);
}

#[test]
fn n_creates_a_changelist_and_switches_to_it() {
    // Core's create leaves the marker alone (ADR 0015); `n` is this
    // frontend's create-then-switch, so one human's next edits land
    // where they just said.
    let (fixture, repo) = repo_with_commit();
    let mut app = App::new("repo");

    run_op(&repo, &mut app, Op::CreateChangelist { name: "wip".into() });

    assert!(app.error_modal.is_none());
    assert_eq!(repo.refresh().unwrap().active.as_deref(), Some("wip"));

    fixture.write("a.txt", "line 1\nedited\nline 3\n");
    let snapshot = repo.refresh().unwrap();
    assert_eq!(
        snapshot.files[0].hunks[0].changelist.as_deref(),
        Some("wip")
    );
}

#[test]
fn s_on_the_unassigned_row_switches_capture_off() {
    // ADR 0015: `s` on unassigned is capture-off, and the executor's
    // half of it is one ordinary switch with `None` as the target.
    let (fixture, repo) = repo_with_commit();
    let mut app = App::new("repo");
    run_op(&repo, &mut app, Op::CreateChangelist { name: "wip".into() });

    run_op(&repo, &mut app, Op::SetActive { changelist: None });

    assert!(app.error_modal.is_none());
    assert_eq!(repo.refresh().unwrap().active, None);
    fixture.write("a.txt", "line 1\nedited\nline 3\n");
    let snapshot = repo.refresh().unwrap();
    assert_eq!(
        snapshot.files[0].hunks[0].changelist, None,
        "capture is off: the edit stays unassigned"
    );
}

#[test]
fn assign_treats_an_existing_target_as_a_valid_target_rather_than_a_create_failure() {
    let (fixture, repo) = repo_with_commit();
    fixture.write("a.txt", "line 1\nedited\nline 3\n");
    repo.create_changelist("wip").unwrap();
    let hunks = repo.refresh().unwrap().files[0].hunks.clone();
    let mut app = App::new("repo");

    run_op(
        &repo,
        &mut app,
        Op::Assign {
            path: "a.txt".into(),
            hunks,
            target: "wip".into(),
            create: true,
        },
    );

    assert!(app.error_modal.is_none(), "the assign is not stranded");
    assert_eq!(entries(&app, Severity::Info).len(), 1, "the assign echoed");
    let snapshot = repo.refresh().unwrap();
    assert_eq!(
        snapshot.files[0].hunks[0].changelist.as_deref(),
        Some("wip")
    );
}

#[test]
fn assign_stops_at_the_create_modal_when_the_target_name_is_refused() {
    let (fixture, repo) = repo_with_commit();
    fixture.write("a.txt", "line 1\nedited\nline 3\n");
    let hunks = repo.refresh().unwrap().files[0].hunks.clone();
    let mut app = App::new("repo");

    run_op(
        &repo,
        &mut app,
        Op::Assign {
            path: "a.txt".into(),
            hunks,
            // A reserved name: the create refuses for a reason that is
            // not "it already exists", so the assign must not run.
            target: gitchange_core::UNASSIGNED.into(),
            create: true,
        },
    );

    assert_eq!(
        app.error_modal.as_ref().map(|modal| modal.title.as_str()),
        Some("Create changelist failed")
    );
    assert!(entries(&app, Severity::Info).is_empty());
}

#[test]
fn the_changelist_ops_stage_and_unstage_the_whole_changelist() {
    let (fixture, repo) = repo_with_commit();
    fixture.write("a.txt", "line 1\nedited\nline 3\n");
    repo.create_changelist("wip").unwrap();
    repo.switch(Some("wip")).unwrap();
    repo.refresh().unwrap(); // the edit auto-captures into `wip`
    let mut app = App::new("repo");

    run_op(
        &repo,
        &mut app,
        Op::StageChangelist {
            changelist: Some("wip".into()),
        },
    );

    assert!(app.error_modal.is_none());
    assert_eq!(
        entries(&app, Severity::Info),
        vec!["staged 1 hunk — 'wip'"],
        "core's echo, on this frontend's info channel"
    );
    assert_eq!(
        fixture.index_content("a.txt").as_deref(),
        Some("line 1\nedited\nline 3\n")
    );

    run_op(
        &repo,
        &mut app,
        Op::UnstageChangelist {
            changelist: Some("wip".into()),
        },
    );

    assert_eq!(
        entries(&app, Severity::Info),
        vec!["staged 1 hunk — 'wip'", "unstaged 1 hunk — 'wip'"]
    );
    assert_eq!(
        fixture.index_content("a.txt").as_deref(),
        Some("line 1\nline 2\nline 3\n"),
        "the index is back at HEAD"
    );
}

// ── executors: the commit flow ──────────────────────────────────────

#[test]
fn opening_the_dialog_derives_the_payload_behind_a_sync_refresh() {
    let (fixture, repo) = repo_with_commit();
    fixture
        .write("a.txt", "line 1\nedited\nline 3\n")
        .stage("a.txt");
    let mut app = App::new("repo");

    run_commit_step(&repo, &mut app, CommitStep::Open { changelist: None });

    let Some(Overlay::Commit(draft)) = &app.overlay else {
        panic!("expected the commit dialog, got {:?}", app.overlay);
    };
    assert_eq!(draft.payload.staged_hunks(), 1);
}

#[test]
fn an_empty_payload_routes_to_the_stage_all_offer_rather_than_the_dialog() {
    let (fixture, repo) = repo_with_commit();
    // Changed but unstaged: core never auto-stages (ADR 0004).
    fixture.write("a.txt", "line 1\nedited\nline 3\n");
    let mut app = App::new("repo");
    app.apply_snapshot(repo.refresh().unwrap());

    run_commit_step(&repo, &mut app, CommitStep::Open { changelist: None });

    assert!(matches!(
        app.overlay,
        Some(Overlay::CommitStageAll { hunks: 1, .. })
    ));
}

#[test]
fn stage_all_and_open_stages_the_changelist_then_opens_the_dialog() {
    let (fixture, repo) = repo_with_commit();
    fixture.write("a.txt", "line 1\nedited\nline 3\n");
    let mut app = App::new("repo");

    run_commit_step(
        &repo,
        &mut app,
        CommitStep::StageAllAndOpen { changelist: None },
    );

    assert_eq!(entries(&app, Severity::Info).len(), 1, "the bulk op echoed");
    let Some(Overlay::Commit(draft)) = &app.overlay else {
        panic!("expected the commit dialog, got {:?}", app.overlay);
    };
    assert_eq!(draft.payload.staged_hunks(), 1);
    assert_eq!(
        fixture.index_content("a.txt").as_deref(),
        Some("line 1\nedited\nline 3\n")
    );
}

#[test]
fn a_confirmed_commit_echoes_the_command_and_the_new_commit_then_closes_the_flow() {
    let (fixture, repo) = repo_with_commit();
    fixture
        .write("a.txt", "line 1\nedited\nline 3\n")
        .stage("a.txt");
    let payload = repo.commit_payload(None).unwrap();
    let mut app = App::new("repo");

    run_commit_step(
        &repo,
        &mut app,
        CommitStep::Commit(draft(None, payload.clone(), "edit line 2")),
    );

    let expected = commit_echo(&CommitOptions::default(), None, &payload);
    assert_eq!(entries(&app, Severity::Info).first(), Some(&&*expected));
    assert!(app.error_modal.is_none());
    assert!(app.overlay.is_none(), "success closes the flow");
    assert_eq!(fixture.commit_count(), 2);
    assert_eq!(
        fixture.head_bytes("a.txt"),
        Some(b"line 1\nedited\nline 3\n".to_vec())
    );
}

/// The transparency echo records only commands git actually ran
/// (ADR 0007). A hook rejection means `git commit` executed and refused,
/// so the echo is pushed — and the modal carries the hook's own stderr
/// verbatim, with the dialog restored exactly as it was confirmed.
#[test]
fn a_hook_rejection_echoes_the_command_and_carries_stderr_verbatim() {
    let (fixture, repo) = repo_with_commit();
    fixture
        .write("a.txt", "line 1\nedited\nline 3\n")
        .stage("a.txt");
    fixture.with_hook(
        "pre-commit",
        "#!/bin/sh\nprintf 'lint: bad line\\nlint: fix it\\n' >&2\nexit 1\n",
    );
    let payload = repo.commit_payload(None).unwrap();
    let confirmed = draft(None, payload.clone(), "edit line 2");
    let mut app = App::new("repo");

    run_commit_step(&repo, &mut app, CommitStep::Commit(confirmed.clone()));

    let expected = commit_echo(&CommitOptions::default(), None, &payload);
    assert!(
        entries(&app, Severity::Info).contains(&&*expected),
        "git ran, so the command is echoed: {:?}",
        app.log
    );
    let modal = app.error_modal.as_ref().expect("the rejection modals");
    assert_eq!(modal.title, "Commit failed");
    // Verbatim means the hook's own bytes and nothing around them: the
    // error's `Display` wraps the same stderr in a "commit rejected:"
    // line, and that wrapper must not reach the modal (ADR 0007 — the
    // detail is the user's own tooling talking to them).
    assert_eq!(modal.detail, "lint: bad line\nlint: fix it\n");
    assert_eq!(
        app.overlay,
        Some(Overlay::Commit(confirmed)),
        "the dialog is restored exactly as confirmed"
    );
    assert_eq!(fixture.commit_count(), 1);
    assert_eq!(
        fixture.state_dir_entries(),
        Vec::<String>::new(),
        "the temp index and message file are discarded (ADR 0004)"
    );
}

#[test]
fn the_operation_guard_refuses_without_echoing_a_command_git_never_ran() {
    let fixture = RepoFixture::new();
    fixture.write("a.txt", "base\n").commit_all("init");
    fixture
        .branch("feature")
        .checkout("feature")
        .write("a.txt", "feature side\n")
        .commit_all("feature edit");
    fixture
        .checkout("main")
        .write("a.txt", "main side\n")
        .commit_all("main edit");
    let repo = Repo::discover(fixture.path()).unwrap();
    // Clean tree: an empty payload, which the guard never gets past.
    let payload = repo.commit_payload(None).unwrap();
    fixture.merge_conflicting("feature");
    let mut app = App::new("repo");

    run_commit_step(
        &repo,
        &mut app,
        CommitStep::Commit(draft(None, payload, "merge")),
    );

    assert!(
        entries(&app, Severity::Info).is_empty(),
        "git never ran, so nothing is echoed: {:?}",
        app.log
    );
    assert_eq!(
        app.error_modal.as_ref().map(|modal| modal.title.as_str()),
        Some("Commit failed")
    );
    assert!(matches!(app.overlay, Some(Overlay::Commit(_))));
}

#[test]
fn a_payload_failure_refuses_without_echoing_a_command_git_never_ran() {
    let (_fixture, repo) = repo_with_commit();
    let mut app = App::new("repo");

    // Nothing staged: core reports it rather than committing an empty
    // payload, and git was never reached.
    let empty = repo.commit_payload(None).unwrap();
    assert!(empty.is_empty(), "the fixture reaches an empty payload");
    run_commit_step(
        &repo,
        &mut app,
        CommitStep::Commit(draft(None, empty, "nothing")),
    );

    assert!(entries(&app, Severity::Info).is_empty());
    assert_eq!(
        app.error_modal.as_ref().map(|modal| modal.title.as_str()),
        Some("Commit failed")
    );
}

#[test]
fn drift_routes_back_to_the_reconfirm_overlay_with_the_fresh_payload() {
    let (fixture, repo) = repo_with_commit();
    fixture
        .write("a.txt", "line 1\nv1\nline 3\n")
        .stage("a.txt");
    let confirmed = repo.commit_payload(None).unwrap();
    // The staged content moves between confirm and commit.
    fixture
        .write("a.txt", "line 1\nv2\nline 3\n")
        .stage("a.txt");
    let mut app = App::new("repo");

    run_commit_step(
        &repo,
        &mut app,
        CommitStep::Commit(draft(None, confirmed.clone(), "edit line 2")),
    );

    let Some(Overlay::CommitDrift { draft, previous }) = &app.overlay else {
        panic!("expected the drift re-confirm, got {:?}", app.overlay);
    };
    assert_eq!(previous, &confirmed, "the dialog's confirmed payload");
    assert_eq!(
        draft.payload,
        repo.commit_payload(None).unwrap(),
        "re-confirm is against the fresh payload"
    );
    assert!(
        app.error_modal.is_none(),
        "drift is a re-confirm, not an error"
    );
    // Drift is not a command git ran either.
    assert_eq!(fixture.commit_count(), 1);
    assert_eq!(entries(&app, Severity::Info).len(), 1);
    assert!(entries(&app, Severity::Info)[0].starts_with("nothing committed"));
}

/// ADR 0004's align option: index := worktree over the changelist's stale
/// hunks, then commit what *that* produced — the payload is re-derived so
/// the drift guard compares the aligned content, never the stale
/// confirmation.
#[test]
fn align_and_commit_re_derives_the_payload_before_committing_it() {
    let (fixture, repo) = repo_with_commit();
    fixture
        .write("a.txt", "line 1\nstaged\nline 3\n")
        .stage("a.txt");
    // Edited again after staging: the hunk is ◑, and the confirmed
    // payload carries the staged bytes, not these.
    fixture.write("a.txt", "line 1\naligned\nline 3\n");
    let confirmed = repo.commit_payload(None).unwrap();
    assert_eq!(confirmed.stale_hunks(), 1, "the fixture reaches ◑");
    let mut app = App::new("repo");

    run_commit_step(
        &repo,
        &mut app,
        CommitStep::AlignAndCommit(draft(None, confirmed, "edit line 2")),
    );

    assert!(app.error_modal.is_none(), "no drift against the stale one");
    assert!(app.overlay.is_none(), "the commit went through");
    assert_eq!(fixture.commit_count(), 2);
    assert_eq!(
        fixture.head_bytes("a.txt"),
        Some(b"line 1\naligned\nline 3\n".to_vec()),
        "the worktree's bytes, aligned into the index and committed"
    );
    // Align's own echo, the command echo, and the committed line.
    assert_eq!(entries(&app, Severity::Info).len(), 3);
}

#[test]
fn an_align_failure_restores_the_dialog_under_the_modal_and_commits_nothing() {
    let (fixture, repo) = repo_with_commit();
    fixture
        .write("a.txt", "line 1\nedited\nline 3\n")
        .stage("a.txt");
    let payload = repo.commit_payload(None).unwrap();
    let confirmed = draft(Some("ghost"), payload, "edit line 2");
    let mut app = App::new("repo");

    run_commit_step(
        &repo,
        &mut app,
        CommitStep::AlignAndCommit(confirmed.clone()),
    );

    assert_eq!(
        app.error_modal.as_ref().map(|modal| modal.title.as_str()),
        Some("Commit failed")
    );
    assert_eq!(app.overlay, Some(Overlay::Commit(confirmed)));
    assert_eq!(fixture.commit_count(), 1);
}

// The align path's third branch — align succeeds, the re-derived payload
// is *still* ◑, so the flow re-warns instead of committing — has no
// deterministic fixture: `align` and `commit_payload` apply the same
// predicate to the same changelist over back-to-back refreshes, so the
// only way a ◑ hunk survives the first and reappears in the second is a
// worktree edit landing between them. That is a real race the code is
// written for (ADR 0004's never-silent rule) and not one a test can stage
// without a seam inside the executor. What the branch *does* once taken
// is covered where it lives: `app/tests.rs::a_stale_payload_routes_
// through_the_warn_overlay` drives `reconfirm_stale`'s overlay, and
// `render.rs::the_stale_warn_overlays_the_dialog_with_the_stale_files`
// renders it. Only the loop's decision to call it is unasserted, and
// recorded here rather than faked.

// ── loop arms ───────────────────────────────────────────────────────

#[test]
fn the_quit_key_leaves_the_loop_without_asking_for_a_refresh() {
    let (_fixture, repo) = repo_with_commit();
    let mut app = App::new("repo");

    // `R` sits behind `q` in the script and must never be reached.
    let (result, refreshes) = drive(&repo, &mut app, &[char_key('q'), char_key('R')]);

    assert!(result.is_ok());
    assert_eq!(refreshes, 0);
}

#[test]
fn the_manual_refresh_key_requests_a_refresh() {
    let (_fixture, repo) = repo_with_commit();
    let mut app = App::new("repo");

    let (result, refreshes) = drive(&repo, &mut app, &[char_key('R'), char_key('R')]);

    assert!(result.is_ok());
    assert_eq!(refreshes, 2);
}

#[test]
fn focus_gained_requests_a_refresh_to_catch_up() {
    let (_fixture, repo) = repo_with_commit();
    let mut app = App::new("repo");

    let (result, refreshes) = drive(
        &repo,
        &mut app,
        // The focus pair: only the regained half asks for a catch-up
        // (ADR 0005).
        &[Event::FocusLost, Event::FocusGained],
    );

    assert!(result.is_ok());
    assert_eq!(refreshes, 1);
}

#[test]
fn keys_that_produce_no_action_request_no_refresh() {
    let (_fixture, repo) = repo_with_commit();
    let mut app = App::new("repo");

    let (result, refreshes) = drive(
        &repo,
        &mut app,
        &[char_key('j'), char_key('k'), char_key('?')],
    );

    assert!(result.is_ok());
    assert_eq!(refreshes, 0);
}

/// A mutation's follow-up request is not an optimization: ADR 0005's
/// self-loop filter ignores gitchange's own writes, so a dropped request
/// is *no* refresh, not a late one — the panel would keep showing
/// pre-mutation state indefinitely.
#[test]
fn a_completed_mutation_requests_a_refresh() {
    let (_fixture, repo) = repo_with_commit();
    let mut app = App::new("repo");

    let mut script = vec![char_key('n')];
    script.extend(typed("wip"));
    script.push(code_key(KeyCode::Enter));
    let (result, refreshes) = drive(&repo, &mut app, &script);

    assert!(result.is_ok());
    assert_eq!(refreshes, 1);
    // The op ran for real, through core's sync handle.
    assert!(
        repo.refresh()
            .unwrap()
            .changelists
            .iter()
            .any(|changelist| changelist.name == "wip")
    );
}

/// Two steps through the one `Action::Commit` arm, because "every commit
/// step" is what ADR 0005 asks the request of: `c` derives an empty
/// payload and offers stage-all, `enter` confirms it into
/// `StageAllAndOpen`. Each step's own work is an executor test above;
/// what this pins is that the arm requests a refresh per step rather than
/// once per dialog.
#[test]
fn every_commit_step_requests_a_refresh() {
    let (fixture, repo) = repo_with_commit();
    // Changed but unstaged, so `c`'s payload is empty and the flow needs
    // a second step to reach the dialog.
    fixture.write("a.txt", "line 1\nedited\nline 3\n");
    let mut app = App::new("repo");
    app.apply_snapshot(repo.refresh().unwrap());

    // `j` scopes the unassigned row — committable like any changelist
    // (ADR 0004).
    let (result, refreshes) = drive(
        &repo,
        &mut app,
        &[char_key('j'), char_key('c'), code_key(KeyCode::Enter)],
    );

    assert!(result.is_ok());
    assert_eq!(refreshes, 2);
    assert!(matches!(app.overlay, Some(Overlay::Commit(_))));
}

/// ADR 0005's mid-refresh half: panels keep the last snapshot and the
/// loop stays fully interactive while a refresh is in flight. The
/// in-flight state is set directly rather than driven through the engine
/// channel — `engine_events_reach_their_app_handlers` pins that the
/// engine arm calls `on_refresh_started`, and the one-ready-arm rule
/// bars a script with both channels live. `RefreshStarted` carries no snapshot, so with the
/// engine silent the App's snapshot can only change if a key path
/// mutates it.
#[test]
fn keys_still_act_while_a_refresh_is_in_flight() {
    let (fixture, repo) = repo_with_commit();
    fixture.write("a.txt", "line 1\nedited\nline 3\n");
    let mut app = App::new("repo");
    // The snapshot the panels hold before the keys: one changed file,
    // no changelists. `apply_snapshot` clears the in-flight marker, so
    // the refresh starts after it lands.
    app.apply_snapshot(repo.refresh().unwrap());
    let started = Instant::now();
    app.on_refresh_started(started);

    // Navigation, then a real mutation: `j` moves the changelist
    // cursor, `n`+name+enter creates a changelist through core's sync
    // ops.
    let mut script = vec![char_key('j'), char_key('n')];
    script.extend(typed("wip"));
    script.push(code_key(KeyCode::Enter));
    let (result, refreshes) = drive(&repo, &mut app, &script);

    assert!(result.is_ok());
    assert_eq!(app.changelist_row, 1, "navigation moved the selection");
    assert!(
        repo.refresh()
            .unwrap()
            .changelists
            .iter()
            .any(|changelist| changelist.name == "wip"),
        "the mutation ran for real"
    );
    assert_eq!(refreshes, 1, "the mutation requested its refresh");
    // The panels still hold the pre-key snapshot: the changelist the
    // mutation created is in the repo (asserted above) but not here —
    // only a refresh completing may swap it in.
    let snapshot = app.snapshot.as_ref().expect("the snapshot survives");
    assert!(snapshot.changelists.is_empty());
    assert_eq!(snapshot.files.len(), 1);
    // No key path clears the in-flight marker. Both instants are the
    // test's own, so this is arithmetic, not timing.
    assert!(app.indicator_visible(started + INDICATOR_DELAY));
}

/// The disconnect is reached the way production would reach it: a real
/// `Engine` is spawned, its receiver cloned, and the `Engine` dropped.
/// Its threads shut down and release the sender, so what the loop sees is
/// a genuinely dead engine rather than a channel the test closed by hand
/// — a shutdown that leaked its sender would hang here.
#[test]
fn a_dead_engine_ends_the_loop_with_the_engine_died_error() {
    let (fixture, repo) = repo_with_commit();
    let engine = Engine::spawn(fixture.path()).unwrap();
    let engine_rx = engine.events().clone();
    drop(engine);
    // Input stays connected and silent, so the engine arm is the only one
    // that can fire.
    let (_input_tx, input_rx) = unbounded();
    let mut app = App::new("repo");

    let (result, refreshes) = run_loop(&repo, &mut app, &engine_rx, &input_rx);

    assert!(
        matches!(result, Err(Error::EngineDied)),
        "expected EngineDied, got {result:?}"
    );
    assert_eq!(refreshes, 0);
}

#[test]
fn engine_events_reach_their_app_handlers() {
    let (_fixture, repo) = repo_with_commit();
    let mut app = App::new("repo");

    // `RefreshStarted` last: `RefreshFailed` clears the in-flight
    // marker, so the order is what lets one run assert all three.
    let (result, refreshes) = drive_engine(
        &repo,
        &mut app,
        vec![
            EngineEvent::ConditionStarted(Condition::WatcherDegraded),
            EngineEvent::RefreshFailed(gitchange_core::Error::NothingStaged),
            EngineEvent::RefreshStarted,
        ],
    );

    assert!(matches!(result, Err(Error::EngineDied)));
    assert!(app.watcher_degraded, "the condition became a pin");
    assert!(app.error_modal.is_some(), "the failure modalled");
    assert!(
        app.indicator_deadline().is_some(),
        "the refresh is in flight"
    );
    assert_eq!(refreshes, 0, "engine events ask for nothing back");
}

// ── the frame's geometry (issue #85) ────────────────────────────────

/// [`run_loop`]'s terminal-keeping twin, for a test that reads what the
/// loop painted rather than what it requested: the same channel
/// discipline, plus the last frame drawn. The loop draws at the top of
/// every iteration, so that frame is the one the run's last event
/// produced.
fn run_loop_frame(
    repo: &Repo,
    app: &mut App,
    engine_events: &Receiver<EngineEvent>,
    input: &Receiver<Event>,
) -> (Result<(), Error>, Buffer) {
    let mut terminal = ratatui::Terminal::new(TestBackend::new(WIDTH, HEIGHT)).unwrap();
    let result = event_loop(&mut terminal, engine_events, input, || {}, repo, app);
    (result, terminal.backend().buffer().clone())
}

/// [`drive_engine`], keeping the frame it ended on: the engine disconnect
/// returns before the next draw, so that frame is the one the App's
/// recorded heights came from.
fn drive_engine_frame(repo: &Repo, app: &mut App, script: Vec<EngineEvent>) -> Buffer {
    let (engine_tx, engine_rx) = unbounded();
    let (_input_tx, input_rx) = unbounded();
    for event in script {
        engine_tx.send(event).unwrap();
    }
    drop(engine_tx);
    let (result, buffer) = run_loop_frame(repo, app, &engine_rx, &input_rx);
    assert!(matches!(result, Err(Error::EngineDied)), "{result:?}");
    buffer
}

/// A panel's frame in a drawn buffer, found by its own title: the top-left
/// corner that titles it, across to the top-right and down to the
/// bottom-left in the same column. The corners come from the theme's own
/// [`Theme::glyphs`]`.panel_border`, so a frame the theme redraws is still
/// found here. Read off what was painted, so nothing here re-derives the
/// layout it checks.
fn panel_frame(buffer: &Buffer, panel: Panel) -> Rect {
    let frame = Theme::default().glyphs.panel_border.to_border_set();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            if buffer[(x, y)].symbol() != frame.top_left {
                continue;
            }
            // Bounded by this panel's own top-right corner: the two
            // columns draw their top borders on one row, and an
            // unbounded read would match the neighbour's title.
            let Some(end) =
                (x + 1..buffer.area.width).find(|&x| buffer[(x, y)].symbol() == frame.top_right)
            else {
                continue;
            };
            let border: String = (x..=end).map(|x| buffer[(x, y)].symbol()).collect();
            if !border.contains(panel.title()) {
                continue;
            }
            let bottom = (y + 1..buffer.area.height)
                .find(|&y| buffer[(x, y)].symbol() == frame.bottom_left)
                .expect("a titled panel closes its frame");
            return Rect {
                x,
                y,
                width: end - x + 1,
                height: bottom - y + 1,
            };
        }
    }
    panic!("panel {} is not drawn", panel.title());
}

/// The rows inside a drawn panel's frame, borders excluded.
fn drawn_content_height(buffer: &Buffer, panel: Panel) -> u16 {
    panel_frame(buffer, panel).height - 2
}

/// The text of a drawn panel's content rows, borders excluded and each
/// row trimmed — so a row reads the same whether or not it carries the
/// selection, which pads its line to the panel's width.
fn drawn_rows(buffer: &Buffer, panel: Panel) -> Vec<String> {
    let frame = panel_frame(buffer, panel);
    (frame.y + 1..frame.bottom() - 1)
        .map(|y| {
            (frame.x + 1..frame.right() - 1)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .trim()
                .to_owned()
        })
        .collect()
}

#[test]
fn the_loop_records_the_content_heights_it_drew() {
    let (fixture, repo) = repo_with_commit();
    // Content in every panel: a changelist row, a changed file, a
    // commit, and the log line the refresh echoes.
    fixture.write("a.txt", "line 1\nedited\nline 3\n");
    repo.create_changelist("wip").unwrap();
    let mut app = App::new("repo");

    let buffer = drive_engine_frame(
        &repo,
        &mut app,
        vec![EngineEvent::RefreshComplete(repo.refresh().unwrap())],
    );

    assert!(app.pins().is_empty(), "a clean repo pins nothing");
    for panel in Panel::ALL {
        assert_eq!(
            app.panel_height(panel),
            drawn_content_height(&buffer, panel),
            "{panel:?} recorded {} against the frame it drew",
            app.panel_height(panel)
        );
    }
}

#[test]
fn the_recorded_log_height_excludes_the_pin_banner() {
    let (_fixture, repo) = repo_with_commit();

    let mut quiet = App::new("repo");
    let unpinned = drive_engine_frame(&repo, &mut quiet, Vec::new());
    let unpinned_frame = drawn_content_height(&unpinned, Panel::Log);

    let mut degraded = App::new("repo");
    let pinned = drive_engine_frame(
        &repo,
        &mut degraded,
        vec![EngineEvent::ConditionStarted(Condition::WatcherDegraded)],
    );

    assert_eq!(degraded.pins().len(), 1, "the condition became a pin");
    // The Log's frame grew a row for the pin (conditions never eat
    // history)...
    let pinned_frame = drawn_content_height(&pinned, Panel::Log);
    assert_eq!(pinned_frame, unpinned_frame + 1);
    // ...and the height the App records did not: the banner is fixed
    // above the scrollable stream, not part of it (ADR 0007).
    assert_eq!(degraded.panel_height(Panel::Log), pinned_frame - 1);
    assert_eq!(degraded.panel_height(Panel::Log), unpinned_frame);
    assert_eq!(quiet.panel_height(Panel::Log), unpinned_frame);
}

// ── paging (issue #84) ──────────────────────────────────────────────

/// [`drive`], keeping the frame it ended on: the loop redraws before it
/// discovers the input disconnect, so that frame is the one the script's
/// last key produced.
fn drive_frame(repo: &Repo, app: &mut App, script: &[Event]) -> Buffer {
    let (_engine_tx, engine_rx) = unbounded::<EngineEvent>();
    let (input_tx, input_rx) = unbounded();
    for event in script {
        input_tx.send(event.clone()).unwrap();
    }
    drop(input_tx);
    let (result, buffer) = run_loop_frame(repo, app, &engine_rx, &input_rx);
    assert!(result.is_ok(), "{result:?}");
    buffer
}

/// A repo with far more changed files than the Files panel can show.
fn repo_with_many_files(count: usize) -> (RepoFixture, Repo) {
    let fixture = RepoFixture::new();
    fixture.write("seed.txt", "seed\n").commit_all("init");
    for index in 0..count {
        fixture.write(&format!("file{index:03}.txt"), "changed\n");
    }
    let repo = Repo::discover(fixture.path()).unwrap();
    (fixture, repo)
}

/// The recorded height reaching the press: the same script run once and
/// twice, each ending on the frame its last page key drew.
///
/// The assertion is the property rather than a row index — a page leaves
/// one row of overlap, so the row that was last visible is the one now at
/// the top — which holds at whatever height a future layout gives the
/// panel.
#[test]
fn a_page_key_moves_by_the_height_the_loop_recorded() {
    let (_fixture, repo) = repo_with_many_files(60);
    // Drill into unassigned first, so the Files rows are one flat list.
    let script = |pages: usize| {
        let mut script = vec![code_key(KeyCode::Down), code_key(KeyCode::Enter)];
        script.extend(std::iter::repeat_n(char_key('.'), pages));
        script
    };

    let mut once = App::new("repo");
    once.apply_snapshot(repo.refresh().unwrap());
    let first = drive_frame(&repo, &mut once, &script(1));

    let mut twice = App::new("repo");
    twice.apply_snapshot(repo.refresh().unwrap());
    let second = drive_frame(&repo, &mut twice, &script(2));

    let height = once.panel_height(Panel::Files);
    assert!(height > 2, "the Files panel is drawn at a usable height");
    assert_eq!(
        once.files_count().0,
        usize::from(height),
        "one page from the first row selects the last visible row"
    );

    let before = drawn_rows(&first, Panel::Files);
    let after = drawn_rows(&second, Panel::Files);
    assert_eq!(
        before.last(),
        after.first(),
        "the row that was last visible is now the first"
    );
    assert_ne!(
        before.first(),
        after.first(),
        "the second page actually scrolled the panel"
    );
}

// ── the smoke test ──────────────────────────────────────────────────

/// The one end-to-end run (ADR 0008's ceiling rule): a real `Engine` over
/// a real repo, a mutation driven in as keystrokes, the refresh it
/// requests coming back as a real `RefreshComplete` and landing on the
/// App. It proves the wiring the recording-closure tests abstract.
///
/// The forwarder thread is harness, not a fake — every event it passes
/// on is the Engine's own. It exists so the run ends deterministically:
/// dropping its sender after the snapshot under test is the loop's
/// engine-disconnect exit, which needs no sleep and no quit key racing
/// the event it is meant to follow. A forwarder that times out instead
/// drops the sender with nothing forwarded, so a broken wiring fails the
/// assertion below rather than hanging.
#[test]
fn a_mutation_key_drives_a_real_engine_refresh_onto_the_app() {
    let (fixture, repo) = repo_with_commit();
    let engine = Engine::spawn(fixture.path()).unwrap();

    let (events_tx, events_rx) = unbounded();
    let real = engine.events().clone();
    let forwarder = std::thread::spawn(move || {
        while let Ok(event) = real.recv_timeout(Duration::from_secs(30)) {
            let done = matches!(&event, EngineEvent::RefreshComplete(snapshot)
                if has_changelist(snapshot, "smoke"));
            if events_tx.send(event).is_err() || done {
                return;
            }
        }
    });

    let (input_tx, input_rx) = unbounded();
    let mut script = vec![char_key('n')];
    script.extend(typed("smoke"));
    script.push(code_key(KeyCode::Enter));
    for event in script {
        input_tx.send(event).unwrap();
    }

    let mut app = App::new("repo");
    let mut terminal = ratatui::Terminal::new(TestBackend::new(WIDTH, HEIGHT)).unwrap();
    let result = event_loop(
        &mut terminal,
        &events_rx,
        &input_rx,
        || engine.request_refresh(),
        &repo,
        &mut app,
    );
    forwarder.join().unwrap();

    assert!(
        matches!(result, Err(Error::EngineDied)),
        "the forwarder's disconnect is this run's exit, got {result:?}"
    );
    let snapshot = app.snapshot.as_ref().expect("a snapshot reached the App");
    assert!(
        has_changelist(snapshot, "smoke"),
        "the keystroke's mutation came back through a real refresh"
    );
}

fn has_changelist(snapshot: &Snapshot, name: &str) -> bool {
    snapshot
        .changelists
        .iter()
        .any(|changelist| changelist.name == name)
}
