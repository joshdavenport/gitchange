//! Rendering smoke tests over ratatui's TestBackend: the panel stack
//! draws, the All view groups and tags, drilled views dim — asserted on
//! buffer text, not exact cells (snapshot-style, opportunistic per
//! ADR 0008).

use std::time::{Duration, Instant};

use gitchange_core::{
    ChangeKind, ChangedFile, Changelist, CommitInfo, Head, Hunk, HunkLine, HunkStage, Snapshot,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend};

use gitchange_tui::app::{App, INDICATOR_DELAY, Panel};
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
                hunks: vec![hunk(8, None, HunkStage::Unstaged)],
            },
            ChangedFile {
                path: "src/print.css".into(),
                kind: ChangeKind::Modified,
                binary: false,
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
fn op_feedback_lines_render_in_the_log_placeholder() {
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());
    app.push_feedback(["a changelist named 'docs' already exists".to_owned()]);
    let text = render(&app);
    assert!(text.contains("! a changelist named 'docs' already exists"));
}

#[test]
fn focused_panel_number_keys_move_the_border_highlight() {
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());
    app.on_key(key(KeyCode::Char('4')));
    assert_eq!(app.focus, Panel::Commits);
    render(&app); // must not panic
}
