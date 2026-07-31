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

use crate::app::{
    App, CommitDraft, DiffLine, ErrorModal, FilesRow, InputKind, LogEntry, MoveRow, Overlay, Panel,
    Scope, Severity, count_noun, payload_counts,
};
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
    // ~20% of the right column at rest, growing one row per pin so
    // conditions never eat history (prototype variant E).
    let pins = app.pins();
    let log_height = (u32::from(right.height) * 20 / 100).max(5) as u16 + pins.len() as u16;
    let [diff, log] =
        Layout::vertical([Constraint::Min(5), Constraint::Length(log_height)]).areas(right);

    draw_status(frame, status, app, theme);
    draw_changelists(frame, changelists, app, theme);
    draw_files(frame, files, app, theme);
    draw_commits(frame, commits, app, theme);
    draw_diff(frame, diff, app, theme);
    draw_log(frame, log, app, &pins, theme);
    draw_keybar(frame, keybar, app, theme, now);
    if let Some(overlay) = &app.overlay {
        draw_overlay(frame, main, app, overlay, theme);
    }
    if app.help_open {
        draw_help(frame, main, theme);
    }
    // Topmost: it swallows every key until dismissed (ADR 0007).
    if let Some(modal) = &app.error_modal {
        draw_error_modal(frame, main, modal, theme);
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

/// Pad a row with trailing spaces to the panel's inner `width` (chars,
/// saturating): Paragraph only paints cells that carry a symbol, so a
/// row-level background would otherwise stop at the text's ragged edge.
fn fill_width(mut line: Line<'static>, width: usize) -> Line<'static> {
    let pad = width.saturating_sub(line.width());
    if pad > 0 {
        line.push_span(Span::raw(" ".repeat(pad)));
    }
    line
}

/// The selected-row treatment every panel shares: the selection
/// background across the full inner `width`.
fn select_row(line: Line<'static>, width: usize, theme: &Theme) -> Line<'static> {
    fill_width(line, width).style(Style::new().bg(theme.colors.selection))
}

fn draw_changelists(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    let width = area.width.saturating_sub(2) as usize;
    let rows = app.changelist_rows();
    let lines: Vec<Line> = rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let mut spans = match &row.scope {
                // Conflicts is a Files-row group, never a Changelists
                // row; the arm exists for exhaustiveness.
                Scope::All | Scope::Conflicts => vec![
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
            let line = Line::from(spans);
            if index == app.changelist_row {
                select_row(line, width, theme)
            } else {
                line
            }
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
    let width = area.width.saturating_sub(2) as usize;
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
                conflicted,
                active,
            } => {
                let color = if *conflicted {
                    theme.colors.conflicted
                } else {
                    tag_color(*unassigned, false, theme)
                };
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
                // A quarantined row has no staging to mark and no hunks
                // to count (ADR 0007): just the U sigil and the path.
                if *kind == ChangeKind::Conflicted {
                    spans.push(kind_span(*kind, theme));
                    spans.push(Span::raw(" "));
                    spans.push(Span::raw(entry.path.clone()));
                } else {
                    spans.push(stage_span(*stage, theme));
                    spans.push(Span::raw(" "));
                    spans.push(kind_span(*kind, theme));
                    spans.push(Span::raw(" "));
                    spans.push(Span::raw(entry.path.clone()));
                    spans.push(Span::styled(format!(" {staged}/{total}"), theme.colors.dim));
                }
                let line = Line::from(spans);
                if selected {
                    select_row(line, width, theme)
                } else {
                    line
                }
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
    let width = area.width.saturating_sub(2) as usize;
    let commits = app
        .snapshot
        .as_ref()
        .map(|snapshot| &snapshot.recent_commits[..])
        .unwrap_or_default();
    let lines: Vec<Line> = commits
        .iter()
        .enumerate()
        .map(|(index, commit)| {
            let line = Line::from(vec![
                Span::styled(commit.short_id.clone(), theme.colors.branch),
                Span::styled(format!(" {} ", initials(&commit.author)), theme.colors.dim),
                Span::raw(commit.summary.clone()),
            ]);
            if index == app.commit_row {
                select_row(line, width, theme)
            } else {
                line
            }
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
    // Hunk mode tracks the selection (a couple of context lines above
    // its header, clamped so the tail never overscrolls); scroll mode
    // uses the App's line offset.
    let scroll = match app.selected_hunk_line() {
        Some(header) => {
            let height = area.height.saturating_sub(2);
            let max = (lines.len() as u16).saturating_sub(height);
            (header as u16).saturating_sub(2).min(max)
        }
        None => app.diff_scroll,
    };
    let alt = app.diff_title();
    let block = panel_block(
        Panel::Diff,
        "Diff",
        (!alt.is_empty()).then_some(alt),
        None,
        app,
        theme,
    );
    frame.render_widget(Paragraph::new(lines).block(block).scroll((scroll, 0)), area);
}

fn render_diff_line(line: DiffLine, theme: &Theme, width: usize) -> Line<'static> {
    match line {
        DiffLine::FileHeader(text) => Line::styled(text, theme.colors.dim),
        DiffLine::Placeholder(text) => Line::styled(text, theme.colors.dim),
        DiffLine::Conflict(text) => Line::styled(text, theme.colors.conflicted),
        DiffLine::Spacer => Line::raw(""),
        DiffLine::HunkHeader {
            text,
            tag,
            foreign,
            selected,
        } => {
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
            decorate(Line::from(spans), foreign, selected, width, theme)
        }
        DiffLine::Content {
            origin,
            text,
            foreign,
            selected,
        } => {
            let style = match origin {
                '+' => Style::new().fg(theme.colors.added),
                '-' => Style::new().fg(theme.colors.deleted),
                ' ' => Style::new().fg(theme.colors.text),
                // No-newline-at-EOF markers and anything exotic.
                _ => Style::new().fg(theme.colors.dim),
            };
            decorate(
                Line::styled(format!("{origin}{text}"), style),
                foreign,
                selected,
                width,
                theme,
            )
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

/// Line-level diff treatments: foreign rows get the DIM attribute (the
/// prototype's ~45% opacity — terminal cells can't blend), the hunk-mode
/// selection gets the selection background (the prototype's outline has
/// no terminal equivalent).
fn decorate(
    mut line: Line<'static>,
    foreign: bool,
    selected: bool,
    width: usize,
    theme: &Theme,
) -> Line<'static> {
    let mut style = Style::new();
    if foreign {
        style = style.add_modifier(Modifier::DIM);
    }
    if selected {
        line = fill_width(line, width);
        style = style.bg(theme.colors.selection);
    }
    line.style(style)
}

/// The Log panel (ADR 0007): the tinted pin banner fixed at top —
/// conditions that currently hold — over one chronological stream of
/// events, newest kept visible.
fn draw_log(frame: &mut Frame, area: Rect, app: &App, pins: &[String], theme: &Theme) {
    let block = panel_block(Panel::Log, "Log", None, None, app, theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [pin_area, stream_area] =
        Layout::vertical([Constraint::Length(pins.len() as u16), Constraint::Min(0)]).areas(inner);

    if !pins.is_empty() {
        let pin_lines: Vec<Line> = pins
            .iter()
            .map(|pin| {
                fill_width(
                    Line::styled(
                        format!("{} {pin}", theme.glyphs.pin),
                        Style::new()
                            .fg(theme.colors.warn)
                            .bg(theme.colors.selection),
                    ),
                    pin_area.width as usize,
                )
            })
            .collect();
        frame.render_widget(Paragraph::new(pin_lines), pin_area);
    }

    let lines: Vec<Line> = app.log.iter().map(|entry| log_line(entry, theme)).collect();
    // Stick to the newest entry, offset by the user's scrollback.
    let bottom = lines.len().saturating_sub(1).saturating_sub(app.log_scroll);
    let scroll = keep_visible(bottom, stream_area.height);
    frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), stream_area);
}

/// One log event: severity glyph plus tinted text — `·` dim, `!` warn,
/// `✗` error (ADR 0007's three fixed levels).
fn log_line(entry: &LogEntry, theme: &Theme) -> Line<'static> {
    let (glyph, color) = match entry.severity {
        Severity::Info => (theme.glyphs.log_info, theme.colors.dim),
        Severity::Notice => (theme.glyphs.log_notice, theme.colors.warn),
        Severity::Error => (theme.glyphs.log_error, theme.colors.deleted),
    };
    Line::from(vec![
        Span::styled(glyph.to_string(), Style::new().fg(color)),
        Span::raw(" "),
        Span::styled(entry.text.clone(), Style::new().fg(color)),
    ])
}

/// The error-modal contract (ADR 0007): title names the operation, the
/// detail renders verbatim and scrollable — hook stderr is the user's
/// own tooling talking to them; truncating it would be hostile.
fn draw_error_modal(frame: &mut Frame, area: Rect, modal: &ErrorModal, theme: &Theme) {
    let detail: Vec<Line> = if modal.detail.trim().is_empty() {
        vec![Line::styled("(no detail)", theme.colors.dim)]
    } else {
        modal
            .detail
            .lines()
            .map(|line| Line::raw(line.to_owned()))
            .collect()
    };
    let width = (detail.iter().map(Line::width).max().unwrap_or(0) as u16 + 4)
        .clamp(48, area.width.max(48));
    // Detail + hint row + blank + borders, capped to the frame.
    let height = (detail.len() as u16 + 4).min(area.height.saturating_sub(2));
    let popup = centered(area, width, height);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.colors.deleted))
        .title(Line::styled(
            modal.title.clone(),
            Style::new().fg(theme.colors.deleted),
        ));
    let inner = block.inner(popup);
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);

    let [detail_area, _, hints_area] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);

    // Clamp the scroll so the tail never overscrolls into blank space.
    let max_scroll = (detail.len() as u16).saturating_sub(detail_area.height);
    let overflows = max_scroll > 0;
    frame.render_widget(
        Paragraph::new(detail).scroll((modal.scroll.min(max_scroll), 0)),
        detail_area,
    );
    let hints: &[(&str, &str)] = if overflows {
        &[("enter/esc", "dismiss"), ("j/k", "scroll")]
    } else {
        &[("enter/esc", "dismiss")]
    };
    frame.render_widget(Paragraph::new(key_hints_line(hints, theme)), hints_area);
}

/// A centered popup rect, clamped to `area`.
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    }
}

