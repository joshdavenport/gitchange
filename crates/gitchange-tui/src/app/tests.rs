use std::time::Duration;

use gitchange_core::{
    BinarySides, BlobInfo, ChangedFile, Changelist, CommitInfo, CommitPayload, FileStage, Head,
    Hunk, HunkIdentity, HunkLine, OidAnchor, PayloadFile,
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

/// Each row of a split file reports its own group's progress, never the
/// whole file's (issue #97) — the glyph a row draws is exactly what its
/// `space` acts on.
#[test]
fn a_split_files_rows_each_count_only_their_own_groups_hunks() {
    let app = app();
    let counted: Vec<(String, String, FileStage, usize, usize)> = app
        .files_rows()
        .into_iter()
        .filter_map(|row| match row {
            FilesRow::File {
                entry,
                stage,
                staged,
                total,
                ..
            } => Some((
                entry.group.label().to_owned(),
                entry.path,
                stage,
                staged,
                total,
            )),
            FilesRow::Header { .. } => None,
        })
        .collect();

    // print.css holds ● at 14 and ○ at 41 for 'fixes', ○ at 63 for
    // 'chores' — 1/3 staged as a whole file, which no row reports.
    assert_eq!(
        counted,
        vec![
            (
                "fixes".into(),
                "src/print.css".into(),
                FileStage::PartiallyStaged,
                1,
                2
            ),
            (
                "chores".into(),
                "src/print.css".into(),
                FileStage::Unstaged,
                0,
                1
            ),
            (
                "chores".into(),
                "src/session.ts".into(),
                FileStage::Unstaged,
                0,
                1
            ),
            (
                "unassigned".into(),
                "src/nav.astro".into(),
                FileStage::Unstaged,
                0,
                1
            ),
        ]
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
    assert!(
        app.log
            .iter()
            .any(|entry| entry.severity == Severity::Info && entry.text == "watcher recovered"),
        "and the moment it ended is logged too, like its onset"
    );

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
        Some(Action::Op(Op::StageOwnedHunks { .. }))
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

// ── hunk mode (ticket #32) ──────────────────────────────────────

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

// ── assign flow (tickets #32/#41) ───────────────────────────────

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

/// Walk the open popup's cursor down onto "+ create new changelist…",
/// however many targets precede it.
fn select_create_new(app: &mut App) {
    for _ in 1..app.assign_rows().len() {
        app.on_key(key(KeyCode::Char('j')));
    }
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
fn assign_rows_list_changelists_then_unassigned_then_create_new() {
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
            AssignRow::Unassigned,
            AssignRow::CreateNew,
        ]
    );
}

/// Unassigned is a target like any other (CONTEXT.md): the popup
/// reaches it, releasing the payload with no target (ADR 0016).
#[test]
fn enter_on_the_unassigned_row_releases_the_hunks() {
    let mut app = app();
    app.on_key(key(KeyCode::Char('3'))); // print.css under 'fixes'
    app.on_key(key(KeyCode::Char('a')));
    app.on_key(key(KeyCode::Char('j')));
    app.on_key(key(KeyCode::Char('j'))); // 'unassigned'
    let action = app.on_key(key(KeyCode::Enter));
    assert!(
        matches!(
            action,
            Some(Action::Op(Op::Assign {
                ref path,
                ref hunks,
                target: AssignTarget::Unassigned,
            })) if path == "src/print.css" && hunks.len() == 2
        ),
        "expected a release op, got {action:?}"
    );
    assert!(app.overlay.is_none(), "confirming closes the popup");
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
    })) = action
    else {
        panic!("expected an assign op, got {action:?}");
    };
    assert_eq!(path, "src/print.css");
    assert_eq!(target, AssignTarget::Existing("chores".into()));
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
            if hunks.len() == 1 && *target == AssignTarget::Existing("fixes".into())
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
    select_create_new(&mut app);
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
        Some(Action::Op(Op::Assign { ref target, .. }))
            if *target == AssignTarget::New("docs".into())
    ));
}

#[test]
fn esc_from_the_escape_hatch_returns_to_the_assign_popup() {
    let mut app = hunk_mode_app();
    app.on_key(key(KeyCode::Char('a')));
    select_create_new(&mut app);
    app.on_key(key(KeyCode::Enter)); // into the input
    app.on_key(key(KeyCode::Esc));
    assert!(matches!(app.overlay, Some(Overlay::Assign { .. })));
    app.on_key(key(KeyCode::Esc));
    assert!(app.overlay.is_none());
}

// ── changelist ops (n/d/r/s) ────────────────────────────────────

// ── space staging (ticket #33) ──────────────────────────────────

#[test]
fn space_in_files_toggles_the_rows_owned_hunks_by_its_marker() {
    let mut app = app();
    // print.css under 'fixes': ● at 14, ○ at 41 — ◐ at row scope.
    app.on_key(key(KeyCode::Char('3')));
    assert_eq!(
        app.on_key(key(KeyCode::Char(' '))),
        Some(Action::Op(Op::StageOwnedHunks {
            path: "src/print.css".into(),
            changelist: Some("fixes".into()),
        })),
        "◐ stages the rest"
    );

    // A row whose owned hunks are all ● toggles the other way — even
    // though 'chores' still holds an ○ hunk in the same file.
    let mut staged = snapshot();
    for hunk in &mut staged.files[1].hunks {
        if hunk.changelist.as_deref() == Some("fixes") {
            hunk.stage = HunkStage::Staged;
        }
    }
    app.apply_snapshot(staged);
    assert_eq!(
        app.on_key(key(KeyCode::Char(' '))),
        Some(Action::Op(Op::UnstageOwnedHunks {
            path: "src/print.css".into(),
            changelist: Some("fixes".into()),
        })),
        "● unstages"
    );
}

