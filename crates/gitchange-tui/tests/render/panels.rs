//! The panel stack itself: every panel draws, the All view groups and
//! tags, drilled views scope, and the keybar advertises only what the
//! keymap has.

use std::time::Duration;
use std::time::Instant;

use gitchange_core::{ChangeKind, ChangedFile, Head};
use ratatui::crossterm::event::KeyCode;
use ratatui::style::Color;

use gitchange_tui::app::{App, INDICATOR_DELAY, Panel};
use gitchange_tui::theme::Theme;

use crate::helpers::{fg_at, find_text, key, render, render_buffer, snapshot, text_of};

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
    // Scoped title counts: 1 own hunk staged of 2, 1 elsewhere.
    assert!(text.contains("print.css (1/2 staged · 1 hunk elsewhere)"));
}

#[test]
fn capture_off_moves_the_active_marker_onto_unassigned() {
    // ADR 0015: `switch unassigned` has no indicator of its own — the
    // `*` moving onto the unassigned row and group header is it, on both
    // the Changelists panel and the All view.
    let mut app = App::new("repo");
    let mut off = snapshot();
    off.active = None;
    app.apply_snapshot(off);
    let text = render(&app);

    assert!(
        text.contains("unassigned *"),
        "the group header wears it\n{text}"
    );
    assert!(!text.contains("fixes *"), "and 'fixes' has given it up");
    // `all` is selected: `s` has no target there, so the bar hides it.
    assert!(!text.contains("switch active"), "{text}");

    app.on_key(key(KeyCode::Char('j')));
    app.on_key(key(KeyCode::Char('j')));
    app.on_key(key(KeyCode::Char('j'))); // 'unassigned'
    let text = render(&app);
    assert!(
        text.contains("switch active"),
        "`s` is live on the unassigned row\n{text}"
    );
}

#[test]
fn the_status_panel_names_the_holder_of_the_active_marker() {
    let theme = Theme::default();
    // The status line reads whole, and the suffix mirrors the
    // Changelists panel's styling for the same item: active-marker
    // glyph, then the name in its ownership colour.
    let assert_line = |app: &App, line: &str, name: Color| {
        let buffer = render_buffer(app);
        let text = text_of(&buffer);
        assert!(text.contains(line), "expected {line:?}\n{text}");
        let (x, y) = find_text(&buffer, line);
        // Cells, not bytes: ✓ and → are multi-byte, one cell each.
        let marker = x + line.chars().take_while(|&c| c != '*').count() as u16;
        assert_eq!(fg_at(&buffer, marker, y), theme.colors.active);
        assert_eq!(fg_at(&buffer, marker + 2, y), name);
    };

    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());
    assert_line(
        &app,
        "✓ repo → feat/print-page * fixes",
        theme.colors.changelist,
    );

    // Capture off (ADR 0015): the marker sits on unassigned, in warn.
    let mut off = snapshot();
    off.active = None;
    app.apply_snapshot(off);
    assert_line(
        &app,
        "✓ repo → feat/print-page * unassigned",
        theme.colors.warn,
    );
}

#[test]
fn the_status_panel_names_the_active_changelist_for_every_head() {
    let mut app = App::new("repo");
    let mut detached = snapshot();
    detached.head = Head::Detached {
        short_id: "91a05c13".into(),
    };
    app.apply_snapshot(detached);
    let text = render(&app);
    assert!(
        text.contains("@91a05c13 (detached) * fixes"),
        "detached head keeps the suffix\n{text}"
    );

    let mut unborn = snapshot();
    unborn.head = Head::Unborn {
        name: "main".into(),
    };
    unborn.active = None;
    app.apply_snapshot(unborn);
    let text = render(&app);
    assert!(
        text.contains("main (no commits yet) * unassigned"),
        "unborn head keeps the suffix\n{text}"
    );
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
    assert!(text.contains("print.css — hunk 1 of 3"));
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
        sides: None,
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
    assert!(text.contains("space toggle stage row"), "{text}");
    // 'all' is scoped: commit isn't afforded, so the bar doesn't lie
    // about it (ADR 0014).
    assert!(!text.contains("c commit"), "{text}");

    app.on_key(key(KeyCode::Esc));
    app.on_key(key(KeyCode::Char('j'))); // drill into 'fixes'
    app.on_key(key(KeyCode::Enter));
    let text = render(&app);
    assert!(text.contains("space toggle stage row"), "{text}");
    assert!(text.contains("c commit"));
}
