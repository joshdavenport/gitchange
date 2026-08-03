//! The TUI's state machine, pure over [`Snapshot`]s: panel focus,
//! drill-down scope, selections and their identity-first survival across
//! snapshot swaps (ADR 0005), and the row/diff view-models the renderer
//! draws. No terminal, no git — everything here is unit-testable with
//! synthetic snapshots (ADR 0008).

use std::time::{Duration, Instant};

use gitchange_core::{
    ChangeKind, ChangedFile, CommitPayload, FileStage, Hunk, HunkStage, Notice, Snapshot,
};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// The deferred-indicator threshold (ADR 0005): refreshes shorter than
/// this show nothing.
pub const INDICATOR_DELAY: Duration = Duration::from_millis(500);

/// Log entries kept — enough history for the panel plus scrollback.
const LOG_CAP: usize = 200;

/// A log event's severity (ADR 0007): three levels, fixed — new event
/// classes map onto these rather than growing the scale. Assigned here
/// in the presentation layer; core's [`Notice`] carries none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// `·` — routine: command echo, refresh ticks, soft no-ops.
    Info,
    /// `!` — automatic membership decisions worth spot-checking.
    Notice,
    /// `✗` — anything that produced an error modal, for the record.
    Error,
}

/// One immutable line of the Log panel's chronological stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    pub severity: Severity,
    pub text: String,
}

/// The error-modal contract (ADR 0007): title names the operation,
/// detail is verbatim and scrollable (hook stderr especially), dismissed
/// with `esc`/`enter`. Held outside [`Overlay`] so a rejection modal can
/// land on top of the restored commit dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorModal {
    pub title: String,
    pub detail: String,
    pub scroll: u16,
}

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
}

/// One row of the Changelists panel — also the drill-down scope the
/// Files and Diff panels render.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// The All pseudo-view: every changed file grouped by changelist.
    All,
    /// The quarantine group (ADR 0007): a Files-row group only, never a
    /// Changelists-panel row or drill scope.
    Conflicts,
    Changelist(String),
    /// The unassigned pseudo-changelist, pinned last.
    Unassigned,
}

impl Scope {
    /// The hunk owner this scope drills into; `None` for All (no owner —
    /// nothing is foreign there) and Conflicts (no hunks at all).
    fn owner(&self) -> Option<Option<&str>> {
        match self {
            Scope::All | Scope::Conflicts => None,
            Scope::Changelist(name) => Some(Some(name)),
            Scope::Unassigned => Some(None),
        }
    }

    pub fn title(&self) -> &str {
        match self {
            Scope::All => "all changelists",
            Scope::Conflicts => "conflicts",
            Scope::Changelist(name) => name,
            Scope::Unassigned => "unassigned",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChangelistRow {
    pub scope: Scope,
    pub count: usize,
    pub active: bool,
}

/// A selected file's identity: owning group plus path, so the same path
/// appearing under two changelists (hunk-level membership) stays two
/// distinct selections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    /// The group the row sits under: `Scope::Changelist`/`Unassigned`.
    pub group: Scope,
    pub path: String,
}

#[derive(Debug, Clone)]
pub enum FilesRow {
    /// A group header in the All view.
    Header {
        label: String,
        count: usize,
        unassigned: bool,
        /// The Conflicts group's error tint (ADR 0007).
        conflicted: bool,
        active: bool,
    },
    File {
        entry: FileEntry,
        stage: FileStage,
        kind: ChangeKind,
        staged: usize,
        total: usize,
        /// Grouped (All) rows indent under their header.
        indent: bool,
    },
}

/// A diff hunk's tag naming its owning changelist (All view: every hunk;
/// drilled views: foreign hunks only, dimmed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HunkTag {
    pub label: String,
    /// Staging glyph next to the label; `None` on unassigned tags
    /// (prototype B).
    pub stage: Option<HunkStage>,
    pub unassigned: bool,
    pub dim: bool,
}

/// One renderable diff line. `foreign` rows belong to another
/// changelist in a drilled view and render dimmed (~45%).
#[derive(Debug, Clone)]
pub enum DiffLine {
    FileHeader(String),
    HunkHeader {
        text: String,
        tag: Option<HunkTag>,
        foreign: bool,
        /// Part of the hunk-mode selection (prototype variant C).
        selected: bool,
    },
    Content {
        origin: char,
        text: String,
        foreign: bool,
        /// Part of the hunk-mode selection (prototype variant C).
        selected: bool,
    },
    /// Blank spacing between hunks.
    Spacer,
    Placeholder(String),
    /// The quarantine placeholder (ADR 0007), error-tinted: no conflict
    /// diff rendering, ever.
    Conflict(String),
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
/// it and hands failures and fail-soft notices back through
/// [`App::push_feedback`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
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
    SetActive {
        name: String,
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

/// Everything the commit dialog holds (commit-flow prototype variant A):
/// the target changelist, the inspected payload, the independent
/// message/body drafts, and the flag toggles. Kept whole through the
/// warn/drift overlays so a failed commit restores the dialog untouched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitDraft {
    /// `None` commits unassigned — committable like any changelist
    /// (ADR 0004).
    pub changelist: Option<String>,
    pub payload: CommitPayload,
    /// The one-line commit subject.
    pub message: String,
    /// The optional multiline body (`tab` reaches it).
    pub body: String,
    /// Which input holds the cursor.
    pub body_focus: bool,
    pub no_verify: bool,
    pub amend: bool,
}

impl CommitDraft {
    fn new(changelist: Option<String>, payload: CommitPayload) -> Self {
        Self {
            changelist,
            payload,
            message: String::new(),
            body: String::new(),
            body_focus: false,
            no_verify: false,
            amend: false,
        }
    }

    /// The dialog title's changelist name.
    pub fn changelist_label(&self) -> &str {
        self.changelist.as_deref().unwrap_or("unassigned")
    }
}

/// "N staged hunks in M files", singulars handled — the payload
/// summary's counts (the dialog appends the ◑ tail, coloured) and the
/// drift notice's was/now line.
pub fn payload_counts(payload: &CommitPayload) -> String {
    format!(
        "{} in {}",
        count_noun(
            payload.staged_hunks() + payload.stale_hunks(),
            "staged hunk"
        ),
        count_noun(payload.file_count(), "file"),
    )
}

/// "1 hunk" / "2 hunks" — the count-plus-noun shape the commit modals
/// keep needing.
pub fn count_noun(count: usize, noun: &str) -> String {
    let plural = if count == 1 { "" } else { "s" };
    format!("{count} {noun}{plural}")
}

/// A modal above the panel stack. At most one is open; it swallows every
/// key until it closes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Overlay {
    /// A one-line text input, text-on-border framing (the commit-flow
    /// prototype's general input convention).
    Input { kind: InputKind, value: String },
    /// Delete-changelist confirmation (brief §3-derived pattern): the
    /// changelist's hunks go to unassigned.
    ConfirmDelete { name: String },
    /// The centered assign popup (prototype variant D).
    Assign { payload: AssignPayload, row: usize },
    /// The all-in-one commit dialog (commit-flow prototype variant A).
    Commit(CommitDraft),
    /// The ◑ staged-stale warn-and-confirm over the dialog (variant B,
    /// ADR 0004).
    CommitStale(CommitDraft),
    /// The zero-staged stage-all offer (variant C, ADR 0004 — core never
    /// auto-stages).
    CommitStageAll {
        changelist: Option<String>,
        /// The changelist's hunk/file counts, captured at open for the
        /// "stage all N hunks in M files" line.
        hunks: usize,
        files: usize,
    },
    /// Drift re-confirm keeping the message (variant D, the ADR 0004
    /// freshness guard): `draft.payload` is already the fresh payload;
    /// `previous` is what the dialog had confirmed.
    CommitDrift {
        draft: CommitDraft,
        previous: CommitPayload,
    },
}

/// What an [`Overlay::Input`] submission means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputKind {
    NewChangelist,
    Rename {
        from: String,
    },
    /// The assign popup's escape hatch: create the named changelist,
    /// then assign the payload into it. `esc` returns to the popup.
    NewChangelistForAssign {
        payload: AssignPayload,
    },
}

