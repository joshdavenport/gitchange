//! Rendering smoke tests over ratatui's TestBackend: the panel stack
//! draws, the All view groups and tags, drilled views dim — asserted on
//! buffer text, not exact cells (snapshot-style, opportunistic per
//! ADR 0008).

use std::time::{Duration, Instant};

use gitchange_core::{
    ChangeKind, ChangedFile, Changelist, CommitInfo, GitOperation, Head, Hunk, HunkLine, HunkStage,
    Snapshot,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend};

use gitchange_tui::app::{App, INDICATOR_DELAY, Panel, Severity};
use gitchange_tui::theme::Theme;
use gitchange_tui::ui;

fn hunk(new_start: u32, changelist: Option<&str>, stage: HunkStage) -> Hunk {
    Hunk {
        old_start: new_start,
        old_lines: 1,
        new_start,
        new_lines: 2,
        lines: vec![
            HunkLine {
                origin: ' ',
                content: "context\n".into(),
            },
            HunkLine {
                origin: '+',
                content: format!("added at {new_start}\n"),
            },
        ],
        stage,
        index_only: false,
        oid_anchor: None,
        changelist: changelist.map(str::to_owned),
    }
}

fn snapshot() -> Snapshot {
    Snapshot {
        files: vec![
            ChangedFile {
                path: "src/nav.astro".into(),
                kind: ChangeKind::Modified,
                binary: false,
                binary_sides: None,
                hunks: vec![hunk(8, None, HunkStage::Unstaged)],
            },
            ChangedFile {
                path: "src/print.css".into(),
                kind: ChangeKind::Modified,
                binary: false,
                binary_sides: None,
                hunks: vec![
                    hunk(14, Some("fixes"), HunkStage::Staged),
                    hunk(63, Some("chores"), HunkStage::Unstaged),
                ],
            },
        ],
        changelists: vec![
            Changelist {
                name: "fixes".into(),
            },
            Changelist {
                name: "chores".into(),
            },
        ],
        active: Some("fixes".into()),
        notices: Vec::new(),
        head: Head::Branch {
            name: "feat/print-page".into(),
        },
        recent_commits: vec![CommitInfo {
            short_id: "91a05c13".into(),
            author: "Josh Davenport-Smith".into(),
            summary: "fix: viewport sizing".into(),
        }],
        operation: None,
    }
}

fn render(app: &App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(140, 40)).unwrap();
    terminal
        .draw(|frame| ui::draw(frame, app, &Theme::default(), Instant::now()))
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    let mut text = String::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            text.push_str(buffer[(x, y)].symbol());
        }
        text.push('\n');
    }
    text
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
fn the_panel_stack_renders_with_the_all_view() {
    let mut app = App::new("works-in-progress");
    app.apply_snapshot(snapshot());
    let text = render(&app);

    for title in ["Status", "Changelists", "Files", "Commits", "Diff", "Log"] {
        assert!(text.contains(title), "missing panel title {title}\n{text}");
    }
    assert!(text.contains("works-in-progress"));
    assert!(text.contains("feat/print-page"));
    // All view: groups with the unassigned group last; a file row.
    assert!(text.contains("▾ fixes *"));
    assert!(text.contains("▾ unassigned"));
    assert!(text.contains("src/print.css"));
    // Commits panel content.
    assert!(text.contains("91a05c13"));
    assert!(text.contains("JD"));
    assert!(text.contains("fix: viewport sizing"));
    // Files panel context + count.
    assert!(text.contains("all changelists"));
    // Diff tags name owning changelists in the All view (the first
    // selectable file is print.css under 'fixes').
    assert!(text.contains("⟨fixes ●⟩"));
    assert!(text.contains("⟨chores ○⟩"));
}

#[test]
fn a_drilled_view_tags_only_foreign_hunks() {
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());
    app.on_key(key(KeyCode::Char('j'))); // drill into 'fixes'
    let text = render(&app);

    assert!(
        text.contains("- fixes"),
        "files panel carries the scope\n{text}"
    );
    // print.css's foreign hunk (chores) is tagged; own hunks are not.
    assert!(text.contains("⟨chores ○⟩"));
    assert!(!text.contains("⟨fixes"));
    // Scoped title counts: 1 own hunk staged of 1, 1 elsewhere.
    assert!(text.contains("print.css (1/1 staged · 1 hunk elsewhere)"));
}

#[test]
fn help_overlay_lists_bindings() {
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());
    app.on_key(key(KeyCode::Char('?')));
    let text = render(&app);
    assert!(text.contains("Keybindings"));
    assert!(text.contains("focus panel"));
}

#[test]
fn the_deferred_indicator_appears_past_the_threshold() {
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());
    app.on_refresh_started(Instant::now() - INDICATOR_DELAY - Duration::from_millis(1));
    let text = render(&app);
    assert!(text.contains("refreshing…"));
}

#[test]
fn an_empty_repo_renders_without_a_snapshot() {
    let app = App::new("repo");
    let text = render(&app);
    assert!(text.contains("loading…"));
    assert!(text.contains("all"));
    assert!(text.contains("unassigned"));
}

