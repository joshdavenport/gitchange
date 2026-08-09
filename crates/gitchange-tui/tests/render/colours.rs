//! Diff origin colours survive the decorator (issue #46): `+`/`-` keep
//! their fg under both the DIM of a foreign row and a selection tint.

use gitchange_core::{HunkIdentity, HunkLine};
use ratatui::crossterm::event::KeyCode;
use ratatui::style::Modifier;

use gitchange_tui::app::App;
use gitchange_tui::theme::Theme;

use crate::helpers::{bg_at, fg_at, find_text, key, render_buffer, snapshot};

#[test]
fn diff_content_lines_keep_their_origin_colours() {
    let theme = Theme::default();
    let mut app = App::new("repo");
    let mut snap = snapshot();
    // The fixture hunks carry no deletions; add one for the `-` case.
    if let HunkIdentity::Text { lines } = &mut snap.files[1].hunks[0].identity {
        lines.push(HunkLine {
            origin: '-',
            content: "removed at 14\n".into(),
        });
    }
    app.apply_snapshot(snap);
    let buffer = render_buffer(&app);

    // Plain rows: `+` added, `-` deleted, context text.
    let (x, y) = find_text(&buffer, "+added at 14");
    assert_eq!(fg_at(&buffer, x, y), theme.colors.added);
    let (x, y) = find_text(&buffer, "-removed at 14");
    assert_eq!(fg_at(&buffer, x, y), theme.colors.deleted);

    // Foreign rows (drilled into 'fixes', the chores hunk dims) keep
    // the origin colour under the DIM modifier.
    app.on_key(key(KeyCode::Char('j')));
    let buffer = render_buffer(&app);
    let (x, y) = find_text(&buffer, "+added at 63");
    assert_eq!(fg_at(&buffer, x, y), theme.colors.added);
    assert!(
        buffer[(x, y)].modifier.contains(Modifier::DIM),
        "foreign row keeps its DIM"
    );
}

#[test]
fn the_hunk_mode_selection_keeps_the_origin_colour_under_its_tint() {
    let theme = Theme::default();
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());
    app.on_key(key(KeyCode::Char('3')));
    app.on_key(key(KeyCode::Enter)); // hunk mode, hunk 1 of print.css
    let buffer = render_buffer(&app);

    let (x, y) = find_text(&buffer, "+added at 14");
    assert_eq!(fg_at(&buffer, x, y), theme.colors.added);
    assert_eq!(bg_at(&buffer, x, y), theme.colors.selection);
}