/// What a confirmed assign popup assigns. The three scopes escalate with
/// keypress difficulty (`a` / `A` / `ctrl+a`); `A` and `ctrl+a` may reach
/// hunks rendered on another row, which only the popup's payload line
/// makes acceptable (ADR 0013's ticket).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssignPayload {
    /// `a` on a Files row: the file's hunks owned by the row's group.
    /// Under the unassigned group this coincides with `A` — the same
    /// hunks by two routes, not a special case.
    FileRow(FileEntry),
    /// `a` in hunk mode: the selected hunk, captured at popup open so a
    /// refresh under the popup can't retarget it (the op content-matches
    /// at apply regardless).
    Hunk { path: String, hunk: Hunk },
    /// `A`: the file's unassigned hunks, wherever they render. Names the
    /// hunks it *takes*, not the target — unassigned is a target like any
    /// other, reached through the popup's rows.
    UnassignedHunks { path: String },
    /// `ctrl+a`: every hunk of the file, hunks owned by other
    /// changelists included.
    AllHunks { path: String },
}

impl AssignPayload {
    /// The file every scope is anchored to.
    fn path(&self) -> &str {
        match self {
            AssignPayload::FileRow(entry) => &entry.path,
            AssignPayload::Hunk { path, .. }
            | AssignPayload::UnassignedHunks { path }
            | AssignPayload::AllHunks { path } => path,
        }
    }
}

