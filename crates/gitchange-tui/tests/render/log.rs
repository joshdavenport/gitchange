//! The Log panel: severity glyphs on the stream, core's advisories
//! arriving verbatim, and pins banding above the stream.

use gitchange_core::{Advisory, ChangeKind, ChangedFile, GitOperation};

use gitchange_tui::app::{App, Severity};

use crate::helpers::{render, snapshot};

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
        sides: None,
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