/// The design's own claim, now true of the op too: the same path under
/// two changelists is two rows, two selections, and two distinct targets
/// (issue #97).
#[test]
fn the_same_path_under_two_changelists_gives_two_space_targets() {
    let mut app = app();
    app.on_key(key(KeyCode::Char('3'))); // print.css under 'fixes'
    let first = app.on_key(key(KeyCode::Char(' ')));
    app.on_key(key(KeyCode::Char('j'))); // print.css under 'chores'
    let second = app.on_key(key(KeyCode::Char(' ')));

    assert_eq!(
        first,
        Some(Action::Op(Op::StageOwnedHunks {
            path: "src/print.css".into(),
            changelist: Some("fixes".into()),
        }))
    );
    assert_eq!(
        second,
        Some(Action::Op(Op::StageOwnedHunks {
            path: "src/print.css".into(),
            changelist: Some("chores".into()),
        })),
        "same path, different row, different target"
    );
}

#[test]
fn space_on_an_unassigned_files_row_targets_the_unowned_hunks() {
    let mut app = app();
    app.on_key(key(KeyCode::Char('3')));
    // print.css/fixes, print.css/chores, session.ts/chores, then
    // nav.astro under the unassigned group.
    for _ in 0..3 {
        app.on_key(key(KeyCode::Char('j')));
    }
    assert_eq!(
        app.on_key(key(KeyCode::Char(' '))),
        Some(Action::Op(Op::StageOwnedHunks {
            path: "src/nav.astro".into(),
            changelist: None,
        })),
        "unassigned is core's None changelist"
    );
}

