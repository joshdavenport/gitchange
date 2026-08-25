//! The ratatui frontend (ADR 0006): a thin consumer of core's Engine.
//! Panels render the last snapshot whole (ADR 0005); a sync crossbeam
//! `Select` main loop multiplexes engine events, terminal input, and the
//! deferred-indicator timer — no async runtime.

// ADR 0006's contract for this crate is "essentially `run()`" — the bin
// consumes nothing else. These modules are `pub` only because the render
// tests (tests/render/) live outside the crate and need to drive App
// and ui::draw directly; `#[doc(hidden)]` keeps them from reading as API
// surface. Don't import them from another crate.
#[doc(hidden)]
pub mod app;
#[doc(hidden)]
pub mod theme;
#[doc(hidden)]
pub mod ui;

#[cfg(test)]
mod tests;

use std::io::IsTerminal;
use std::time::Instant;

use crossbeam_channel::{Receiver, at, never, select, unbounded};
use gitchange_core::{
    CommitMessage, CommitOptions, CommitOutcome, Condition, Deletion, Engine, EngineEvent,
    OpOutcome, Release, Repo, Undeletable, commit_echo,
};
use ratatui::Terminal;
use ratatui::backend::Backend;
use ratatui::crossterm::event::{DisableFocusChange, EnableFocusChange, Event, KeyEventKind};
use ratatui::crossterm::execute;

use app::{Action, App, AssignTarget, CommitDraft, CommitStep, Op, PanelHeights, Severity};
use theme::Theme;

/// Error-modal titles reused across several ops' failure paths, so each
/// wording is spelled once (titles that occur once stay inline — a const
/// used once is noise).
const COMMIT_FAILED: &str = "Commit failed";
const STAGE_FAILED: &str = "Stage failed";
const UNSTAGE_FAILED: &str = "Unstage failed";
const CREATE_CHANGELIST_FAILED: &str = "Create changelist failed";
const SWITCH_CHANGELIST_FAILED: &str = "Switch changelist failed";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Core(#[from] gitchange_core::Error),
    #[error("terminal error: {0}")]
    Terminal(#[from] std::io::Error),
    /// The pre-flight guard's refusal: bare `gitchange` was run with a
    /// pipe or file on stdin or stdout. The message is a decided string,
    /// so it lives here rather than being assembled at the call site.
    #[error(
        "the TUI needs a terminal on stdin and stdout; \
         run 'gitchange --help' for the command-line surface"
    )]
    NotATerminal,
    /// The Engine's event channel disconnected: its threads died.
    #[error("the refresh engine terminated unexpectedly")]
    EngineDied,
}

