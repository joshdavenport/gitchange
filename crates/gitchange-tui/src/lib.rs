//! The ratatui frontend (ADR 0006): a thin consumer of core's Engine.
//! Panels render the last snapshot whole (ADR 0005); a sync crossbeam
//! `Select` main loop multiplexes engine events, terminal input, and the
//! deferred-indicator timer — no async runtime.

pub mod app;
pub mod theme;
pub mod ui;

use std::time::Instant;

use crossbeam_channel::{at, never, select, unbounded};
use gitchange_core::{CommitOptions, CommitOutcome, Engine, EngineEvent, HunkStage, Notice, Repo};
use ratatui::crossterm::event::{
    DisableFocusChange, EnableFocusChange, Event, KeyEventKind, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;

use app::{Action, App, CommitDraft, CommitStep, Op};
use theme::Theme;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Core(#[from] gitchange_core::Error),
    #[error("terminal error: {0}")]
    Terminal(#[from] std::io::Error),
    /// The Engine's event channel disconnected: its threads died.
    #[error("the refresh engine terminated unexpectedly")]
    EngineDied,
}

/// Run the TUI until quit. The Engine's initial refresh arrives unasked,
/// so the first snapshot needs no request.
pub fn run() -> Result<(), Error> {
    let cwd = std::env::current_dir()?;
    let engine = Engine::spawn(&cwd)?;
    // Sync mutations run on this handle, never the Engine's (its Repo
    // belongs to the refresh thread).
    let repo = Repo::discover(&cwd)?;
    let repo_name = engine
        .workdir()
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repository".to_owned());

    let mut terminal = ratatui::init();
    let _ = execute!(std::io::stdout(), EnableFocusChange);
    // Kitty-protocol terminals report `shift+enter` (hunk mode's
    // add-selected-hunk) only with this flag; elsewhere the push is a
    // harmless no-op and shift+enter reads as plain enter.
    let _ = execute!(
        std::io::stdout(),
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    );
    let result = event_loop(&mut terminal, &engine, &repo, App::new(repo_name));
    let _ = execute!(std::io::stdout(), PopKeyboardEnhancementFlags);
    let _ = execute!(std::io::stdout(), DisableFocusChange);
    ratatui::restore();
    result
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    engine: &Engine,
    repo: &Repo,
    mut app: App,
) -> Result<(), Error> {
    // Terminal input on its own thread, bridged into the Select loop.
    // It parks in a blocking read; process exit reaps it.
    let (input_tx, input_rx) = unbounded();
    std::thread::spawn(move || {
        while let Ok(event) = ratatui::crossterm::event::read() {
            if input_tx.send(event).is_err() {
                return;
            }
        }
    });

    let theme = Theme::default();
    loop {
        let now = Instant::now();
        terminal.draw(|frame| ui::draw(frame, &app, &theme, now))?;

        // Wake exactly when the deferred indicator becomes due.
        let timer = match app.indicator_deadline() {
            Some(deadline) if deadline > now => at(deadline),
            _ => never(),
        };

        select! {
            recv(engine.events()) -> event => match event {
                Ok(EngineEvent::RefreshStarted) => app.on_refresh_started(Instant::now()),
                Ok(EngineEvent::RefreshComplete(snapshot)) => app.apply_snapshot(snapshot),
                Ok(EngineEvent::RefreshFailed(error)) => app.on_refresh_failed(error.to_string()),
                // Conditions get their pinned rendering with ticket #34.
                Ok(_) => {}
                Err(_) => return Err(Error::EngineDied),
            },
            recv(input_rx) -> event => match event {
                Ok(Event::Key(key)) if key.kind != KeyEventKind::Release => {
                    match app.on_key(key) {
                        Some(Action::Quit) => return Ok(()),
                        Some(Action::Refresh) => engine.request_refresh(),
                        Some(Action::Op(op)) => {
                            app.push_feedback(run_op(repo, op));
                            // Mutation-triggered refresh: bypasses the
                            // debounce (ADR 0005).
                            engine.request_refresh();
                        }
                        Some(Action::Commit(step)) => {
                            run_commit_step(repo, &mut app, step);
                            // Every step mutates or synchronously
                            // refreshed the state file; the immediate
                            // request keeps the snapshot in step
                            // (ADR 0005).
                            engine.request_refresh();
                        }
                        None => {}
                    }
                }
                // Catch up on whatever happened while we were unfocused
                // (ADR 0005).
                Ok(Event::FocusGained) => engine.request_refresh(),
                Ok(_) => {}
                Err(_) => return Ok(()),
            },
            recv(timer) -> _ => {}
        }
    }
}

/// Execute one sync op, returning the lines the Log placeholder shows —
/// errors and fail-soft notices; success is silent (the refresh that
/// follows is the feedback).
fn run_op(repo: &Repo, op: Op) -> Vec<String> {
    let done = |result: Result<(), gitchange_core::Error>| match result {
        Ok(()) => Vec::new(),
        Err(error) => vec![error.to_string()],
    };
    match op {
        Op::CreateChangelist { name } => done(repo.create_changelist(&name)),
        Op::RenameChangelist { from, to } => done(repo.rename_changelist(&from, &to)),
        Op::DeleteChangelist { name } => done(repo.delete_changelist(&name)),
        Op::SetActive { name } => done(repo.switch(&name)),
        Op::StageFile { path } => done(repo.stage_file(&path)),
        Op::UnstageFile { path } => done(repo.unstage_file(&path)),
        Op::StageHunk { path, hunk } => hunk_op(repo.stage_hunk(&path, &hunk)),
        Op::UnstageHunk { path, hunk } => hunk_op(repo.unstage_hunk(&path, &hunk)),
        Op::Move {
            path,
            hunks,
            target,
            create,
        } => {
            // A name that already exists is a valid target: fall through
            // to the move rather than stranding it behind the create.
            if create
                && let Err(error) = repo.create_changelist(&target)
                && !matches!(error, gitchange_core::Error::ChangelistExists { .. })
            {
                return vec![error.to_string()];
            }
            match repo.move_hunks(&path, &hunks, Some(&target)) {
                Ok(notices) => notices.iter().map(notice_line).collect(),
                Err(error) => vec![error.to_string()],
            }
        }
    }
}

/// A fail-soft hunk op's outcome as Log placeholder lines: notices when
/// it applied partially or not at all, the error otherwise.
fn hunk_op(result: Result<Vec<Notice>, gitchange_core::Error>) -> Vec<String> {
    match result {
        Ok(notices) => notices.iter().map(notice_line).collect(),
        Err(error) => vec![error.to_string()],
    }
}

/// Execute one commit-flow IO step (ticket #33, ADR 0004); outcomes go
/// back into the App — dialog opened, drift re-confirm, or the dialog
/// restored with a feedback line on failure.
fn run_commit_step(repo: &Repo, app: &mut App, step: CommitStep) {
    match step {
        CommitStep::Open { changelist } => open_commit_dialog(repo, app, changelist),
        CommitStep::StageAllAndOpen { changelist } => {
            // Stage the changelist's unstaged hunks from a fresh
            // snapshot, fail-soft per hunk, then fall into the dialog.
            match repo.refresh() {
                Ok(snapshot) => {
                    let owner = changelist.as_deref();
                    for file in &snapshot.files {
                        for hunk in &file.hunks {
                            if hunk.changelist.as_deref() == owner
                                && hunk.stage == HunkStage::Unstaged
                            {
                                app.push_feedback(hunk_op(repo.stage_hunk(&file.path, hunk)));
                            }
                        }
                    }
                    open_commit_dialog(repo, app, changelist);
                }
                Err(error) => app.push_feedback([error.to_string()]),
            }
        }
        CommitStep::Commit(draft) => run_commit(repo, app, draft),
        CommitStep::AlignAndCommit(mut draft) => {
            // The ◑ warn's align option: index := worktree over the
            // changelist's stale hunks, then commit what that produced —
            // the payload is re-derived so the drift guard compares the
            // aligned content, not the stale confirmation.
            match repo.align(draft.changelist.as_deref()) {
                Ok(notices) => app.push_feedback(notices.iter().map(notice_line)),
                Err(error) => {
                    app.push_feedback([error.to_string()]);
                    app.restore_commit_dialog(draft);
                    return;
                }
            }
            match repo.commit_payload(draft.changelist.as_deref()) {
                Ok(payload) => {
                    draft.payload = payload;
                    // Align is fail-soft and edits can land mid-flow, so
                    // ◑ can survive it: re-warn instead of committing
                    // content whose flag the user never saw (ADR 0004 —
                    // never silent).
                    if draft.payload.stale_hunks() > 0 {
                        app.reconfirm_stale(draft);
                    } else {
                        run_commit(repo, app, draft);
                    }
                }
                Err(error) => {
                    app.push_feedback([error.to_string()]);
                    app.restore_commit_dialog(draft);
                }
            }
        }
    }
}

/// Derive the payload behind a sync refresh and open the dialog — or the
/// stage-all offer when nothing is staged (ADR 0004: core never
/// auto-stages; `commit_payload` reports empty rather than erroring).
fn open_commit_dialog(repo: &Repo, app: &mut App, changelist: Option<String>) {
    match repo.commit_payload(changelist.as_deref()) {
        Ok(payload) if payload.is_empty() => app.offer_stage_all(changelist),
        Ok(payload) => app.open_commit_dialog(changelist, payload),
        Err(error) => app.push_feedback([error.to_string()]),
    }
}

/// Run the confirmed commit. Failure restores the dialog exactly as
/// confirmed (the issue's dialog-restore path); drift loops back to the
/// re-confirm overlay with the fresh payload.
fn run_commit(repo: &Repo, app: &mut App, draft: CommitDraft) {
    let message = if draft.body.trim().is_empty() {
        draft.message.clone()
    } else {
        format!("{}\n\n{}", draft.message, draft.body.trim_end())
    };
    let options = CommitOptions {
        no_verify: draft.no_verify,
        amend: draft.amend,
    };
    match repo.commit(
        draft.changelist.as_deref(),
        &message,
        &options,
        Some(&draft.payload),
    ) {
        Ok(CommitOutcome::Committed { .. }) => app.commit_succeeded(),
        Ok(CommitOutcome::Drifted { payload }) => app.commit_drifted(draft, payload),
        Err(error) => {
            app.push_feedback([error.to_string()]);
            app.restore_commit_dialog(draft);
        }
    }
}

/// Minimal notice rendering until ticket #34's Log vocabulary; moves
/// only ever raise `StaleHunk`.
fn notice_line(notice: &Notice) -> String {
    match notice {
        Notice::StaleHunk { path, new_start } => {
            format!("hunk at {path}:{new_start} changed since the last refresh; nothing applied")
        }
        other => format!("{other:?}"),
    }
}