/// A drilled scope renders one row per file, and that row still carries
/// its group — the op must not fall back to the whole file.
#[test]
fn space_in_a_drilled_scope_stays_scoped_to_that_changelist() {
    let mut app = app();
    app.on_key(key(KeyCode::Char('2')));
    app.on_key(key(KeyCode::Char('j'))); // 'fixes'
    assert_eq!(app.scope(), Scope::Changelist("fixes".into()));
    app.on_key(key(KeyCode::Char('3')));

    assert_eq!(
        app.on_key(key(KeyCode::Char(' '))),
        Some(Action::Op(Op::StageOwnedHunks {
            path: "src/print.css".into(),
            changelist: Some("fixes".into()),
        }))
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
fn space_outside_files_hunk_mode_and_changelists_does_nothing() {
    let mut app = app();
    let logged = app.log.len();
    app.on_key(key(KeyCode::Char('0'))); // diff scroll mode
    assert_eq!(app.on_key(key(KeyCode::Char(' '))), None);
    app.on_key(key(KeyCode::Char('4'))); // commits
    assert_eq!(app.on_key(key(KeyCode::Char(' '))), None);
    assert_eq!(
        app.log.len(),
        logged,
        "a panel with nothing to stage hides the key silently"
    );
}

// ── space on a changelist (issue #90) ───────────────────────────

#[test]
fn space_on_a_changelist_stages_it_while_any_hunk_is_unstaged() {
    let mut app = app(); // Changelists focus
    app.on_key(key(KeyCode::Char('j'))); // 'fixes': ● at 14, ○ at 41
    assert_eq!(
        app.on_key(key(KeyCode::Char(' '))),
        Some(Action::Op(Op::StageChangelist {
            changelist: Some("fixes".into())
        })),
        "a mixed changelist stages"
    );

    // A ◑ alongside a ● takes the stage direction too: `space` aligns
    // the stale hunk rather than unstaging the pair.
    let mut stale = snapshot();
    stale.files[1].hunks[1].stage = HunkStage::StagedStale;
    app.apply_snapshot(stale);
    assert_eq!(
        app.on_key(key(KeyCode::Char(' '))),
        Some(Action::Op(Op::StageChangelist {
            changelist: Some("fixes".into())
        })),
        "◑ + ● stages"
    );
}

#[test]
fn space_on_a_fully_staged_changelist_unstages_it() {
    let mut app = app();
    let mut staged = snapshot();
    for file in &mut staged.files {
        for hunk in &mut file.hunks {
            if hunk.changelist.as_deref() == Some("fixes") {
                hunk.stage = HunkStage::Staged;
            }
        }
    }
    app.apply_snapshot(staged);
    app.on_key(key(KeyCode::Char('j'))); // 'fixes'
    assert_eq!(
        app.on_key(key(KeyCode::Char(' '))),
        Some(Action::Op(Op::UnstageChangelist {
            changelist: Some("fixes".into())
        }))
    );
}

#[test]
fn space_on_unassigned_scopes_the_op_to_the_unowned_hunks() {
    let mut app = app();
    app.on_key(key(KeyCode::Char('j')));
    app.on_key(key(KeyCode::Char('j')));
    app.on_key(key(KeyCode::Char('j'))); // 'unassigned', pinned last
    assert_eq!(app.scope(), Scope::Unassigned);
    assert_eq!(
        app.on_key(key(KeyCode::Char(' '))),
        Some(Action::Op(Op::StageChangelist { changelist: None })),
        "unassigned is core's None changelist"
    );
}

#[test]
fn space_on_an_empty_changelist_still_asks_for_the_echo() {
    // A row owning no hunks takes the stage direction, whose op answers
    // `nothing to stage — 'empty'` (ADR 0007) instead of going quiet.
    let mut app = app();
    let mut empty = snapshot();
    empty.changelists.push(Changelist {
        name: "empty".into(),
    });
    app.apply_snapshot(empty);
    for _ in 0..3 {
        app.on_key(key(KeyCode::Char('j'))); // 'empty', after the other two
    }
    assert_eq!(app.scope(), Scope::Changelist("empty".into()));
    assert_eq!(
        app.on_key(key(KeyCode::Char(' '))),
        Some(Action::Op(Op::StageChangelist {
            changelist: Some("empty".into())
        }))
    );
}

#[test]
fn space_on_the_all_row_logs_the_all_is_a_view_reason() {
    let mut app = app(); // Changelists focus, 'all' selected
    assert_eq!(app.on_key(key(KeyCode::Char(' '))), None);
    assert!(
        app.log
            .iter()
            .any(|entry| entry.text == "select a changelist to stage — all is a view"),
        "the disabled reason logs on the press: {:?}",
        app.log
    );
    assert!(
        !bar(&app).iter().any(|hint| hint.starts_with("space")),
        "and the bar doesn't advertise the key"
    );
}

// ── commit flow (ticket #33) ────────────────────────────────────

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
fn payload_counts_pluralizes_hunks_and_files() {
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
fn s_switches_the_scoped_changelist_and_all_is_inert() {
    let mut app = app();
    app.on_key(key(KeyCode::Char('j')));
    app.on_key(key(KeyCode::Char('j'))); // 'chores'
    assert_eq!(
        app.on_key(key(KeyCode::Char('s'))),
        Some(Action::Op(Op::SetActive {
            changelist: Some("chores".into())
        }))
    );
    // `a` is the assign key now, and the Changelists panel has
    // nothing for it to assign.
    assert_eq!(app.on_key(key(KeyCode::Char('a'))), None);
    assert!(app.overlay.is_none());

    app.on_key(key(KeyCode::Esc)); // back to 'all'
    assert_eq!(app.on_key(key(KeyCode::Char('s'))), None);
    assert_eq!(app.on_key(key(KeyCode::Char('d'))), None);
    assert!(app.overlay.is_none(), "the all row takes no ops");
}

#[test]
fn s_on_the_unassigned_row_switches_to_unassigned() {
    // Capture-off (ADR 0015): unassigned is a switchable target, so `s`
    // acts on its row while the other ops keys stay disabled there.
    let mut app = app();
    for _ in 0..3 {
        app.on_key(key(KeyCode::Char('j')));
    }
    assert_eq!(app.scope(), Scope::Unassigned, "precondition");
    assert_eq!(
        app.on_key(key(KeyCode::Char('s'))),
        Some(Action::Op(Op::SetActive { changelist: None }))
    );

    assert_eq!(app.on_key(key(KeyCode::Char('d'))), None);
    assert_eq!(app.on_key(key(KeyCode::Char('r'))), None);
    assert!(
        app.overlay.is_none(),
        "unassigned is built in: nothing to delete or rename"
    );
}

#[test]
fn the_unassigned_row_wears_the_active_marker_when_it_holds_it() {
    // ADR 0015: the `*` is capture-off's only indicator, so the
    // unassigned row renders it on the same terms as a changelist row.
    let mut app = app();
    let rows = app.changelist_rows();
    assert!(rows[1].active, "precondition: 'fixes' holds the marker");
    assert!(!rows[3].active);

    let mut snapshot = snapshot();
    snapshot.active = None;
    app.apply_snapshot(snapshot);

    let rows = app.changelist_rows();
    assert!(
        rows[3].active,
        "capture-off puts the `*` on the unassigned row"
    );
    assert!(
        !rows[1].active,
        "and takes it off the changelist that held it"
    );
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
    // row scope, so the toggle re-stages (index := worktree). A binary
    // is one whole-file hunk (ADR 0009), so the row op reduces to the
    // same whole-file index write core's hunk path already makes.
    let mut app = binary_app(binary_file(
        ChangeKind::Modified,
        Some(blob(10)),
        Some(blob(20)),
        HunkStage::StagedStale,
    ));
    assert_eq!(
        app.on_key(key(KeyCode::Char(' '))),
        Some(Action::Op(Op::StageOwnedHunks {
            path: "assets/logo.png".into(),
            changelist: None,
        }))
    );
}

// ── the binding core & its disabled reasons (ADR 0014, issue #65) ───

/// The bar as the user reads it: `key label` per hint.
fn bar(app: &App) -> Vec<String> {
    app.key_hints()
        .into_iter()
        .map(|(key, label)| format!("{key} {label}"))
        .collect()
}

#[test]
fn a_bar_spelling_names_the_primary_key_only() {
    // The movement pair is the only multi-spelling record, and since
    // issue #86 no arm mentions it — so no bar assertion covers the
    // primary-only rule any more. Assert it against `bar_spelling`
    // directly: the overlay shows `j/k, ↓/↑`, the bar shows `j/k`.
    use BindingId::*;
    assert_eq!(keymap::bar_spelling(&[MoveDown, MoveUp]), "j/k");
}

#[test]
fn the_changelists_bar_reflects_what_the_scoped_row_affords() {
    let mut app = app(); // Changelists focus, 'all' selected
    assert_eq!(
        bar(&app),
        [
            "enter files",
            "n new",
            "0-5 panels",
            "R refresh",
            "? keybindings",
            "q quit",
        ]
    );
    app.on_key(key(KeyCode::Char('j'))); // 'fixes'
    assert_eq!(
        bar(&app),
        [
            "enter files",
            "space toggle stage changelist",
            "n new",
            "d delete",
            "r rename",
            "s switch active",
            "c commit",
            "0-5 panels",
            "R refresh",
            "? keybindings",
            "q quit",
        ]
    );
}

#[test]
fn the_files_bar_reflects_what_the_scoped_row_affords() {
    let mut app = app();
    app.on_key(key(KeyCode::Char('3'))); // Files, 'all' scoped
    assert_eq!(
        bar(&app),
        [
            "space toggle stage row",
            "enter hunks",
            "a/A/ctrl+a assign group / unassigned / all",
            "n new",
            "esc back",
            "0-5 panels",
            "R refresh",
            "? keybindings",
            "q quit",
        ]
    );
    app.on_key(key(KeyCode::Esc)); // back out to Changelists
    app.on_key(key(KeyCode::Char('j'))); // 'fixes'
    app.on_key(key(KeyCode::Enter)); // drill to Files
    assert_eq!(
        bar(&app),
        [
            "space toggle stage row",
            "enter hunks",
            "a/A/ctrl+a assign group / unassigned / all",
            "c commit",
            "n new",
            "d delete",
            "r rename",
            "s switch active",
            "esc back",
            "0-5 panels",
            "R refresh",
            "? keybindings",
            "q quit",
        ]
    );
}

#[test]
fn the_hunk_mode_bar_scopes_commit_like_everywhere_else() {
    // Drilled into 'fixes': the full hunk-mode bar, commit included.
    let mut app = app();
    app.on_key(key(KeyCode::Char('j')));
    app.on_key(key(KeyCode::Enter)); // files
    app.on_key(key(KeyCode::Enter)); // hunk mode
    assert_eq!(
        bar(&app),
        [
            "space toggle stage hunk",
            "a/A/ctrl+a assign hunk / unassigned / all",
            "c commit changelist",
            "esc back to files",
            "? keybindings",
        ]
    );

    // Hunk mode under the All view: same bar without commit — the
    // scoped row doesn't afford it.
    let mut all_scoped = App::new("repo");
    all_scoped.apply_snapshot(snapshot());
    all_scoped.on_key(key(KeyCode::Char('3')));
    all_scoped.on_key(key(KeyCode::Enter));
    assert_eq!(
        bar(&all_scoped),
        [
            "space toggle stage hunk",
            "a/A/ctrl+a assign hunk / unassigned / all",
            "esc back to files",
            "? keybindings",
        ]
    );
}

#[test]
fn the_status_commits_and_log_bars_match_verbatim() {
    // With no blurred hunk to assign and the movement mention gone
    // (issue #86), these three arms contribute nothing of their own and
    // collapse onto the shared tail. That collapse is intended: the
    // border says which panel has focus, not the bar.
    const TAIL: [&str; 4] = ["0-5 panels", "R refresh", "? keybindings", "q quit"];
    let mut app = app();
    app.on_key(key(KeyCode::Char('1'))); // Status
    assert_eq!(bar(&app), TAIL);
    app.on_key(key(KeyCode::Char('4'))); // Commits
    assert_eq!(bar(&app), TAIL);
    app.on_key(key(KeyCode::Char('5'))); // Log
    assert_eq!(bar(&app), TAIL);
}

#[test]
fn the_scroll_mode_diff_bar_advertises_the_file_scoped_assigns() {
    let mut app = app();
    app.on_key(key(KeyCode::Char('0')));
    assert_eq!(
        bar(&app),
        [
            "A/ctrl+a assign unassigned / all",
            "esc back",
            "0-5 panels",
            "R refresh",
            "? keybindings",
            "q quit",
        ]
    );
}

#[test]
fn a_blurred_hunk_advertises_and_serves_assign_in_any_panel() {
    let mut app = app();
    app.on_key(key(KeyCode::Char('3')));
    app.on_key(key(KeyCode::Enter)); // hunk mode on print.css
    app.on_key(key(KeyCode::Char('4'))); // blur to Commits — selection survives
    assert_eq!(
        bar(&app),
        [
            "a/A/ctrl+a assign hunk / unassigned / all",
            "0-5 panels",
            "R refresh",
            "? keybindings",
            "q quit",
        ]
    );
    // The advertised key acts: `a` assigns the blurred hunk from here.
    assert_eq!(app.on_key(key(KeyCode::Char('a'))), None);
    assert!(matches!(
        &app.overlay,
        Some(Overlay::Assign {
            payload: AssignPayload::Hunk { .. },
            ..
        })
    ));
    app.on_key(key(KeyCode::Esc)); // close the popup

    // Ending the selection ends the advertisement.
    app.on_key(key(KeyCode::Char('0'))); // scroll-mode Diff resets it
    app.on_key(key(KeyCode::Char('4')));
    assert_eq!(
        bar(&app),
        ["0-5 panels", "R refresh", "? keybindings", "q quit"]
    );
}

#[test]
fn a_char_binding_never_fires_with_control_held() {
    // The keymap is explicit: `ctrl+<letter>` is its own binding space
    // (ADR 0013), so a plain-char binding is not an accidental alias
    // for its ctrl form. An alias, if ever wanted, is a second spelling
    // on the record — never a fallthrough.
    let mut app = app();
    app.on_key(key(KeyCode::Char('j'))); // 'fixes' — d/r/s/c all live
    let logged = app.log.len();
    // `ctrl+c` stays out: it is the protocol-level quit above the keymap.
    for c in ['q', 'd', 'r', 's', 'j'] {
        assert_eq!(
            app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)),
            None,
            "ctrl+{c} fired a plain-char binding"
        );
    }
    assert!(app.overlay.is_none());
    assert_eq!(app.changelist_row, 1, "ctrl+j did not move the selection");
    assert_eq!(app.log.len(), logged, "unbound ctrl chords stay silent");
}

#[test]
fn pressing_an_ops_key_on_a_pseudo_row_logs_why() {
    let mut app = app(); // 'all' selected
    assert_eq!(app.on_key(key(KeyCode::Char('d'))), None);
    assert!(app.overlay.is_none());
    assert!(app.log.iter().any(|entry| {
        entry.severity == Severity::Info && entry.text == "select a changelist — all is a view"
    }));

    app.on_key(key(KeyCode::Char('j')));
    app.on_key(key(KeyCode::Char('j')));
    app.on_key(key(KeyCode::Char('j'))); // 'unassigned'
    // `d` has nothing to delete there; `s` does act (ADR 0015), which is
    // the one op key that separates the two pseudo rows.
    assert_eq!(app.on_key(key(KeyCode::Char('d'))), None);
    assert!(app.log.iter().any(|entry| {
        entry.severity == Severity::Info
            && entry.text == "select a changelist — unassigned is built-in"
    }));
}

#[test]
fn ops_keys_outside_their_panels_stay_silent() {
    let mut app = app();
    app.on_key(key(KeyCode::Char('4'))); // Commits — no changelist scope
    let logged = app.log.len();
    assert_eq!(app.on_key(key(KeyCode::Char('d'))), None);
    assert_eq!(app.on_key(key(KeyCode::Char('r'))), None);
    assert_eq!(app.on_key(key(KeyCode::Char('s'))), None);
    assert!(app.overlay.is_none());
    assert_eq!(app.log.len(), logged, "an empty reason stays off the log");
}

#[test]
fn the_bar_hides_c_while_an_operation_is_in_progress() {
    let mut app = app();
    let mut busy = snapshot();
    busy.operation = Some(gitchange_core::GitOperation::Merge);
    app.apply_snapshot(busy);
    app.on_key(key(KeyCode::Char('j'))); // 'fixes' — otherwise committable
    // The bar and the operation pin tell one story: everything but
    // commit stays, verbatim.
    assert_eq!(
        bar(&app),
        [
            "enter files",
            "space toggle stage changelist",
            "n new",
            "d delete",
            "r rename",
            "s switch active",
            "0-5 panels",
            "R refresh",
            "? keybindings",
            "q quit",
        ]
    );
    app.on_key(key(KeyCode::Enter)); // files
    app.on_key(key(KeyCode::Enter)); // hunk mode
    assert_eq!(
        bar(&app),
        [
            "space toggle stage hunk",
            "a/A/ctrl+a assign hunk / unassigned / all",
            "esc back to files",
            "? keybindings",
        ],
        "the hunk-mode bar hides commit too"
    );
}

// ── viewport geometry (issue #85) ───────────────────────────────────

#[test]
fn a_never_rendered_app_pages_by_a_single_row() {
    let app = App::new("repo");
    for panel in Panel::ALL {
        assert_eq!(app.panel_height(panel), 0, "{panel:?}");
        // No Option at the call site, no panic, no zero-length step:
        // the great majority of these tests never draw a frame.
        assert_eq!(app.page(panel), 1, "{panel:?}");
    }
}

#[test]
fn a_recorded_height_pages_by_a_screen_less_one_row_of_overlap() {
    let mut app = App::new("repo");
    app.set_panel_heights(PanelHeights::from_fn(|panel| match panel {
        Panel::Files => 20,
        Panel::Log => 2,
        Panel::Diff => 1,
        _ => 0,
    }));
    assert_eq!(app.panel_height(Panel::Files), 20);
    assert_eq!(app.page(Panel::Files), 19);
    assert_eq!(app.page(Panel::Log), 1);
    // A one-row panel still moves a row: the overlap never eats the step.
    assert_eq!(app.page(Panel::Diff), 1);
    // A panel the frame never gave a height keeps the unrendered step.
    assert_eq!(app.page(Panel::Commits), 1);
}

#[test]
fn every_panel_keys_its_own_height() {
    // A distinct height per panel: reading any one back must give that
    // panel's own number, not a neighbour's slot.
    let heights = PanelHeights::from_fn(|panel| panel.slot() as u16 + 1);
    for (index, panel) in Panel::ALL.into_iter().enumerate() {
        assert_eq!(heights.get(panel), index as u16 + 1, "{panel:?}");
    }
}

// ── paging (issue #84) ──────────────────────────────────────────────

const PAGE_DOWN: KeyCode = KeyCode::Char('.');
const PAGE_UP: KeyCode = KeyCode::Char(',');

/// A snapshot of `count` single-hunk unassigned files and no changelists,
/// so a drill into unassigned renders them as one flat list — row index
/// and entry index are the same number there. Carries a dozen commits
/// too, so the Commits panel is never degenerate in a test that pages
/// through every panel.
fn many_files(count: usize) -> Snapshot {
    Snapshot {
        files: (0..count)
            .map(|index| {
                file(
                    &format!("src/file{index:03}.rs"),
                    vec![hunk(index as u32 + 1, None, HunkStage::Unstaged)],
                )
            })
            .collect(),
        changelists: Vec::new(),
        active: None,
        advisories: Vec::new(),
        head: Head::Branch {
            name: "main".into(),
        },
        recent_commits: (0..12)
            .map(|index| CommitInfo {
                short_id: format!("{index:07}"),
                author: "Josh Davenport-Smith".into(),
                summary: format!("commit {index}"),
            })
            .collect(),
        operation: None,
    }
}

/// Give every panel `height` content rows, as one drawn frame would.
fn viewport(app: &mut App, height: u16) {
    app.set_panel_heights(PanelHeights::from_fn(|_| height));
}

/// Drill from the Changelists panel into the unassigned scope, whose
/// Files rows are one flat list — so a row index and an entry index are
/// the same number there.
fn drill_into_unassigned(app: &mut App) {
    app.on_key(key(KeyCode::Down)); // all → unassigned
    app.on_key(key(KeyCode::Enter)); // → files
    assert_eq!(app.focus, Panel::Files);
}

/// An App over `snapshot`, drilled into unassigned with the Files panel
/// focused and every panel `height` rows tall.
fn drilled_app(snapshot: Snapshot, height: u16) -> App {
    let mut app = App::new("repo");
    app.apply_snapshot(snapshot);
    viewport(&mut app, height);
    drill_into_unassigned(&mut app);
    app
}

/// [`drilled_app`] over `count` single-hunk files.
fn files_app(count: usize, height: u16) -> App {
    drilled_app(many_files(count), height)
}

/// [`drilled_app`] over one file of `hunks` two-line hunks, whose headers
/// land on lines 2, 6, 10, … — four lines apart.
fn diff_app(hunks: usize, height: u16) -> App {
    let mut snapshot = many_files(1);
    snapshot.files[0].hunks = (0..hunks)
        .map(|index| hunk(index as u32 * 10 + 1, None, HunkStage::Unstaged))
        .collect();
    drilled_app(snapshot, height)
}

#[test]
fn paging_the_files_panel_steps_a_screen_less_a_row_of_overlap() {
    // Twenty-one content rows: from the first row the page lands on the
    // twenty-first — the row that was last visible — so the fold row is
    // shared between the two screens and nothing is stepped over.
    let mut app = files_app(60, 21);
    assert_eq!(app.files_count(), (1, 60));
    app.on_key(key(PAGE_DOWN));
    assert_eq!(app.files_count().0, 21, "the last visible row");
    app.on_key(key(PAGE_DOWN));
    assert_eq!(app.files_count().0, 41);
    app.on_key(key(PAGE_UP));
    assert_eq!(app.files_count().0, 21, "up reverses it exactly");
    app.on_key(key(PAGE_UP));
    assert_eq!(app.files_count().0, 1);
}

#[test]
fn paging_a_list_clamps_at_both_ends_and_repeats_harmlessly() {
    let mut app = files_app(30, 21);
    app.on_key(key(PAGE_DOWN));
    app.on_key(key(PAGE_DOWN));
    assert_eq!(
        app.files_count(),
        (30, 30),
        "the tail clamps to the last row"
    );
    app.on_key(key(PAGE_DOWN));
    assert_eq!(app.files_count(), (30, 30), "pressing again is idempotent");
    app.on_key(key(PAGE_UP));
    app.on_key(key(PAGE_UP));
    assert_eq!(app.files_count(), (1, 30));
    app.on_key(key(PAGE_UP));
    assert_eq!(app.files_count(), (1, 30));
}

#[test]
fn a_page_measures_the_panel_it_moves() {
    // A distinct height either side of the focused panel: paging Files
    // must take Files' own screen, not a neighbour's.
    let mut app = files_app(60, 21);
    app.set_panel_heights(PanelHeights::from_fn(|panel| match panel {
        Panel::Files => 11,
        _ => 31,
    }));
    app.on_key(key(PAGE_DOWN));
    assert_eq!(app.files_count().0, 11);
}

#[test]
fn a_never_rendered_app_pages_a_list_by_a_single_row() {
    // No frame has been drawn, so every panel's height is zero — the
    // press must still move, and by exactly one row.
    let mut app = App::new("repo");
    app.apply_snapshot(many_files(60));
    drill_into_unassigned(&mut app);
    app.on_key(key(PAGE_DOWN));
    assert_eq!(app.files_count().0, 2);
}

/// Leave a diff scrolled away from its top and a hunk selection blurred
/// out of the Diff panel — the two things moving the file selection has
/// to clear — driving it all through the keymap.
fn scrolled_with_a_blurred_hunk(app: &mut App) {
    app.on_key(key(KeyCode::Char('0'))); // diff, scroll mode
    for _ in 0..3 {
        app.on_key(key(KeyCode::Char('j')));
    }
    app.on_key(key(KeyCode::Char('3'))); // back to files
    app.on_key(key(KeyCode::Enter)); // hunk mode
    app.on_key(key(KeyCode::Char('3'))); // blur it, still selected
    assert_eq!(app.diff_scroll, 3);
    assert_eq!(app.hunk_sel, Some(0));
}

#[test]
fn a_page_leaves_the_state_an_equal_run_of_single_steps_leaves() {
    let mut paged = files_app(60, 21);
    let mut stepped = files_app(60, 21);
    for app in [&mut paged, &mut stepped] {
        scrolled_with_a_blurred_hunk(app);
    }
    paged.on_key(key(PAGE_DOWN));
    for _ in 0..20 {
        stepped.on_key(key(KeyCode::Char('j')));
    }
    assert_eq!(paged.files_count().0, 21);
    assert_eq!(paged.file_sel, stepped.file_sel);
    assert_eq!(
        paged.diff_scroll, 0,
        "the new file's diff starts at the top"
    );
    assert_eq!(
        paged.hunk_sel, None,
        "the blurred hunk belonged to the old file"
    );
    assert_eq!(paged.diff_scroll, stepped.diff_scroll);
    assert_eq!(paged.hunk_sel, stepped.hunk_sel);
}

#[test]
fn paging_the_changelists_panel_lands_on_the_last_row() {
    // The panel fits its four rows today, so the clamp is the whole
    // behaviour — and it stays correct once the panel can scroll (#87)
    // without a code path of its own.
    let mut app = app();
    viewport(&mut app, 4);
    assert_eq!(app.focus, Panel::Changelists);
    app.on_key(key(PAGE_DOWN));
    assert_eq!(app.scope(), Scope::Unassigned, "the last row");
    app.on_key(key(PAGE_UP));
    assert_eq!(app.scope(), Scope::All);
}

#[test]
fn paging_the_commits_panel_steps_and_clamps() {
    let mut app = App::new("repo");
    app.apply_snapshot(many_files(1)); // a dozen commits
    viewport(&mut app, 10);
    app.on_key(key(KeyCode::Char('4')));
    assert_eq!(app.focus, Panel::Commits);
    app.on_key(key(PAGE_DOWN));
    assert_eq!(app.commit_row, 9);
    app.on_key(key(PAGE_DOWN));
    assert_eq!(app.commit_row, 11, "clamped to the last commit");
    app.on_key(key(PAGE_UP));
    assert_eq!(app.commit_row, 2);
}

#[test]
fn paging_the_diff_in_scroll_mode_stops_at_the_content() {
    let mut app = diff_app(10, 10);
    app.on_key(key(KeyCode::Char('0')));
    assert_eq!(app.hunk_sel, None, "explicit Diff focus is scroll mode");
    app.on_key(key(PAGE_DOWN));
    assert_eq!(app.diff_scroll, 9);
    let last = app.diff_lines().len() as u16 - 1;
    for _ in 0..10 {
        app.on_key(key(PAGE_DOWN));
    }
    assert_eq!(app.diff_scroll, last, "no page reaches blank space");
    for _ in 0..10 {
        app.on_key(key(PAGE_UP));
    }
    assert_eq!(app.diff_scroll, 0);
}

#[test]
fn a_hunk_header_on_the_fold_row_is_taken() {
    // Headers four lines apart, a five-row panel: the fold — four lines
    // on from the current header — is exactly the next header, and the
    // shared fold row is what makes it the one taken.
    let mut app = diff_app(6, 5);
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.hunk_sel, Some(0), "enter selects the first hunk");
    assert_eq!(app.hunk_header_lines()[..2], [2, 6]);
    app.on_key(key(PAGE_DOWN));
    assert_eq!(app.hunk_sel, Some(1));
    app.on_key(key(PAGE_UP));
    assert_eq!(app.hunk_sel, Some(0));
}

