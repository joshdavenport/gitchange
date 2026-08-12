//! The TUI's state machine, pure over [`Snapshot`]s: panel focus,
//! drill-down scope, selections and their identity-first survival across
//! snapshot swaps (ADR 0005), and the row/diff view-models the renderer
//! draws. No terminal, no git — everything here is unit-testable with
//! synthetic snapshots (ADR 0008).

mod keymap;
mod overlay;
mod selection;
mod status;
#[cfg(test)]
mod tests;
mod view;

pub use keymap::help_rows;
pub use overlay::{AssignPayload, AssignRow, CommitDraft, InputKind, Overlay, payload_counts};
pub use status::{ErrorModal, INDICATOR_DELAY, LogEntry, Severity, advisory_entry};
pub use view::{ChangelistRow, DiffLine, FileEntry, FilesRow, Group, HunkTag, Scope};

use keymap::{BindingId, Capability};
use selection::Motion;

use std::time::Instant;

use gitchange_core::{ChangeKind, FileStage, Hunk, HunkStage, Snapshot};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// The lazygit-style panel stack: `1-5` plus the dominant `0` Diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Status,
    Changelists,
    Files,
    Commits,
    Diff,
    Log,
}

impl Panel {
    pub const ALL: [Panel; 6] = [
        Panel::Status,
        Panel::Changelists,
        Panel::Files,
        Panel::Commits,
        Panel::Diff,
        Panel::Log,
    ];

    pub fn number(self) -> char {
        match self {
            Panel::Status => '1',
            Panel::Changelists => '2',
            Panel::Files => '3',
            Panel::Commits => '4',
            Panel::Diff => '0',
            Panel::Log => '5',
        }
    }

    /// The inverse of [`Panel::number`]: which panel a digit key focuses,
    /// if any. Derived from `number`, so the border prefix and the
    /// keybinding cannot disagree.
    pub fn from_number(c: char) -> Option<Panel> {
        Self::ALL.into_iter().find(|panel| panel.number() == c)
    }

    /// The panel's border title — an exhaustive match so a new variant
    /// fails to compile rather than falling back to the wrong title.
    pub fn title(self) -> &'static str {
        match self {
            Panel::Status => "Status",
            Panel::Changelists => "Changelists",
            Panel::Files => "Files",
            Panel::Commits => "Commits",
            Panel::Diff => "Diff",
            Panel::Log => "Log",
        }
    }

    /// The panel's slot in [`Panel::ALL`], for keying a fixed-size
    /// per-panel array. Read from `ALL` rather than written out, so the
    /// keys and the array [`PanelHeights`] builds from `ALL` cannot
    /// disagree.
    fn slot(self) -> usize {
        Self::ALL
            .into_iter()
            .position(|panel| panel == self)
            .expect("ALL lists every panel")
    }
}

/// Per-panel content heights as of the last drawn frame (issue #85):
/// borders excluded, and for the Log the pinned-conditions banner
/// excluded too. The renderer computes them; the main loop hands them
/// here so an action's magnitude can depend on how tall its panel was.
///
/// Every panel has a height at all times — zero until the first frame —
/// so reading one needs no option handling. See [`App::page`] for what
/// that zero means to a page-sized step.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PanelHeights([u16; Panel::ALL.len()]);

impl PanelHeights {
    /// Measure every panel. [`Panel::ALL`] drives the call, so the set is
    /// total by construction: a panel cannot be left behind at a default
    /// by a caller that forgot it.
    pub fn from_fn(height: impl FnMut(Panel) -> u16) -> Self {
        Self(Panel::ALL.map(height))
    }

    pub fn get(&self, panel: Panel) -> u16 {
        self.0[panel.slot()]
    }
}

/// What a key press asks the main loop to do beyond mutating the App.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Quit,
    /// The manual refresh key (ADR 0005).
    Refresh,
    /// A synchronous mutation for the main loop's own `Repo` handle
    /// (never the Engine's); a refresh request follows it.
    Op(Op),
    /// One commit-flow IO step (ticket #33); outcomes come back through
    /// the App's `commit_*`/`open_*` methods.
    Commit(CommitStep),
}

