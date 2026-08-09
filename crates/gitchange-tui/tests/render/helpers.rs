//! The fixtures and buffer readers every render module shares: a
//! two-file snapshot, one frame at a pinned size, and cell-level
//! assertions over the result.

use std::time::Instant;

use gitchange_core::{
    ChangeKind, ChangedFile, Changelist, CommitInfo, Head, Hunk, HunkIdentity, HunkLine, HunkStage,
    Snapshot,
};
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Color;
use ratatui::{Terminal, backend::TestBackend};

use gitchange_tui::app::App;
use gitchange_tui::theme::Theme;
use gitchange_tui::ui;

pub(crate) fn hunk(new_start: u32, changelist: Option<&str>, stage: HunkStage) -> Hunk {
    Hunk {
        old_start: new_start,
        old_lines: 1,
        new_start,
        new_lines: 2,
        stage,
        index_only: false,
        identity: HunkIdentity::Text {
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
        },
        changelist: changelist.map(str::to_owned),
    }
}

pub(crate) fn snapshot() -> Snapshot {
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
        advisories: Vec::new(),
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

pub(crate) fn render_buffer(app: &App) -> Buffer {
    render_buffer_themed(app, &Theme::default())
}

pub(crate) fn render_buffer_themed(app: &App, theme: &Theme) -> Buffer {
    render_buffer_sized(app, theme, 140, 40)
}

/// One frame at an arbitrary terminal size — every other render helper
/// is this one with a size pinned.
pub(crate) fn render_buffer_sized(app: &App, theme: &Theme, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| ui::draw(frame, app, theme, Instant::now()))
        .unwrap();
    terminal.backend().buffer().clone()
}

pub(crate) fn render(app: &App) -> String {
    text_of(&render_buffer(app))
}

/// A buffer's rows as newline-joined text — split from `render` for the
/// tests that assert on both the text and the cells behind it.
pub(crate) fn text_of(buffer: &Buffer) -> String {
    let mut text = String::new();
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            text.push_str(buffer[(x, y)].symbol());
        }
        text.push('\n');
    }
    text
}

/// The (x, y) of the first occurrence of `needle` in the buffer.
pub(crate) fn find_text(buffer: &Buffer, needle: &str) -> (u16, u16) {
    for y in 0..buffer.area.height {
        let symbols: Vec<&str> = (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect();
        if let Some(byte) = symbols.concat().find(needle) {
            let mut offset = 0;
            for (x, symbol) in symbols.iter().enumerate() {
                if offset == byte {
                    return (x as u16, y);
                }
                offset += symbol.len();
            }
        }
    }
    panic!("text {needle:?} not found in buffer");
}

/// The x of the first `│` border cell at/right of `from` on row `y` —
/// the panel edge a row tint must stop short of.
pub(crate) fn border_right_of(buffer: &Buffer, from: u16, y: u16) -> u16 {
    (from..buffer.area.width)
        .find(|&x| buffer[(x, y)].symbol() == "│")
        .unwrap_or_else(|| panic!("no │ border right of ({from},{y})"))
}

pub(crate) fn bg_at(buffer: &Buffer, x: u16, y: u16) -> Color {
    buffer[(x, y)].bg
}

pub(crate) fn fg_at(buffer: &Buffer, x: u16, y: u16) -> Color {
    buffer[(x, y)].fg
}

/// Assert every cell in `x_start..x_end` on row `y` carries `bg` —
/// pass `Color::Reset` to assert the absence of a tint.
pub(crate) fn assert_bg_span(buffer: &Buffer, y: u16, x_start: u16, x_end: u16, bg: Color) {
    for x in x_start..x_end {
        assert_eq!(
            bg_at(buffer, x, y),
            bg,
            "cell ({x},{y}) {:?} bg",
            buffer[(x, y)].symbol()
        );
    }
}

pub(crate) fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}
