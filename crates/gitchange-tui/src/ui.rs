//! Rendering: the lazygit panel stack from the App's view-models.
//! Layout and treatments follow `docs/prototypes/tui-prototype.html`
//! (variants A/B); every glyph and colour comes from [`Theme`].

use std::time::Instant;

use gitchange_core::{ChangeKind, FileStage, Head, HunkStage};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

use crate::app::{App, DiffLine, FilesRow, Panel, Scope};
use crate::theme::Theme;

pub fn draw(frame: &mut Frame, app: &App, theme: &Theme, now: Instant) {
    let [main, keybar] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(frame.area());
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(44), Constraint::Percentage(56)]).areas(main);

    let changelists_height = app.changelist_rows().len() as u16 + 2;
    let [status, changelists, files, commits] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(changelists_height),
        Constraint::Min(5),
        Constraint::Percentage(26),
    ])
    .areas(left);
    let [diff, log] =
        Layout::vertical([Constraint::Min(5), Constraint::Percentage(20)]).areas(right);

    draw_status(frame, status, app, theme);
    draw_changelists(frame, changelists, app, theme);
    draw_files(frame, files, app, theme);
    draw_commits(frame, commits, app, theme);
    draw_diff(frame, diff, app, theme);
    draw_log(frame, log, app, theme);
    draw_keybar(frame, keybar, app, theme, now);
    if app.help_open {
        draw_help(frame, main, theme);
    }
}

/// A prototype-style panel frame: `[n]─Title - alt` on the top border,
/// the position count on the bottom-right.
fn panel_block<'a>(
    panel: Panel,
    title: &'a str,
    alt: Option<String>,
    count: Option<String>,
    app: &App,
    theme: &Theme,
) -> Block<'a> {
    let focused = app.focus == panel;
    let border = if focused {
        theme.colors.border_focus
    } else {
        theme.colors.border
    };
    let title_color = if focused {
        theme.colors.border_focus
    } else {
        theme.colors.title
    };
    let mut spans = vec![
        Span::styled(format!("[{}]─", panel.number()), theme.colors.dim),
        Span::styled(title, Style::new().fg(title_color)),
    ];
    if let Some(alt) = alt {
        spans.push(Span::styled(format!(" - {alt}"), theme.colors.dim));
    }
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(border))
        .title(Line::from(spans));
    if let Some(count) = count {
        block = block.title_bottom(Line::styled(count, theme.colors.dim).right_aligned());
    }
    block
}

fn draw_status(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let lead = |theme: &Theme| {
        vec![
            Span::styled(format!("{} ", theme.glyphs.ok), theme.colors.staged),
            Span::raw(app.repo_name.clone()),
            Span::styled(format!(" {} ", theme.glyphs.arrow), theme.colors.dim),
        ]
    };
    let line = match app.snapshot.as_ref().map(|snapshot| &snapshot.head) {
        Some(Head::Branch { name }) => {
            let mut spans = lead(theme);
            spans.push(Span::styled(name.clone(), theme.colors.branch));
            Line::from(spans)
        }
        Some(Head::Detached { short_id }) => {
            let mut spans = lead(theme);
            spans.push(Span::styled(format!("@{short_id}"), theme.colors.branch));
            spans.push(Span::styled(" (detached)", theme.colors.warn));
            Line::from(spans)
        }
        Some(Head::Unborn { name }) => {
            let mut spans = lead(theme);
            spans.push(Span::styled(name.clone(), theme.colors.branch));
            spans.push(Span::styled(" (no commits yet)", theme.colors.dim));
            Line::from(spans)
        }
        None => Line::styled("loading…", theme.colors.dim),
    };
    let block = panel_block(Panel::Status, "Status", None, None, app, theme);
    frame.render_widget(Paragraph::new(line).block(block), area);
}