/// One synchronous repo mutation (ticket #32). The main loop executes
/// it and hands the outcome back on the presentation channels
/// (ADR 0007): a hard failure through [`App::show_error`], fail-soft
/// advisories through [`App::push_advisories`], and core's echo for
/// the work that applied through [`App::push_log`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// `n`: create a changelist and switch to it. Core's create never
    /// moves the active marker (ADR 0015) — pairing it with a switch is
    /// this frontend's reading of `n`, where one human works one thing
    /// at a time. The assign popup's "+ create new changelist…" is the
    /// other creation path and stays [`Op::Assign`]'s, marker untouched.
    CreateChangelist {
        name: String,
    },
    RenameChangelist {
        from: String,
        to: String,
    },
    DeleteChangelist {
        name: String,
    },
    /// `s`: set the active changelist; `None` is unassigned — capture
    /// off (ADR 0015).
    SetActive {
        changelist: Option<String>,
    },
    /// Assign snapshot hunks to a changelist. `create` makes the target
    /// first — the assign popup's "+ create new changelist…" escape
    /// hatch.
    Assign {
        path: String,
        hunks: Vec<Hunk>,
        target: String,
        create: bool,
    },
    /// `space` on a Files row: whole-file staging, `git add` semantics.
    StageFile {
        path: String,
    },
    /// `space` on a `●` Files row: `git reset -- <path>` semantics.
    UnstageFile {
        path: String,
    },
    /// `space` on a `○`/`◑` hunk: index := worktree for its region.
    StageHunk {
        path: String,
        hunk: Hunk,
    },
    /// `space` on a `●` hunk: index := HEAD for its region.
    UnstageHunk {
        path: String,
        hunk: Hunk,
    },
    /// `space` on a Changelists row holding any `○`/`◑` hunk: index :=
    /// worktree across the whole changelist (`None` = unassigned).
    StageChangelist {
        changelist: Option<String>,
    },
    /// `space` on a Changelists row whose hunks are all `●`: index :=
    /// HEAD across the whole changelist (`None` = unassigned).
    UnstageChangelist {
        changelist: Option<String>,
    },
}

/// The commit flow's IO steps (ADR 0004), executed by the main loop on
/// its own `Repo` handle. The toggle/branching logic stays in [`App`];
/// these carry everything the loop needs so no dialog state lives
/// outside the [`Overlay`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitStep {
    /// `c`: derive the payload behind a synchronous refresh, then open
    /// the dialog (or the stage-all offer on an empty payload).
    Open { changelist: Option<String> },
    /// The zero-staged offer confirmed: stage the changelist's unstaged
    /// hunks, then derive the payload and open the dialog.
    StageAllAndOpen { changelist: Option<String> },
    /// The confirmed dialog: commit against its inspected payload — the
    /// drift guard compares this exact payload (ADR 0004).
    Commit(CommitDraft),
    /// The ◑ warn's align option: index := worktree over the
    /// changelist's ◑ hunks, then commit the aligned payload.
    AlignAndCommit(CommitDraft),
}

