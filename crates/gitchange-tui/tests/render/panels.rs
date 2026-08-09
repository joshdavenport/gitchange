//! The panel stack itself: every panel draws, the All view groups and
//! tags, drilled views scope, and the keybar advertises only what the
//! keymap has.

use std::time::Duration;
use std::time::Instant;

use gitchange_core::{ChangeKind, ChangedFile};
use ratatui::crossterm::event::KeyCode;

use gitchange_tui::app::{App, INDICATOR_DELAY, Panel};

use crate::helpers::{key, render, snapshot};

#[test]
fn the_panel_stack_renders_with_the_all_view() {
    let mut app = App::new("works-in-progress");
    app.apply_snapshot(snapshot());
    let text = render(&app);

    for panel in Panel::ALL {
        let title = panel.title();
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
fn the_deferred_indicator_appears_past_the_threshold() {
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());
    app.on_refresh_started(Instant::now() - INDICATOR_DELAY - Duration::from_millis(1));
    let text = render(&app);
    assert!(text.contains("refreshing…"));
    // Mid-refresh, nothing clears (ADR 0005): the frame still carries
    // the snapshot's changelists, files, and commits alongside the
    // indicator.
    assert!(text.contains("▾ fixes *"));
    assert!(text.contains("src/print.css"));
    assert!(text.contains("fix: viewport sizing"));
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
    // The bar advertises exactly the bindings the keymap has (ADR 0013).
    assert!(text.contains("a/A/ctrl+a"));
    assert!(text.contains("assign hunk / unassigned / all"));
    assert!(!text.contains("shift+enter"));
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
fn focused_panel_number_keys_move_the_border_highlight() {
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());
    app.on_key(key(KeyCode::Char('4')));
    assert_eq!(app.focus, Panel::Commits);
    render(&app); // must not panic
}

#[test]
fn the_keybar_shows_staging_and_commit_hints() {
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());
    app.on_key(key(KeyCode::Char('3')));
    let text = render(&app);
    assert!(text.contains("space toggle stage file"), "{text}");
    // 'all' is scoped: commit isn't afforded, so the bar doesn't lie
    // about it (ADR 0014).
    assert!(!text.contains("c commit"), "{text}");

    app.on_key(key(KeyCode::Esc));
    app.on_key(key(KeyCode::Char('j'))); // drill into 'fixes'
    app.on_key(key(KeyCode::Enter));
    let text = render(&app);
    assert!(text.contains("space toggle stage file"), "{text}");
    assert!(text.contains("c commit"));
}