/// One row of the assign popup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssignRow {
    Changelist {
        name: String,
        active: bool,
    },
    /// "+ create new changelist…", pinned last.
    CreateNew,
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
    /// Hunk-mode selection (prototype variant C): `Some` while `enter`
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
        }
    }

    // ── engine events ───────────────────────────────────────────────

    pub fn on_refresh_started(&mut self, now: Instant) {
        self.refresh_started = Some(now);
    }

    /// A hard refresh failure: the error-modal contract (ADR 0007), with
    /// one softening — a degraded engine polls every few seconds, and
    /// re-modalling the same persistent failure each tick would be the
    /// refresh-triggered interruption the ADR rejects, so an unchanged
    /// failure repeats silently.
    pub fn on_refresh_failed(&mut self, error: String) {
        self.refresh_started = None;
        if self.last_refresh_failure.as_deref() == Some(error.as_str()) {
            return;
        }
        self.last_refresh_failure = Some(error.clone());
        self.show_error("Refresh failed", error);
    }

    /// `ConditionStarted(WatcherDegraded)`: pin the condition and log
    /// the moment it began (the pin is the condition, the event marks
    /// its start — ADR 0007).
    pub fn on_watcher_degraded(&mut self) {
        if self.watcher_degraded {
            return;
        }
        self.watcher_degraded = true;
        self.push_log(
            Severity::Notice,
            "watcher unavailable — falling back to polling",
        );
    }

    /// `ConditionEnded(WatcherDegraded)`: the pin self-clears.
    pub fn on_watcher_recovered(&mut self) {
        if !self.watcher_degraded {
            return;
        }
        self.watcher_degraded = false;
        self.push_log(Severity::Info, "watcher recovered");
    }

    /// The live condition set as pin lines, top of the Log panel
    /// (ADR 0007): condition-bound, self-clearing, never dismissable.
    pub fn pins(&self) -> Vec<String> {
        let mut pins = Vec::new();
        // Operation first, watcher second — prototype variant E's order.
        if let Some(snapshot) = &self.snapshot
            && let Some(operation) = snapshot.operation
        {
            let conflicted = snapshot.conflicted_files().len();
            let tail = if conflicted > 0 {
                format!("{conflicted} conflicted")
            } else {
                "commit disabled".to_owned()
            };
            pins.push(format!("{} in progress — {tail}", operation.label()));
        }
        if self.watcher_degraded {
            pins.push("watcher unavailable — polling".to_owned());
        }
        if let Some(snapshot) = &self.snapshot
            && matches!(snapshot.head, gitchange_core::Head::Detached { .. })
        {
            pins.push("detached HEAD — commits belong to no branch".to_owned());
        }
        pins
    }

    /// Whether the deferred refresh indicator shows (ADR 0005): only
    /// once a refresh has been in flight past the threshold.
    pub fn indicator_visible(&self, now: Instant) -> bool {
        self.refresh_started
            .is_some_and(|started| now.duration_since(started) >= INDICATOR_DELAY)
    }

    /// The instant the indicator becomes due, for the main loop's timer.
    pub fn indicator_deadline(&self) -> Option<Instant> {
        self.refresh_started
            .map(|started| started + INDICATOR_DELAY)
    }

    /// Swap in a whole snapshot (ADR 0005), preserving selections
    /// identity-first: changelist by name, file by (group, path) then by
    /// path alone, else the nearest sibling by visual position.
    pub fn apply_snapshot(&mut self, snapshot: Snapshot) {
        self.refresh_started = None;
        self.last_refresh_failure = None;

        // The moment an operation condition begins is an event
        // (ADR 0007) — the pin itself carries the live state after.
        let old_operation = self.snapshot.as_ref().and_then(|old| old.operation);
        if let Some(operation) = snapshot.operation
            && old_operation != Some(operation)
        {
            let conflicted = snapshot.conflicted_files().len();
            let tail = if conflicted > 0 {
                let plural = if conflicted == 1 { "" } else { "s" };
                format!("{conflicted} file{plural} conflicted, commit disabled")
            } else {
                "commit disabled".to_owned()
            };
            self.push_log(
                Severity::Notice,
                format!("{} detected — {tail}", operation.label()),
            );
        }

        // The refresh echo (variant E): quiet when nothing moved, so a
        // degraded engine's polling ticks don't flood the stream.
        let hunks: usize = snapshot.files.iter().map(ChangedFile::total_hunks).sum();
        let conflicted = snapshot.conflicted_files().len();
        let counts = (snapshot.files.len(), hunks, conflicted);
        if self.last_refresh_echo != Some(counts) {
            self.last_refresh_echo = Some(counts);
            let mut echo = format!("refresh — {} files · {hunks} hunks", snapshot.files.len());
            if conflicted > 0 {
                echo.push_str(&format!(" ({conflicted} conflicted)"));
            }
            self.push_log(Severity::Info, echo);
        }

        // This refresh's automatic membership decisions, each exactly
        // once (a decision becomes a record; there is no replay).
        self.push_notices(&snapshot.notices);

        let old_scope = self.scope();
        let old_entries = self.file_entries();
        let old_file_pos = self
            .file_sel
            .as_ref()
            .and_then(|sel| old_entries.iter().position(|entry| entry == sel));
        let old_commit = self.snapshot.as_ref().and_then(|snapshot| {
            snapshot
                .recent_commits
                .get(self.commit_row)
                .map(|commit| commit.short_id.clone())
        });
        // The hunk-mode selection's identity: its verbatim lines (the
        // membership-anchor shape) plus position for tie-breaking.
        let old_hunk = self.hunk_sel.and_then(|index| {
            self.selected_file()
                .and_then(|file| file.hunks.get(index))
                .map(|hunk| (hunk.lines.clone(), hunk.new_start))
        });

        self.snapshot = Some(snapshot);

        // Changelist selection: by scope identity, else nearest sibling.
        let rows = self.changelist_rows();
        self.changelist_row = rows
            .iter()
            .position(|row| row.scope == old_scope)
            .unwrap_or_else(|| self.changelist_row.min(rows.len().saturating_sub(1)));

        // File selection: exact (group, path) → same path elsewhere →
        // nearest sibling at the old visual position.
        let entries = self.file_entries();
        let survived = self.file_sel.take().and_then(|sel| {
            if entries.contains(&sel) {
                Some(sel)
            } else {
                entries.iter().find(|entry| entry.path == sel.path).cloned()
            }
        });
        self.file_sel = match survived {
            Some(sel) => Some(sel),
            None => {
                self.diff_scroll = 0;
                let position = old_file_pos.unwrap_or(0);
                entries
                    .get(position.min(entries.len().saturating_sub(1)))
                    .cloned()
            }
        };

        // Hunk-mode selection: by content match (same lines, nearest
        // start), else clamped; hunk mode ends when nothing selectable
        // remains.
        if let Some(index) = self.hunk_sel {
            let hunks = self
                .selected_file()
                .map(|file| &file.hunks[..])
                .unwrap_or_default();
            self.hunk_sel = if hunks.is_empty() {
                if self.focus == Panel::Diff {
                    self.focus = Panel::Files;
                }
                None
            } else {
                let survived = old_hunk.as_ref().and_then(|(lines, new_start)| {
                    hunks
                        .iter()
                        .enumerate()
                        .filter(|(_, hunk)| hunk.lines == *lines)
                        .min_by_key(|(_, hunk)| hunk.new_start.abs_diff(*new_start))
                        .map(|(position, _)| position)
                });
                Some(survived.unwrap_or_else(|| index.min(hunks.len() - 1)))
            };
        }

        // Commit selection: by id, else clamped.
        let commits = self
            .snapshot
            .as_ref()
            .map(|snapshot| &snapshot.recent_commits[..])
            .unwrap_or_default();
        self.commit_row = old_commit
            .and_then(|id| commits.iter().position(|commit| commit.short_id == id))
            .unwrap_or_else(|| self.commit_row.min(commits.len().saturating_sub(1)));
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
        match key.code {
            KeyCode::Char('q') => return Some(Action::Quit),
            KeyCode::Char('R') => return Some(Action::Refresh),
            KeyCode::Char('?') => self.help_open = true,
            KeyCode::Char('1') => self.set_focus(Panel::Status),
            KeyCode::Char('2') => self.set_focus(Panel::Changelists),
            KeyCode::Char('3') => self.focus_files(),
            KeyCode::Char('4') => self.set_focus(Panel::Commits),
            KeyCode::Char('5') => self.set_focus(Panel::Log),
            // Explicit Diff focus is scroll mode; hunk mode only enters
            // through `enter` on a file.
            KeyCode::Char('0') => self.set_focus(Panel::Diff),
            KeyCode::Char('j') | KeyCode::Down => self.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_selection(-1),
            KeyCode::Char('n') => {
                self.overlay = Some(Overlay::Input {
                    kind: InputKind::NewChangelist,
                    value: String::new(),
                });
            }
            // The assign keymap (ADR 0013), scope escalating with
            // keypress difficulty. `ctrl+a` is matched shift-agnostically
            // so `ctrl+shift+a` — which most terminals cannot tell apart
            // from it — does the same thing rather than something else.
            KeyCode::Char('a' | 'A') if ctrl => {
                if let Some(path) = self.assign_file_path() {
                    self.open_assign(AssignPayload::AllHunks { path });
                }
            }
            KeyCode::Char('a') => {
                if let Some(payload) = self.selected_assign_payload() {
                    self.open_assign(payload);
                }
            }
            KeyCode::Char('A') => {
                if let Some(path) = self.assign_file_path() {
                    self.open_assign(AssignPayload::UnassignedHunks { path });
                }
            }
            KeyCode::Char('s') => {
                if let Some(name) = self.scoped_changelist() {
                    return Some(Action::Op(Op::SetActive { name }));
                }
            }
            KeyCode::Char('r') => {
                if let Some(name) = self.scoped_changelist() {
                    self.overlay = Some(Overlay::Input {
                        kind: InputKind::Rename { from: name.clone() },
                        value: name,
                    });
                }
            }
            KeyCode::Char('d') => {
                if let Some(name) = self.scoped_changelist() {
                    self.overlay = Some(Overlay::ConfirmDelete { name });
                }
            }
            KeyCode::Char(' ') => return self.stage_toggle(),
            KeyCode::Char('c') => {
                // The operation guard (ADR 0007): while a git operation
                // is in progress `c` is a soft no-op — the pin names the
                // operation, this line says why nothing opened.
                if let Some(operation) = self.snapshot.as_ref().and_then(|s| s.operation) {
                    self.push_log(
                        Severity::Info,
                        format!(
                            "{} in progress — conclude or abort it first",
                            operation.label()
                        ),
                    );
                    return None;
                }
                // `c` on the All view is a polite no-op with a message
                // (ADR 0004) — commit needs one changelist scoped;
                // unassigned is committable like any changelist.
                match self.scope() {
                    Scope::All | Scope::Conflicts => {
                        self.push_log(
                            Severity::Info,
                            "select a changelist to commit — all is a view",
                        );
                    }
                    Scope::Changelist(name) => {
                        return Some(Action::Commit(CommitStep::Open {
                            changelist: Some(name),
                        }));
                    }
                    Scope::Unassigned => {
                        return Some(Action::Commit(CommitStep::Open { changelist: None }));
                    }
                }
            }
            KeyCode::Enter => match self.focus {
                // Drill-down only: 2 → select changelist → 3 → files →
                // hunk mode. `enter` assigns nothing anywhere — the Diff
                // panel is the end of the drill, and the key it used to
                // share with assign was the misfire ADR 0013 bars.
                Panel::Changelists => self.focus_files(),
                Panel::Files => self.enter_hunk_mode(),
                Panel::Diff => {}
                _ => {}
            },
            KeyCode::Esc => match self.focus {
                Panel::Diff => {
                    self.hunk_sel = None;
                    self.focus = Panel::Files;
                }
                Panel::Files => self.focus = Panel::Changelists,
                Panel::Changelists => self.select_changelist_row(0),
                _ => {}
            },
            _ => {}
        }
        None
    }

    /// Route a key into the open overlay. Overlays swallow everything;
    /// the ones producing an [`Op`] close as they return it.
    fn on_overlay_key(&mut self, key: KeyEvent) -> Option<Action> {
        let overlay = self.overlay.take()?;
        match overlay {
            Overlay::Input { kind, mut value } => match key.code {
                KeyCode::Esc => {
                    // The escape hatch returns to the popup it left.
                    if let InputKind::NewChangelistForAssign { payload } = kind {
                        let row = self.assign_rows().len().saturating_sub(1);
                        self.overlay = Some(Overlay::Assign { payload, row });
                    }
                    None
                }
                KeyCode::Enter => {
                    let name = value.trim().to_owned();
                    if name.is_empty() {
                        self.overlay = Some(Overlay::Input { kind, value });
                        return None;
                    }
                    match kind {
                        InputKind::NewChangelist => Some(Action::Op(Op::CreateChangelist { name })),
                        InputKind::Rename { from } => {
                            if from == name {
                                return None;
                            }
                            Some(Action::Op(Op::RenameChangelist { from, to: name }))
                        }
                        InputKind::NewChangelistForAssign { payload } => {
                            self.assign_op(payload, name, true)
                        }
                    }
                }
                KeyCode::Backspace => {
                    value.pop();
                    self.overlay = Some(Overlay::Input { kind, value });
                    None
                }
                KeyCode::Char(c) => {
                    value.push(c);
                    self.overlay = Some(Overlay::Input { kind, value });
                    None
                }
                _ => {
                    self.overlay = Some(Overlay::Input { kind, value });
                    None
                }
            },
            Overlay::ConfirmDelete { name } => match key.code {
                KeyCode::Enter => Some(Action::Op(Op::DeleteChangelist { name })),
                KeyCode::Esc => None,
                _ => {
                    self.overlay = Some(Overlay::ConfirmDelete { name });
                    None
                }
            },
            Overlay::Commit(draft) => self.on_commit_dialog_key(key, draft),
            Overlay::CommitStale(draft) => match key.code {
                // Commit as-is: ◑ content ships exactly as the index
                // holds it (ADR 0004 — never silent, never blocking).
                KeyCode::Enter => Some(Action::Commit(CommitStep::Commit(draft))),
                KeyCode::Char('w') => Some(Action::Commit(CommitStep::AlignAndCommit(draft))),
                KeyCode::Esc => {
                    self.overlay = Some(Overlay::Commit(draft));
                    None
                }
                _ => {
                    self.overlay = Some(Overlay::CommitStale(draft));
                    None
                }
            },
            Overlay::CommitStageAll {
                changelist,
                hunks,
                files,
            } => match key.code {
                KeyCode::Enter => Some(Action::Commit(CommitStep::StageAllAndOpen { changelist })),
                KeyCode::Esc => None,
                _ => {
                    self.overlay = Some(Overlay::CommitStageAll {
                        changelist,
                        hunks,
                        files,
                    });
                    None
                }
            },
            Overlay::CommitDrift { draft, previous } => match key.code {
                // The fresh payload was shown; committing it is the
                // re-confirmation (message kept in the draft).
                KeyCode::Enter => Some(Action::Commit(CommitStep::Commit(draft))),
                KeyCode::Char('e') => {
                    self.overlay = Some(Overlay::Commit(draft));
                    None
                }
                KeyCode::Esc => None,
                _ => {
                    self.overlay = Some(Overlay::CommitDrift { draft, previous });
                    None
                }
            },
            Overlay::Assign { payload, row } => {
                let rows = self.assign_rows();
                let row = row.min(rows.len().saturating_sub(1));
                match key.code {
                    KeyCode::Esc => None,
                    KeyCode::Char('j') | KeyCode::Down => {
                        let row = step(row, 1, rows.len());
                        self.overlay = Some(Overlay::Assign { payload, row });
                        None
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        let row = step(row, -1, rows.len());
                        self.overlay = Some(Overlay::Assign { payload, row });
                        None
                    }
                    KeyCode::Enter => match rows.into_iter().nth(row) {
                        Some(AssignRow::Changelist { name, .. }) => {
                            self.assign_op(payload, name, false)
                        }
                        Some(AssignRow::CreateNew) => {
                            self.overlay = Some(Overlay::Input {
                                kind: InputKind::NewChangelistForAssign { payload },
                                value: String::new(),
                            });
                            None
                        }
                        None => None,
                    },
                    _ => {
                        self.overlay = Some(Overlay::Assign { payload, row });
                        None
                    }
                }
            }
        }
    }

    /// The commit dialog's keys (prototype variant A): `enter` commit
    /// (from the message; in the body it breaks the line), `tab` toggles
    /// message/body, `ctrl+n`/`ctrl+a` toggle the flags, `esc` cancels.
    fn on_commit_dialog_key(&mut self, key: KeyEvent, mut draft: CommitDraft) -> Option<Action> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => return None,
            KeyCode::Tab => draft.body_focus = !draft.body_focus,
            // Shift-agnostic like every other `ctrl+<letter>` (ADR 0013).
            KeyCode::Char('n' | 'N') if ctrl => draft.no_verify = !draft.no_verify,
            KeyCode::Char('a' | 'A') if ctrl => draft.amend = !draft.amend,
            KeyCode::Enter if draft.body_focus => draft.body.push('\n'),
            KeyCode::Enter => {
                // An empty subject would only bounce off `git commit`;
                // ignore it like the input overlays do.
                if draft.message.trim().is_empty() {
                    self.overlay = Some(Overlay::Commit(draft));
                    return None;
                }
                // ◑ in the payload → warn-and-confirm first (variant B);
                // otherwise straight to the commit step.
                if draft.payload.stale_hunks() > 0 {
                    self.overlay = Some(Overlay::CommitStale(draft));
                    return None;
                }
                return Some(Action::Commit(CommitStep::Commit(draft)));
            }
            KeyCode::Backspace => {
                if draft.body_focus {
                    draft.body.pop();
                } else {
                    draft.message.pop();
                }
            }
            KeyCode::Char(c) if !ctrl => {
                if draft.body_focus {
                    draft.body.push(c);
                } else {
                    draft.message.push(c);
                }
            }
            _ => {}
        }
        self.overlay = Some(Overlay::Commit(draft));
        None
    }

    // ── commit-flow outcomes (fed back by the main loop) ────────────

    /// A derived payload arrived for `c`'s dialog (variant A). Empty
    /// payloads route through [`App::offer_stage_all`] instead.
    pub fn open_commit_dialog(&mut self, changelist: Option<String>, payload: CommitPayload) {
        self.overlay = Some(Overlay::Commit(CommitDraft::new(changelist, payload)));
    }

    /// The payload came back empty: offer stage-all-and-commit
    /// (variant C) when the changelist has hunks at all, otherwise say
    /// why nothing opened.
    pub fn offer_stage_all(&mut self, changelist: Option<String>) {
        let owner = changelist.as_deref();
        let mut hunks = 0;
        let mut files = 0;
        for file in self
            .snapshot
            .as_ref()
            .map(|snapshot| &snapshot.files[..])
            .unwrap_or_default()
        {
            let owned = file
                .hunks
                .iter()
                .filter(|hunk| hunk.changelist.as_deref() == owner)
                .count();
            if owned > 0 {
                files += 1;
                hunks += owned;
            }
        }
        if hunks == 0 {
            let label = owner.unwrap_or("unassigned");
            self.push_log(Severity::Info, format!("nothing to commit in '{label}'"));
            return;
        }
        self.overlay = Some(Overlay::CommitStageAll {
            changelist,
            hunks,
            files,
        });
    }

    /// The refresh-before-commit guard tripped (ADR 0004): re-confirm
    /// against the fresh payload, message and flags kept (variant D).
    pub fn commit_drifted(&mut self, mut draft: CommitDraft, fresh: CommitPayload) {
        let previous = std::mem::replace(&mut draft.payload, fresh);
        self.overlay = Some(Overlay::CommitDrift { draft, previous });
    }

    /// A failed commit (hook rejection, anything git refused) restores
    /// the dialog exactly as confirmed — losing a composed message to a
    /// linter complaint is rage-inducing (ADR 0007). The rejection modal
    /// lands on top via [`App::show_error`]; retry is `enter` again, or
    /// `ctrl+n` for `--no-verify`, both already in the dialog.
    pub fn restore_commit_dialog(&mut self, draft: CommitDraft) {
        self.overlay = Some(Overlay::Commit(draft));
    }

    /// The align-and-commit path found ◑ hunks *still* in the payload
    /// (align is fail-soft, and edits can land mid-flow): re-warn
    /// rather than commit content the user hasn't seen flagged —
    /// ADR 0004's warn-and-confirm is never silent.
    pub fn reconfirm_stale(&mut self, draft: CommitDraft) {
        self.overlay = Some(Overlay::CommitStale(draft));
    }

    /// Success closes the flow. No toast — the new commit appearing in
    /// the Commits panel is the feedback.
    pub fn commit_succeeded(&mut self) {
        self.overlay = None;
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

    /// `enter` on a file (prototype variant C): focus the Diff panel
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

    /// The changelist the ops keys (`d`/`r`/`s`) act on: the selected
    /// Changelists row while it names one, from the panels that show it.
    fn scoped_changelist(&self) -> Option<String> {
        if !matches!(self.focus, Panel::Changelists | Panel::Files) {
            return None;
        }
        match self.scope() {
            Scope::Changelist(name) => Some(name),
            Scope::All | Scope::Conflicts | Scope::Unassigned => None,
        }
    }

    /// `space`'s decide-by-current-state toggle (ticket #33): in hunk
    /// mode the selected hunk (`○`/`◑` → stage, `●` → unstage), in the
    /// Files panel the whole file (`●` → unstage, else stage). Core
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
        if self.focus != Panel::Files {
            return None;
        }
        let file = self.selected_file()?;
        let path = file.path.clone();
        // `space` on quarantined content politely refuses (ADR 0007) —
        // three index stages are a workflow gitchange doesn't serve.
        if file.kind == ChangeKind::Conflicted {
            self.push_log(
                Severity::Info,
                format!("{path} is conflicted — resolve outside gitchange"),
            );
            return None;
        }
        let op = match file.stage() {
            FileStage::Staged => Op::UnstageFile { path },
            FileStage::Unstaged | FileStage::PartiallyStaged => Op::StageFile { path },
        };
        Some(Action::Op(op))
    }

    /// What `a` assigns: the Files row's group-owned hunks from the Files
    /// panel, the hunk-mode selection otherwise — including while that
    /// selection is blurred, the cross-panel reach issue #45 keeps its
    /// cursor visible for.
    fn selected_assign_payload(&self) -> Option<AssignPayload> {
        if self.focus == Panel::Files {
            return self.file_sel.clone().map(AssignPayload::FileRow);
        }
        let index = self.hunk_sel?;
        let file = self.selected_file()?;
        file.hunks
            .get(index)
            .cloned()
            .map(|hunk| AssignPayload::Hunk {
                path: file.path.clone(),
                hunk,
            })
    }

    /// The file the file-scoped assign keys (`A`, `ctrl+a`) act on: the
    /// Files selection, which is also what the Diff panel renders and
    /// what hunk mode is drilled into. `None` from the panels that show
    /// no file, unless a hunk selection is still live there (issue #45).
    fn assign_file_path(&self) -> Option<String> {
        if !matches!(self.focus, Panel::Files | Panel::Diff) && self.hunk_sel.is_none() {
            return None;
        }
        self.selected_file().map(|file| file.path.clone())
    }

    /// Open the assign popup when the payload has hunks in it. An empty
    /// payload — `A` on a file with nothing unassigned, most often — is a
    /// polite no-op with a Log line, never an error modal (ADR 0007).
    fn open_assign(&mut self, payload: AssignPayload) {
        let hunks = self
            .resolve_payload(&payload)
            .map_or(0, |(_, hunks)| hunks.len());
        if hunks > 0 {
            self.overlay = Some(Overlay::Assign { payload, row: 0 });
        } else {
            let reason = self.empty_payload_reason(&payload);
            self.push_log(Severity::Info, reason);
        }
    }

    /// Why nothing opened, in the Log's voice: which scope came up empty
    /// on which file. A quarantined path says so instead — it holds no
    /// assignable hunks by construction, and "no hunks" alone would read
    /// as a bug next to the conflict placeholder in the Diff panel.
    fn empty_payload_reason(&self, payload: &AssignPayload) -> String {
        let path = payload.path();
        let conflicted = self.snapshot.as_ref().is_some_and(|snapshot| {
            snapshot
                .files
                .iter()
                .any(|file| file.path == path && file.kind == ChangeKind::Conflicted)
        });
        if conflicted {
            return format!("{path} is conflicted — resolve outside gitchange");
        }
        match payload {
            AssignPayload::FileRow(entry) => format!(
                "no hunks in '{}' for {path} — nothing to assign",
                entry.group.title(),
            ),
            AssignPayload::UnassignedHunks { .. } => {
                format!("no unassigned hunks in {path} — nothing to assign")
            }
            // A selected hunk resolves without consulting the snapshot,
            // so it only lands here with no snapshot at all — which is
            // also the only way the file scopes reach this arm.
            AssignPayload::Hunk { .. } | AssignPayload::AllHunks { .. } => {
                format!("no hunks in {path} — nothing to assign")
            }
        }
    }

    /// Resolve a payload against the current snapshot and emit the assign
    /// op. A payload that no longer resolves closes silently — the op
    /// content-validates again at apply anyway.
    fn assign_op(
        &mut self,
        payload: AssignPayload,
        target: String,
        create: bool,
    ) -> Option<Action> {
        let (path, hunks) = self.resolve_payload(&payload)?;
        if hunks.is_empty() {
            return None;
        }
        Some(Action::Op(Op::Assign {
            path,
            hunks,
            target,
            create,
        }))
    }

    fn resolve_payload(&self, payload: &AssignPayload) -> Option<(String, Vec<Hunk>)> {
        let snapshot = self.snapshot.as_ref()?;
        let path = payload.path();
        let file = || snapshot.files.iter().find(|file| file.path == path);
        let hunks = match payload {
            AssignPayload::Hunk { hunk, .. } => vec![hunk.clone()],
            AssignPayload::AllHunks { .. } => file()?.hunks.clone(),
            AssignPayload::UnassignedHunks { .. } => owned_hunks(file()?, None),
            AssignPayload::FileRow(entry) => owned_hunks(file()?, entry.group.owner()?),
        };
        Some((path.to_owned(), hunks))
    }

    fn move_selection(&mut self, delta: isize) {
        match self.focus {
            Panel::Changelists => {
                let rows = self.changelist_rows().len();
                let row = step(self.changelist_row, delta, rows);
                self.select_changelist_row(row);
            }
            Panel::Files => {
                let entries = self.file_entries();
                let position = self
                    .file_sel
                    .as_ref()
                    .and_then(|sel| entries.iter().position(|entry| entry == sel))
                    .unwrap_or(0);
                let next = entries.get(step(position, delta, entries.len())).cloned();
                if next != self.file_sel {
                    self.diff_scroll = 0;
                    // A blurred hunk selection belongs to the old file.
                    self.hunk_sel = None;
                }
                self.file_sel = next;
            }
            Panel::Commits => {
                let commits = self
                    .snapshot
                    .as_ref()
                    .map_or(0, |snapshot| snapshot.recent_commits.len());
                self.commit_row = step(self.commit_row, delta, commits);
            }
            Panel::Diff => {
                // Hunk mode: j/k walk hunks (the renderer keeps the
                // selection visible); scroll mode: line scrolling,
                // clamped to the content so `j` can't scroll into blank
                // space indefinitely.
                if let Some(index) = self.hunk_sel {
                    let hunks = self.selected_file().map_or(0, |file| file.hunks.len());
                    self.hunk_sel = Some(step(index, delta, hunks));
                } else {
                    let max = self.diff_lines().len().saturating_sub(1) as u16;
                    self.diff_scroll = if delta > 0 {
                        self.diff_scroll.saturating_add(1).min(max)
                    } else {
                        self.diff_scroll.saturating_sub(1)
                    };
                }
            }
            Panel::Log => {
                // Offset from the stream's bottom: j back toward the
                // newest entry, k into history.
                self.log_scroll = if delta > 0 {
                    self.log_scroll.saturating_sub(1)
                } else {
                    (self.log_scroll + 1).min(self.log.len().saturating_sub(1))
                };
            }
            Panel::Status => {}
        }
    }

    /// Select a Changelists row, rescoping the Files panel: selection
    /// moves to the new scope's first file.
    fn select_changelist_row(&mut self, row: usize) {
        if row == self.changelist_row {
            return;
        }
        self.changelist_row = row;
        self.diff_scroll = 0;
        self.hunk_sel = None;
        self.file_sel = self.file_entries().first().cloned();
    }

    // ── view-models ─────────────────────────────────────────────────

    pub fn scope(&self) -> Scope {
        self.changelist_rows()
            .get(self.changelist_row)
            .map(|row| row.scope.clone())
            .unwrap_or(Scope::All)
    }

    /// Changelists panel rows: `all`, user changelists in user order,
    /// `unassigned` pinned last.
    pub fn changelist_rows(&self) -> Vec<ChangelistRow> {
        let Some(snapshot) = &self.snapshot else {
            return vec![
                ChangelistRow {
                    scope: Scope::All,
                    count: 0,
                    active: false,
                },
                ChangelistRow {
                    scope: Scope::Unassigned,
                    count: 0,
                    active: false,
                },
            ];
        };
        let mut rows = vec![ChangelistRow {
            scope: Scope::All,
            count: snapshot.files.len(),
            active: false,
        }];
        for changelist in &snapshot.changelists {
            rows.push(ChangelistRow {
                scope: Scope::Changelist(changelist.name.clone()),
                count: snapshot.files_in(Some(&changelist.name)).len(),
                active: snapshot.active.as_deref() == Some(changelist.name.as_str()),
            });
        }
        rows.push(ChangelistRow {
            scope: Scope::Unassigned,
            count: snapshot.files_in(None).len(),
            active: false,
        });
        rows
    }

    /// Files panel rows for the current scope. All: groups in changelist
    /// order with the unassigned group last only when non-empty —
    /// `gitchange status`'s grouping semantics. Drilled: a flat list.
    pub fn files_rows(&self) -> Vec<FilesRow> {
        let Some(snapshot) = &self.snapshot else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        match self.scope() {
            Scope::All => {
                // The Conflicts group renders first (ADR 0007): live,
                // shrinking in real time as files are resolved.
                let conflicted = snapshot.conflicted_files();
                if !conflicted.is_empty() {
                    rows.push(FilesRow::Header {
                        label: "conflicts".into(),
                        count: conflicted.len(),
                        unassigned: false,
                        conflicted: true,
                        active: false,
                    });
                    for file in conflicted {
                        rows.push(file_row(file, Scope::Conflicts, true));
                    }
                }
                for changelist in &snapshot.changelists {
                    let files = snapshot.files_in(Some(&changelist.name));
                    rows.push(FilesRow::Header {
                        label: changelist.name.clone(),
                        count: files.len(),
                        unassigned: false,
                        conflicted: false,
                        active: snapshot.active.as_deref() == Some(changelist.name.as_str()),
                    });
                    let group = Scope::Changelist(changelist.name.clone());
                    for file in files {
                        rows.push(file_row(file, group.clone(), true));
                    }
                }
                let unassigned = snapshot.files_in(None);
                if !unassigned.is_empty() {
                    rows.push(FilesRow::Header {
                        label: "unassigned".into(),
                        count: unassigned.len(),
                        unassigned: true,
                        conflicted: false,
                        active: false,
                    });
                    for file in unassigned {
                        rows.push(file_row(file, Scope::Unassigned, true));
                    }
                }
            }
            scope @ (Scope::Changelist(_) | Scope::Unassigned) => {
                let name = match &scope {
                    Scope::Changelist(name) => Some(name.as_str()),
                    _ => None,
                };
                for file in snapshot.files_in(name) {
                    rows.push(file_row(file, scope.clone(), false));
                }
            }
            // Never a Changelists-panel row, so never a drill scope.
            Scope::Conflicts => {}
        }
        rows
    }

    /// The selectable file entries, in row order.
    pub fn file_entries(&self) -> Vec<FileEntry> {
        self.files_rows()
            .into_iter()
            .filter_map(|row| match row {
                FilesRow::File { entry, .. } => Some(entry),
                FilesRow::Header { .. } => None,
            })
            .collect()
    }

    /// The selected file's position among selectable entries, 1-based,
    /// for the panel's `n of m` count.
    pub fn files_count(&self) -> (usize, usize) {
        let entries = self.file_entries();
        let position = self
            .file_sel
            .as_ref()
            .and_then(|sel| entries.iter().position(|entry| entry == sel))
            .map_or(0, |position| position + 1);
        (position, entries.len())
    }

    pub fn selected_file(&self) -> Option<&ChangedFile> {
        let sel = self.file_sel.as_ref()?;
        self.snapshot
            .as_ref()?
            .files
            .iter()
            .find(|file| file.path == sel.path)
    }

    /// The Diff panel's title context: file name plus staged counts —
    /// scoped to the drilled changelist's own hunks, with a dim note of
    /// how many hunks live elsewhere.
    pub fn diff_title(&self) -> String {
        let Some(file) = self.selected_file() else {
            return String::new();
        };
        let name = file.path.rsplit('/').next().unwrap_or(&file.path);
        if file.kind == ChangeKind::Conflicted {
            return format!("{name} (conflicted)");
        }
        // Hunk mode replaces the staged counts with the selection
        // position (prototype variant C).
        if let Some(index) = self.hunk_sel {
            return format!("{name} — hunk {} of {}", index + 1, file.total_hunks());
        }
        match self.scope().owner() {
            None => format!(
                "{name} ({}/{} staged)",
                file.staged_hunks(),
                file.total_hunks()
            ),
            Some(owner) => {
                let own: Vec<_> = file
                    .hunks
                    .iter()
                    .filter(|hunk| hunk.changelist.as_deref() == owner)
                    .collect();
                let staged_own = own
                    .iter()
                    .filter(|hunk| hunk.stage == HunkStage::Staged)
                    .count();
                let elsewhere = file.total_hunks() - own.len();
                let mut title = format!("{name} ({staged_own}/{} staged", own.len());
                if elsewhere > 0 {
                    let plural = if elsewhere == 1 { "hunk" } else { "hunks" };
                    title.push_str(&format!(" · {elsewhere} {plural} elsewhere"));
                }
                title.push(')');
                title
            }
        }
    }

    /// The Diff panel body for the selected file. All view: every hunk
    /// tagged with its owning changelist. Drilled: own hunks untagged,
    /// foreign hunks dimmed with a dim tag.
    pub fn diff_lines(&self) -> Vec<DiffLine> {
        let Some(file) = self.selected_file() else {
            return Vec::new();
        };
        let mut lines = vec![
            DiffLine::FileHeader(format!("--- a/{}", file.path)),
            DiffLine::FileHeader(format!("+++ b/{}", file.path)),
        ];
        if file.kind == ChangeKind::Conflicted {
            lines.push(DiffLine::Conflict(
                "conflicted — resolve outside gitchange".into(),
            ));
            return lines;
        }
        if file.binary {
            // The one-line sized placeholder (ADR 0009) — the text is
            // what says "binary"; no new glyphs.
            lines.push(DiffLine::Placeholder(binary_placeholder(file)));
            return lines;
        }
        if file.hunks.is_empty() {
            lines.push(DiffLine::Placeholder("no hunks".into()));
            return lines;
        }
        let scope = self.scope();
        let owner = scope.owner();
        for (index, hunk) in file.hunks.iter().enumerate() {
            let foreign = owner.is_some_and(|owner| !owns(hunk, owner));
            let selected = self.hunk_sel == Some(index);
            let tag = if owner.is_none() || foreign {
                let unassigned = hunk.changelist.is_none();
                Some(HunkTag {
                    label: hunk
                        .changelist
                        .clone()
                        .unwrap_or_else(|| "unassigned".into()),
                    stage: (!unassigned).then_some(hunk.stage),
                    unassigned,
                    dim: foreign,
                })
            } else {
                None
            };
            if index > 0 {
                lines.push(DiffLine::Spacer);
            }
            lines.push(DiffLine::HunkHeader {
                text: format!(
                    "@@ -{},{} +{},{} @@",
                    hunk.old_start, hunk.old_lines, hunk.new_start, hunk.new_lines
                ),
                tag,
                foreign,
                selected,
            });
            for line in &hunk.lines {
                lines.push(DiffLine::Content {
                    origin: line.origin,
                    text: line.content.trim_end_matches('\n').to_owned(),
                    foreign,
                    selected,
                });
            }
        }
        lines
    }

    /// The selected hunk's header position within [`App::diff_lines`],
    /// for the renderer's keep-visible scroll in hunk mode.
    pub fn selected_hunk_line(&self) -> Option<usize> {
        self.hunk_sel?;
        self.diff_lines()
            .iter()
            .position(|line| matches!(line, DiffLine::HunkHeader { selected: true, .. }))
    }

    /// The assign popup's rows (prototype variant D): every changelist,
    /// the active one annotated, then the create-new escape hatch.
    pub fn assign_rows(&self) -> Vec<AssignRow> {
        let mut rows: Vec<AssignRow> = self
            .snapshot
            .as_ref()
            .map(|snapshot| {
                snapshot
                    .changelists
                    .iter()
                    .map(|changelist| AssignRow::Changelist {
                        name: changelist.name.clone(),
                        active: snapshot.active.as_deref() == Some(changelist.name.as_str()),
                    })
                    .collect()
            })
            .unwrap_or_default();
        rows.push(AssignRow::CreateNew);
        rows
    }

    /// The assign popup's subject line: the payload stated before it
    /// lands — how many hunks, in which file, and which other changelists
    /// they are coming out of. `A` and `ctrl+a` are allowed to reach
    /// hunks rendered on another row *because* this line names them; drop
    /// it and the keymap is back to filing hunks silently.
    pub fn assign_description(&self, payload: &AssignPayload) -> String {
        let Some((path, hunks)) = self.resolve_payload(payload) else {
            return self.empty_payload_reason(payload);
        };
        // The `A` scope's noun already names where its hunks come from, so
        // it takes no provenance tail — "1 unassigned hunk (1 from
        // unassigned)" says one thing twice.
        let (noun, provenance) = match payload {
            AssignPayload::UnassignedHunks { .. } => ("unassigned hunk", Vec::new()),
            _ => ("hunk", self.assign_provenance(&hunks)),
        };
        let mut line = format!("{} in {path}", count_noun(hunks.len(), noun));
        if let AssignPayload::Hunk { hunk, .. } = payload {
            line.push_str(&format!(
                " @@ -{},{} +{},{}",
                hunk.old_start, hunk.old_lines, hunk.new_start, hunk.new_lines
            ));
        }
        if !provenance.is_empty() {
            let sources: Vec<String> = provenance
                .iter()
                .map(|(label, count)| {
                    if hunks.len() == 1 {
                        format!("from {label}")
                    } else {
                        format!("{count} from {label}")
                    }
                })
                .collect();
            line.push_str(&format!(" ({})", sources.join(", ")));
        }
        line
    }

    /// The changelist an assign payload is stated relative to: the
    /// drilled changelist, or — in the All view, where the panel shows no
    /// single owner — the selected row's own group, so off-row reach is
    /// named there too. `Some(None)` is unassigned; the outer `None` means
    /// no owner is in view at all (nothing to call foreign).
    fn assign_owner(&self) -> Option<Option<String>> {
        let scope = self.scope();
        if let Some(owner) = scope.owner() {
            return Some(owner.map(str::to_owned));
        }
        let group = &self.file_sel.as_ref()?.group;
        Some(group.owner()?.map(str::to_owned))
    }

    /// The payload's foreign sources as `(label, count)` in payload
    /// order: the changelists owning payload hunks that the row the user
    /// is standing on does not. Foreign is the diff view-model's own
    /// notion, owner-relative through [`App::assign_owner`].
    fn assign_provenance(&self, hunks: &[Hunk]) -> Vec<(String, usize)> {
        let Some(owner) = self.assign_owner() else {
            return Vec::new();
        };
        let mut sources: Vec<(String, usize)> = Vec::new();
        for hunk in hunks {
            if owns(hunk, owner.as_deref()) {
                continue;
            }
            let label = match &hunk.changelist {
                Some(name) => format!("'{name}'"),
                None => "unassigned".to_owned(),
            };
            match sources.iter_mut().find(|(seen, _)| *seen == label) {
                Some((_, count)) => *count += 1,
                None => sources.push((label, 1)),
            }
        }
        sources
    }

    /// Append one event to the Log panel's stream (ADR 0007), keeping a
    /// capped tail.
    pub fn push_log(&mut self, severity: Severity, text: impl Into<String>) {
        self.log.push(LogEntry {
            severity,
            text: text.into(),
        });
        let overflow = self.log.len().saturating_sub(LOG_CAP);
        if overflow > 0 {
            self.log.drain(..overflow);
        }
    }

    /// Append core notices in the Log vocabulary — the path every
    /// fail-soft op outcome takes.
    pub fn push_notices<'a>(&mut self, notices: impl IntoIterator<Item = &'a Notice>) {
        for notice in notices {
            let entry = notice_entry(notice);
            self.push_log(entry.severity, entry.text);
        }
    }

    /// The error-modal contract (ADR 0007): title names the operation,
    /// detail is verbatim and scrollable; every modal is also logged at
    /// `✗` so the record survives dismissal.
    pub fn show_error(&mut self, title: impl Into<String>, detail: impl Into<String>) {
        let title = title.into();
        let detail = detail.into();
        let first = detail
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or_default()
            .trim();
        let text = if first.is_empty() {
            title.clone()
        } else {
            format!("{title} — {first}")
        };
        self.push_log(Severity::Error, text);
        self.error_modal = Some(ErrorModal {
            title,
            detail,
            scroll: 0,
        });
    }

    /// Contextual keybar hints for the focused panel (staging and commit
    /// keys arrive with #33). Hunk mode swaps in its own bar (prototype
    /// variant C). The assign trio is one hint with its three scopes in
    /// key order — the bar has to match the keymap exactly (ADR 0013), and
    /// three separate hints crowd out the rest of the bar.
    pub fn key_hints(&self) -> Vec<(&'static str, &'static str)> {
        if self.hunk_mode_focused() {
            return vec![
                ("j/k", "next/prev hunk"),
                ("space", "stage/unstage hunk"),
                ("a/A/ctrl+a", "assign hunk / unassigned / all"),
                ("c", "commit changelist"),
                ("esc", "back to files"),
                ("?", "keybindings"),
            ];
        }
        let mut hints: Vec<(&'static str, &'static str)> = match self.focus {
            Panel::Changelists => vec![
                ("j/k", "move"),
                ("enter", "files"),
                ("n", "new"),
                ("d", "delete"),
                ("r", "rename"),
                ("s", "switch active"),
                ("c", "commit"),
            ],
            Panel::Files => vec![
                ("j/k", "move"),
                ("space", "stage file"),
                ("enter", "hunks"),
                ("a/A/ctrl+a", "assign group / unassigned / all"),
                ("c", "commit"),
                ("n", "new"),
                ("d", "delete"),
                ("r", "rename"),
                ("s", "switch active"),
                ("esc", "back"),
            ],
            Panel::Diff => vec![
                ("j/k", "scroll"),
                ("A/ctrl+a", "assign unassigned / all"),
                ("esc", "back"),
            ],
            Panel::Commits => vec![("j/k", "move")],
            Panel::Log => vec![("j/k", "scroll")],
            Panel::Status => Vec::new(),
        };
        // A hunk selection that survived a blur (issue #45) keeps the
        // assign keys live in whatever panel now holds focus, so the bar
        // has to say so there too — an advertised keymap that omits a live
        // key is the same dishonesty ADR 0013 bars in the other direction.
        if self.hunk_sel.is_some() && !matches!(self.focus, Panel::Files | Panel::Diff) {
            hints.push(("a/A/ctrl+a", "assign hunk / unassigned / all"));
        }
        hints.extend([
            ("0-5", "panels"),
            ("R", "refresh"),
            ("?", "keybindings"),
            ("q", "quit"),
        ]);
        hints
    }
}