#[test]
fn a_page_skips_the_short_hunks_before_the_fold() {
    // A nine-row panel's fold is eight lines on: two whole hunks. The
    // one in between is paged over, not walked — that is paging.
    let mut app = diff_app(6, 9);
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(PAGE_DOWN));
    assert_eq!(app.hunk_sel, Some(2));
}

#[test]
fn a_hunk_taller_than_the_panel_is_left_in_one_press() {
    let mut snapshot = many_files(1);
    let mut tall = hunk(20, None, HunkStage::Unstaged);
    tall.identity = HunkIdentity::Text {
        lines: (0..20)
            .map(|index| HunkLine {
                origin: '+',
                content: format!("line {index}\n"),
            })
            .collect(),
    };
    snapshot.files[0].hunks = vec![
        hunk(1, None, HunkStage::Unstaged),
        tall,
        hunk(60, None, HunkStage::Unstaged),
    ];
    let mut app = drilled_app(snapshot, 6);
    app.on_key(key(KeyCode::Enter)); // hunk mode
    app.on_key(key(KeyCode::Char('j'))); // onto the tall hunk
    assert_eq!(app.hunk_sel, Some(1));
    app.on_key(key(PAGE_DOWN));
    assert_eq!(
        app.hunk_sel,
        Some(2),
        "twenty unread lines go by in one press — reading them is j/k's job"
    );
}

