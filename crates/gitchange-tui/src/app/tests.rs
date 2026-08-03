use std::time::Duration;

use gitchange_core::{
    BinarySides, BlobInfo, ChangedFile, Changelist, CommitInfo, CommitPayload, Head, Hunk,
    HunkIdentity, HunkLine, OidAnchor, PayloadFile,
};

use super::*;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

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

fn file(path: &str, hunks: Vec<Hunk>) -> ChangedFile {
    ChangedFile {
        path: path.into(),
        kind: ChangeKind::Modified,
        binary: false,
        binary_sides: None,
        hunks,
    }
}

/// Two changelists (first active) plus an unassigned hunk; print.css
/// spans two groups by hunk-level membership.
fn snapshot() -> Snapshot {
    Snapshot {
        files: vec![
            file("src/nav.astro", vec![hunk(8, None, HunkStage::Unstaged)]),
            file(
                "src/print.css",
                vec![
                    hunk(14, Some("fixes"), HunkStage::Staged),
                    hunk(41, Some("fixes"), HunkStage::Unstaged),
                    hunk(63, Some("chores"), HunkStage::Unstaged),
                ],
            ),
            file(
                "src/session.ts",
                vec![hunk(3, Some("chores"), HunkStage::Unstaged)],
            ),
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
            name: "main".into(),
        },
        recent_commits: vec![
            CommitInfo {
                short_id: "aaaa111".into(),
                author: "Josh Davenport-Smith".into(),
                summary: "second".into(),
            },
            CommitInfo {
                short_id: "bbbb222".into(),
                author: "Josh Davenport-Smith".into(),
                summary: "init".into(),
            },
        ],
        operation: None,
    }
}

fn app() -> App {
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot());
    app
}

#[test]
fn changelist_rows_are_all_then_user_order_then_unassigned_last() {
    let app = app();
    let rows = app.changelist_rows();
    let scopes: Vec<&Scope> = rows.iter().map(|row| &row.scope).collect();
    assert_eq!(
        scopes,
        vec![
            &Scope::All,
            &Scope::Changelist("fixes".into()),
            &Scope::Changelist("chores".into()),
            &Scope::Unassigned,
        ]
    );
    assert!(rows[1].active, "the active changelist is marked");
    assert_eq!(rows[0].count, 3, "all counts every changed file");
    assert_eq!(
        rows[2].count, 2,
        "hunk-level membership: print.css + session.ts"
    );
    assert_eq!(rows[3].count, 1);
}

#[test]
fn all_view_groups_files_by_changelist_with_unassigned_last() {
    let app = app();
    let rows = app.files_rows();
    let mut labels = Vec::new();
    for row in &rows {
        match row {
            FilesRow::Header { label, .. } => labels.push(format!("[{label}]")),
            FilesRow::File { entry, .. } => labels.push(entry.path.clone()),
        }
    }
    assert_eq!(
        labels,
        vec![
            "[fixes]",
            "src/print.css",
            "[chores]",
            "src/print.css",
            "src/session.ts",
            "[unassigned]",
            "src/nav.astro",
        ],
        "a file with hunks in two changelists appears in both groups"
    );
}

#[test]
fn drilling_into_a_changelist_flattens_its_files() {
    let mut app = app();
    app.on_key(key(KeyCode::Char('j'))); // select 'fixes'
    assert_eq!(app.scope(), Scope::Changelist("fixes".into()));
    let entries = app.file_entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path, "src/print.css");
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.focus, Panel::Files);
    assert_eq!(app.file_sel.as_ref().unwrap().path, "src/print.css");
}

#[test]
fn jk_in_files_moves_between_files_skipping_headers() {
    let mut app = app();
    app.on_key(key(KeyCode::Char('3')));
    assert_eq!(app.file_sel.as_ref().unwrap().path, "src/print.css");
    app.on_key(key(KeyCode::Char('j')));
    let sel = app.file_sel.clone().unwrap();
    assert_eq!(
        (sel.group, sel.path.as_str()),
        (Group::Changelist("chores".into()), "src/print.css"),
        "same path under the next group is a distinct row"
    );
    app.on_key(key(KeyCode::Char('j')));
    assert_eq!(app.file_sel.as_ref().unwrap().path, "src/session.ts");
    app.on_key(key(KeyCode::Char('j')));
    assert_eq!(app.file_sel.as_ref().unwrap().path, "src/nav.astro");
    app.on_key(key(KeyCode::Char('j')));
    assert_eq!(
        app.file_sel.as_ref().unwrap().path,
        "src/nav.astro",
        "selection clamps at the last file"
    );
}

#[test]
fn esc_walks_back_out_of_the_drill_down() {
    let mut app = app();
    app.on_key(key(KeyCode::Char('0')));
    assert_eq!(app.focus, Panel::Diff);
    app.on_key(key(KeyCode::Esc));
    assert_eq!(app.focus, Panel::Files);
    app.on_key(key(KeyCode::Esc));
    assert_eq!(app.focus, Panel::Changelists);
    app.on_key(key(KeyCode::Char('j')));
    assert_ne!(app.scope(), Scope::All);
    app.on_key(key(KeyCode::Esc));
    assert_eq!(app.scope(), Scope::All, "esc at the top selects all");
}

#[test]
fn all_view_tags_every_hunk_with_its_owner() {
    let mut app = app();
    app.on_key(key(KeyCode::Char('3')));
    let lines = app.diff_lines();
    let tags: Vec<(String, bool)> = lines
        .iter()
        .filter_map(|line| match line {
            DiffLine::HunkHeader { tag: Some(tag), .. } => Some((tag.label.clone(), tag.dim)),
            _ => None,
        })
        .collect();
    assert_eq!(
        tags,
        vec![
            ("fixes".into(), false),
            ("fixes".into(), false),
            ("chores".into(), false),
        ],
        "every hunk tagged, none dimmed, in the All view"
    );
    assert!(
        !lines.iter().any(|line| matches!(
            line,
            DiffLine::Content { foreign: true, .. } | DiffLine::HunkHeader { foreign: true, .. }
        )),
        "nothing is foreign in the All view"
    );
}