pub struct App {
    /// Directory name of the worktree root, for the Status line.
    pub repo_name: String,
    pub snapshot: Option<Snapshot>,
    pub focus: Panel,
    /// Index into [`App::changelist_rows`].
    pub changelist_row: usize,
    pub file_sel: Option<FileEntry>,
    pub commit_row: usize,
    pub diff_scroll: u16,
    /// Hunk-mode selection: `Some` while `enter`
    /// on a file has focused the Diff panel for per-hunk work; `0`-key
    /// focus is plain scroll mode (`None`). Survives a blur to another
    /// panel (issue #45) — the cursor stays visible for the cross-panel
    /// assign keys — and ends when the underlying file selection moves.
    pub hunk_sel: Option<usize>,
    /// The open modal, if any.
    pub overlay: Option<Overlay>,
    /// The Log panel's chronological stream (ADR 0007), newest last.
    pub log: Vec<LogEntry>,
    /// Lines scrolled up from the stream's bottom (`5`-focus + j/k);
    /// zero sticks to the newest entry.
    pub log_scroll: usize,
    /// The open error modal (ADR 0007), above any [`Overlay`].
    pub error_modal: Option<ErrorModal>,
    /// Whether the Engine reported `Condition::WatcherDegraded` — the
    /// watcher pin's state, self-clearing on `ConditionEnded`.
    pub watcher_degraded: bool,
    pub help_open: bool,
    /// When the in-flight refresh started; drives the deferred
    /// indicator.
    refresh_started: Option<Instant>,
    /// The last refresh failure's text: a polling engine retries a
    /// persistent failure every few seconds, and re-modalling the same
    /// error each tick is the interruption ADR 0007 rejects. Cleared by
    /// the next successful refresh, so a changed failure still modals.
    last_refresh_failure: Option<String>,
    /// The (files, hunks, conflicted) counts last echoed to the log —
    /// polling refreshes that change nothing stay quiet.
    last_refresh_echo: Option<(usize, usize, usize)>,
    /// What the last frame gave each panel (issue #85). The App holds no
    /// other geometry: everything else the renderer needs it derives at
    /// draw time.
    panel_heights: PanelHeights,
}

impl App {
    pub fn new(repo_name: impl Into<String>) -> Self {
        Self {
            repo_name: repo_name.into(),
            snapshot: None,
            focus: Panel::Changelists,
            changelist_row: 0,
            file_sel: None,
            commit_row: 0,
            diff_scroll: 0,
            hunk_sel: None,
            overlay: None,
            log: Vec::new(),
            log_scroll: 0,
            error_modal: None,
            watcher_degraded: false,
            help_open: false,
            refresh_started: None,
            last_refresh_failure: None,
            last_refresh_echo: None,
            panel_heights: PanelHeights::default(),
        }
    }

    // ── viewport geometry (issue #85) ───────────────────────────────

    /// Record what the frame just drawn gave each panel. The main loop
    /// calls this after every `terminal.draw`, so the heights read back
    /// are the screen the user was looking at when they pressed a key.
    pub fn set_panel_heights(&mut self, heights: PanelHeights) {
        self.panel_heights = heights;
    }

    /// `panel`'s content height as of the last frame; zero before the
    /// first one.
    pub fn panel_height(&self, panel: Panel) -> u16 {
        self.panel_heights.get(panel)
    }

    /// How far a page-sized step moves in `panel`: one screen, less a
    /// row of overlap so the reader keeps a line of context. Never zero
    /// — an App that has never been drawn pages by a single row rather
    /// than standing still.
    pub fn page(&self, panel: Panel) -> usize {
        usize::from(self.panel_height(panel).saturating_sub(1).max(1))
    }

    // ── input ───────────────────────────────────────────────────────

