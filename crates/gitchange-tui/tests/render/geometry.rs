//! The left column's geometry (issue #87).
//!
//! These read panel heights off the rendered frame rather than asserting
//! height tuples: the invariants are "every panel is drawn", "the cursor
//! is on screen" and "Commits keeps its share", all of which survive
//! anyone re-tuning the column.

use gitchange_core::{Changelist, CommitInfo, Head, Snapshot};
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout, Rect};

use gitchange_tui::app::{App, Panel};
use gitchange_tui::theme::Theme;

use crate::frame::{cursor_rows, left_panel_height};
use crate::helpers::{key, render_buffer_sized, text_of};

/// A repo with `count` user changelists and nothing else — the
/// Changelists panel draws `count + 2` rows, `all` and `unassigned`
/// bracketing them.
fn changelists_snapshot(count: usize) -> Snapshot {
    Snapshot {
        files: Vec::new(),
        changelists: (0..count)
            .map(|index| Changelist {
                name: format!("list-{index:03}"),
            })
            .collect(),
        active: None,
        head: Head::Branch {
            name: "main".into(),
        },
        recent_commits: vec![CommitInfo {
            short_id: "91a05c13".into(),
            author: "Josh Davenport-Smith".into(),
            summary: "fix: viewport sizing".into(),
        }],
        operation: None,
    }
}

/// The frames the column tests measure: 80 columns wide, `height` rows
/// tall — the narrow shape the failure was reproduced at.
fn render_buffer_at(app: &App, height: u16) -> Buffer {
    render_buffer_sized(app, &Theme::default(), 80, height)
}

/// Changelist counts spanning the panel's whole range: a handful that
/// fits any terminal, tens that overflow a short one, hundreds that
/// overflow every one.
const CHANGELIST_COUNTS: [usize; 8] = [0, 1, 5, 10, 20, 40, 100, 300];

#[test]
fn every_left_column_panel_is_drawn_from_twenty_rows_up() {
    let column = [
        Panel::Status,
        Panel::Changelists,
        Panel::Files,
        Panel::Commits,
    ];
    for count in CHANGELIST_COUNTS {
        let mut app = App::new("repo");
        app.apply_snapshot(changelists_snapshot(count));
        for height in 20..=120 {
            let buffer = render_buffer_at(&app, height);
            for panel in column {
                let rows = left_panel_height(&buffer, panel.title());
                assert!(
                    rows >= 3,
                    "{} has no content row at height {height} with {count} changelists\n{}",
                    panel.title(),
                    text_of(&buffer)
                );
            }
        }
    }
}

#[test]
fn the_changelists_cursor_is_on_screen_at_every_scroll_position() {
    for count in CHANGELIST_COUNTS {
        let rows = count + 2;
        // First row, middle, last row, and one step past it.
        for steps in [0, rows / 2, rows - 1, rows] {
            let mut app = App::new("repo");
            app.apply_snapshot(changelists_snapshot(count));
            for _ in 0..steps {
                app.on_key(key(KeyCode::Char('j')));
            }
            for height in [20, 24, 30, 40, 60, 120] {
                let buffer = render_buffer_at(&app, height);
                let visible = cursor_rows(&buffer, Panel::Changelists.title());
                assert_eq!(
                    visible.len(),
                    1,
                    "cursor rows {visible:?} at height {height}, \
                     {count} changelists, {steps} steps down\n{}",
                    text_of(&buffer)
                );
                // The last row is the one the bug hid: the bottom-right
                // count read 'unassigned' as selected, and the Files and
                // Diff panels followed it, with no cursor on screen.
                if steps >= rows - 1 {
                    assert!(
                        visible[0].contains("unassigned"),
                        "cursor sits on {:?}, not the last row, at height {height} \
                         with {count} changelists",
                        visible[0]
                    );
                }
            }
        }
    }
}

#[test]
fn a_changelists_panel_under_the_ceiling_is_exactly_its_own_size() {
    for count in [0, 1, 5, 10] {
        let mut app = App::new("repo");
        app.apply_snapshot(changelists_snapshot(count));
        let buffer = render_buffer_at(&app, 60);
        let expected = app.changelist_rows().len() as u16 + 2;
        assert_eq!(
            left_panel_height(&buffer, Panel::Changelists.title()),
            expected,
            "{count} changelists at height 60\n{}",
            text_of(&buffer)
        );
    }
    // Spelled out once, unchanged from before the cap: three changelist
    // rows — `all`, one list, `unassigned` — draw a five-row panel.
    let mut app = App::new("repo");
    app.apply_snapshot(changelists_snapshot(1));
    let buffer = render_buffer_at(&app, 40);
    assert_eq!(left_panel_height(&buffer, Panel::Changelists.title()), 5);
}

#[test]
fn the_commits_panel_keeps_the_share_of_the_column_it_had() {
    for count in CHANGELIST_COUNTS {
        let mut app = App::new("repo");
        app.apply_snapshot(changelists_snapshot(count));
        // Far enough up to cross every column height where rounding the
        // percentage the wrong way would diverge: 25, 75, 125 and 175.
        for height in 20..=180 {
            // ratatui's own answer for the percentage this panel carried,
            // over the same column: the frame less the keybar row.
            let column = Rect::new(0, 0, 80, height - 1);
            let [_, share]: [Rect; 2] =
                Layout::vertical([Constraint::Min(0), Constraint::Percentage(26)]).areas(column);
            // From 20 rows up the percentage clears the panel's floor, so
            // the floor never masks a divergence here.
            assert!(share.height >= 5, "floor reached at height {height}");
            let buffer = render_buffer_at(&app, height);
            assert_eq!(
                left_panel_height(&buffer, Panel::Commits.title()),
                share.height,
                "Commits at height {height} with {count} changelists\n{}",
                text_of(&buffer)
            );
        }
    }
}