#[test]
fn a_page_with_no_hunk_at_the_fold_clamps_to_the_end() {
    let mut app = diff_app(3, 40);
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(PAGE_DOWN));
    assert_eq!(app.hunk_sel, Some(2), "clamped to the last hunk");
    app.on_key(key(PAGE_DOWN));
    assert_eq!(app.hunk_sel, Some(2), "pressing again is idempotent");
    app.on_key(key(PAGE_UP));
    assert_eq!(app.hunk_sel, Some(0));
    app.on_key(key(PAGE_UP));
    assert_eq!(app.hunk_sel, Some(0));
}

#[test]
fn paging_the_log_moves_a_screen_of_the_stream() {
    let mut app = App::new("repo");
    for index in 0..60 {
        app.push_log(Severity::Info, format!("entry {index}"));
    }
    viewport(&mut app, 10);
    app.on_key(key(KeyCode::Char('5')));
    assert_eq!(app.focus, Panel::Log);
    // The offset is from the stream's bottom, so up pages into history
    // and down comes back toward the newest entry — the direction `k`
    // and `j` already take.
    app.on_key(key(PAGE_UP));
    assert_eq!(app.log_scroll, 9);
    for _ in 0..10 {
        app.on_key(key(PAGE_UP));
    }
    assert_eq!(app.log_scroll, 59, "stopped at the oldest retained entry");
    app.on_key(key(PAGE_DOWN));
    assert_eq!(app.log_scroll, 50);
    for _ in 0..10 {
        app.on_key(key(PAGE_DOWN));
    }
    assert_eq!(app.log_scroll, 0, "back to the newest");
}

