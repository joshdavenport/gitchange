//! Rendering smoke tests over ratatui's TestBackend: the panel stack
//! draws, the All view groups and tags, drilled views dim — asserted on
//! buffer text, not exact cells (snapshot-style, opportunistic per
//! ADR 0008).

use std::time::{Duration, Instant};

use gitchange_core::{
    Advisory, BinarySides, BlobInfo, ChangeKind, ChangedFile, Changelist, CommitInfo, GitOperation,
    Head, Hunk, HunkIdentity, HunkLine, HunkStage, Snapshot,
};
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier};
use ratatui::{Terminal, backend::TestBackend};

use gitchange_tui::app::{App, INDICATOR_DELAY, Panel, Severity};
use gitchange_tui::theme::Theme;
use gitchange_tui::ui;

fn hunk(new_start: u32, changelist: Option<&str>, stage: HunkStage) -> Hunk {
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

fn snapshot() -> Snapshot {
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

fn render_buffer(app: &App) -> Buffer {
    render_buffer_themed(app, &Theme::default())
}

fn render_buffer_themed(app: &App, theme: &Theme) -> Buffer {
    render_buffer_sized(app, theme, 140, 40)
}

/// One frame at an arbitrary terminal size — every other render helper
/// is this one with a size pinned.
fn render_buffer_sized(app: &App, theme: &Theme, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| ui::draw(frame, app, theme, Instant::now()))
        .unwrap();
    terminal.backend().buffer().clone()
}

fn render(app: &App) -> String {
    text_of(&render_buffer(app))
}

/// A buffer's rows as newline-joined text — split from `render` for the
/// tests that assert on both the text and the cells behind it.
fn text_of(buffer: &Buffer) -> String {
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
fn find_text(buffer: &Buffer, needle: &str) -> (u16, u16) {
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
fn border_right_of(buffer: &Buffer, from: u16, y: u16) -> u16 {
    (from..buffer.area.width)
        .find(|&x| buffer[(x, y)].symbol() == "│")
        .unwrap_or_else(|| panic!("no │ border right of ({from},{y})"))
}

fn bg_at(buffer: &Buffer, x: u16, y: u16) -> Color {
    buffer[(x, y)].bg
}

fn fg_at(buffer: &Buffer, x: u16, y: u16) -> Color {
    buffer[(x, y)].fg
}

/// Assert every cell in `x_start..x_end` on row `y` carries `bg` —
/// pass `Color::Reset` to assert the absence of a tint.
fn assert_bg_span(buffer: &Buffer, y: u16, x_start: u16, x_end: u16, bg: Color) {
    for x in x_start..x_end {
        assert_eq!(
            bg_at(buffer, x, y),
            bg,
            "cell ({x},{y}) {:?} bg",
            buffer[(x, y)].symbol()
        );
    }
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

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
    // Scoped title counts: 1 own hunk staged of 1, 1 elsewhere.
    assert!(text.contains("print.css (1/1 staged · 1 hunk elsewhere)"));
}

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
    assert!(text.contains("print.css — hunk 1 of 2"));
    // The bar advertises exactly the bindings the keymap has (ADR 0013).
    assert!(text.contains("a/A/ctrl+a"));
    assert!(text.contains("assign hunk / unassigned / all"));
    assert!(!text.contains("shift+enter"));
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
fn log_entries_render_with_their_severity_glyphs() {
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());
    app.push_log(Severity::Info, "staged hunk — src/print.css @@ -41,6");
    app.push_log(Severity::Notice, "auto-captured 1 hunk → 'fixes'");
    app.push_log(Severity::Error, "Commit failed — pre-commit hook exited 1");
    let text = render(&app);
    assert!(text.contains("· staged hunk — src/print.css @@ -41,6"));
    assert!(text.contains("! auto-captured 1 hunk → 'fixes'"));
    assert!(text.contains("✗ Commit failed — pre-commit hook exited 1"));
}

#[test]
fn the_head_move_dormancy_advisory_reaches_the_log_panel() {
    // ADR 0012's advisory renders like any other (ADR 0007): pushed with
    // the snapshot that carried it, in core's phrasing, at `!`. Driven
    // through `apply_snapshot` rather than `push_log` so the mapping from
    // a real `Advisory` is part of what's asserted. (A repo scenario
    // producing one can't reach the TUI until the run loop has a seam —
    // #59; core's own coverage of that scenario is `head_moves.rs`.)
    let advisory = Advisory::HeadMoveDormancy {
        path: "src/print.css".into(),
        changelists: vec!["chores".into()],
    };
    let mut app = App::new("repo");
    let mut moved = snapshot();
    moved.advisories = vec![advisory.clone()];
    app.apply_snapshot(moved);

    // Taken from the advisory rather than spelled out, so a reworded
    // message can't drift past this test (ADR 0006: core owns phrasing,
    // the frontend adds the severity glyph and nothing else) — and cut to
    // what the panel shows at this width, since it truncates the tail.
    let line: String = format!("! {}", advisory.message())
        .chars()
        .take(60)
        .collect();
    assert!(
        render(&app).contains(&line),
        "expected {line:?} in the Log panel"
    );
}

#[test]
fn pins_render_as_a_banner_atop_the_log_stream() {
    let mut app = App::new("repo");
    let mut busy = snapshot();
    busy.operation = Some(GitOperation::Rebase);
    busy.files.push(ChangedFile {
        path: "src/merge.ts".into(),
        kind: ChangeKind::Conflicted,
        binary: false,
        binary_sides: None,
        hunks: Vec::new(),
    });
    app.apply_snapshot(busy);
    app.on_watcher_degraded();
    let text = render(&app);
    assert!(text.contains("▲ watcher unavailable — polling"));
    assert!(text.contains("▲ rebase in progress — 1 conflicted"));
    // The banner sits above the stream: the pin row precedes the
    // rebase-detected event line.
    let pin = text.find("▲ rebase in progress").unwrap();
    let event = text.find("· rebase detected").unwrap();
    assert!(pin < event, "pins render above the event stream");
}

#[test]
fn the_conflicts_group_renders_first_without_stage_marks() {
    let mut app = App::new("repo");
    let mut busy = snapshot();
    busy.files.push(ChangedFile {
        path: "src/merge.ts".into(),
        kind: ChangeKind::Conflicted,
        binary: false,
        binary_sides: None,
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

#[test]
fn focused_panel_number_keys_move_the_border_highlight() {
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());
    app.on_key(key(KeyCode::Char('4')));
    assert_eq!(app.focus, Panel::Commits);
    render(&app); // must not panic
}

// ── commit flow overlays (ticket #33) ───────────────────────────────

fn payload(staged: usize, stale: usize) -> gitchange_core::CommitPayload {
    gitchange_core::CommitPayload {
        files: vec![gitchange_core::PayloadFile {
            path: "src/print.css".into(),
            staged_hunks: staged,
            stale_hunks: stale,
            hunks: Vec::new(),
            whole_file: None,
        }],
    }
}

#[test]
fn the_commit_dialog_renders_summary_inputs_and_flags() {
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());
    app.open_commit_dialog(Some("fixes".into()), payload(5, 1));
    for c in "fix: sizing".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    app.on_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL));
    let text = render(&app);
    assert!(text.contains("Commit changelist — fixes"), "{text}");
    assert!(text.contains("payload: 6 staged hunks in 1 file · 1 ◑"));
    assert!(text.contains("message"));
    assert!(text.contains("fix: sizing"));
    assert!(text.contains("…optional (tab)"));
    assert!(text.contains("[x] --no-verify"));
    assert!(text.contains("[ ] --amend"));
    assert!(text.contains("ctrl+n"));
}

#[test]
fn the_stale_warn_overlays_the_dialog_with_the_stale_files() {
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());
    app.open_commit_dialog(Some("fixes".into()), payload(4, 1));
    for c in "fix: x".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    app.on_key(key(KeyCode::Enter));
    let text = render(&app);
    assert!(text.contains("◑ staged-stale hunks in payload"), "{text}");
    assert!(text.contains("1 of 5 payload hunks is ◑"));
    assert!(text.contains("src/print.css"));
    assert!(text.contains("align index to worktree & commit"));
}