/// A modal's bordered block: focus-coloured border with the title on the
/// top border — the text-on-border framing convention.
fn modal_block(title: &str, theme: &Theme) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.colors.border_focus))
        .title(Line::styled(
            title.to_owned(),
            Style::new().fg(theme.colors.border_focus),
        ))
}

/// The `enter <verb> · esc cancel` hint line modals carry.
fn modal_hints(verb: &str, theme: &Theme) -> Line<'static> {
    key_hints_line(&[("enter", verb), ("esc", "cancel")], theme)
}

/// A `key label · key label` hint line — the commit modals carry more
/// than the two-verb [`modal_hints`] shape.
fn key_hints_line(pairs: &[(&str, &str)], theme: &Theme) -> Line<'static> {
    let mut spans = Vec::new();
    for (index, (key, label)) in pairs.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled(" · ", theme.colors.dim));
        }
        spans.push(Span::styled((*key).to_owned(), theme.colors.warn));
        spans.push(Span::styled(format!(" {label}"), theme.colors.dim));
    }
    Line::from(spans)
}

fn draw_overlay(frame: &mut Frame, area: Rect, app: &App, overlay: &Overlay, theme: &Theme) {
    match overlay {
        Overlay::Input { kind, value } => {
            let title = match kind {
                InputKind::NewChangelist | InputKind::NewChangelistForMove { .. } => {
                    "New changelist"
                }
                InputKind::Rename { .. } => "Rename changelist",
            };
            let popup = centered(area, 44, 3);
            let line = Line::from(vec![
                Span::raw(value.clone()),
                Span::styled(" ", Style::new().add_modifier(Modifier::REVERSED)),
            ]);
            frame.render_widget(Clear, popup);
            frame.render_widget(Paragraph::new(line).block(modal_block(title, theme)), popup);
        }
        Overlay::ConfirmDelete { name } => {
            let lines = vec![
                Line::raw(format!("Delete '{name}'?")),
                Line::styled("Its hunks move to unassigned.", theme.colors.dim),
                Line::raw(""),
                modal_hints("delete", theme),
            ];
            let width = (lines.iter().map(Line::width).max().unwrap_or(0) as u16 + 4).max(44);
            let popup = centered(area, width, lines.len() as u16 + 2);
            frame.render_widget(Clear, popup);
            frame.render_widget(
                Paragraph::new(lines).block(modal_block("Delete changelist", theme)),
                popup,
            );
        }
        Overlay::Commit(draft) => draw_commit_dialog(frame, area, draft, theme, true),
        Overlay::CommitStale(draft) => {
            // The dialog stays visible under the warn (prototype
            // variant B) — its state is untouched, only the cursor goes.
            draw_commit_dialog(frame, area, draft, theme, false);
            draw_stale_warn(frame, area, draft, theme);
        }
        Overlay::CommitStageAll {
            changelist,
            hunks,
            files,
        } => {
            let label = changelist.as_deref().unwrap_or("unassigned");
            let lines = vec![
                Line::from(vec![
                    Span::styled(
                        label.to_owned(),
                        Style::new().fg(changelist_color(changelist.is_some(), theme)),
                    ),
                    Span::raw(" has no staged hunks."),
                ]),
                Line::raw(format!(
                    "Stage all {} in {} and commit?",
                    count_noun(*hunks, "hunk"),
                    count_noun(*files, "file"),
                )),
                Line::raw(""),
                key_hints_line(
                    &[("enter", "stage all & continue"), ("esc", "cancel")],
                    theme,
                ),
            ];
            let width = (lines.iter().map(Line::width).max().unwrap_or(0) as u16 + 4).max(52);
            let popup = centered(area, width, lines.len() as u16 + 2);
            frame.render_widget(Clear, popup);
            frame.render_widget(
                Paragraph::new(lines).block(modal_block("Nothing staged", theme)),
                popup,
            );
        }
        Overlay::CommitDrift { draft, previous } => {
            let was = payload_counts(previous);
            let mut now_spans = vec![Span::styled(
                format!("was: {was} → now: "),
                theme.colors.dim,
            )];
            now_spans.extend(payload_spans(&draft.payload, theme.colors.text, theme));
            let lines = vec![
                Line::raw("The working tree changed while you were writing the message."),
                Line::from(now_spans),
                Line::raw(""),
                Line::from(vec![
                    Span::styled("message (kept)  ", theme.colors.dim),
                    Span::raw(draft.message.clone()),
                ]),
                Line::raw(""),
                key_hints_line(
                    &[
                        ("enter", "commit updated payload"),
                        ("e", "edit message"),
                        ("esc", "cancel"),
                    ],
                    theme,
                ),
            ];
            let width = (lines.iter().map(Line::width).max().unwrap_or(0) as u16 + 4).max(60);
            let popup = centered(area, width, lines.len() as u16 + 2);
            frame.render_widget(Clear, popup);
            frame.render_widget(
                Paragraph::new(lines).block(warn_block("Payload changed — re-confirm", theme)),
                popup,
            );
        }
        Overlay::Move { payload, row } => {
            let rows = app.move_rows();
            let selected = (*row).min(rows.len().saturating_sub(1));
            let mut lines = vec![
                Line::styled(app.move_description(payload), theme.colors.dim),
                Line::raw(""),
            ];
            let mut selected_line = None;
            for (index, move_row) in rows.iter().enumerate() {
                let line = match move_row {
                    MoveRow::Changelist { name, active } => {
                        let mut spans = vec![
                            Span::raw("  "),
                            Span::styled(name.clone(), theme.colors.changelist),
                        ];
                        if *active {
                            spans.push(Span::styled(" (active)", theme.colors.dim));
                        }
                        Line::from(spans)
                    }
                    MoveRow::CreateNew => Line::from(vec![
                        Span::raw("  "),
                        Span::styled("+", theme.colors.staged),
                        Span::raw(" create new changelist…"),
                    ]),
                };
                if matches!(move_row, MoveRow::CreateNew) {
                    lines.push(Line::styled(
                        "─".repeat(30),
                        Style::new().fg(theme.colors.border),
                    ));
                }
                if index == selected {
                    selected_line = Some(lines.len());
                }
                lines.push(line);
            }
            lines.push(Line::raw(""));
            lines.push(modal_hints("move", theme));
            let width = (lines.iter().map(Line::width).max().unwrap_or(0) as u16 + 4).max(40);
            // The popup's width follows its widest row, so the selected
            // row can only be padded to the inner width once it's known.
            if let Some(index) = selected_line {
                let line = std::mem::take(&mut lines[index]);
                lines[index] = select_row(line, width.saturating_sub(2) as usize, theme);
            }
            let popup = centered(area, width, lines.len() as u16 + 2);
            frame.render_widget(Clear, popup);
            frame.render_widget(
                Paragraph::new(lines).block(modal_block("Move to changelist", theme)),
                popup,
            );
        }
    }
}