#[test]
fn the_page_keys_do_nothing_on_the_status_panel() {
    let mut app = app();
    viewport(&mut app, 10);
    app.on_key(key(KeyCode::Char('1')));
    assert_eq!(app.focus, Panel::Status);
    let before = app.log.len();
    app.on_key(key(PAGE_DOWN));
    app.on_key(key(PAGE_UP));
    assert_eq!(app.focus, Panel::Status);
    assert_eq!(app.log.len(), before, "a no-op movement says nothing");
}

#[test]
fn the_keypad_page_keys_are_the_same_action() {
    // One script over every panel, run twice on the two spellings: the
    // state they leave must be indistinguishable.
    let run = |down: KeyCode, up: KeyCode| {
        let mut app = files_app(60, 11);
        for _ in 0..30 {
            app.push_log(Severity::Info, "entry");
        }
        app.on_key(key(down));
        app.on_key(key(down));
        app.on_key(key(up));
        app.on_key(key(KeyCode::Enter)); // hunk mode
        app.on_key(key(down));
        app.on_key(key(KeyCode::Char('0'))); // diff, scroll mode
        app.on_key(key(down));
        for panel in [
            Panel::Log,
            Panel::Commits,
            Panel::Changelists,
            Panel::Status,
        ] {
            app.on_key(key(KeyCode::Char(panel.number())));
            app.on_key(key(down));
            app.on_key(key(up));
            app.on_key(key(down));
        }
        (
            app.file_sel.clone(),
            app.hunk_sel,
            app.diff_scroll,
            app.log_scroll,
            app.changelist_row,
            app.commit_row,
            app.focus,
            app.log.len(),
        )
    };
    assert_eq!(
        run(PAGE_DOWN, PAGE_UP),
        run(KeyCode::PageDown, KeyCode::PageUp)
    );
}

