//! Panel geometry read off a rendered frame rather than asserted as
//! height tuples, so the invariants survive anyone re-tuning the column.
//! Used by the focus and geometry modules.

use ratatui::buffer::Buffer;

use gitchange_tui::theme::Theme;

use crate::helpers::text_of;

/// A buffer row as text.
pub(crate) fn row_text(buffer: &Buffer, y: u16) -> String {
    (0..buffer.area.width)
        .map(|x| buffer[(x, y)].symbol())
        .collect()
}

/// The rows a left-column panel occupies, borders included, read off its
/// own frame: the titled top corner down to the next bottom corner.
/// `None` when the panel is not drawn, or is drawn without a frame to
/// measure.
pub(crate) fn left_panel_rows(buffer: &Buffer, title: &str) -> Option<(u16, u16)> {
    // The frame's own glyphs, so a theme that redraws the border does not
    // hide every panel from this helper.
    let frame = Theme::default().glyphs.panel_border.to_border_set();
    let corner = |y: u16| buffer[(0, y)].symbol();
    let top = (0..buffer.area.height)
        .find(|&y| corner(y) == frame.top_left && row_text(buffer, y).contains(title))?;
    let bottom = (top + 1..buffer.area.height).find(|&y| corner(y) != frame.vertical_left)?;
    (corner(bottom) == frame.bottom_left).then_some((top, bottom))
}

/// The last cell of a left-column panel's bottom-right count, found from
/// the corner the count is inset off: the trailing rule sits against the
/// corner, the count's own last cell one further left.
pub(crate) fn count_tail(buffer: &Buffer, title: &str) -> (u16, u16) {
    let frame = Theme::default().glyphs.panel_border.to_border_set();
    let (_, bottom) =
        left_panel_rows(buffer, title).unwrap_or_else(|| panic!("panel {title} is not drawn"));
    let corner = (0..buffer.area.width)
        .find(|&x| buffer[(x, bottom)].symbol() == frame.bottom_right)
        .unwrap_or_else(|| panic!("panel {title} does not close its bottom border"));
    let x = corner - 2;
    // A count ends in its total, so the cell is a digit. Asserted here
    // because the border shares the focus colour: a caller reading a
    // border cell by mistake would still see the colour it expects.
    let symbol = buffer[(x, bottom)].symbol();
    assert!(
        symbol.chars().all(|c| c.is_ascii_digit()),
        "panel {title} count tail is {symbol:?}, not a digit"
    );
    (x, bottom)
}

/// A left-column panel's outer height, borders included.
pub(crate) fn left_panel_height(buffer: &Buffer, title: &str) -> u16 {
    let (top, bottom) = left_panel_rows(buffer, title).unwrap_or_else(|| {
        panic!(
            "panel {title} is not drawn\n{}",
            text_of(buffer).replace('\n', "|\n")
        )
    });
    bottom - top + 1
}

/// The panel's content rows carrying the selection cursor — one, in a
/// frame that draws the selection at all.
pub(crate) fn cursor_rows(buffer: &Buffer, title: &str) -> Vec<String> {
    let (top, bottom) =
        left_panel_rows(buffer, title).unwrap_or_else(|| panic!("panel {title} is not drawn"));
    (top + 1..bottom)
        .map(|y| row_text(buffer, y))
        .filter(|row| row.contains('❯'))
        .collect()
}