#[test]
fn hunk_mode_swaps_the_title_and_keybar() {
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());
    app.on_key(key(KeyCode::Char('3')));
    app.on_key(key(KeyCode::Enter));
    let text = render(&app);
    assert!(text.contains("print.css — hunk 1 of 2"));
    assert!(text.contains("add all hunks to changelist"));
    assert!(text.contains("shift+enter"));
}

#[test]
fn the_move_popup_lists_changelists_and_the_escape_hatch() {
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());
    app.on_key(key(KeyCode::Char('3')));
    app.on_key(key(KeyCode::Char('m')));
    let text = render(&app);
    assert!(text.contains("Move to changelist"));
    assert!(text.contains("M src/print.css"));
    assert!(text.contains("fixes (active)"));
    assert!(text.contains("+ create new changelist…"));
    assert!(text.contains("enter move · esc cancel"));
}

#[test]
fn text_inputs_frame_their_label_on_the_border() {
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());
    app.on_key(key(KeyCode::Char('n')));
    for c in "docs".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    let text = render(&app);
    assert!(text.contains("New changelist"));
    assert!(text.contains("docs"));
}

#[test]
fn the_delete_confirmation_names_the_unassigned_aftermath() {
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());
    app.on_key(key(KeyCode::Char('j'))); // select 'fixes'
    app.on_key(key(KeyCode::Char('d')));
    let text = render(&app);
    assert!(text.contains("Delete changelist"));
    assert!(text.contains("Delete 'fixes'?"));
    assert!(text.contains("Its hunks move to unassigned."));
}

#[test]
fn log_entries_render_with_their_severity_glyphs() {
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());
    app.push_log(Severity::Info, "staged hunk — src/print.css @@ -41,6");
    app.push_log(Severity::Notice, "auto-captured 1 hunk → 'fixes'");
    app.push_log(Severity::Error, "Commit failed — pre-commit hook exited 1");
    let text = render(&app);
    assert!(text.contains("· staged hunk — src/print.css @@ -41,6"));
    assert!(text.contains("! auto-captured 1 hunk → 'fixes'"));
    assert!(text.contains("✗ Commit failed — pre-commit hook exited 1"));
}

#[test]
fn pins_render_as_a_banner_atop_the_log_stream() {
    let mut app = App::new("repo");
    let mut busy = snapshot();
    busy.operation = Some(GitOperation::Rebase);
    busy.files.push(ChangedFile {
        path: "src/merge.ts".into(),
        kind: ChangeKind::Conflicted,
        binary: false,
        binary_sides: None,
        hunks: Vec::new(),
    });
    app.apply_snapshot(busy);
    app.on_watcher_degraded();
    let text = render(&app);
    assert!(text.contains("▲ watcher unavailable — polling"));
    assert!(text.contains("▲ rebase in progress — 1 conflicted"));
    // The banner sits above the stream: the pin row precedes the
    // rebase-detected event line.
    let pin = text.find("▲ rebase in progress").unwrap();
    let event = text.find("! rebase detected").unwrap();
    assert!(pin < event, "pins render above the event stream");
}

#[test]
fn the_conflicts_group_renders_first_without_stage_marks() {
    let mut app = App::new("repo");
    let mut busy = snapshot();
    busy.files.push(ChangedFile {
        path: "src/merge.ts".into(),
        kind: ChangeKind::Conflicted,
        binary: false,
        binary_sides: None,
        hunks: Vec::new(),
    });
    app.apply_snapshot(busy);
    let text = render(&app);
    assert!(text.contains("▾ conflicts (1)"));
    assert!(text.contains("U src/merge.ts"));
    let conflicts = text.find("▾ conflicts").unwrap();
    let fixes = text.find("▾ fixes").unwrap();
    assert!(conflicts < fixes, "the Conflicts group renders first");
    assert!(
        !text.contains("○ U src/merge.ts"),
        "no stage mark on a quarantined row"
    );
}

#[test]
fn the_error_modal_renders_the_detail_verbatim() {
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());
    app.show_error(
        "Commit failed",
        "husky - pre-commit hook exited with code 1\neslint: 3 problems",
    );
    let text = render(&app);
    assert!(text.contains("Commit failed"));
    assert!(text.contains("husky - pre-commit hook exited with code 1"));
    assert!(text.contains("eslint: 3 problems"));
    assert!(text.contains("dismiss"));
}

#[test]
fn focused_panel_number_keys_move_the_border_highlight() {
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());
    app.on_key(key(KeyCode::Char('4')));
    assert_eq!(app.focus, Panel::Commits);
    render(&app); // must not panic
}

// ── commit flow overlays (ticket #33, commit-flow prototype A–D) ────

fn payload(staged: usize, stale: usize) -> gitchange_core::CommitPayload {
    gitchange_core::CommitPayload {
        files: vec![gitchange_core::PayloadFile {
            path: "src/print.css".into(),
            staged_hunks: staged,
            stale_hunks: stale,
            hunks: Vec::new(),
            whole_file: None,
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

#[test]
fn the_keybar_shows_staging_and_commit_hints() {
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());
    app.on_key(key(KeyCode::Char('3')));
    let text = render(&app);
    assert!(text.contains("space stage file"), "{text}");
    assert!(text.contains("c commit"));
}