#[test]
fn the_stage_all_offer_names_the_counts() {
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());
    app.offer_stage_all(Some("chores".into()));
    let text = render(&app);
    assert!(text.contains("Nothing staged"), "{text}");
    assert!(text.contains("chores has no staged hunks."));
    assert!(text.contains("Stage all 1 hunk in 1 file and commit?"));
    assert!(text.contains("stage all & continue"));
}

#[test]
fn the_drift_reconfirm_keeps_the_message_and_shows_was_now() {
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());
    app.open_commit_dialog(Some("fixes".into()), payload(5, 0));
    for c in "fix: x".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    let Some(gitchange_tui::app::Action::Commit(gitchange_tui::app::CommitStep::Commit(draft))) =
        app.on_key(key(KeyCode::Enter))
    else {
        panic!("expected a commit step");
    };
    app.commit_drifted(draft, payload(4, 0));
    let text = render(&app);
    assert!(text.contains("Payload changed — re-confirm"), "{text}");
    assert!(text.contains("was: 5 staged hunks in 1 file → now: 4 staged hunks in 1 file"));
    assert!(text.contains("message (kept)"));
    assert!(text.contains("fix: x"));
    assert!(text.contains("commit updated payload"));
}

// ── full-width selection tints (issue #45) ──────────────────────────

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