#[test]
fn drilled_view_dims_foreign_hunks_and_tags_only_them() {
    let mut app = app();
    app.on_key(key(KeyCode::Char('j'))); // drill into 'fixes'
    app.on_key(key(KeyCode::Enter));
    let lines = app.diff_lines();
    let headers: Vec<(Option<String>, bool)> = lines
        .iter()
        .filter_map(|line| match line {
            DiffLine::HunkHeader { tag, foreign, .. } => {
                Some((tag.as_ref().map(|tag| tag.label.clone()), *foreign))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        headers,
        vec![(None, false), (None, false), (Some("chores".into()), true),],
        "own hunks untagged; the foreign hunk carries a dim tag"
    );
    let foreign_tag = lines.iter().find_map(|line| match line {
        DiffLine::HunkHeader { tag: Some(tag), .. } => Some(tag.clone()),
        _ => None,
    });
    assert!(foreign_tag.unwrap().dim);
}

#[test]
fn unassigned_hunks_tag_without_a_staging_glyph() {
    let mut app = app();
    app.on_key(key(KeyCode::Char('3')));
    // Select nav.astro (the unassigned file).
    for _ in 0..3 {
        app.on_key(key(KeyCode::Char('j')));
    }
    let lines = app.diff_lines();
    let tag = lines
        .iter()
        .find_map(|line| match line {
            DiffLine::HunkHeader { tag: Some(tag), .. } => Some(tag.clone()),
            _ => None,
        })
        .unwrap();
    assert!(tag.unassigned);
    assert_eq!(tag.label, "unassigned");
    assert_eq!(tag.stage, None);
}

#[test]
fn diff_title_scopes_staged_counts_to_the_drilled_changelist() {
    let mut app = app();
    app.on_key(key(KeyCode::Char('3')));
    assert_eq!(app.diff_title(), "print.css (1/3 staged)");
    app.on_key(key(KeyCode::Esc)); // back to changelists
    app.on_key(key(KeyCode::Char('j'))); // drill into 'fixes'
    assert_eq!(
        app.diff_title(),
        "print.css (1/2 staged · 1 hunk elsewhere)",
        "counts scoped to the drilled changelist's own hunks"
    );
}

#[test]
fn file_selection_survives_a_snapshot_swap_by_identity() {
    let mut app = app();
    app.on_key(key(KeyCode::Char('3')));
    app.on_key(key(KeyCode::Char('j'))); // print.css under 'chores'
    let before = app.file_sel.clone().unwrap();

    // A refresh lands with a new file sorted before the selection.
    let mut next = snapshot();
    next.files.insert(
        0,
        file(
            "a-new.txt",
            vec![hunk(1, Some("fixes"), HunkStage::Unstaged)],
        ),
    );
    app.apply_snapshot(next);

    assert_eq!(
        app.file_sel.unwrap(),
        before,
        "selection kept by (group, path)"
    );
}

#[test]
fn a_vanished_selection_falls_to_the_nearest_sibling() {
    let mut app = app();
    app.on_key(key(KeyCode::Char('3')));
    app.on_key(key(KeyCode::Char('j')));
    app.on_key(key(KeyCode::Char('j'))); // session.ts (position 3)

    let mut next = snapshot();
    next.files.retain(|file| file.path != "src/session.ts");
    app.apply_snapshot(next);

    let sel = app.file_sel.unwrap();
    assert_eq!(
        sel.path, "src/nav.astro",
        "nearest sibling at the old position"
    );
}

#[test]
fn changelist_selection_survives_by_name_else_nearest() {
    let mut app = app();
    app.on_key(key(KeyCode::Char('j')));
    app.on_key(key(KeyCode::Char('j'))); // 'chores'
    assert_eq!(app.scope(), Scope::Changelist("chores".into()));

    // Reordered: 'chores' first — selection follows the name.
    let mut next = snapshot();
    next.changelists.reverse();
    app.apply_snapshot(next);
    assert_eq!(app.scope(), Scope::Changelist("chores".into()));

    // Deleted: nearest sibling by position.
    let mut next = snapshot();
    next.changelists
        .retain(|changelist| changelist.name != "chores");
    app.apply_snapshot(next);
    assert_eq!(app.scope(), Scope::Changelist("fixes".into()));
}

#[test]
fn commit_selection_survives_by_id_else_clamps() {
    let mut app = app();
    app.focus = Panel::Commits;
    app.on_key(key(KeyCode::Char('j')));
    assert_eq!(app.commit_row, 1);

    let mut next = snapshot();
    next.recent_commits.insert(
        0,
        CommitInfo {
            short_id: "cccc333".into(),
            author: "JD".into(),
            summary: "third".into(),
        },
    );
    app.apply_snapshot(next);
    assert_eq!(
        app.commit_row, 2,
        "still on 'init' after a new commit lands"
    );
}

#[test]
fn indicator_defers_until_the_threshold() {
    let mut app = app();
    let start = Instant::now();
    app.on_refresh_started(start);
    assert!(!app.indicator_visible(start + Duration::from_millis(100)));
    assert!(app.indicator_visible(start + INDICATOR_DELAY));
    app.apply_snapshot(snapshot());
    assert!(!app.indicator_visible(start + Duration::from_secs(2)));
}

#[test]
fn a_failed_refresh_keeps_the_last_snapshot_and_modals() {
    let mut app = app();
    app.on_refresh_started(Instant::now());
    app.on_refresh_failed("boom".into());
    assert!(app.snapshot.is_some());
    let modal = app.error_modal.as_ref().expect("the error modal opens");
    assert_eq!(modal.title, "Refresh failed");
    assert_eq!(modal.detail, "boom");
    assert!(
        app.log
            .iter()
            .any(|entry| entry.severity == Severity::Error
                && entry.text.contains("Refresh failed")),
        "every modal is also logged at ✗ (ADR 0007)"
    );
    assert!(!app.indicator_visible(Instant::now() + Duration::from_secs(2)));
}

#[test]
fn a_repeated_refresh_failure_does_not_remodal() {
    let mut app = app();
    app.on_refresh_failed("boom".into());
    app.on_key(key(KeyCode::Esc));
    assert!(app.error_modal.is_none());

    // The polling engine retries and fails identically: silent.
    app.on_refresh_failed("boom".into());
    assert!(app.error_modal.is_none(), "same failure never re-modals");

    // A success clears the memory; the next failure modals again.
    app.apply_snapshot(snapshot());
    app.on_refresh_failed("boom".into());
    assert!(app.error_modal.is_some());
}

#[test]
fn the_error_modal_swallows_keys_scrolls_and_dismisses() {
    let mut app = app();
    app.show_error("Commit failed", "line one\nline two");
    let before = app.scope();
    assert_eq!(app.on_key(key(KeyCode::Char('j'))), None);
    assert_eq!(app.error_modal.as_ref().unwrap().scroll, 1);
    assert_eq!(app.on_key(key(KeyCode::Char('k'))), None);
    assert_eq!(app.error_modal.as_ref().unwrap().scroll, 0);
    assert_eq!(app.scope(), before, "keys never leak past the modal");
    app.on_key(key(KeyCode::Enter));
    assert!(app.error_modal.is_none(), "enter dismisses");
}

#[test]
fn conditions_pin_and_self_clear() {
    let mut app = app();
    assert!(app.pins().is_empty());

    app.on_watcher_degraded();
    assert_eq!(app.pins(), vec!["watcher unavailable — polling"]);
    assert!(
        app.log.iter().any(|entry| entry.severity == Severity::Info
            && entry.text.contains("falling back to polling")),
        "the event marks the moment the condition began"
    );
    app.on_watcher_recovered();
    assert!(app.pins().is_empty(), "pins self-clear, never dismissed");

    // Operation + detached HEAD pins derive from the snapshot.
    let mut busy = snapshot();
    busy.operation = Some(gitchange_core::GitOperation::Rebase);
    busy.head = Head::Detached {
        short_id: "abc1234".into(),
    };
    busy.files.push(ChangedFile {
        path: "src/session.ts".into(),
        kind: ChangeKind::Conflicted,
        binary: false,
        binary_sides: None,
        hunks: Vec::new(),
    });
    app.apply_snapshot(busy);
    assert_eq!(
        app.pins(),
        vec![
            "rebase in progress — 1 conflicted".to_owned(),
            "detached HEAD — commits belong to no branch".to_owned(),
        ]
    );
    assert!(
        app.log
            .iter()
            .any(|entry| entry.text.contains("rebase detected")),
        "the operation's start is logged once"
    );
}

#[test]
fn advisories_land_in_the_log_at_notice_severity() {
    let mut app = app();
    let mut next = snapshot();
    next.advisories = vec![gitchange_core::Advisory::AutoCaptured {
        path: "src/print.css".into(),
        new_start: 41,
        changelist: "fixes".into(),
    }];
    app.apply_snapshot(next);
    assert!(app.log.iter().any(|entry| {
        entry.severity == Severity::Notice
            && entry
                .text
                .contains("auto-captured hunk at src/print.css:41")
            && entry.text.contains("'fixes'")
    }));
}

#[test]
fn the_operation_guard_makes_c_a_soft_noop() {
    let mut app = app();
    let mut busy = snapshot();
    busy.operation = Some(gitchange_core::GitOperation::Merge);
    app.apply_snapshot(busy);

    app.on_key(key(KeyCode::Char('j'))); // 'fixes' — a committable scope
    assert_eq!(
        app.on_key(key(KeyCode::Char('c'))),
        None,
        "commit is globally guarded during a git operation"
    );
    assert!(app.overlay.is_none());
    assert!(app.log.iter().any(|entry| {
        entry.severity == Severity::Info
            && entry.text == "merge in progress — conclude or abort it first"
    }));

    // Staging is never operation-guarded.
    app.on_key(key(KeyCode::Char('3')));
    assert!(matches!(
        app.on_key(key(KeyCode::Char(' '))),
        Some(Action::Op(Op::StageFile { .. }))
    ));
}

#[test]
fn conflicted_files_group_first_and_space_refuses() {
    let mut app = app();
    let mut busy = snapshot();
    busy.files.insert(
        0,
        ChangedFile {
            path: "src/merge.ts".into(),
            kind: ChangeKind::Conflicted,
            binary: false,
            binary_sides: None,
            hunks: Vec::new(),
        },
    );
    app.apply_snapshot(busy);

    let rows = app.files_rows();
    let Some(FilesRow::Header {
        label, conflicted, ..
    }) = rows.first()
    else {
        panic!("expected a header first, got {rows:?}");
    };
    assert_eq!(label, "conflicts");
    assert!(conflicted);
    let Some(FilesRow::File { entry, .. }) = rows.get(1) else {
        panic!("expected the conflicted file second");
    };
    assert_eq!(entry.path, "src/merge.ts");
    assert_eq!(entry.group, Group::Conflicts);

    // `space` on conflicted content politely refuses (info).
    app.on_key(key(KeyCode::Char('3')));
    for _ in 0..app.file_entries().len() {
        app.on_key(key(KeyCode::Char('k'))); // up to the conflicted row
    }
    assert_eq!(app.file_sel.as_ref().unwrap().path, "src/merge.ts");
    assert_eq!(app.on_key(key(KeyCode::Char(' '))), None);
    assert!(app.log.iter().any(|entry| {
        entry.severity == Severity::Info && entry.text.contains("resolve outside gitchange")
    }));

    // The diff shows the one-line placeholder, never conflict text.
    assert!(app.diff_lines().iter().any(|line| matches!(
        line,
        DiffLine::Conflict(text) if text == "conflicted — resolve outside gitchange"
    )));
    assert_eq!(app.diff_title(), "merge.ts (conflicted)");
}

#[test]
fn help_opens_and_closes_without_leaking_keys() {
    let mut app = app();
    app.on_key(key(KeyCode::Char('?')));
    assert!(app.help_open);
    let before = app.scope();
    app.on_key(key(KeyCode::Char('j'))); // swallowed by help
    assert_eq!(app.scope(), before);
    app.on_key(key(KeyCode::Esc));
    assert!(!app.help_open);
}

#[test]
fn quit_and_refresh_actions() {
    let mut app = app();
    assert_eq!(app.on_key(key(KeyCode::Char('R'))), Some(Action::Refresh));
    assert_eq!(app.on_key(key(KeyCode::Char('q'))), Some(Action::Quit));
    assert_eq!(
        app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        Some(Action::Quit)
    );
}

#[test]
fn diff_scroll_clamps_to_content() {
    let mut app = app();
    app.on_key(key(KeyCode::Char('0')));
    let lines = app.diff_lines().len() as u16;
    for _ in 0..200 {
        app.on_key(key(KeyCode::Char('j')));
    }
    assert_eq!(app.diff_scroll, lines - 1);
    for _ in 0..200 {
        app.on_key(key(KeyCode::Char('k')));
    }
    assert_eq!(app.diff_scroll, 0);
}

#[test]
fn files_count_is_position_of_total() {
    let mut app = app();
    app.on_key(key(KeyCode::Char('3')));
    app.on_key(key(KeyCode::Char('j')));
    assert_eq!(app.files_count(), (2, 4));
}

// ── hunk mode (ticket #32, prototype variant C) ─────────────────

/// Focus Files with print.css (3 hunks) selected and enter hunk mode.
fn hunk_mode_app() -> App {
    let mut app = app();
    app.on_key(key(KeyCode::Char('3')));
    app.on_key(key(KeyCode::Enter));
    app
}

#[test]
fn enter_on_a_file_enters_hunk_mode_and_esc_leaves_it() {
    let mut app = hunk_mode_app();
    assert_eq!(app.focus, Panel::Diff);
    assert_eq!(app.hunk_sel, Some(0));
    assert_eq!(app.diff_title(), "print.css — hunk 1 of 3");
    app.on_key(key(KeyCode::Esc));
    assert_eq!(app.focus, Panel::Files);
    assert_eq!(app.hunk_sel, None, "esc leaves hunk mode, not the app");
}

#[test]
fn zero_key_diff_focus_is_scroll_mode_not_hunk_mode() {
    let mut app = app();
    app.on_key(key(KeyCode::Char('0')));
    assert_eq!(app.focus, Panel::Diff);
    assert_eq!(app.hunk_sel, None);
    app.on_key(key(KeyCode::Char('j')));
    assert_eq!(app.diff_scroll, 1, "j scrolls lines in scroll mode");
}

#[test]
fn jk_walks_hunks_clamped_and_marks_the_diff_selection() {
    let mut app = hunk_mode_app();
    app.on_key(key(KeyCode::Char('j')));
    assert_eq!(app.hunk_sel, Some(1));
    assert_eq!(app.diff_title(), "print.css — hunk 2 of 3");
    app.on_key(key(KeyCode::Char('j')));
    app.on_key(key(KeyCode::Char('j')));
    assert_eq!(app.hunk_sel, Some(2), "selection clamps at the last hunk");
    assert_eq!(app.diff_scroll, 0, "hunk mode never line-scrolls");

    let selected: Vec<bool> = app
        .diff_lines()
        .iter()
        .filter_map(|line| match line {
            DiffLine::HunkHeader { selected, .. } => Some(*selected),
            _ => None,
        })
        .collect();
    assert_eq!(selected, vec![false, false, true]);
    assert!(app.selected_hunk_line().is_some());
}

#[test]
fn hunk_selection_survives_a_snapshot_swap_by_content() {
    let mut app = hunk_mode_app();
    app.on_key(key(KeyCode::Char('j'))); // hunk at 41
    let before = app.selected_file().unwrap().hunks[1].clone();

    // A refresh lands with a new hunk inserted before the selection.
    let mut next = snapshot();
    next.files[1]
        .hunks
        .insert(0, hunk(2, Some("fixes"), HunkStage::Unstaged));
    app.apply_snapshot(next);

    let index = app.hunk_sel.unwrap();
    assert_eq!(index, 2, "selection follows the hunk's content");
    assert_eq!(
        app.selected_file().unwrap().hunks[index].identity,
        before.identity
    );
}

#[test]
fn a_vanished_hunk_selection_clamps_and_an_empty_file_exits_hunk_mode() {
    let mut app = hunk_mode_app();
    app.on_key(key(KeyCode::Char('j')));
    app.on_key(key(KeyCode::Char('j'))); // last hunk

    let mut next = snapshot();
    next.files[1].hunks.truncate(1);
    app.apply_snapshot(next);
    assert_eq!(app.hunk_sel, Some(0), "clamped to the remaining hunk");

    let mut gone = snapshot();
    gone.files[1].hunks.clear();
    app.apply_snapshot(gone);
    assert_eq!(app.hunk_sel, None);
    assert_eq!(app.focus, Panel::Files, "nothing selectable ends hunk mode");
}

// ── assign flow (tickets #32/#41, prototype variant D) ──────────

/// The shared fixture owns every print.css hunk; this one leaves the
/// middle one unassigned so the `A` scope has something to find and
/// `ctrl+a` has two foreign sources to name.
fn mixed_app() -> App {
    let mut snapshot = snapshot();
    snapshot.files[1].hunks[1].changelist = None;
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot);
    app.on_key(key(KeyCode::Char('3'))); // print.css under 'fixes'
    app
}

/// The payload the open popup carries, resolved to hunk positions.
fn payload_starts(app: &App) -> Vec<u32> {
    let Some(Overlay::Assign { payload, .. }) = &app.overlay else {
        panic!("no assign popup open, got {:?}", app.overlay);
    };
    let (_, hunks) = app.resolve_payload(payload).expect("payload resolves");
    hunks.iter().map(|hunk| hunk.new_start).collect()
}

#[test]
fn assign_rows_list_changelists_with_active_annotated_then_create_new() {
    let app = app();
    assert_eq!(
        app.assign_rows(),
        vec![
            AssignRow::Changelist {
                name: "fixes".into(),
                active: true,
            },
            AssignRow::Changelist {
                name: "chores".into(),
                active: false,
            },
            AssignRow::CreateNew,
        ]
    );
}

#[test]
fn a_in_files_opens_the_popup_and_enter_assigns_the_rows_group_hunks() {
    let mut app = app();
    app.on_key(key(KeyCode::Char('3'))); // print.css under 'fixes'
    app.on_key(key(KeyCode::Char('a')));
    assert!(matches!(
        app.overlay,
        Some(Overlay::Assign {
            payload: AssignPayload::FileRow(_),
            row: 0
        })
    ));

    app.on_key(key(KeyCode::Char('j'))); // select 'chores'
    let action = app.on_key(key(KeyCode::Enter));
    let Some(Action::Op(Op::Assign {
        path,
        hunks,
        target,
        create,
    })) = action
    else {
        panic!("expected an assign op, got {action:?}");
    };
    assert_eq!(path, "src/print.css");
    assert_eq!(target, "chores");
    assert!(!create);
    assert_eq!(
        hunks.iter().map(|hunk| hunk.new_start).collect::<Vec<_>>(),
        vec![14, 41],
        "only the row's group-owned hunks are assigned"
    );
    assert!(app.overlay.is_none(), "confirming closes the popup");
}

#[test]
fn a_in_hunk_mode_targets_the_selected_hunk() {
    let mut app = hunk_mode_app();
    app.on_key(key(KeyCode::Char('j'))); // hunk at 41
    app.on_key(key(KeyCode::Char('a')));
    assert!(matches!(
        app.overlay,
        Some(Overlay::Assign {
            payload: AssignPayload::Hunk { ref hunk, .. },
            ..
        }) if hunk.new_start == 41
    ));
    let action = app.on_key(key(KeyCode::Enter)); // → 'fixes'
    assert!(matches!(
        action,
        Some(Action::Op(Op::Assign { ref hunks, ref target, .. }))
            if hunks.len() == 1 && target == "fixes"
    ));
}

#[test]
fn shift_a_targets_the_files_unassigned_hunks_from_files_and_hunk_mode() {
    let mut app = mixed_app();
    app.on_key(key(KeyCode::Char('A')));
    assert!(matches!(
        app.overlay,
        Some(Overlay::Assign {
            payload: AssignPayload::UnassignedHunks { ref path },
            ..
        }) if path == "src/print.css"
    ));
    assert_eq!(payload_starts(&app), vec![41], "only the unassigned hunk");
    app.on_key(key(KeyCode::Esc));

    // Same scope from hunk mode, where the cursor sits on a hunk the
    // payload does not include — the off-row reach the popup states.
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.hunk_sel, Some(0), "cursor on the 'fixes' hunk at 14");
    app.on_key(key(KeyCode::Char('A')));
    assert_eq!(payload_starts(&app), vec![41]);
}