fn draw_changelists(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let rows = app.changelist_rows();
    let lines: Vec<Line> = rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let mut spans = match &row.scope {
                Scope::All => vec![
                    Span::styled(format!("{} ", theme.glyphs.all), theme.colors.dim),
                    Span::raw("all"),
                ],
                Scope::Changelist(name) => vec![
                    if row.active {
                        Span::styled(
                            format!("{} ", theme.glyphs.active),
                            Style::new().fg(theme.colors.active).bold(),
                        )
                    } else {
                        Span::raw("  ")
                    },
                    Span::styled(name.clone(), theme.colors.changelist),
                ],
                Scope::Unassigned => vec![
                    Span::styled(format!("{} ", theme.glyphs.unassigned), theme.colors.warn),
                    Span::styled("unassigned", theme.colors.warn),
                ],
            };
            spans.push(Span::styled(format!(" ({})", row.count), theme.colors.dim));
            let mut line = Line::from(spans);
            if index == app.changelist_row {
                line = line.style(Style::new().bg(theme.colors.selection));
            }
            line
        })
        .collect();
    let count = format!("{} of {}", app.changelist_row + 1, rows.len());
    let block = panel_block(
        Panel::Changelists,
        "Changelists",
        None,
        Some(count),
        app,
        theme,
    );
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn draw_files(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let rows = app.files_rows();
    let mut selected_row = 0;
    let lines: Vec<Line> = rows
        .iter()
        .enumerate()
        .map(|(index, row)| match row {
            FilesRow::Header {
                label,
                count,
                unassigned,
                active,
            } => {
                let color = tag_color(*unassigned, false, theme);
                let mut text = format!("{} {label}", theme.glyphs.group);
                if *active {
                    text.push_str(&format!(" {}", theme.glyphs.active));
                }
                Line::from(vec![
                    Span::styled(text, color),
                    Span::styled(format!(" ({count})"), theme.colors.dim),
                ])
            }
            FilesRow::File {
                entry,
                stage,
                kind,
                staged,
                total,
                indent,
            } => {
                let selected = app.file_sel.as_ref() == Some(entry);
                if selected {
                    selected_row = index;
                }
                let mut spans = Vec::new();
                if *indent {
                    spans.push(Span::raw("  "));
                }
                spans.push(stage_span(*stage, theme));
                spans.push(Span::raw(" "));
                spans.push(kind_span(*kind, theme));
                spans.push(Span::raw(" "));
                spans.push(Span::raw(entry.path.clone()));
                spans.push(Span::styled(format!(" {staged}/{total}"), theme.colors.dim));
                let mut line = Line::from(spans);
                if selected {
                    line = line.style(Style::new().bg(theme.colors.selection));
                }
                line
            }
        })
        .collect();
    let (position, total) = app.files_count();
    let block = panel_block(
        Panel::Files,
        "Files",
        Some(app.scope().title().to_owned()),
        Some(format!("{position} of {total}")),
        app,
        theme,
    );
    let scroll = keep_visible(selected_row, area.height.saturating_sub(2));
    frame.render_widget(Paragraph::new(lines).block(block).scroll((scroll, 0)), area);
}

fn draw_commits(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let commits = app
        .snapshot
        .as_ref()
        .map(|snapshot| &snapshot.recent_commits[..])
        .unwrap_or_default();
    let lines: Vec<Line> = commits
        .iter()
        .enumerate()
        .map(|(index, commit)| {
            let mut line = Line::from(vec![
                Span::styled(commit.short_id.clone(), theme.colors.branch),
                Span::styled(format!(" {} ", initials(&commit.author)), theme.colors.dim),
                Span::raw(commit.summary.clone()),
            ]);
            if index == app.commit_row {
                line = line.style(Style::new().bg(theme.colors.selection));
            }
            line
        })
        .collect();
    let count = if commits.is_empty() {
        "0 of 0".to_owned()
    } else {
        format!("{} of {}", app.commit_row + 1, commits.len())
    };
    let block = panel_block(Panel::Commits, "Commits", None, Some(count), app, theme);
    let scroll = keep_visible(app.commit_row, area.height.saturating_sub(2));
    frame.render_widget(Paragraph::new(lines).block(block).scroll((scroll, 0)), area);
}

fn draw_diff(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let width = area.width.saturating_sub(2) as usize;
    let lines: Vec<Line> = app
        .diff_lines()
        .into_iter()
        .map(|line| render_diff_line(line, theme, width))
        .collect();
    let alt = app.diff_title();
    let block = panel_block(
        Panel::Diff,
        "Diff",
        (!alt.is_empty()).then_some(alt),
        None,
        app,
        theme,
    );
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .scroll((app.diff_scroll, 0)),
        area,
    );
}

fn render_diff_line(line: DiffLine, theme: &Theme, width: usize) -> Line<'static> {
    match line {
        DiffLine::FileHeader(text) => Line::styled(text, theme.colors.dim),
        DiffLine::Placeholder(text) => Line::styled(text, theme.colors.dim),
        DiffLine::Spacer => Line::raw(""),
        DiffLine::HunkHeader { text, tag, foreign } => {
            let mut spans = vec![Span::styled(
                text.clone(),
                Style::new().fg(theme.colors.hunk_header),
            )];
            if let Some(tag) = tag {
                let glyph = tag
                    .stage
                    .map(|stage| format!(" {}", hunk_stage_glyph(stage, theme)))
                    .unwrap_or_default();
                let pill = format!(
                    "{}{}{glyph}{}",
                    theme.glyphs.tag_open, tag.label, theme.glyphs.tag_close
                );
                // The prototype floats the tag at the header's right
                // edge; pad to the panel width, minimum two spaces.
                let pad = width
                    .saturating_sub(text.chars().count() + pill.chars().count())
                    .max(2);
                spans.push(Span::raw(" ".repeat(pad)));
                spans.push(Span::styled(
                    pill,
                    Style::new().fg(tag_color(tag.unassigned, tag.dim, theme)),
                ));
            }
            dim_if(Line::from(spans), foreign)
        }
        DiffLine::Content {
            origin,
            text,
            foreign,
        } => {
            let style = match origin {
                '+' => Style::new().fg(theme.colors.added),
                '-' => Style::new().fg(theme.colors.deleted),
                ' ' => Style::new().fg(theme.colors.text),
                // No-newline-at-EOF markers and anything exotic.
                _ => Style::new().fg(theme.colors.dim),
            };
            dim_if(Line::styled(format!("{origin}{text}"), style), foreign)
        }
    }
}