/// A warn-framed modal block — the ◑ warn and drift re-confirm frames.
fn warn_block(title: &str, theme: &Theme) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.colors.warn))
        .title(Line::styled(
            title.to_owned(),
            Style::new().fg(theme.colors.warn),
        ))
}

/// The block cursor the input overlays share.
fn cursor() -> Span<'static> {
    Span::styled(" ", Style::new().add_modifier(Modifier::REVERSED))
}

/// A changelist name's colour: the changelist tint for named ones, the
/// warning tint for unassigned — the cascade the commit modals share.
fn changelist_color(named: bool, theme: &Theme) -> ratatui::style::Color {
    if named {
        theme.colors.changelist
    } else {
        theme.colors.warn
    }
}

/// The payload counts with the ◑ tail coloured — "5 staged hunks in
/// 2 files · 1 ◑" (the issue's one-line summary), shared by the dialog
/// and the drift notice's now side.
fn payload_spans(
    payload: &gitchange_core::CommitPayload,
    counts_color: ratatui::style::Color,
    theme: &Theme,
) -> Vec<Span<'static>> {
    let mut spans = vec![Span::styled(
        payload_counts(payload),
        Style::new().fg(counts_color),
    )];
    let stale = payload.stale_hunks();
    if stale > 0 {
        spans.push(Span::styled(format!(" · {stale} ◑"), theme.colors.warn));
    }
    spans
}