#[test]
fn ctrl_a_targets_every_hunk_of_the_file_foreign_included() {
    let mut app = mixed_app();
    app.on_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
    assert!(matches!(
        app.overlay,
        Some(Overlay::Assign {
            payload: AssignPayload::AllHunks { ref path },
            ..
        }) if path == "src/print.css"
    ));
    assert_eq!(payload_starts(&app), vec![14, 41, 63]);
    app.on_key(key(KeyCode::Esc));

    // Same scope from hunk mode, cursor on the first hunk.
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.hunk_sel, Some(0));
    app.on_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
    assert_eq!(payload_starts(&app), vec![14, 41, 63]);
}

#[test]
fn ctrl_shift_a_does_exactly_what_ctrl_a_does() {
    // ADR 0013: the control byte a plain terminal sends carries no
    // shift bit, so SHIFT must not be consulted — under either
    // encoding a terminal might use for the letter.
    for code in [KeyCode::Char('a'), KeyCode::Char('A')] {
        let mut app = mixed_app();
        app.on_key(KeyEvent::new(
            code,
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        ));
        assert!(
            matches!(
                app.overlay,
                Some(Overlay::Assign {
                    payload: AssignPayload::AllHunks { .. },
                    ..
                })
            ),
            "{code:?} with ctrl+shift must assign all hunks, got {:?}",
            app.overlay
        );
        assert_eq!(payload_starts(&app), vec![14, 41, 63]);
    }
}