    pub fn on_key(&mut self, key: KeyEvent) -> Option<Action> {
        // Every `ctrl+<letter>` binding is matched on CONTROL alone
        // (ADR 0013): the control byte a plain terminal sends carries no
        // shift bit, so consulting SHIFT would make `ctrl+shift+<letter>`
        // fire a different action than the user asked for.
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && matches!(key.code, KeyCode::Char('c' | 'C')) {
            return Some(Action::Quit);
        }
        // The error modal sits above everything (ADR 0007): `esc`/`enter`
        // dismiss, j/k scroll the verbatim detail. No inline actions.
        if let Some(modal) = &mut self.error_modal {
            match key.code {
                KeyCode::Esc | KeyCode::Enter => self.error_modal = None,
                KeyCode::Char('j') | KeyCode::Down => {
                    modal.scroll = modal.scroll.saturating_add(1);
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    modal.scroll = modal.scroll.saturating_sub(1);
                }
                _ => {}
            }
            return None;
        }
        if self.help_open {
            match key.code {
                KeyCode::Esc | KeyCode::Char('?') => self.help_open = false,
                KeyCode::Char('q') => return Some(Action::Quit),
                _ => {}
            }
            return None;
        }
        if self.overlay.is_some() {
            return self.on_overlay_key(key);
        }
        // The binding core (ADR 0014): resolve the key to its record,
        // then consult the same disabled-reason the keybar hides by — a
        // non-empty reason logs on the press (ADR 0007's channel), an
        // empty one drops it silently.
        let binding = keymap::binding_for(key)?;
        if let Some(reason) = self.disabled_reason(binding.capability) {
            if !reason.is_empty() {
                self.push_log(Severity::Info, reason);
            }
            return None;
        }
        match binding.id {
            BindingId::Quit => return Some(Action::Quit),
            BindingId::Refresh => return Some(Action::Refresh),
            BindingId::Help => self.help_open = true,
            BindingId::FocusPanel => {
                // Explicit Diff focus is scroll mode; hunk mode only
                // enters through `enter` on a file.
                if let KeyCode::Char(c) = key.code {
                    match Panel::from_number(c).expect("PanelDigit matched") {
                        Panel::Files => self.focus_files(),
                        panel => self.set_focus(panel),
                    }
                }
            }
            BindingId::MoveDown => self.move_selection(Motion::Row, 1),
            BindingId::MoveUp => self.move_selection(Motion::Row, -1),
            BindingId::PageDown => self.move_selection(Motion::Page, 1),
            BindingId::PageUp => self.move_selection(Motion::Page, -1),
            BindingId::JumpBottom => self.move_selection(Motion::Jump, 1),
            BindingId::JumpTop => self.move_selection(Motion::Jump, -1),
            BindingId::NewChangelist => {
                self.overlay = Some(Overlay::Input {
                    kind: InputKind::NewChangelist,
                    value: String::new(),
                });
            }
            // The assign trio (ADR 0013), scope escalating with keypress
            // difficulty. Empty payloads stay press-time no-ops inside
            // the helpers — content, not context (ADR 0014).
            BindingId::AssignSelected => {
                if let Some(payload) = self.selected_assign_payload() {
                    self.open_assign(payload);
                }
            }
            BindingId::AssignUnassigned => {
                if let Some(path) = self.assign_file_path() {
                    self.open_assign(AssignPayload::UnassignedHunks { path });
                }
            }
            BindingId::AssignAll => {
                if let Some(path) = self.assign_file_path() {
                    self.open_assign(AssignPayload::AllHunks { path });
                }
            }
            // `s` reaches one target the other ops keys don't:
            // unassigned, whose `*` is capture-off (ADR 0015). All is
            // already out — the capability disabled it above.
            BindingId::SwitchActive => match self.scope() {
                Scope::All => {}
                Scope::Changelist(name) => {
                    return Some(Action::Op(Op::SetActive {
                        changelist: Some(name),
                    }));
                }
                Scope::Unassigned => {
                    return Some(Action::Op(Op::SetActive { changelist: None }));
                }
            },
            BindingId::RenameChangelist => {
                if let Some(name) = self.scoped_changelist() {
                    self.overlay = Some(Overlay::Input {
                        kind: InputKind::Rename { from: name.clone() },
                        value: name,
                    });
                }
            }
            BindingId::DeleteChangelist => {
                if let Some(name) = self.scoped_changelist() {
                    self.overlay = Some(Overlay::ConfirmDelete { name });
                }
            }
            BindingId::StageToggle => return self.stage_toggle(),
            BindingId::Commit => match self.scope() {
                // Unreachable: the Commit capability disabled All above.
                Scope::All => {}
                Scope::Changelist(name) => {
                    return Some(Action::Commit(CommitStep::Open {
                        changelist: Some(name),
                    }));
                }
                // Unassigned is committable like any changelist (ADR 0004).
                Scope::Unassigned => {
                    return Some(Action::Commit(CommitStep::Open { changelist: None }));
                }
            },
            BindingId::DrillIn => match self.focus {
                // Drill-down only: 2 → select changelist → 3 → files →
                // hunk mode. `enter` assigns nothing anywhere — the Diff
                // panel is the end of the drill, and the key it used to
                // share with assign was the misfire ADR 0013 bars.
                Panel::Changelists => self.focus_files(),
                Panel::Files => self.enter_hunk_mode(),
                Panel::Diff => {}
                _ => {}
            },
            BindingId::Back => match self.focus {
                Panel::Diff => {
                    self.hunk_sel = None;
                    self.focus = Panel::Files;
                }
                Panel::Files => self.focus = Panel::Changelists,
                Panel::Changelists => self.select_changelist_row(0),
                _ => {}
            },
        }
        None
    }