// ── diff origin colours survive the decorator (issue #46) ───────────

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

// ── focus-conditional selection + cursors (issue #45) ───────────────

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
fn the_panel_number_prefix_follows_focus() {
    let theme = Theme::default();
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());

    // Default focus is Changelists: its whole `[2]─` prefix takes the
    // focus colour; the blurred Files panel's `[3]─` stays dim.
    let buffer = render_buffer(&app);
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
            theme.colors.dim,
            "blurred [3]─ cell {dx}"
        );
    }

    // Focus Files: the colours swap.
    app.on_key(key(KeyCode::Char('3')));
    let buffer = render_buffer(&app);
    for dx in 0..4 {
        assert_eq!(
            fg_at(&buffer, cx + dx, cy),
            theme.colors.dim,
            "blurred [2]─ cell {dx}"
        );
        assert_eq!(
            fg_at(&buffer, fx + dx, fy),
            theme.colors.border_focus,
            "focused [3]─ cell {dx}"
        );
    }
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

// ── binary whole-file hunks (ADR 0009, issue #43) ───────────────────

/// A changed binary as core reports it: one whole-file hunk anchored on
/// the blob-OID pair, which `BinarySides` derives so the fixture can't
/// drift from ADR 0009's anchor. Sizes match the `binary` sandbox, so
/// what this renders is what gets eyeballed there.
fn binary_file(changelist: &str) -> ChangedFile {
    let sides = BinarySides {
        head: Some(BlobInfo {
            oid: "9ebcc32e".into(),
            size: 12_698,
        }),
        changed: Some(BlobInfo {
            oid: "0d4f9284".into(),
            size: 15_462,
        }),
    };
    ChangedFile {
        path: "assets/logo.png".into(),
        kind: ChangeKind::Modified,
        binary: true,
        hunks: vec![Hunk {
            old_start: 0,
            old_lines: 0,
            new_start: 0,
            new_lines: 0,
            stage: HunkStage::Unstaged,
            index_only: false,
            identity: HunkIdentity::WholeFile {
                oids: sides.oid_anchor(),
            },
            changelist: Some(changelist.to_owned()),
        }],
        binary_sides: Some(sides),
    }
}

