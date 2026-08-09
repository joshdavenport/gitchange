//! Focus-conditional selection and cursors (issue #45): the tint follows
//! focus and disappears on blur, while the cursor glyph persists.

use ratatui::buffer::Buffer;
use ratatui::crossterm::event::KeyCode;
use ratatui::style::Color;

use gitchange_tui::app::{App, Panel};
use gitchange_tui::theme::Theme;

use crate::frame::count_tail;
use crate::helpers::{
    assert_bg_span, border_right_of, fg_at, find_text, key, render_buffer, render_buffer_themed,
    snapshot,
};

#[test]
fn the_selection_tint_follows_panel_focus() {
    let theme = Theme::default();
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());

    // Default focus is Changelists: its selected row tints; the Files
    // and Commits selections do not.
    let buffer = render_buffer(&app);
    let (cx, cy) = find_text(&buffer, "≡ all");
    let c_border = border_right_of(&buffer, cx, cy);
    let (fx, fy) = find_text(&buffer, "src/print.css 1/2");
    let f_border = border_right_of(&buffer, fx, fy);
    let (kx, ky) = find_text(&buffer, "91a05c13");
    let k_border = border_right_of(&buffer, kx, ky);
    assert_bg_span(&buffer, cy, cx, c_border, theme.colors.selection);
    assert_bg_span(&buffer, fy, fx, f_border, Color::Reset);
    assert_bg_span(&buffer, ky, kx, k_border, Color::Reset);

    // Focus Files: the tints swap.
    app.on_key(key(KeyCode::Char('3')));
    let buffer = render_buffer(&app);
    assert_bg_span(&buffer, cy, cx, c_border, Color::Reset);
    assert_bg_span(&buffer, fy, fx, f_border, theme.colors.selection);
    assert_bg_span(&buffer, ky, kx, k_border, Color::Reset);

    // Focus Diff (scroll mode): no panel shows a selection tint.
    app.on_key(key(KeyCode::Char('0')));
    let buffer = render_buffer(&app);
    assert_bg_span(&buffer, cy, cx, c_border, Color::Reset);
    assert_bg_span(&buffer, fy, fx, f_border, Color::Reset);
    assert_bg_span(&buffer, ky, kx, k_border, Color::Reset);
}

#[test]
fn hunk_headers_share_one_cursor_column_in_hunk_mode() {
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());

    // Outside hunk mode there is no cursor column on headers.
    let buffer = render_buffer(&app);
    let (plain_x, _) = find_text(&buffer, "@@ -14");

    app.on_key(key(KeyCode::Char('3')));
    app.on_key(key(KeyCode::Enter)); // hunk mode, hunk 1 selected
    let buffer = render_buffer(&app);
    let (sel_x, sel_y) = find_text(&buffer, "@@ -14");
    let (other_x, other_y) = find_text(&buffer, "@@ -63");
    assert_eq!(sel_x, other_x, "headers stay aligned");
    assert_eq!(sel_x, plain_x + 2, "one cursor column ahead of scroll mode");
    assert_eq!(buffer[(sel_x - 2, sel_y)].symbol(), "❯");
    // The unselected header keeps a blank stand-in, not a collapsed row.
    assert_eq!(buffer[(other_x - 2, other_y)].symbol(), " ");
    assert_eq!(buffer[(other_x - 1, other_y)].symbol(), " ");
}

#[test]
fn the_panel_frame_text_follows_focus() {
    // Three distinct colours, because the default theme leaves both
    // `border` and `title` at Reset: against defaults, blurred frame text
    // would read the same whether it were styled or not.
    let mut theme = Theme::default();
    theme.colors.border = Color::Blue;
    theme.colors.title = Color::Magenta;
    theme.colors.border_focus = Color::Green;
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());

    // Every text a frame carries moves together: the `[n]─` prefix on the
    // top border and the count on the bottom. Default focus is
    // Changelists, so its frame text takes the focus colour and the
    // blurred Files frame takes the title colour. Both sides read the
    // theme's own tokens — the point is that the text follows focus, not
    // which two colours a theme picks.
    let buffer = render_buffer_themed(&app, &theme);
    let (cx, cy) = find_text(&buffer, "[2]─");
    let (fx, fy) = find_text(&buffer, "[3]─");
    for dx in 0..4 {
        assert_eq!(
            fg_at(&buffer, cx + dx, cy),
            theme.colors.border_focus,
            "focused [2]─ cell {dx}"
        );
        assert_eq!(
            fg_at(&buffer, fx + dx, fy),
            theme.colors.title,
            "blurred [3]─ cell {dx}"
        );
    }
    let (ccx, ccy) = count_tail(&buffer, Panel::Changelists.title());
    let (fcx, fcy) = count_tail(&buffer, Panel::Files.title());
    assert_eq!(
        fg_at(&buffer, ccx, ccy),
        theme.colors.border_focus,
        "focused Changelists count"
    );
    assert_eq!(
        fg_at(&buffer, fcx, fcy),
        theme.colors.title,
        "blurred Files count"
    );

    // Focus Files: the colours swap, top border and bottom count alike.
    app.on_key(key(KeyCode::Char('3')));
    let buffer = render_buffer_themed(&app, &theme);
    for dx in 0..4 {
        assert_eq!(
            fg_at(&buffer, cx + dx, cy),
            theme.colors.title,
            "blurred [2]─ cell {dx}"
        );
        assert_eq!(
            fg_at(&buffer, fx + dx, fy),
            theme.colors.border_focus,
            "focused [3]─ cell {dx}"
        );
    }
    assert_eq!(
        fg_at(&buffer, ccx, ccy),
        theme.colors.title,
        "blurred Changelists count"
    );
    assert_eq!(
        fg_at(&buffer, fcx, fcy),
        theme.colors.border_focus,
        "focused Files count"
    );
}

