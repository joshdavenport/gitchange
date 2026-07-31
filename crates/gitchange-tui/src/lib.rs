//! The ratatui frontend (ADR 0006): a thin consumer of core's Engine.
//! Panels render the last snapshot whole (ADR 0005); a sync crossbeam
//! `Select` main loop multiplexes engine events, terminal input, and the
//! deferred-indicator timer — no async runtime.

pub mod app;
pub mod theme;
pub mod ui;

use std::time::Instant;

use crossbeam_channel::{at, never, select, unbounded};
use gitchange_core::{Engine, EngineEvent, Notice, Repo};
use ratatui::crossterm::event::{
    DisableFocusChange, EnableFocusChange, Event, KeyEventKind, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;

use app::{Action, App, Op};
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