/// ADR 0013's invariant, guarded over the source of the modules that
/// handle keys: no binding may branch on SHIFT. A terminal cannot be
/// relied upon to report it on a non-printing key, and
/// `ctrl+shift+<letter>` is indistinguishable from `ctrl+<letter>` —
/// so a binding that reads it degrades into some *other* action rather
/// than into nothing. Shifted letters (`A`, `R`) are matched as their
/// own `KeyCode::Char`, which needs no modifier test. A guard, not a
/// proof: the behavioural half is
/// [`ctrl_shift_a_does_exactly_what_ctrl_a_does`].
#[test]
fn no_binding_inspects_the_shift_modifier() {
    for (name, source) in [
        ("app/mod.rs", include_str!("mod.rs")),
        ("app/overlay.rs", include_str!("overlay.rs")),
        ("app/selection.rs", include_str!("selection.rs")),
        ("app/status.rs", include_str!("status.rs")),
        ("app/view.rs", include_str!("view.rs")),
        ("ui.rs", include_str!("../ui.rs")),
        ("lib.rs", include_str!("../lib.rs")),
    ] {
        // This module's own SHIFT mentions live past the split.
        let production = source.split("#[cfg(test)]").next().unwrap_or_default();
        assert!(
            !production.contains("KeyModifiers::SHIFT"),
            "{name} inspects SHIFT"
        );
    }
}

