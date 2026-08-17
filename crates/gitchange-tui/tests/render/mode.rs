//! Mode hunks beside content hunks (ADR 0017, issues #101, #104): the
//! mode delta draws as the file's first hunk row, tagged, glyphed and
//! dimmed like any other, and the file row counts it.

use gitchange_core::{ChangeKind, ChangedFile, Hunk, HunkIdentity, HunkStage, ModeDelta};
use ratatui::crossterm::event::KeyCode;
use ratatui::style::Modifier;

use gitchange_tui::app::App;
use gitchange_tui::theme::Theme;

use crate::helpers::{find_text, hunk, key, render_buffer, snapshot, text_of};

/// The row the mode delta draws, as the sandbox's `mixed-mode` scenario
/// shows it: `100644 → 100755` on a shell script.
const MODE_ROW: &str = "Mode changed (100644 → 100755)";

/// A mode hunk as core reports it (ADR 0017): no lines, no blob pair,
/// identity carried by the path alone.
fn mode_hunk(stage: HunkStage, changelist: Option<&str>) -> Hunk {
    Hunk {
        old_start: 0,
        old_lines: 0,
        new_start: 0,
        new_lines: 0,
        stage,
        index_only: false,
        identity: HunkIdentity::ModeChange,
        changelist: changelist.map(str::to_owned),
    }
}

/// A chmod'd, content-edited file: the mode hunk first (git's pseudo-hunk
/// #0 position), then the content hunk. `mode_delta` is the file's, since
/// mode bits are snapshot data rather than hunk identity.
fn mixed_file(mode: Hunk, content: Hunk) -> ChangedFile {
    ChangedFile {
        path: "scripts/release.sh".into(),
        kind: ChangeKind::Modified,
        binary: false,
        hunks: vec![mode, content],
        sides: None,
        mode_delta: Some(ModeDelta::Mode {
            before: 0o100644,
            after: 0o100755,
        }),
    }
}

/// An app carrying `file` beside the snapshot's ordinary ones, so the
/// mixed file draws in company.
fn app_on(file: ChangedFile) -> App {
    let mut app = App::new("repo");
    let mut snap = snapshot();
    snap.files.push(file);
    app.apply_snapshot(snap);
    app
}

/// Focus the Files panel and walk the cursor onto the mixed file — the
/// row position depends on the scope, so every test asks for it by path.
fn select_the_script(app: &mut App) {
    app.on_key(key(KeyCode::Char('3')));
    while app.selected_file().map(|file| file.path.as_str()) != Some("scripts/release.sh") {
        app.on_key(key(KeyCode::Char('j')));
    }
}

#[test]
fn a_mixed_files_mode_delta_draws_as_its_first_hunk_row() {
    // Issue #101's forward corner: a worktree chmod beside the staged
    // edit its worktree reverted. The mode row leads, tagged with its own
    // `○` where the content hunk's tag reads `◑`, and the file row counts
    // both hunks.
    let mut app = app_on(mixed_file(
        mode_hunk(HunkStage::Unstaged, Some("fixes")),
        hunk(14, Some("fixes"), HunkStage::StagedStale),
    ));
    select_the_script(&mut app);
    let buffer = render_buffer(&app);
    let text = text_of(&buffer);

    assert!(text.contains("scripts/release.sh 0/2"), "{text}");
    assert!(text.contains("release.sh (0/2 staged)"), "{text}");
    let (_, mode_y) = find_text(&buffer, MODE_ROW);
    let (_, content_y) = find_text(&buffer, "@@ -14,1 +14,2 @@");
    assert!(mode_y < content_y, "the mode row sits first\n{text}");
    // The All view tags every hunk, and the mode row's glyph is its own.
    assert!(text.contains("⟨fixes ○⟩"), "{text}");
    assert!(text.contains("⟨fixes ◑⟩"), "{text}");

    // Hunk mode opens on the mode row: it is a hunk like any other, so
    // the polite no-op does not apply (ADR 0017).
    app.on_key(key(KeyCode::Enter));
    let text = text_of(&render_buffer(&app));
    assert!(text.contains("release.sh — hunk 1 of 2"), "{text}");
}

#[test]
fn a_foreign_mode_row_dims_and_keeps_its_tag() {
    // The mirror corner, split across changelists: the staged flip is
    // 'chores' work while the edit is the drilled changelist's, so the
    // mode row dims and names its owner — the same treatment a foreign
    // text hunk gets.
    let theme = Theme::default();
    let mut app = app_on(mixed_file(
        mode_hunk(HunkStage::Staged, Some("chores")),
        hunk(14, Some("fixes"), HunkStage::Unstaged),
    ));
    app.on_key(key(KeyCode::Char('j'))); // drill into 'fixes'
    select_the_script(&mut app);
    let buffer = render_buffer(&app);
    let text = text_of(&buffer);

    let (x, y) = find_text(&buffer, MODE_ROW);
    assert!(
        buffer[(x, y)].modifier.contains(Modifier::DIM),
        "the foreign mode row dims\n{text}"
    );
    let (tag_x, tag_y) = find_text(&buffer, "⟨chores ●⟩");
    assert_eq!(tag_y, y, "its tag rides the same row");
    assert_eq!(buffer[(tag_x + 1, tag_y)].fg, theme.colors.dim);
    // Scoped counts: one own hunk, the mode hunk elsewhere.
    assert!(
        text.contains("release.sh (0/1 staged · 1 hunk elsewhere)"),
        "{text}"
    );
}