    /// The shared disabled-reason predicate (ADR 0014): why a
    /// capability's keys won't act right now, one answer consulted by
    /// dispatch and the keybar alike. `None` means live; `Some("")`
    /// hides silently (the wrong panel — nothing worth logging); a
    /// non-empty reason hides the binding from the bar and logs when the
    /// key is pressed anyway. Context guards only — cheap reads of
    /// already-loaded state, derived per frame by the bar.
    fn disabled_reason(&self, capability: Capability) -> Option<String> {
        match capability {
            Capability::Always => None,
            Capability::ChangelistOps => {
                if !matches!(self.focus, Panel::Changelists | Panel::Files) {
                    return Some(String::new());
                }
                match self.scope() {
                    Scope::Changelist(_) => None,
                    Scope::All => Some("select a changelist — all is a view".into()),
                    Scope::Unassigned => {
                        Some("select a changelist — unassigned is built-in".into())
                    }
                }
            }
            Capability::SwitchActive => {
                if !matches!(self.focus, Panel::Changelists | Panel::Files) {
                    return Some(String::new());
                }
                // Unassigned is a switchable target (ADR 0015), so only
                // All — a view over many changelists — is left out.
                match self.scope() {
                    Scope::Changelist(_) | Scope::Unassigned => None,
                    Scope::All => Some("select a changelist — all is a view".into()),
                }
            }
            Capability::Assign => {
                let live =
                    matches!(self.focus, Panel::Files | Panel::Diff) || self.hunk_sel.is_some();
                (!live).then(String::new)
            }
            Capability::Stage => {
                // The three panels `space` has a scope in: the focused
                // hunk selection, a Files row, a Changelists row.
                if self.hunk_mode_focused() || self.focus == Panel::Files {
                    return None;
                }
                if self.focus != Panel::Changelists {
                    return Some(String::new());
                }
                // All is a view over many changelists rather than one of
                // them — the same reason commit and the ops keys give.
                match self.scope() {
                    Scope::Changelist(_) | Scope::Unassigned => None,
                    Scope::All => Some("select a changelist to stage — all is a view".into()),
                }
            }
            Capability::Commit => {
                // The operation guard (ADR 0007): the next commit would
                // conclude the operation — the pin names it, pressing
                // `c` logs why nothing opened.
                if let Some(operation) = self.snapshot.as_ref().and_then(|s| s.operation) {
                    return Some(operation.in_progress_message());
                }
                // Commit needs one changelist scoped (ADR 0004); All is
                // a view over many.
                match self.scope() {
                    Scope::All => Some("select a changelist to commit — all is a view".into()),
                    Scope::Changelist(_) | Scope::Unassigned => None,
                }
            }
        }
    }

    /// Move panel focus. Hunk mode survives a blur to another panel —
    /// its cursor must stay visible because the assign keys act on it
    /// cross-panel (issue #45) — but refocusing the Diff panel itself
    /// (`0`) is scroll mode, so that resets it.
    fn set_focus(&mut self, panel: Panel) {
        self.focus = panel;
        if panel == Panel::Diff {
            self.hunk_sel = None;
        }
    }

    /// A hunk selection owns keys (and the keybar) only while the Diff
    /// panel is focused — a blurred one stays visible for the
    /// cross-panel assign keys but claims nothing else (issue #45).
    fn hunk_mode_focused(&self) -> bool {
        self.focus == Panel::Diff && self.hunk_sel.is_some()
    }