#[test]
fn under_the_unassigned_group_a_and_shift_a_reach_the_same_hunks() {
    let mut app = mixed_app();
    app.on_key(key(KeyCode::Char('2')));
    for _ in 0..3 {
        app.on_key(key(KeyCode::Char('j'))); // all → fixes → chores → unassigned
    }
    assert_eq!(app.scope(), Scope::Unassigned);
    app.on_key(key(KeyCode::Char('3')));
    app.on_key(key(KeyCode::Char('j'))); // nav.astro → print.css

    let row = app.selected_assign_payload().expect("a row is selected");
    assert!(matches!(row, AssignPayload::FileRow(_)));
    assert_eq!(
        app.resolve_payload(&row),
        app.resolve_payload(&AssignPayload::UnassignedHunks {
            path: "src/print.css".into()
        }),
        "the row's group is unassigned, so `a` and `A` coincide"
    );
}

#[test]
fn an_empty_payload_is_a_polite_no_op_with_a_log_line() {
    // Every print.css hunk is owned, so `A` has nothing to assign.
    let mut app = app();
    app.on_key(key(KeyCode::Char('3')));
    let logged = app.log.len();
    app.on_key(key(KeyCode::Char('A')));
    assert!(app.overlay.is_none(), "no popup over an empty payload");
    assert!(app.error_modal.is_none(), "an empty scope is not an error");
    assert_eq!(app.log.len(), logged + 1);
    let entry = app.log.last().unwrap();
    assert_eq!(entry.severity, Severity::Info);
    assert_eq!(
        entry.text,
        "no unassigned hunks in src/print.css — nothing to assign"
    );
}

#[test]
fn assign_on_a_conflicted_row_says_why_rather_than_no_hunks() {
    let mut app = app();
    let mut busy = snapshot();
    busy.files.insert(
        0,
        ChangedFile {
            path: "src/merge.ts".into(),
            kind: ChangeKind::Conflicted,
            binary: false,
            binary_sides: None,
            hunks: Vec::new(),
        },
    );
    app.apply_snapshot(busy);
    app.on_key(key(KeyCode::Char('3')));
    for _ in 0..app.file_entries().len() {
        app.on_key(key(KeyCode::Char('k'))); // up to the conflicted row
    }
    assert_eq!(app.file_sel.as_ref().unwrap().path, "src/merge.ts");

    for assign in [
        key(KeyCode::Char('a')),
        key(KeyCode::Char('A')),
        KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
    ] {
        let logged = app.log.len();
        assert_eq!(app.on_key(assign), None);
        assert!(app.overlay.is_none());
        assert_eq!(app.log.len(), logged + 1);
        assert_eq!(
            app.log.last().unwrap().text,
            "src/merge.ts is conflicted — resolve outside gitchange"
        );
    }
}

#[test]
fn the_assign_popup_states_the_payload_before_it_lands() {
    let mut app = mixed_app();
    app.on_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
    let Some(Overlay::Assign { payload, .. }) = app.overlay.clone() else {
        panic!("expected the assign popup");
    };
    assert_eq!(
        app.assign_description(&payload),
        "3 hunks in src/print.css (1 from unassigned, 1 from 'chores')",
        "off-row reach is named: counts first, then whose hunks they are"
    );
    app.on_key(key(KeyCode::Esc));

    app.on_key(key(KeyCode::Char('A')));
    let Some(Overlay::Assign { payload, .. }) = app.overlay.clone() else {
        panic!("expected the assign popup");
    };
    assert_eq!(
        app.assign_description(&payload),
        "1 unassigned hunk in src/print.css",
        "the noun names the source, so no redundant provenance"
    );
}

#[test]
fn the_popup_names_a_single_hunks_source_when_it_is_foreign() {
    let mut app = app();
    app.on_key(key(KeyCode::Char('2')));
    app.on_key(key(KeyCode::Char('j'))); // drill into 'fixes'
    app.on_key(key(KeyCode::Char('3')));
    app.on_key(key(KeyCode::Enter)); // hunk mode on print.css
    app.on_key(key(KeyCode::Char('j')));
    app.on_key(key(KeyCode::Char('j'))); // the 'chores' hunk at 63
    app.on_key(key(KeyCode::Char('a')));
    let Some(Overlay::Assign { payload, .. }) = app.overlay.clone() else {
        panic!("expected the assign popup");
    };
    assert_eq!(
        app.assign_description(&payload),
        "1 hunk in src/print.css @@ -63,1 +63,2 (from 'chores')"
    );
}