/// Run the TUI until quit. The Engine's initial refresh arrives unasked,
/// so the first snapshot needs no request.
///
/// Refuses with [`Error::NotATerminal`] when stdin or stdout is not a
/// terminal — before the Engine is spawned, so nothing is decided and
/// nothing is drawn.
pub fn run() -> Result<(), Error> {
    let cwd = std::env::current_dir()?;
    // Sync mutations run on this handle, never the Engine's (its Repo
    // belongs to the refresh thread). Discovery comes first so that
    // outside a repository the diagnostic stays `not a git repository`
    // rather than the terminal refusal below.
    let repo = Repo::discover(&cwd)?;
    // Pre-flight: refuse a non-terminal invocation before the Engine is
    // spawned (#110). Engine refreshes persist (ADR 0005), so a refusal
    // that ran one would have decided something on its way out —
    // capture, record writes, a baseline stamp. Refusing on *stdout*
    // too is deliberate: a TUI drawn into a pipe is useless, and the
    // CLI named in the message is the answer for both.
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(Error::NotATerminal);
    }
    let engine = Engine::spawn(&cwd)?;
    // Static context, read once off the sync handle — workdir is
    // process-constant, so it is not the snapshot channel's to carry.
    let repo_name = repo
        .workdir()
        .and_then(|dir| {
            dir.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "repository".to_owned());

    // The fallible initialiser, not `ratatui::init()`: that one panics,
    // which would exit `101` and break the 0/1/2 contract. The guard
    // above covers the common case; this closes the residual one, where
    // both streams are terminals and setup still fails.
    let mut terminal = match ratatui::try_init() {
        Ok(terminal) => terminal,
        // Setup is several steps, so a failure can land with raw mode
        // already on and no terminal to turn it off. `init()`'s panic
        // hook used to undo that; the error path has to undo it here or
        // the refusal hands back a wrecked shell. `restore` reports its
        // own failure and returns — it is not the panicking one.
        Err(err) => {
            ratatui::restore();
            return Err(err.into());
        }
    };
    let _ = execute!(std::io::stdout(), EnableFocusChange);
    // No kitty keyboard-protocol push: nothing in the keymap depends on a
    // modifier only that protocol reports (ADR 0013), so the flags would
    // be dead weight — and a standing invitation to bind against them.
    // Focus events are unaffected; they are not part of that protocol.

    // Terminal input on its own thread, bridged into the Select loop.
    // It parks in a blocking read; process exit reaps it. It is spawned
    // here rather than inside the loop so the loop can take any receiver
    // (ADR 0008's seam) — but still after raw mode and focus reporting
    // are on, so the first read sees the terminal in the same state it
    // always did.
    let (input_tx, input_rx) = unbounded();
    std::thread::spawn(move || {
        while let Ok(event) = ratatui::crossterm::event::read() {
            if input_tx.send(event).is_err() {
                return;
            }
        }
    });

    let mut app = App::new(repo_name);
    let result = event_loop(
        &mut terminal,
        engine.events(),
        &input_rx,
        || engine.request_refresh(),
        &repo,
        &mut app,
    );
    let _ = execute!(std::io::stdout(), DisableFocusChange);
    ratatui::restore();
    result
}