/// A core notice in the Log vocabulary. Severity is assigned here — the
/// spec renders automatic membership decisions at `!`, fail-soft
/// stale-hunk outcomes included; core keeps severity out (ADR 0007).
pub fn notice_entry(notice: &Notice) -> LogEntry {
    let (severity, text) = match notice {
        Notice::AutoCaptured {
            path,
            new_start,
            changelist,
        } => (
            Severity::Notice,
            format!("auto-captured hunk at {path}:{new_start} → '{changelist}'"),
        ),
        Notice::AmbiguousOverlap {
            path,
            new_start,
            candidates,
            assigned_to,
        } => {
            let overlap = candidates
                .iter()
                .map(|name| format!("'{name}'"))
                .collect::<Vec<_>>()
                .join(", ");
            let text = match assigned_to {
                Some(name) => format!(
                    "auto-captured hunk at {path}:{new_start} → '{name}' (ambiguous overlap: {overlap})"
                ),
                None => format!(
                    "hunk at {path}:{new_start} left unassigned (ambiguous overlap: {overlap})"
                ),
            };
            (Severity::Notice, text)
        }
        Notice::DormantRevival {
            path,
            changelist,
            hunks,
        } => {
            let destination = match changelist {
                Some(name) => format!("'{name}'"),
                None => "unassigned".into(),
            };
            (
                Severity::Notice,
                format!(
                    "restored {} to {destination} — {path}",
                    count_noun(*hunks, "hunk")
                ),
            )
        }
        Notice::StaleHunk { path, new_start } => (
            Severity::Notice,
            format!("hunk at {path}:{new_start} changed since the last refresh; nothing applied"),
        ),
        Notice::HeadMoveDormancy { path, changelists } => {
            let list = changelists
                .iter()
                .map(|name| format!("'{name}'"))
                .collect::<Vec<_>>()
                .join(", ");
            (
                Severity::Notice,
                format!(
                    "external HEAD move changed {path} — records in {list} went dormant; affected hunks captured to active"
                ),
            )
        }
    };
    LogEntry { severity, text }
}