#[test]
fn a_binary_renders_its_sized_placeholder_dim_with_no_hunk_rows() {
    let theme = Theme::default();
    let mut app = App::new("repo");
    let mut snap = snapshot();
    snap.files.push(binary_file("fixes"));
    app.apply_snapshot(snap);
    app.on_key(key(KeyCode::Char('3'))); // focus Files
    app.on_key(key(KeyCode::Char('j'))); // select the PNG

    let buffer = render_buffer(&app);
    let text = text_of(&buffer);

    // The whole-file hunk counts as one hunk in the row and the title.
    assert!(text.contains("assets/logo.png 0/1"), "{text}");
    assert!(text.contains("logo.png (0/1 staged)"), "{text}");
    // One dim placeholder line carrying both sizes; no diff body at all.
    let (x, y) = find_text(&buffer, "Binary file changed (12.4 KB → 15.1 KB)");
    assert_eq!(fg_at(&buffer, x, y), theme.colors.dim);
    assert!(
        !text.contains("@@ -0,0"),
        "no hunk header for a binary\n{text}"
    );

    // Hunk-mode entry is a deliberate no-op, so the frame it would have
    // changed — title, cursor column, tint — is byte-identical after.
    app.on_key(key(KeyCode::Enter));
    assert_eq!(render_buffer(&app), buffer, "enter redrew the binary");
}

#[test]
fn the_keybar_shows_staging_and_commit_hints() {
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());
    app.on_key(key(KeyCode::Char('3')));
    let text = render(&app);
    assert!(text.contains("space stage file"), "{text}");
    // 'all' is scoped: commit isn't afforded, so the bar doesn't lie
    // about it (ADR 0014).
    assert!(!text.contains("c commit"), "{text}");

    app.on_key(key(KeyCode::Esc));
    app.on_key(key(KeyCode::Char('j'))); // drill into 'fixes'
    app.on_key(key(KeyCode::Enter));
    let text = render(&app);
    assert!(text.contains("space stage file"), "{text}");
    assert!(text.contains("c commit"));
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

// ── the left column's geometry (issue #87) ──────────────────────────
//
// These read panel heights off the rendered frame rather than asserting
// height tuples: the invariants are "every panel is drawn", "the cursor
// is on screen" and "Commits keeps its share", all of which survive
// anyone re-tuning the column.

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
        advisories: Vec::new(),
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

/// A buffer row as text.
fn row_text(buffer: &Buffer, y: u16) -> String {
    (0..buffer.area.width)
        .map(|x| buffer[(x, y)].symbol())
        .collect()
}

/// The rows a left-column panel occupies, borders included, read off its
/// own frame: the titled top corner down to the next bottom corner.
/// `None` when the panel is not drawn, or is drawn without a frame to
/// measure.
fn left_panel_rows(buffer: &Buffer, title: &str) -> Option<(u16, u16)> {
    let corner = |y: u16| buffer[(0, y)].symbol();
    let top = (0..buffer.area.height)
        .find(|&y| corner(y) == "╭" && row_text(buffer, y).contains(title))?;
    let bottom = (top + 1..buffer.area.height).find(|&y| corner(y) != "│")?;
    (corner(bottom) == "╰").then_some((top, bottom))
}

/// A left-column panel's outer height, borders included.
fn left_panel_height(buffer: &Buffer, title: &str) -> u16 {
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
fn cursor_rows(buffer: &Buffer, title: &str) -> Vec<String> {
    let (top, bottom) =
        left_panel_rows(buffer, title).unwrap_or_else(|| panic!("panel {title} is not drawn"));
    (top + 1..bottom)
        .map(|y| row_text(buffer, y))
        .filter(|row| row.contains('❯'))
        .collect()
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