/// The all-in-one commit dialog (commit-flow prototype variant A):
/// one-line payload summary, independent message/body inputs with
/// text-on-border framing, flag toggles, hints. `active` drops the
/// cursor while the ◑ warn sits on top.
fn draw_commit_dialog(
    frame: &mut Frame,
    area: Rect,
    draft: &CommitDraft,
    theme: &Theme,
    active: bool,
) {
    let popup = centered(area, 64, 13);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.colors.border_focus))
        .title(Line::from(vec![
            Span::styled(
                "Commit changelist — ",
                Style::new().fg(theme.colors.border_focus),
            ),
            Span::styled(
                draft.changelist_label().to_owned(),
                Style::new().fg(changelist_color(draft.changelist.is_some(), theme)),
            ),
        ]));
    let inner = block.inner(popup);
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);

    let [
        payload_area,
        message_area,
        body_area,
        flags_area,
        hints_area,
    ] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Length(5),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);

    let mut summary = vec![Span::styled("payload: ", theme.colors.dim)];
    summary.extend(payload_spans(&draft.payload, theme.colors.dim, theme));
    frame.render_widget(Paragraph::new(Line::from(summary)), payload_area);

    // Text-on-border input framing: the label sits on the input's own
    // border; the focused input carries the cursor and focus colour.
    let input_block = |label: &str, focused: bool| {
        let color = if focused && active {
            theme.colors.border_focus
        } else {
            theme.colors.border
        };
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(color))
            .title(Line::styled(label.to_owned(), theme.colors.dim))
    };

    let mut message_line = Line::from(Span::raw(draft.message.clone()));
    if active && !draft.body_focus {
        message_line.push_span(cursor());
    }
    frame.render_widget(
        Paragraph::new(message_line).block(input_block("message", !draft.body_focus)),
        message_area,
    );

    let body_lines: Vec<Line> = if draft.body.is_empty() && (!active || !draft.body_focus) {
        vec![Line::styled("…optional (tab)", theme.colors.dim)]
    } else {
        let mut lines: Vec<Line> = draft
            .body
            .split('\n')
            .map(|line| Line::raw(line.to_owned()))
            .collect();
        if active && draft.body_focus {
            lines
                .last_mut()
                .expect("split yields at least one line")
                .push_span(cursor());
        }
        lines
    };
    // Keep the cursor's line visible once the body outgrows the input.
    let body_scroll = keep_visible(body_lines.len().saturating_sub(1), 3);
    frame.render_widget(
        Paragraph::new(body_lines)
            .block(input_block("body", draft.body_focus))
            .scroll((body_scroll, 0)),
        body_area,
    );

    let flag = |set: bool, label: &str| {
        let mark = if set { "[x]" } else { "[ ]" };
        let style = if set {
            Style::new().fg(theme.colors.warn)
        } else {
            Style::new().fg(theme.colors.dim)
        };
        Span::styled(format!("{mark} {label}"), style)
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            flag(draft.no_verify, "--no-verify"),
            Span::raw("   "),
            flag(draft.amend, "--amend"),
        ])),
        flags_area,
    );

    frame.render_widget(
        Paragraph::new(key_hints_line(
            &[
                ("enter", "commit"),
                ("tab", "body"),
                ("ctrl+n", "no-verify"),
                ("ctrl+a", "amend"),
                ("esc", "cancel"),
            ],
            theme,
        )),
        hints_area,
    );
}