#[test]
fn the_page_keys_are_one_help_row_and_no_keybar_arm() {
    let paging: Vec<(String, String)> = help_rows('→')
        .into_iter()
        .filter(|(_, label)| label.starts_with("page"))
        .collect();
    let [(keys, _)] = &paging[..] else {
        panic!("the two records merge into one help row, got {paging:?}");
    };
    for spelling in [".", ",", "PgDn", "PgUp"] {
        assert!(keys.contains(spelling), "{keys} names {spelling}");
    }
    // The bar advertises no movement key, and these are movement keys.
    let mut app = app();
    for panel in Panel::ALL {
        app.on_key(key(KeyCode::Char(panel.number())));
        for (keys, _) in app.key_hints() {
            assert!(
                !keys.contains('.') && !keys.contains(','),
                "{panel:?} bar names a page key: {keys}"
            );
        }
    }
}

// ── jumps (issue #88) ───────────────────────────────────────────────

const JUMP_BOTTOM: KeyCode = KeyCode::Char('>');
const JUMP_TOP: KeyCode = KeyCode::Char('<');

#[test]
fn jumping_the_files_panel_reaches_either_end_from_anywhere() {
    let mut app = files_app(60, 21);
    app.on_key(key(JUMP_BOTTOM));
    assert_eq!(app.files_count(), (60, 60), "the last row");
    app.on_key(key(JUMP_BOTTOM));
    assert_eq!(app.files_count(), (60, 60), "pressing again is idempotent");
    app.on_key(key(JUMP_TOP));
    assert_eq!(app.files_count(), (1, 60), "the first row");
    app.on_key(key(JUMP_TOP));
    assert_eq!(app.files_count(), (1, 60));
    // From a position that is neither end, both keys still reach one.
    app.on_key(key(PAGE_DOWN));
    assert_eq!(app.files_count().0, 21);
    app.on_key(key(JUMP_BOTTOM));
    assert_eq!(app.files_count().0, 60);
}

#[test]
fn a_jump_needs_no_viewport() {
    // No frame has been drawn, so every panel's height is zero. A page
    // would move a single row; a jump is absolute and must not care.
    let mut app = App::new("repo");
    app.apply_snapshot(many_files(60));
    drill_into_unassigned(&mut app);
    app.on_key(key(JUMP_BOTTOM));
    assert_eq!(app.files_count().0, 60);
}

#[test]
fn jumping_the_commits_panel_reaches_either_end() {
    let mut app = App::new("repo");
    app.apply_snapshot(many_files(1)); // a dozen commits
    viewport(&mut app, 10);
    app.on_key(key(KeyCode::Char('4')));
    assert_eq!(app.focus, Panel::Commits);
    let last = app.snapshot.as_ref().unwrap().recent_commits.len() - 1;
    app.on_key(key(JUMP_BOTTOM));
    assert_eq!(app.commit_row, last, "the oldest retained commit");
    app.on_key(key(JUMP_TOP));
    assert_eq!(app.commit_row, 0);
}

