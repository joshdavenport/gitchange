//! The ratatui frontend (ADR 0006): a thin consumer of core's Engine.
//! Panels render the last snapshot whole (ADR 0005); a sync crossbeam
//! `Select` main loop multiplexes engine events, terminal input, and the
//! deferred-indicator timer — no async runtime.

pub mod app;
pub mod theme;
pub mod ui;

use std::time::Instant;

use crossbeam_channel::{at, never, select, unbounded};
use gitchange_core::{
    CommitOptions, CommitOutcome, Condition, Engine, EngineEvent, HunkStage, Repo,
};
use ratatui::crossterm::event::{DisableFocusChange, EnableFocusChange, Event, KeyEventKind};
use ratatui::crossterm::execute;

use app::{Action, App, CommitDraft, CommitStep, Op, Severity, count_noun};
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
    // No kitty keyboard-protocol push: nothing in the keymap depends on a
    // modifier only that protocol reports (ADR 0013), so the flags would
    // be dead weight — and a standing invitation to bind against them.
    // Focus events are unaffected; they are not part of that protocol.
    let result = event_loop(&mut terminal, &engine, &repo, App::new(repo_name));
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
                // Conditions become pins (ADR 0007): started sets, ended
                // self-clears — never manually dismissable.
                Ok(EngineEvent::ConditionStarted(Condition::WatcherDegraded)) => {
                    app.on_watcher_degraded();
                }
                Ok(EngineEvent::ConditionEnded(Condition::WatcherDegraded)) => {
                    app.on_watcher_recovered();
                }
                Ok(_) => {}
                Err(_) => return Err(Error::EngineDied),
            },
            recv(input_rx) -> event => match event {
                Ok(Event::Key(key)) if key.kind != KeyEventKind::Release => {
                    match app.on_key(key) {
                        Some(Action::Quit) => return Ok(()),
                        Some(Action::Refresh) => engine.request_refresh(),
                        Some(Action::Op(op)) => {
                            run_op(repo, &mut app, op);
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

/// Execute one sync op. Successful index surgery echoes at `·` — the
/// lazygit-style transparency that earns trust (ADR 0007) — fail-soft
/// notices land at `!`, and hard failures take the error-modal contract.
fn run_op(repo: &Repo, app: &mut App, op: Op) {
    let done = |app: &mut App,
                title: &str,
                echo: Option<String>,
                result: Result<(), gitchange_core::Error>| {
        match result {
            Ok(()) => {
                if let Some(echo) = echo {
                    app.push_log(Severity::Info, echo);
                }
            }
            Err(error) => app.show_error(title, error.to_string()),
        }
    };
    match op {
        Op::CreateChangelist { name } => done(
            app,
            "Create changelist failed",
            None,
            repo.create_changelist(&name),
        ),
        Op::RenameChangelist { from, to } => done(
            app,
            "Rename changelist failed",
            None,
            repo.rename_changelist(&from, &to),
        ),
        Op::DeleteChangelist { name } => done(
            app,
            "Delete changelist failed",
            None,
            repo.delete_changelist(&name),
        ),
        Op::SetActive { name } => done(app, "Switch changelist failed", None, repo.switch(&name)),
        Op::StageFile { path } => {
            let echo = format!("staged file — {path}");
            done(app, "Stage failed", Some(echo), repo.stage_file(&path));
        }
        Op::UnstageFile { path } => {
            let echo = format!("unstaged file — {path}");
            done(app, "Unstage failed", Some(echo), repo.unstage_file(&path));
        }
        Op::StageHunk { path, hunk } => {
            let echo = format!(
                "staged hunk — {path} @@ -{},{}",
                hunk.old_start, hunk.old_lines
            );
            hunk_op(app, "Stage failed", echo, repo.stage_hunk(&path, &hunk));
        }
        Op::UnstageHunk { path, hunk } => {
            let echo = format!(
                "unstaged hunk — {path} @@ -{},{}",
                hunk.old_start, hunk.old_lines
            );
            hunk_op(app, "Unstage failed", echo, repo.unstage_hunk(&path, &hunk));
        }
        Op::Assign {
            path,
            hunks,
            target,
            create,
        } => {
            // A name that already exists is a valid target: fall through
            // to the assign rather than stranding it behind the create.
            if create
                && let Err(error) = repo.create_changelist(&target)
                && !matches!(error, gitchange_core::Error::ChangelistExists { .. })
            {
                app.show_error("Create changelist failed", error.to_string());
                return;
            }
            match repo.assign_hunks(&path, &hunks, Some(&target)) {
                Ok(notices) => {
                    // Stale hunks failed soft; the rest were assigned.
                    let assigned = hunks.len().saturating_sub(notices.len());
                    if assigned > 0 {
                        app.push_log(
                            Severity::Info,
                            format!(
                                "assigned {} — {path} → '{target}'",
                                count_noun(assigned, "hunk")
                            ),
                        );
                    }
                    app.push_notices(&notices);
                }
                Err(error) => app.show_error("Assign failed", error.to_string()),
            }
        }
    }
}

/// A fail-soft hunk op's outcome: the echo when it fully applied, `!`
/// notices when it applied partially or not at all, the modal otherwise.
fn hunk_op(
    app: &mut App,
    title: &str,
    echo: String,
    result: Result<Vec<gitchange_core::Notice>, gitchange_core::Error>,
) {
    match result {
        Ok(notices) if notices.is_empty() => app.push_log(Severity::Info, echo),
        Ok(notices) => app.push_notices(&notices),
        Err(error) => app.show_error(title, error.to_string()),
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
                    let mut staged = 0;
                    for file in &snapshot.files {
                        for hunk in &file.hunks {
                            if hunk.changelist.as_deref() == owner
                                && hunk.stage == HunkStage::Unstaged
                            {
                                match repo.stage_hunk(&file.path, hunk) {
                                    Ok(notices) if notices.is_empty() => staged += 1,
                                    Ok(notices) => app.push_notices(&notices),
                                    Err(error) => {
                                        app.show_error("Stage failed", error.to_string());
                                    }
                                }
                            }
                        }
                    }
                    if staged > 0 {
                        let label = owner.unwrap_or("unassigned");
                        app.push_log(
                            Severity::Info,
                            format!("staged {} — '{label}'", count_noun(staged, "hunk")),
                        );
                    }
                    open_commit_dialog(repo, app, changelist);
                }
                Err(error) => app.show_error("Commit failed", error.to_string()),
            }
        }
        CommitStep::Commit(draft) => run_commit(repo, app, draft),
        CommitStep::AlignAndCommit(mut draft) => {
            // The ◑ warn's align option: index := worktree over the
            // changelist's stale hunks, then commit what that produced —
            // the payload is re-derived so the drift guard compares the
            // aligned content, not the stale confirmation.
            match repo.align(draft.changelist.as_deref()) {
                Ok(notices) => {
                    app.push_log(
                        Severity::Info,
                        format!("aligned index to worktree — '{}'", draft.changelist_label()),
                    );
                    app.push_notices(&notices);
                }
                Err(error) => {
                    app.show_error("Commit failed", error.to_string());
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
                    app.show_error("Commit failed", error.to_string());
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
        Err(error) => app.show_error("Commit failed", error.to_string()),
    }
}

/// Run the confirmed commit, echoing the shelled-out command (ADR 0007's
/// transparency channel). Failure restores the dialog exactly as
/// confirmed with the rejection modal on top — hook stderr verbatim;
/// drift loops back to the re-confirm overlay with the fresh payload.
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
    let mut echo = String::from("git commit");
    if options.no_verify {
        echo.push_str(" --no-verify");
    }
    if options.amend {
        echo.push_str(" --amend");
    }
    let hunks = draft.payload.staged_hunks() + draft.payload.stale_hunks();
    echo.push_str(&format!(
        " (temp index — '{}', {})",
        draft.changelist_label(),
        count_noun(hunks, "hunk"),
    ));
    match repo.commit(
        draft.changelist.as_deref(),
        &message,
        &options,
        Some(&draft.payload),
    ) {
        Ok(CommitOutcome::Committed { oid }) => {
            app.push_log(Severity::Info, echo);
            let short = oid.get(..7).unwrap_or(&oid);
            app.push_log(
                Severity::Info,
                format!("committed {short} \"{}\"", draft.message),
            );
            app.commit_succeeded();
        }
        Ok(CommitOutcome::Drifted { payload }) => {
            app.push_log(
                Severity::Info,
                "nothing committed — payload changed since confirm",
            );
            app.commit_drifted(draft, payload);
        }
        Err(error) => {
            // The transparency echo records only commands git actually
            // ran: a hook rejection means `git commit` executed and
            // refused; guard/drift/payload failures mean it never did.
            if matches!(error, gitchange_core::Error::HookRejected { .. }) {
                app.push_log(Severity::Info, echo);
            }
            // Hook stderr is the user's own tooling talking to them
            // (ADR 0007): the modal carries it verbatim, scrollable.
            let detail = match &error {
                gitchange_core::Error::HookRejected { stderr } => stderr.clone(),
                other => other.to_string(),
            };
            app.show_error("Commit failed", detail);
            app.restore_commit_dialog(draft);
        }
    }
}