#[test]
fn enter_is_drill_in_only_and_never_assigns() {
    let mut app = hunk_mode_app();
    // Hunk mode is the end of the drill; enter does nothing there,
    // with or without shift (which a plain terminal drops anyway).
    assert_eq!(app.on_key(key(KeyCode::Enter)), None);
    assert!(app.overlay.is_none());
    assert_eq!(
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)),
        None
    );
    assert!(app.overlay.is_none());

    // Diff scroll mode too.
    app.on_key(key(KeyCode::Char('0')));
    assert_eq!(app.on_key(key(KeyCode::Enter)), None);
    assert!(app.overlay.is_none());
}

#[test]
fn m_is_unbound() {
    let mut hunks = hunk_mode_app();
    assert_eq!(hunks.on_key(key(KeyCode::Char('m'))), None);
    assert!(hunks.overlay.is_none());

    let mut files = app();
    files.on_key(key(KeyCode::Char('3')));
    assert_eq!(files.on_key(key(KeyCode::Char('m'))), None);
    assert!(files.overlay.is_none());
}

#[test]
fn the_create_new_escape_hatch_yields_a_create_and_assign_op() {
    let mut app = hunk_mode_app();
    app.on_key(key(KeyCode::Char('a'))); // open popup (selected hunk)
    app.on_key(key(KeyCode::Char('j')));
    app.on_key(key(KeyCode::Char('j'))); // '+ create new changelist…'
    app.on_key(key(KeyCode::Enter));
    assert!(matches!(
        app.overlay,
        Some(Overlay::Input {
            kind: InputKind::NewChangelistForAssign { .. },
            ..
        })
    ));
    for c in "docs".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    let action = app.on_key(key(KeyCode::Enter));
    assert!(matches!(
        action,
        Some(Action::Op(Op::Assign { ref target, create: true, .. })) if target == "docs"
    ));
}

#[test]
fn esc_from_the_escape_hatch_returns_to_the_assign_popup() {
    let mut app = hunk_mode_app();
    app.on_key(key(KeyCode::Char('a')));
    app.on_key(key(KeyCode::Char('j')));
    app.on_key(key(KeyCode::Char('j')));
    app.on_key(key(KeyCode::Enter)); // into the input
    app.on_key(key(KeyCode::Esc));
    assert!(matches!(app.overlay, Some(Overlay::Assign { .. })));
    app.on_key(key(KeyCode::Esc));
    assert!(app.overlay.is_none());
}

// ── changelist ops (n/d/r/s) ────────────────────────────────────

// ── space staging (ticket #33) ──────────────────────────────────

#[test]
fn space_in_files_toggles_whole_file_by_its_marker() {
    let mut app = app();
    app.on_key(key(KeyCode::Char('3'))); // print.css (◐ partial)
    assert_eq!(
        app.on_key(key(KeyCode::Char(' '))),
        Some(Action::Op(Op::StageFile {
            path: "src/print.css".into()
        })),
        "◐ stages the rest"
    );

    // A fully-staged file toggles the other way.
    let mut staged = snapshot();
    for hunk in &mut staged.files[1].hunks {
        hunk.stage = HunkStage::Staged;
    }
    app.apply_snapshot(staged);
    assert_eq!(
        app.on_key(key(KeyCode::Char(' '))),
        Some(Action::Op(Op::UnstageFile {
            path: "src/print.css".into()
        })),
        "● unstages"
    );
}

#[test]
fn space_in_hunk_mode_toggles_the_selected_hunk() {
    let mut app = hunk_mode_app(); // print.css: ● at 14, ○ at 41, ○ at 63
    let action = app.on_key(key(KeyCode::Char(' ')));
    assert!(
        matches!(
            action,
            Some(Action::Op(Op::UnstageHunk { ref path, ref hunk }))
                if path == "src/print.css" && hunk.new_start == 14
        ),
        "● hunk unstages, got {action:?}"
    );

    app.on_key(key(KeyCode::Char('j')));
    let action = app.on_key(key(KeyCode::Char(' ')));
    assert!(
        matches!(
            action,
            Some(Action::Op(Op::StageHunk { ref hunk, .. })) if hunk.new_start == 41
        ),
        "○ hunk stages, got {action:?}"
    );

    // A ◑ hunk stages too: index := worktree re-stages the edit.
    let mut stale = snapshot();
    stale.files[1].hunks[1].stage = HunkStage::StagedStale;
    app.apply_snapshot(stale);
    let action = app.on_key(key(KeyCode::Char(' ')));
    assert!(
        matches!(
            action,
            Some(Action::Op(Op::StageHunk { ref hunk, .. })) if hunk.new_start == 41
        ),
        "◑ hunk stages, got {action:?}"
    );
}

#[test]
fn space_outside_files_and_hunk_mode_does_nothing() {
    let mut app = app();
    assert_eq!(app.on_key(key(KeyCode::Char(' '))), None); // changelists
    app.on_key(key(KeyCode::Char('0'))); // diff scroll mode
    assert_eq!(app.on_key(key(KeyCode::Char(' '))), None);
}

// ── commit flow (ticket #33, commit-flow prototype A–D) ─────────

fn payload(staged: usize, stale: usize) -> CommitPayload {
    CommitPayload {
        files: vec![PayloadFile {
            path: "src/print.css".into(),
            staged_hunks: staged,
            stale_hunks: stale,
            hunks: Vec::new(),
            whole_file: None,
        }],
    }
}

/// Open the dialog and type a subject line.
fn dialog_app(payload: CommitPayload) -> App {
    let mut app = app();
    app.open_commit_dialog(Some("fixes".into()), payload);
    for c in "fix: x".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    app
}

#[test]
fn c_on_all_is_a_polite_noop_and_opens_the_flow_scoped() {
    let mut app = app();
    assert_eq!(app.on_key(key(KeyCode::Char('c'))), None);
    assert!(app.overlay.is_none(), "c on the All view opens nothing");
    assert!(
        app.log
            .iter()
            .any(|entry| entry.severity == Severity::Info && entry.text.contains("all is a view")),
        "…but says why (ADR 0004: a no-op with a message)"
    );

    app.on_key(key(KeyCode::Char('j'))); // 'fixes'
    assert_eq!(
        app.on_key(key(KeyCode::Char('c'))),
        Some(Action::Commit(CommitStep::Open {
            changelist: Some("fixes".into())
        }))
    );

    for _ in 0..2 {
        app.on_key(key(KeyCode::Char('j'))); // down to 'unassigned'
    }
    assert_eq!(
        app.on_key(key(KeyCode::Char('c'))),
        Some(Action::Commit(CommitStep::Open { changelist: None })),
        "unassigned is committable"
    );
}

