//! Overlays and modals: the help sheet, the assign popup, text inputs,
//! the delete confirmation, and the error modal.

use ratatui::crossterm::event::KeyCode;

use gitchange_tui::app::App;
use gitchange_tui::theme::Theme;

use crate::helpers::{key, render, render_buffer_themed, snapshot, text_of};

#[test]
fn help_overlay_lists_bindings() {
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());
    app.on_key(key(KeyCode::Char('?')));
    let text = render(&app);
    assert!(text.contains("Keybindings"));
    assert!(text.contains("focus panel"));
    // The assign trio and the switch key it displaced (ADR 0013).
    assert!(text.contains("assign hunk"));
    assert!(text.contains("assign the file's unassigned hunks"));
    assert!(text.contains("ctrl+a"));
    assert!(text.contains("switch active changelist"));
    // Retired: neither survives anywhere in the overlay.
    assert!(!text.contains("shift+enter"));
    assert!(!text.contains("move file to changelist"));
}

#[test]
fn the_help_overlay_derives_spellings_and_themes_its_arrows() {
    let mut theme = Theme::default();
    theme.glyphs.arrow = '»';
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());
    app.on_key(key(KeyCode::Char('?')));
    let text = text_of(&render_buffer_themed(&app, &theme));
    // Derived spellings: the panel digits from the panel numbering, and
    // each movement pair merged into one row with all its keys — blanks
    // between the two spellings, since a comma is itself a bound key.
    assert!(text.contains("1-5, 0"), "{text}");
    assert!(text.contains("j/k  ↓/↑"), "{text}");
    assert!(text.contains("./,  PgDn/PgUp"), "{text}");
    assert!(text.contains(">/<"), "{text}");
    // The drill arrows take the theme's glyph — no `→` literals left.
    assert!(
        text.contains("drill in: changelist » files » hunk mode"),
        "{text}"
    );
    assert!(
        text.contains("back (diff » files » changelists » all)"),
        "{text}"
    );
}

#[test]
fn the_assign_popup_lists_changelists_and_the_escape_hatch() {
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());
    app.on_key(key(KeyCode::Char('3')));
    app.on_key(key(KeyCode::Char('a')));
    let text = render(&app);
    assert!(text.contains("Assign to changelist"));
    // The payload states itself before it lands: the row is print.css
    // under 'fixes', which owns one of its two hunks.
    assert!(text.contains("1 hunk in src/print.css"));
    assert!(text.contains("fixes (active)"));
    assert!(text.contains("+ create new changelist…"));
    assert!(text.contains("enter assign · esc cancel"));
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
    assert!(text.contains("Its hunks become unassigned."));
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