#[test]
fn jumping_the_changelists_panel_reaches_either_end() {
    let mut app = app();
    viewport(&mut app, 4);
    assert_eq!(app.focus, Panel::Changelists);
    app.on_key(key(JUMP_BOTTOM));
    assert_eq!(app.scope(), Scope::Unassigned, "the last row");
    app.on_key(key(JUMP_TOP));
    assert_eq!(app.scope(), Scope::All);
}

#[test]
fn jumping_the_diff_in_scroll_mode_stops_at_the_content() {
    let mut app = diff_app(10, 10);
    app.on_key(key(KeyCode::Char('0')));
    assert_eq!(app.hunk_sel, None, "explicit Diff focus is scroll mode");
    let last = app.diff_lines().len() as u16 - 1;
    app.on_key(key(JUMP_BOTTOM));
    assert_eq!(app.diff_scroll, last, "no jump reaches blank space");
    app.on_key(key(JUMP_BOTTOM));
    assert_eq!(app.diff_scroll, last, "pressing again is idempotent");
    app.on_key(key(JUMP_TOP));
    assert_eq!(app.diff_scroll, 0);
    app.on_key(key(JUMP_TOP));
    assert_eq!(app.diff_scroll, 0);
}

#[test]
fn jumping_in_hunk_mode_takes_the_last_and_first_hunk() {
    let mut app = diff_app(6, 5);
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.hunk_sel, Some(0), "enter selects the first hunk");
    app.on_key(key(JUMP_BOTTOM));
    assert_eq!(app.hunk_sel, Some(5), "the file's last hunk");
    app.on_key(key(JUMP_BOTTOM));
    assert_eq!(app.hunk_sel, Some(5), "pressing again is idempotent");
    app.on_key(key(JUMP_TOP));
    assert_eq!(app.hunk_sel, Some(0));
    app.on_key(key(JUMP_TOP));
    assert_eq!(app.hunk_sel, Some(0));
}

#[test]
fn jumping_the_log_reaches_the_oldest_and_the_newest() {
    let mut app = App::new("repo");
    for index in 0..60 {
        app.push_log(Severity::Info, format!("entry {index}"));
    }
    viewport(&mut app, 10);
    app.on_key(key(KeyCode::Char('5')));
    assert_eq!(app.focus, Panel::Log);
    // The offset is from the stream's bottom, so `<` reaches history's
    // far end and `>` comes back to the newest entry — the direction
    // `k`/`,` and `j`/`.` already take in this panel. A jump is the
    // limit of a long run of the movement key, not the maximum of the
    // offset behind it.
    app.on_key(key(JUMP_TOP));
    assert_eq!(
        app.log_scroll,
        app.log.len() - 1,
        "the oldest retained entry"
    );
    app.on_key(key(JUMP_TOP));
    assert_eq!(app.log_scroll, app.log.len() - 1);
    app.on_key(key(JUMP_BOTTOM));
    assert_eq!(app.log_scroll, 0, "back to the newest");
    app.on_key(key(JUMP_BOTTOM));
    assert_eq!(app.log_scroll, 0);
}

#[test]
fn a_jump_leaves_the_state_the_equivalent_run_of_single_steps_leaves() {
    let mut jumped = files_app(60, 21);
    let mut stepped = files_app(60, 21);
    for app in [&mut jumped, &mut stepped] {
        scrolled_with_a_blurred_hunk(app);
    }
    jumped.on_key(key(JUMP_BOTTOM));
    for _ in 0..59 {
        stepped.on_key(key(KeyCode::Char('j')));
    }
    assert_eq!(jumped.files_count().0, 60);
    assert_eq!(jumped.file_sel, stepped.file_sel);
    assert_eq!(
        jumped.diff_scroll, 0,
        "the new file's diff starts at the top"
    );
    assert_eq!(
        jumped.hunk_sel, None,
        "the blurred hunk belonged to the old file"
    );
    assert_eq!(jumped.diff_scroll, stepped.diff_scroll);
    assert_eq!(jumped.hunk_sel, stepped.hunk_sel);
}

#[test]
fn the_jump_keys_act_with_the_shift_that_typed_them() {
    // A terminal reports `>` as its own character byte, with or without
    // the SHIFT that produced it (ADR 0013). Both reports must act.
    let mut app = files_app(60, 21);
    app.on_key(KeyEvent::new(JUMP_BOTTOM, KeyModifiers::SHIFT));
    assert_eq!(app.files_count().0, 60);
    app.on_key(KeyEvent::new(JUMP_TOP, KeyModifiers::SHIFT));
    assert_eq!(app.files_count().0, 1);
}

#[test]
fn the_jump_keys_do_nothing_on_the_status_panel() {
    let mut app = app();
    viewport(&mut app, 10);
    app.on_key(key(KeyCode::Char('1')));
    assert_eq!(app.focus, Panel::Status);
    let before = app.log.len();
    app.on_key(key(JUMP_BOTTOM));
    app.on_key(key(JUMP_TOP));
    assert_eq!(app.focus, Panel::Status);
    assert_eq!(app.log.len(), before, "a no-op movement says nothing");
}

#[test]
fn the_jump_keys_are_one_help_row_and_no_keybar_arm() {
    let jumps: Vec<(String, String)> = help_rows('→')
        .into_iter()
        .filter(|(_, label)| label.starts_with("jump"))
        .collect();
    let [(keys, _)] = &jumps[..] else {
        panic!("the two records merge into one help row, got {jumps:?}");
    };
    for spelling in [">", "<"] {
        assert!(keys.contains(spelling), "{keys} names {spelling}");
    }
    // The bar advertises no movement key, and these are movement keys.
    let mut app = app();
    for panel in Panel::ALL {
        app.on_key(key(KeyCode::Char(panel.number())));
        for (keys, _) in app.key_hints() {
            assert!(
                !keys.contains('>') && !keys.contains('<'),
                "{panel:?} bar names a jump key: {keys}"
            );
        }
    }
}