#[test]
fn dialog_edits_message_body_and_flags_then_commits() {
    let mut app = dialog_app(payload(2, 0));
    app.on_key(key(KeyCode::Tab));
    for c in "why".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    app.on_key(key(KeyCode::Enter)); // newline in the body, not commit
    for c in "more".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    app.on_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL));
    app.on_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
    app.on_key(key(KeyCode::Tab)); // back to the message
    let action = app.on_key(key(KeyCode::Enter));
    let Some(Action::Commit(CommitStep::Commit(draft))) = action else {
        panic!("expected a commit step, got {action:?}");
    };
    assert_eq!(draft.message, "fix: x");
    assert_eq!(draft.body, "why\nmore");
    assert!(draft.no_verify && draft.amend);
    assert_eq!(draft.changelist.as_deref(), Some("fixes"));
    assert!(app.overlay.is_none(), "confirming closes the dialog");
}

#[test]
fn dialog_ignores_enter_on_an_empty_message_and_esc_cancels() {
    let mut app = app();
    app.open_commit_dialog(Some("fixes".into()), payload(1, 0));
    assert_eq!(app.on_key(key(KeyCode::Enter)), None);
    assert!(matches!(app.overlay, Some(Overlay::Commit(_))));
    // Overlays swallow global keys: 'q' types, it doesn't quit.
    assert_eq!(app.on_key(key(KeyCode::Char('q'))), None);
    app.on_key(key(KeyCode::Esc));
    assert!(app.overlay.is_none());
}

#[test]
fn a_stale_payload_routes_through_the_warn_overlay() {
    let mut app = dialog_app(payload(4, 1));
    assert_eq!(app.on_key(key(KeyCode::Enter)), None);
    assert!(
        matches!(app.overlay, Some(Overlay::CommitStale(_))),
        "◑ in the payload warns before committing"
    );

    // esc returns to the dialog with the draft intact.
    app.on_key(key(KeyCode::Esc));
    let Some(Overlay::Commit(ref draft)) = app.overlay else {
        panic!("expected the dialog back, got {:?}", app.overlay);
    };
    assert_eq!(draft.message, "fix: x");

    // enter on the warn commits as-is.
    app.on_key(key(KeyCode::Enter));
    let action = app.on_key(key(KeyCode::Enter));
    assert!(matches!(
        action,
        Some(Action::Commit(CommitStep::Commit(_)))
    ));

    // w aligns index to worktree, then commits.
    let mut app = dialog_app(payload(4, 1));
    app.on_key(key(KeyCode::Enter));
    let action = app.on_key(key(KeyCode::Char('w')));
    assert!(matches!(
        action,
        Some(Action::Commit(CommitStep::AlignAndCommit(_)))
    ));
}

#[test]
fn an_empty_payload_offers_stage_all_with_the_changelists_counts() {
    let mut app = app();
    app.offer_stage_all(Some("fixes".into()));
    assert_eq!(
        app.overlay,
        Some(Overlay::CommitStageAll {
            changelist: Some("fixes".into()),
            hunks: 2,
            files: 1,
        }),
        "fixes owns two print.css hunks"
    );
    let action = app.on_key(key(KeyCode::Enter));
    assert_eq!(
        action,
        Some(Action::Commit(CommitStep::StageAllAndOpen {
            changelist: Some("fixes".into())
        }))
    );

    // A hunk-less changelist gets a log line, not an offer.
    let mut next = snapshot();
    next.changelists.push(Changelist {
        name: "empty".into(),
    });
    app.apply_snapshot(next);
    app.offer_stage_all(Some("empty".into()));
    assert!(app.overlay.is_none());
    assert!(app.log.iter().any(|entry| entry.text.contains("empty")));
}

#[test]
fn hook_rejection_modal_lands_over_the_restored_dialog() {
    let mut app = dialog_app(payload(5, 0));
    let action = app.on_key(key(KeyCode::Enter));
    let Some(Action::Commit(CommitStep::Commit(draft))) = action else {
        panic!("expected a commit step, got {action:?}");
    };

    // The main loop's Err(HookRejected) path: dialog restored as
    // confirmed, rejection modal on top (ADR 0007).
    app.show_error(
        "Commit failed",
        "husky - pre-commit hook exited with code 1",
    );
    app.restore_commit_dialog(draft);

    // The modal swallows keys; nothing reaches the dialog beneath.
    app.on_key(key(KeyCode::Char('x')));
    assert!(app.error_modal.is_some());
    app.on_key(key(KeyCode::Esc));
    assert!(app.error_modal.is_none(), "esc dismisses the modal only");
    let Some(Overlay::Commit(restored)) = &app.overlay else {
        panic!("the dialog survives beneath the modal");
    };
    assert_eq!(restored.message, "fix: x", "the composed message is kept");
}

#[test]
fn drift_reconfirms_with_the_message_kept() {
    let mut app = dialog_app(payload(5, 0));
    let action = app.on_key(key(KeyCode::Enter));
    let Some(Action::Commit(CommitStep::Commit(draft))) = action else {
        panic!("expected a commit step, got {action:?}");
    };

    // The main loop reports drift with a fresh payload.
    app.commit_drifted(draft, payload(4, 0));
    let Some(Overlay::CommitDrift {
        ref draft,
        ref previous,
    }) = app.overlay
    else {
        panic!("expected the drift overlay, got {:?}", app.overlay);
    };
    assert_eq!(draft.message, "fix: x", "message kept");
    assert_eq!(draft.payload, payload(4, 0), "payload replaced");
    assert_eq!(*previous, payload(5, 0));

    // e goes back to editing with everything kept.
    app.on_key(key(KeyCode::Char('e')));
    assert!(matches!(app.overlay, Some(Overlay::Commit(_))));

    // enter from the dialog re-commits the updated payload.
    let action = app.on_key(key(KeyCode::Enter));
    assert!(matches!(
        action,
        Some(Action::Commit(CommitStep::Commit(ref draft)))
            if draft.payload == payload(4, 0)
    ));
}

#[test]
fn a_failed_commit_restores_the_dialog_and_success_closes_it() {
    let mut app = dialog_app(payload(3, 0));
    let Some(Action::Commit(CommitStep::Commit(draft))) = app.on_key(key(KeyCode::Enter)) else {
        panic!("expected a commit step");
    };
    app.restore_commit_dialog(draft.clone());
    assert!(
        matches!(app.overlay, Some(Overlay::Commit(ref restored)) if *restored == draft),
        "the dialog comes back exactly as confirmed"
    );
    app.commit_succeeded();
    assert!(
        app.overlay.is_none(),
        "no toast; the refresh is the feedback"
    );
}

#[test]
fn payload_counts_wording_matches_the_prototype() {
    let mut multi = payload(4, 1);
    multi.files.push(PayloadFile {
        path: "src/a.ts".into(),
        staged_hunks: 0,
        stale_hunks: 0,
        hunks: Vec::new(),
        whole_file: None,
    });
    assert_eq!(payload_counts(&multi), "5 staged hunks in 2 files");
    assert_eq!(payload_counts(&payload(1, 0)), "1 staged hunk in 1 file");
    let draft = CommitDraft {
        changelist: Some("fixes".into()),
        payload: payload(1, 0),
        message: String::new(),
        body: String::new(),
        body_focus: false,
        no_verify: false,
        amend: false,
    };
    assert_eq!(draft.changelist_label(), "fixes");
}