    fn focus_files(&mut self) {
        self.set_focus(Panel::Files);
        if self.file_sel.is_none() {
            self.file_sel = self.file_entries().first().cloned();
        }
    }

    /// `enter` on a file: focus the Diff panel
    /// with its first hunk selected. Hunk-less files have nothing to
    /// select, and a binary's one whole-file hunk is a state that can
    /// only waste keypresses — polite no-op, no log event (ADR 0009).
    fn enter_hunk_mode(&mut self) {
        let has_hunks = self
            .selected_file()
            .is_some_and(|file| !file.binary && !file.hunks.is_empty());
        if has_hunks {
            self.focus = Panel::Diff;
            self.hunk_sel = Some(0);
        }
    }

    /// The changelist the ops keys (`d`/`r`/`s`) act on: the scoped
    /// row's name. Whether the keys are live at all is
    /// [`Capability::ChangelistOps`]'s answer — dispatch consults it
    /// before landing here, so this is extraction, not a guard.
    fn scoped_changelist(&self) -> Option<String> {
        match self.scope() {
            Scope::Changelist(name) => Some(name),
            Scope::All | Scope::Unassigned => None,
        }
    }

    /// `space`'s decide-by-current-state toggle (ticket #33): in hunk
    /// mode the selected hunk (`○`/`◑` → stage, `●` → unstage), in the
    /// Files panel the whole file (`●` → unstage, else stage), in the
    /// Changelists panel every hunk of the scoped row (issue #90). Core
    /// exposes stage and unstage separately; the toggle lives here.
    fn stage_toggle(&mut self) -> Option<Action> {
        if self.hunk_mode_focused() {
            let index = self.hunk_sel?;
            let file = self.selected_file()?;
            let hunk = file.hunks.get(index)?.clone();
            let path = file.path.clone();
            let op = match hunk.stage {
                HunkStage::Staged => Op::UnstageHunk { path, hunk },
                HunkStage::Unstaged | HunkStage::StagedStale => Op::StageHunk { path, hunk },
            };
            return Some(Action::Op(op));
        }
        if self.focus == Panel::Changelists {
            return self.stage_toggle_changelist();
        }
        if self.focus != Panel::Files {
            return None;
        }
        let file = self.selected_file()?;
        let path = file.path.clone();
        // `space` on quarantined content politely refuses (ADR 0007) —
        // three index stages are a workflow gitchange doesn't serve.
        if file.kind == ChangeKind::Conflicted {
            self.push_log(Severity::Info, gitchange_core::conflicted_hint(&path));
            return None;
        }
        let op = match file.stage() {
            FileStage::Staged => Op::UnstageFile { path },
            FileStage::Unstaged | FileStage::PartiallyStaged => Op::StageFile { path },
        };
        Some(Action::Op(op))
    }

    /// `space` on a Changelists row (issue #90): the same toggle at
    /// changelist scope, over every hunk the row owns across files. Any
    /// `○`/`◑` among them takes the stage direction, mirroring the
    /// per-hunk key; only an all-`●` row unstages. A row that owns
    /// nothing takes the stage direction too, so core answers with its
    /// nothing-to-stage echo rather than silence.
    fn stage_toggle_changelist(&self) -> Option<Action> {
        let scope = self.scope();
        // `None` is the All row, which owns no hunks — unreachable, since
        // the Stage capability disabled it before dispatch got here.
        let owner = scope.owner()?;
        let snapshot = self.snapshot.as_ref()?;
        let mut owned = snapshot
            .files
            .iter()
            .flat_map(|file| &file.hunks)
            .filter(|hunk| view::owns(hunk, owner))
            .peekable();
        let all_staged =
            owned.peek().is_some() && owned.all(|hunk| hunk.stage == HunkStage::Staged);
        let changelist = owner.map(str::to_owned);
        Some(Action::Op(if all_staged {
            Op::UnstageChangelist { changelist }
        } else {
            Op::StageChangelist { changelist }
        }))
    }
}