fn file_row(file: &ChangedFile, group: Scope, indent: bool) -> FilesRow {
    FilesRow::File {
        entry: FileEntry {
            group,
            path: file.path.clone(),
        },
        stage: file.stage(),
        kind: file.kind,
        staged: file.staged_hunks(),
        total: file.total_hunks(),
        indent,
    }
}

/// Whether `owner` (`None` = unassigned) owns `hunk` — the one ownership
/// test the diff view-model tags foreign hunks by and the assign payloads
/// select by, so the popup can never disagree with what the panel drew.
fn owns(hunk: &Hunk, owner: Option<&str>) -> bool {
    hunk.changelist.as_deref() == owner
}

/// A file's hunks owned by `owner`, in file order.
fn owned_hunks(file: &ChangedFile, owner: Option<&str>) -> Vec<Hunk> {
    file.hunks
        .iter()
        .filter(|hunk| owns(hunk, owner))
        .cloned()
        .collect()
}

/// The one-letter change-kind sigil the Files rows carry.
pub fn kind_sigil(kind: ChangeKind) -> char {
    match kind {
        ChangeKind::Added => 'A',
        ChangeKind::Modified => 'M',
        ChangeKind::Deleted => 'D',
        ChangeKind::TypeChanged => 'T',
        ChangeKind::Untracked => '?',
        ChangeKind::Conflicted => 'U',
    }
}