#[test]
fn the_commits_selection_tint_disappears_entirely_on_blur() {
    let theme = Theme::default();
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());
    app.on_key(key(KeyCode::Char('4')));
    let buffer = render_buffer(&app);
    let (x, y) = find_text(&buffer, "91a05c13");
    let border = border_right_of(&buffer, x, y);
    assert_bg_span(&buffer, y, x, border, theme.colors.selection);

    app.on_key(key(KeyCode::Char('2')));
    let buffer = render_buffer(&app);
    assert_bg_span(&buffer, y, x, border, Color::Reset);
    // No persistent marker of any kind: no cursor glyph on the row.
    for cell_x in 1..border {
        assert_ne!(buffer[(cell_x, y)].symbol(), "❯");
    }
}

#[test]
fn the_changelists_cursor_persists_on_blur_and_never_recolours() {
    let theme = Theme::default();
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());
    app.on_key(key(KeyCode::Char('j'))); // select 'fixes'

    let assert_cursor_row = |buffer: &Buffer, bg: Color| {
        let (x, y) = find_text(buffer, "* fixes");
        assert_eq!(buffer[(x - 2, y)].symbol(), "❯", "cursor leads the row");
        assert_eq!(fg_at(buffer, x - 2, y), theme.colors.cursor);
        // The type glyph and name keep their own colours.
        assert_eq!(fg_at(buffer, x, y), theme.colors.active);
        assert_eq!(fg_at(buffer, x + 2, y), theme.colors.changelist);
        let border = border_right_of(buffer, x, y);
        assert_bg_span(buffer, y, x - 2, border, bg);
    };

    assert_cursor_row(&render_buffer(&app), theme.colors.selection);
    app.on_key(key(KeyCode::Char('4'))); // blur: cursor stays, tint goes
    assert_cursor_row(&render_buffer(&app), Color::Reset);
}

#[test]
fn unselected_changelist_rows_keep_a_blank_cursor_column() {
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());
    let buffer = render_buffer(&app); // 'all' selected

    // The blank stand-in keeps the glyph and name columns aligned.
    let (all_x, all_y) = find_text(&buffer, "≡ all");
    let (fixes_x, fixes_y) = find_text(&buffer, "* fixes");
    assert_eq!(fixes_x, all_x, "glyph columns align");
    let (chores_x, _) = find_text(&buffer, "chores (1)");
    assert_eq!(chores_x, all_x + 2, "name columns align");
    assert_eq!(buffer[(all_x - 2, all_y)].symbol(), "❯");
    assert_eq!(buffer[(all_x - 2, fixes_y)].symbol(), " ");
    assert_eq!(buffer[(all_x - 1, fixes_y)].symbol(), " ");
}

#[test]
fn the_files_stage_glyph_doubles_as_the_cursor_on_the_selected_row() {
    let theme = Theme::default();
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());
    app.on_key(key(KeyCode::Char('3')));

    // print.css (selected, partially staged ◐) takes the cursor colour;
    // nav.astro's ○ keeps its normal dim.
    let assert_glyphs = |buffer: &Buffer| {
        let (x, y) = find_text(buffer, "src/print.css 1/2");
        assert_eq!(buffer[(x - 4, y)].symbol(), "◐");
        assert_eq!(fg_at(buffer, x - 4, y), theme.colors.cursor);
        let (ox, oy) = find_text(buffer, "src/nav.astro");
        assert_eq!(buffer[(ox - 4, oy)].symbol(), "○");
        assert_eq!(fg_at(buffer, ox - 4, oy), theme.colors.dim);
    };

    assert_glyphs(&render_buffer(&app));
    app.on_key(key(KeyCode::Char('4'))); // blur: the cursor colour stays
    assert_glyphs(&render_buffer(&app));
}

#[test]
fn the_hunk_cursor_glyph_survives_a_blur() {
    let theme = Theme::default();
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());
    app.on_key(key(KeyCode::Char('3')));
    app.on_key(key(KeyCode::Enter)); // hunk mode, hunk 1 selected

    let buffer = render_buffer(&app);
    let (x, y) = find_text(&buffer, "@@ -14");
    assert_eq!(buffer[(x - 2, y)].symbol(), "❯");
    assert_eq!(fg_at(&buffer, x - 2, y), theme.colors.cursor);
    let border = border_right_of(&buffer, x, y);
    assert_bg_span(&buffer, y, x, border, theme.colors.selection);

    // Blur to Commits: the selection survives (the assign keys act on it
    // cross-panel), the cursor stays, only the tint goes.
    app.on_key(key(KeyCode::Char('4')));
    assert_eq!(app.hunk_sel, Some(0), "hunk mode survives a blur");
    let buffer = render_buffer(&app);
    assert_eq!(buffer[(x - 2, y)].symbol(), "❯");
    assert_eq!(fg_at(&buffer, x - 2, y), theme.colors.cursor);
    assert_bg_span(&buffer, y, x, border, Color::Reset);
}

#[test]
fn the_cursor_tokens_drive_the_rendering() {
    let mut theme = Theme::default();
    theme.glyphs.cursor = '▶';
    theme.colors.cursor = Color::LightBlue;
    // The default must stay distinct from the glyphs it sits beside.
    let default = Theme::default();
    assert_ne!(default.colors.cursor, default.colors.active);
    assert_ne!(default.colors.cursor, default.colors.changelist);

    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());
    let buffer = render_buffer_themed(&app, &theme);
    let (x, y) = find_text(&buffer, "≡ all");
    assert_eq!(buffer[(x - 2, y)].symbol(), "▶");
    assert_eq!(fg_at(&buffer, x - 2, y), Color::LightBlue);
}