/// The changelist/unassigned/dim colour cascade every ownership label
/// shares (rows, headers, tags).
fn tag_color(unassigned: bool, dim: bool, theme: &Theme) -> ratatui::style::Color {
    if dim {
        theme.colors.dim
    } else if unassigned {
        theme.colors.warn
    } else {
        theme.colors.changelist
    }
}

/// The drilled-view ~45% treatment: terminal cells can't blend, so
/// foreign rows use the DIM attribute.
fn dim_if(line: Line<'static>, foreign: bool) -> Line<'static> {
    if foreign {
        line.style(Style::new().add_modifier(Modifier::DIM))
    } else {
        line
    }
}

fn draw_log(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    // Placeholder: the Log panel's stream, pins, and severities are
    // ticket #34.
    let mut lines = vec![Line::styled("(log arrives with #34)", theme.colors.dim)];
    if let Some(error) = &app.last_refresh_error {
        lines.push(Line::styled(
            format!("{} refresh failed — {error}", theme.glyphs.error),
            theme.colors.deleted,
        ));
    }
    let block = panel_block(Panel::Log, "Log", None, None, app, theme);
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn draw_keybar(frame: &mut Frame, area: Rect, app: &App, theme: &Theme, now: Instant) {
    let mut spans = Vec::new();
    for (index, (key, label)) in app.key_hints().into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(key, theme.colors.warn));
        spans.push(Span::styled(format!(" {label}"), theme.colors.dim));
    }
    let right = if app.indicator_visible(now) {
        Span::styled(
            format!("{} refreshing…", theme.glyphs.refreshing),
            theme.colors.warn,
        )
    } else {
        Span::styled(
            format!("gitchange {}", env!("CARGO_PKG_VERSION")),
            theme.colors.dim,
        )
    };
    let left_width: usize = spans.iter().map(|span| span.content.chars().count()).sum();
    let pad = (area.width as usize).saturating_sub(left_width + right.content.chars().count() + 2);
    spans.push(Span::raw(" ".repeat(pad)));
    spans.push(right);
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_help(frame: &mut Frame, area: Rect, theme: &Theme) {
    const BINDINGS: &[(&str, &str)] = &[
        ("1-5, 0", "focus panel (0 = diff)"),
        ("j/k, ↓/↑", "move within panel / scroll diff"),
        ("enter", "open selected changelist's files"),
        ("esc", "back (diff → files → changelists → all)"),
        ("R", "refresh now"),
        ("?", "toggle keybindings"),
        ("q", "quit"),
    ];
    let width = 52.min(area.width);
    let height = (BINDINGS.len() as u16 + 2).min(area.height);
    let popup = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    let lines: Vec<Line> = BINDINGS
        .iter()
        .map(|(key, label)| {
            Line::from(vec![
                Span::styled(format!("{key:>10}"), theme.colors.warn),
                Span::styled(format!("  {label}"), theme.colors.text),
            ])
        })
        .collect();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.colors.border_focus))
        .title(Line::styled(
            "Keybindings",
            Style::new().fg(theme.colors.border_focus),
        ));
    frame.render_widget(Clear, popup);
    frame.render_widget(Paragraph::new(lines).block(block), popup);
}

fn stage_span(stage: FileStage, theme: &Theme) -> Span<'static> {
    match stage {
        FileStage::Staged => Span::styled(theme.glyphs.staged.to_string(), theme.colors.staged),
        FileStage::PartiallyStaged => Span::styled(
            theme.glyphs.partially_staged.to_string(),
            theme.colors.staged,
        ),
        FileStage::Unstaged => Span::styled(theme.glyphs.unstaged.to_string(), theme.colors.dim),
    }
}

fn hunk_stage_glyph(stage: HunkStage, theme: &Theme) -> char {
    match stage {
        HunkStage::Staged => theme.glyphs.staged,
        HunkStage::Unstaged => theme.glyphs.unstaged,
        HunkStage::StagedStale => theme.glyphs.staged_stale,
    }
}

fn kind_span(kind: ChangeKind, theme: &Theme) -> Span<'static> {
    let (sigil, color) = match kind {
        ChangeKind::Added => ('A', theme.colors.added),
        ChangeKind::Modified => ('M', theme.colors.modified),
        ChangeKind::Deleted => ('D', theme.colors.deleted),
        ChangeKind::TypeChanged => ('T', theme.colors.modified),
        ChangeKind::Untracked => ('?', theme.colors.dim),
        ChangeKind::Conflicted => ('U', theme.colors.conflicted),
    };
    Span::styled(sigil.to_string(), Style::new().fg(color))
}

/// Two-letter author initials for the Commits panel.
fn initials(author: &str) -> String {
    author
        .split_whitespace()
        .filter_map(|word| word.chars().next())
        .take(2)
        .flat_map(char::to_uppercase)
        .collect()
}

/// Scroll offset keeping `selected` inside a viewport of `height` rows.
fn keep_visible(selected: usize, height: u16) -> u16 {
    (selected as u16).saturating_sub(height.saturating_sub(1))
}
