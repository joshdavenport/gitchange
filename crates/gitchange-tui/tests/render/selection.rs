//! Full-width selection tints (issue #45): a selected row's tint runs
//! from the inner left edge to the border and stops there.

use ratatui::crossterm::event::KeyCode;
use ratatui::style::Color;

use gitchange_tui::app::App;
use gitchange_tui::theme::Theme;

use crate::helpers::{
    assert_bg_span, bg_at, border_right_of, fg_at, find_text, key, render_buffer, snapshot,
};

#[test]
fn the_selected_files_row_tint_spans_the_full_inner_width() {
    let theme = Theme::default();
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());
    app.on_key(key(KeyCode::Char('3'))); // the tint needs Files focus
    let buffer = render_buffer(&app);

    // The initially selected file row (print.css, under 'fixes').
    let (x, y) = find_text(&buffer, "src/print.css 1/2");
    let border = border_right_of(&buffer, x, y);
    // From the inner left edge past the text's end up to the border.
    assert_bg_span(&buffer, y, 1, border, theme.colors.selection);
    assert_eq!(
        bg_at(&buffer, border, y),
        Color::Reset,
        "border stays untinted"
    );
}

#[test]
fn a_non_selected_files_row_carries_no_tint() {
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());
    let buffer = render_buffer(&app);

    let (x, y) = find_text(&buffer, "src/nav.astro");
    let border = border_right_of(&buffer, x, y);
    assert_bg_span(&buffer, y, 1, border, Color::Reset);
}

#[test]
fn the_hunk_mode_selection_tint_spans_the_full_inner_width() {
    let theme = Theme::default();
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());
    app.on_key(key(KeyCode::Char('3')));
    app.on_key(key(KeyCode::Enter)); // hunk mode, hunk 1 of print.css
    let buffer = render_buffer(&app);

    // Both the selected hunk's header and its content rows tint.
    for needle in ["@@ -14", "+added at 14"] {
        let (x, y) = find_text(&buffer, needle);
        let border = border_right_of(&buffer, x, y);
        assert_bg_span(&buffer, y, x, border, theme.colors.selection);
        assert_eq!(
            bg_at(&buffer, border, y),
            Color::Reset,
            "border stays untinted"
        );
    }
    // The unselected hunk does not.
    let (x, y) = find_text(&buffer, "+added at 63");
    let border = border_right_of(&buffer, x, y);
    assert_bg_span(&buffer, y, x, border, Color::Reset);
}

#[test]
fn the_pin_banner_tint_spans_the_full_inner_width() {
    let theme = Theme::default();
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());
    app.on_watcher_degraded();
    let buffer = render_buffer(&app);

    let (x, y) = find_text(&buffer, "▲ watcher unavailable");
    let border = border_right_of(&buffer, x, y);
    assert_bg_span(&buffer, y, x, border, theme.colors.selection);
    assert_eq!(
        fg_at(&buffer, x, y),
        theme.colors.warn,
        "banner keeps its warn fg"
    );
    assert_eq!(
        bg_at(&buffer, border, y),
        Color::Reset,
        "border stays untinted"
    );
}

#[test]
fn the_assign_popup_selection_tint_spans_the_popup_inner_width() {
    let theme = Theme::default();
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());
    app.on_key(key(KeyCode::Char('3')));
    app.on_key(key(KeyCode::Char('a')));
    let buffer = render_buffer(&app);

    let (x, y) = find_text(&buffer, "fixes (active)");
    let border = border_right_of(&buffer, x, y);
    assert_bg_span(&buffer, y, x, border, theme.colors.selection);
    assert_eq!(
        bg_at(&buffer, border, y),
        Color::Reset,
        "border stays untinted"
    );
}