/// The Diff panel's one-line binary placeholder (ADR 0009):
/// `Binary file changed (12.4 KB → 15.1 KB)`, with added/deleted
/// variants showing the single existing side's size. Side presence, not
/// change kind, picks the variant — an index-only binary has no worktree
/// kind to trust.
fn binary_placeholder(file: &ChangedFile) -> String {
    let sides = file.binary_sides.as_ref();
    let head = sides.and_then(|sides| sides.head.as_ref());
    let changed = sides.and_then(|sides| sides.changed.as_ref());
    match (head, changed) {
        (Some(head), Some(changed)) => format!(
            "Binary file changed ({} → {})",
            human_size(head.size),
            human_size(changed.size)
        ),
        (None, Some(changed)) => format!("Binary file added ({})", human_size(changed.size)),
        (Some(head), None) => format!("Binary file deleted ({})", human_size(head.size)),
        (None, None) => "Binary file changed".into(),
    }
}

/// `12.4 KB`-style size, 1024-based, one decimal above bytes.
fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 3] = ["KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = None;
    for next in UNITS {
        if value < 1024.0 {
            break;
        }
        value /= 1024.0;
        unit = Some(next);
    }
    match unit {
        Some(unit) => format!("{value:.1} {unit}"),
        None => format!("{bytes} B"),
    }
}