/// The main loop, over everything it actually consumes rather than the
/// concrete terminal and Engine `run()` hands it (ADR 0008): the backend
/// is generic so tests drive a `TestBackend`, input arrives as a plain
/// receiver, the Engine is decomposed into its event stream plus the
/// refresh request it emits, and the app is borrowed so a test can read
/// the state the loop left behind.
fn event_loop<B: Backend>(
    terminal: &mut Terminal<B>,
    engine_events: &Receiver<EngineEvent>,
    input: &Receiver<Event>,
    request_refresh: impl Fn(),
    repo: &Repo,
    app: &mut App,
) -> Result<(), Error> {
    let theme = Theme::default();
    loop {
        let now = Instant::now();
        // The same pure layout the draw divides the frame with, recorded
        // on the App afterwards (issue #85): a page key measures the
        // screen the user was looking at when they pressed it. The draw
        // runs before `select!` reads any input, so the heights are at
        // most one frame stale and never absent — including across a
        // resize.
        let mut heights = PanelHeights::default();
        terminal.draw(|frame| {
            heights = ui::panel_areas(frame.area(), app).content_heights();
            ui::draw(frame, app, &theme, now);
        })?;
        app.set_panel_heights(heights);

        // Wake exactly when the deferred indicator becomes due.
        let timer = match app.indicator_deadline() {
            Some(deadline) if deadline > now => at(deadline),
            _ => never(),
        };

        select! {
            recv(engine_events) -> event => match event {
                Ok(EngineEvent::RefreshStarted) => app.on_refresh_started(Instant::now()),
                Ok(EngineEvent::RefreshComplete(refreshed)) => app.apply_refresh(refreshed),
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
            recv(input) -> event => match event {
                Ok(Event::Key(key)) if key.kind != KeyEventKind::Release => {
                    match app.on_key(key) {
                        Some(Action::Quit) => return Ok(()),
                        Some(Action::Refresh) => request_refresh(),
                        Some(Action::Op(op)) => {
                            run_op(repo, app, op);
                            // Mutation-triggered refresh: bypasses the
                            // debounce (ADR 0005).
                            request_refresh();
                        }
                        Some(Action::Commit(step)) => {
                            run_commit_step(repo, app, step);
                            // Every step mutates or synchronously
                            // refreshed the state file; the immediate
                            // request keeps the snapshot in step
                            // (ADR 0005).
                            request_refresh();
                        }
                        None => {}
                    }
                }
                // Catch up on whatever happened while we were unfocused
                // (ADR 0005).
                Ok(Event::FocusGained) => request_refresh(),
                Ok(_) => {}
                Err(_) => return Ok(()),
            },
            recv(timer) -> _ => {}
        }
    }
}

/// Execute one sync op. What it decided echoes at `·` — core composes
/// the echo alongside the work it executed (ADR 0006/0007), this
/// frontend only assigns the channel — advisories land at `!`, and hard
/// failures take the error-modal contract.
fn run_op(repo: &Repo, app: &mut App, op: Op) {
    match op {
        // `n` is create-then-switch (`Op::CreateChangelist`): core
        // creates without moving the marker, so the switch is a second
        // op. A failed create leaves the marker alone. Both echo, so the
        // Log records the two decisions the one keypress made.
        Op::CreateChangelist { name } => {
            match repo.create_changelist(&name) {
                Ok(outcome) => log_outcome(app, outcome),
                Err(error) => {
                    app.show_error(CREATE_CHANGELIST_FAILED, error.to_string());
                    return;
                }
            }
            op_outcome(app, SWITCH_CHANGELIST_FAILED, repo.switch(Some(&name)));
        }
        Op::RenameChangelist { from, to } => op_outcome(
            app,
            "Rename changelist failed",
            repo.rename_changelist(&from, &to),
        ),
        // The confirm dialog *is* this frontend's override of the records
        // guard (ADR 0015's parity with the CLI's `-D`): the user has
        // already been warned and said yes, so the op is asked to
        // release. What it releases lands in the Log as a notice, core's
        // phrasing like every other.
        Op::DeleteChangelist { name } => {
            const FAILED: &str = "Delete changelist failed";
            match repo.delete_changelists(&[&name], Release::Forced) {
                Ok(Deletion::Done(outcome)) => log_outcome(app, outcome),
                // Forced, so the records guard cannot fire and the only
                // offender left is a name that vanished between the
                // dialog and the write — another actor's delete landing
                // first, in a tree they share.
                Ok(Deletion::Refused(offenders)) => app.show_error(
                    FAILED,
                    offenders
                        .iter()
                        .map(Undeletable::message)
                        .collect::<Vec<String>>()
                        .join("\n"),
                ),
                Err(error) => app.show_error(FAILED, error.to_string()),
            }
        }
        Op::SetActive { changelist } => op_outcome(
            app,
            SWITCH_CHANGELIST_FAILED,
            repo.switch(changelist.as_deref()),
        ),
        Op::StageOwnedHunks { path, changelist } => {
            op_outcome(
                app,
                STAGE_FAILED,
                repo.stage_owned_hunks(&path, changelist.as_deref()),
            );
        }
        Op::UnstageOwnedHunks { path, changelist } => {
            op_outcome(
                app,
                UNSTAGE_FAILED,
                repo.unstage_owned_hunks(&path, changelist.as_deref()),
            );
        }
        Op::StageHunk { path, hunk } => {
            op_outcome(app, STAGE_FAILED, repo.stage_hunk(&path, &hunk));
        }
        Op::UnstageHunk { path, hunk } => {
            op_outcome(app, UNSTAGE_FAILED, repo.unstage_hunk(&path, &hunk));
        }
        Op::StageChangelist { changelist } => {
            op_outcome(
                app,
                STAGE_FAILED,
                repo.stage_changelist(changelist.as_deref()),
            );
        }
        Op::UnstageChangelist { changelist } => {
            op_outcome(
                app,
                UNSTAGE_FAILED,
                repo.unstage_changelist(changelist.as_deref()),
            );
        }
        Op::Assign {
            path,
            hunks,
            target,
        } => {
            // `None` releases to unassigned (ADR 0016).
            let name = match &target {
                AssignTarget::Existing(name) => Some(name.as_str()),
                AssignTarget::New(name) => {
                    // A name that already exists is a valid target: fall
                    // through to the assign rather than stranding it
                    // behind the create.
                    match repo.create_changelist(name) {
                        Ok(outcome) => log_outcome(app, outcome),
                        Err(gitchange_core::Error::ChangelistExists { .. }) => {}
                        Err(error) => {
                            app.show_error(CREATE_CHANGELIST_FAILED, error.to_string());
                            return;
                        }
                    }
                    Some(name.as_str())
                }
                AssignTarget::Unassigned => None,
            };
            op_outcome(app, "Assign failed", repo.assign_hunks(&path, &hunks, name));
        }
    }
}

/// A mutating op's outcome: core's echo at `·` for what it decided
/// (`None` when it decided nothing), `!` advisories for what failed soft
/// or moved on its own, the modal otherwise.
fn op_outcome(app: &mut App, title: &str, result: Result<OpOutcome, gitchange_core::Error>) {
    match result {
        Ok(outcome) => log_outcome(app, outcome),
        Err(error) => app.show_error(title, error.to_string()),
    }
}

/// A successful outcome onto the Log panel's two channels — core's
/// phrasing throughout (ADR 0006), this frontend assigning only the
/// severity (ADR 0007).
fn log_outcome(app: &mut App, outcome: OpOutcome) {
    if let Some(echo) = outcome.echo {
        app.push_log(Severity::Info, echo);
    }
    app.push_advisories(&outcome.advisories);
}

/// Execute one commit-flow IO step (ticket #33, ADR 0004); outcomes go
/// back into the App — dialog opened, drift re-confirm, or the dialog
/// restored with a feedback line on failure.
fn run_commit_step(repo: &Repo, app: &mut App, step: CommitStep) {
    match step {
        CommitStep::Open { changelist } => open_commit_dialog(repo, app, changelist),
        CommitStep::StageAllAndOpen { changelist } => {
            // Stage the changelist's unstaged hunks (core's bulk op,
            // fail-soft per hunk), then fall into the dialog.
            match repo.stage_all(changelist.as_deref()) {
                Ok(outcome) => {
                    log_outcome(app, outcome);
                    open_commit_dialog(repo, app, changelist);
                }
                Err(error) => app.show_error(COMMIT_FAILED, error.to_string()),
            }
        }
        CommitStep::Commit(draft) => run_commit(repo, app, draft),
        CommitStep::AlignAndCommit(mut draft) => {
            // The ◑ warn's align option: index := worktree over the
            // changelist's stale hunks, then commit what that produced —
            // the payload is re-derived so the drift guard compares the
            // aligned content, not the stale confirmation.
            match repo.align(draft.changelist.as_deref()) {
                Ok(outcome) => log_outcome(app, outcome),
                Err(error) => {
                    app.show_error(COMMIT_FAILED, error.to_string());
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
                    app.show_error(COMMIT_FAILED, error.to_string());
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
        Err(error) => app.show_error(COMMIT_FAILED, error.to_string()),
    }
}

/// Run the confirmed commit, echoing the shelled-out command (ADR 0007's
/// transparency channel; core composes the line, ADR 0006). Failure
/// restores the dialog exactly as confirmed with the rejection modal on
/// top — hook stderr verbatim; drift loops back to the re-confirm
/// overlay with the fresh payload.
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
    // The dialog always composes a message, amend included, so the
    // message-keeping mode is the CLI's alone (spec #151).
    let message = CommitMessage::Given(&message);
    let echo = commit_echo(
        &options,
        message,
        draft.changelist.as_deref(),
        &draft.payload,
    );
    match repo.commit(
        draft.changelist.as_deref(),
        message,
        &options,
        Some(&draft.payload),
    ) {
        Ok(CommitOutcome::Committed { short_id, .. }) => {
            app.push_log(Severity::Info, echo);
            app.push_log(
                Severity::Info,
                format!("committed {short_id} \"{}\"", draft.message),
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
            app.show_error(COMMIT_FAILED, detail);
            app.restore_commit_dialog(draft);
        }
    }
}