/// The ◑ staged-stale warn-and-confirm (prototype variant B, ADR 0004),
/// drawn over the dimmed dialog.
fn draw_stale_warn(frame: &mut Frame, area: Rect, draft: &CommitDraft, theme: &Theme) {
    let stale = draft.payload.stale_hunks();
    let total = draft.payload.staged_hunks() + stale;
    let mut lines = vec![
        Line::raw(format!(
            "{stale} of {total} payload hunks {} ◑ — the index holds a",
            if stale == 1 { "is" } else { "are" },
        )),
        Line::raw("different version than the worktree. Committing as-is"),
        Line::raw("commits content that isn't what you see."),
        Line::raw(""),
    ];
    for file in &draft.payload.files {
        if file.stale_hunks == 0 {
            continue;
        }
        lines.push(Line::from(vec![
            Span::styled("◑ ", theme.colors.warn),
            Span::raw(file.path.clone()),
            Span::styled(
                format!(" ({})", count_noun(file.stale_hunks, "stale hunk")),
                theme.colors.dim,
            ),
        ]));
    }
    lines.push(Line::raw(""));
    lines.push(key_hints_line(
        &[
            ("enter", "commit as-is"),
            ("w", "align index to worktree & commit"),
            ("esc", "back"),
        ],
        theme,
    ));
    let width = (lines.iter().map(Line::width).max().unwrap_or(0) as u16 + 4).max(56);
    let popup = centered(area, width, lines.len() as u16 + 2);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).block(warn_block("◑ staged-stale hunks in payload", theme)),
        popup,
    );
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
        ("j/k, ↓/↑", "move within panel / hunks / scroll diff"),
        ("enter", "drill in: changelist → files → hunk mode"),
        ("enter", "hunk mode: add all hunks to changelist"),
        ("shift+enter", "hunk mode: add hunk to changelist"),
        ("space", "stage/unstage file (hunk mode: hunk)"),
        ("c", "commit changelist"),
        ("m", "move file to changelist"),
        ("n", "new changelist"),
        ("d", "delete changelist"),
        ("r", "rename changelist"),
        ("a", "set active changelist"),
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
    let color = match kind {
        ChangeKind::Added => theme.colors.added,
        ChangeKind::Modified | ChangeKind::TypeChanged => theme.colors.modified,
        ChangeKind::Deleted => theme.colors.deleted,
        ChangeKind::Untracked => theme.colors.dim,
        ChangeKind::Conflicted => theme.colors.conflicted,
    };
    Span::styled(
        crate::app::kind_sigil(kind).to_string(),
        Style::new().fg(color),
    )
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