#[test]
fn n_opens_an_input_whose_submission_creates_a_changelist() {
    let mut app = app();
    app.on_key(key(KeyCode::Char('n')));
    // Overlays swallow global keys: 'q' types, it doesn't quit.
    assert_eq!(app.on_key(key(KeyCode::Char('q'))), None);
    app.on_key(key(KeyCode::Backspace));
    for c in "docs".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    let action = app.on_key(key(KeyCode::Enter));
    assert_eq!(
        action,
        Some(Action::Op(Op::CreateChangelist {
            name: "docs".into()
        }))
    );
}

#[test]
fn submitting_an_empty_input_is_ignored() {
    let mut app = app();
    app.on_key(key(KeyCode::Char('n')));
    assert_eq!(app.on_key(key(KeyCode::Enter)), None);
    assert!(matches!(app.overlay, Some(Overlay::Input { .. })));
}

#[test]
fn r_prefills_a_rename_for_the_scoped_changelist() {
    let mut app = app();
    app.on_key(key(KeyCode::Char('j'))); // select 'fixes'
    app.on_key(key(KeyCode::Char('r')));
    let Some(Overlay::Input {
        kind: InputKind::Rename { ref from },
        ref value,
    }) = app.overlay
    else {
        panic!("expected a rename input, got {:?}", app.overlay);
    };
    assert_eq!(from, "fixes");
    assert_eq!(value, "fixes");
    app.on_key(key(KeyCode::Char('2')));
    let action = app.on_key(key(KeyCode::Enter));
    assert_eq!(
        action,
        Some(Action::Op(Op::RenameChangelist {
            from: "fixes".into(),
            to: "fixes2".into()
        }))
    );
}

#[test]
fn d_confirms_before_deleting_and_esc_cancels() {
    let mut app = app();
    app.on_key(key(KeyCode::Char('j')));
    app.on_key(key(KeyCode::Char('d')));
    app.on_key(key(KeyCode::Esc));
    assert!(app.overlay.is_none(), "esc cancels the delete");

    app.on_key(key(KeyCode::Char('d')));
    let action = app.on_key(key(KeyCode::Enter));
    assert_eq!(
        action,
        Some(Action::Op(Op::DeleteChangelist {
            name: "fixes".into()
        }))
    );
}

#[test]
fn s_switches_the_scoped_changelist_and_pseudo_rows_are_inert() {
    let mut app = app();
    app.on_key(key(KeyCode::Char('j')));
    app.on_key(key(KeyCode::Char('j'))); // 'chores'
    assert_eq!(
        app.on_key(key(KeyCode::Char('s'))),
        Some(Action::Op(Op::SetActive {
            name: "chores".into()
        }))
    );
    // `a` is the assign key now, and the Changelists panel has
    // nothing for it to assign.
    assert_eq!(app.on_key(key(KeyCode::Char('a'))), None);
    assert!(app.overlay.is_none());

    app.on_key(key(KeyCode::Esc)); // back to 'all'
    assert_eq!(app.on_key(key(KeyCode::Char('s'))), None);
    assert_eq!(app.on_key(key(KeyCode::Char('d'))), None);
    assert!(app.overlay.is_none(), "all/unassigned rows take no ops");
}

fn blob(size: u64) -> BlobInfo {
    BlobInfo {
        oid: format!("oid-{size}"),
        size,
    }
}

fn binary_file(
    kind: ChangeKind,
    head: Option<BlobInfo>,
    changed: Option<BlobInfo>,
    stage: HunkStage,
) -> ChangedFile {
    ChangedFile {
        path: "assets/logo.png".into(),
        kind,
        binary: true,
        binary_sides: Some(BinarySides {
            head: head.clone(),
            changed: changed.clone(),
        }),
        hunks: vec![Hunk {
            old_start: 0,
            old_lines: 0,
            new_start: 0,
            new_lines: 0,
            stage,
            index_only: false,
            identity: HunkIdentity::WholeFile {
                oids: OidAnchor {
                    head: head.map(|blob| blob.oid),
                    changed: changed.map(|blob| blob.oid),
                },
            },
            changelist: None,
        }],
    }
}

fn binary_app(file: ChangedFile) -> App {
    let mut snapshot = snapshot();
    snapshot.files = vec![file];
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot);
    app.on_key(key(KeyCode::Char('3'))); // focus Files, select the file
    app
}

#[test]
fn binary_diff_placeholder_shows_sizes_per_variant() {
    // ADR 0009: one-line sized placeholder, added/deleted variants
    // with a single size; the text is what says "binary".
    let modified = binary_app(binary_file(
        ChangeKind::Modified,
        Some(blob(12_698)),
        Some(blob(15_462)),
        HunkStage::Unstaged,
    ));
    assert!(modified.diff_lines().iter().any(|line| matches!(
        line,
        DiffLine::Placeholder(text) if text == "Binary file changed (12.4 KB \u{2192} 15.1 KB)"
    )));

    let added = binary_app(binary_file(
        ChangeKind::Untracked,
        None,
        Some(blob(512)),
        HunkStage::Unstaged,
    ));
    assert!(added.diff_lines().iter().any(|line| matches!(
        line,
        DiffLine::Placeholder(text) if text == "Binary file added (512 B)"
    )));

    let deleted = binary_app(binary_file(
        ChangeKind::Deleted,
        Some(blob(2 * 1024 * 1024)),
        None,
        HunkStage::Unstaged,
    ));
    assert!(deleted.diff_lines().iter().any(|line| matches!(
        line,
        DiffLine::Placeholder(text) if text == "Binary file deleted (2.0 MB)"
    )));
}

#[test]
fn enter_on_a_binary_is_a_polite_no_op() {
    // ADR 0009: hunk-mode entry on a binary selection does nothing —
    // no focus change, no selection, no log event.
    let mut app = binary_app(binary_file(
        ChangeKind::Modified,
        Some(blob(10)),
        Some(blob(20)),
        HunkStage::Unstaged,
    ));
    let logged = app.log.len();
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.focus, Panel::Files);
    assert!(app.hunk_sel.is_none());
    assert_eq!(app.log.len(), logged);
}

#[test]
fn space_on_a_stale_binary_restages_the_file() {
    // `space` reads the derived marker: a `\u{25d1}` binary file is `\u{25d0}` at
    // file level, so the toggle re-stages (index := worktree).
    let mut app = binary_app(binary_file(
        ChangeKind::Modified,
        Some(blob(10)),
        Some(blob(20)),
        HunkStage::StagedStale,
    ));
    assert_eq!(
        app.on_key(key(KeyCode::Char(' '))),
        Some(Action::Op(Op::StageFile {
            path: "assets/logo.png".into()
        }))
    );
}
