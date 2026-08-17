//! The commit flow's overlays (ticket #33): the dialog, the stale warn,
//! the stage-all offer, and the drift re-confirm.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use gitchange_tui::app::App;

use crate::helpers::{key, render, snapshot};

fn payload(staged: usize, stale: usize) -> gitchange_core::CommitPayload {
    gitchange_core::CommitPayload {
        files: vec![gitchange_core::PayloadFile {
            path: "src/print.css".into(),
            staged_hunks: staged,
            stale_hunks: stale,
            hunks: Vec::new(),
            whole_file: None,
            mode: None,
        }],
    }
}

#[test]
fn the_commit_dialog_renders_summary_inputs_and_flags() {
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());
    app.open_commit_dialog(Some("fixes".into()), payload(5, 1));
    for c in "fix: sizing".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    app.on_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL));
    let text = render(&app);
    assert!(text.contains("Commit changelist — fixes"), "{text}");
    assert!(text.contains("payload: 6 staged hunks in 1 file · 1 ◑"));
    assert!(text.contains("message"));
    assert!(text.contains("fix: sizing"));
    assert!(text.contains("…optional (tab)"));
    assert!(text.contains("[x] --no-verify"));
    assert!(text.contains("[ ] --amend"));
    assert!(text.contains("ctrl+n"));
}

#[test]
fn the_stale_warn_overlays_the_dialog_with_the_stale_files() {
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());
    app.open_commit_dialog(Some("fixes".into()), payload(4, 1));
    for c in "fix: x".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    app.on_key(key(KeyCode::Enter));
    let text = render(&app);
    assert!(text.contains("◑ staged-stale hunks in payload"), "{text}");
    assert!(text.contains("1 of 5 payload hunks is ◑"));
    assert!(text.contains("src/print.css"));
    assert!(text.contains("align index to worktree & commit"));
}

#[test]
fn the_stage_all_offer_names_the_counts() {
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());
    app.offer_stage_all(Some("chores".into()));
    let text = render(&app);
    assert!(text.contains("Nothing staged"), "{text}");
    assert!(text.contains("chores has no staged hunks."));
    assert!(text.contains("Stage all 1 hunk in 1 file and commit?"));
    assert!(text.contains("stage all & continue"));
}

#[test]
fn the_drift_reconfirm_keeps_the_message_and_shows_was_now() {
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());
    app.open_commit_dialog(Some("fixes".into()), payload(5, 0));
    for c in "fix: x".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    let Some(gitchange_tui::app::Action::Commit(gitchange_tui::app::CommitStep::Commit(draft))) =
        app.on_key(key(KeyCode::Enter))
    else {
        panic!("expected a commit step");
    };
    app.commit_drifted(draft, payload(4, 0));
    let text = render(&app);
    assert!(text.contains("Payload changed — re-confirm"), "{text}");
    assert!(text.contains("was: 5 staged hunks in 1 file → now: 4 staged hunks in 1 file"));
    assert!(text.contains("message (kept)"));
    assert!(text.contains("fix: x"));
    assert!(text.contains("commit updated payload"));
}