/// Step an index by `delta`, clamped to `[0, len)`.
fn step(index: usize, delta: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    index.saturating_add_signed(delta).min(len - 1)
}

#[cfg(test)]
mod tests {
    use gitchange_core::{
        BinarySides, BlobInfo, Changelist, CommitInfo, Head, Hunk, HunkLine, OidAnchor, PayloadFile,
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
            stage,
            index_only: false,
            oid_anchor: None,
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
            notices: Vec::new(),
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
            (Scope::Changelist("chores".into()), "src/print.css"),
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
                DiffLine::Content { foreign: true, .. }
                    | DiffLine::HunkHeader { foreign: true, .. }
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
            app.log
                .iter()
                .any(|entry| entry.severity == Severity::Notice
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
    fn notices_land_in_the_log_at_notice_severity() {
        let mut app = app();
        let mut next = snapshot();
        next.notices = vec![gitchange_core::Notice::AutoCaptured {
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
        assert_eq!(entry.group, Scope::Conflicts);

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
            app.selected_file().unwrap().hunks[index].lines,
            before.lines
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
            ("app.rs", include_str!("app.rs")),
            ("ui.rs", include_str!("ui.rs")),
            ("lib.rs", include_str!("lib.rs")),
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
                .any(|entry| entry.severity == Severity::Info
                    && entry.text.contains("all is a view")),
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
        let Some(Action::Commit(CommitStep::Commit(draft))) = app.on_key(key(KeyCode::Enter))
        else {
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
                lines: Vec::new(),
                stage,
                index_only: false,
                oid_anchor: Some(OidAnchor {
                    head: head.map(|blob| blob.oid),
                    changed: changed.map(|blob| blob.oid),
                }),
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
}
