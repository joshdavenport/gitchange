//! The TUI's state machine, pure over [`Snapshot`]s: panel focus,
//! drill-down scope, selections and their identity-first survival across
//! snapshot swaps (ADR 0005), and the row/diff view-models the renderer
//! draws. No terminal, no git — everything here is unit-testable with
//! synthetic snapshots (ADR 0008).

use std::time::{Duration, Instant};

use gitchange_core::{ChangeKind, ChangedFile, FileStage, HunkStage, Snapshot};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// The deferred-indicator threshold (ADR 0005): refreshes shorter than
/// this show nothing.
pub const INDICATOR_DELAY: Duration = Duration::from_millis(500);

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
    Changelist(String),
    /// The unassigned pseudo-changelist, pinned last.
    Unassigned,
}

impl Scope {
    /// The hunk owner this scope drills into; `None` for All (no owner —
    /// nothing is foreign there).
    fn owner(&self) -> Option<Option<&str>> {
        match self {
            Scope::All => None,
            Scope::Changelist(name) => Some(Some(name)),
            Scope::Unassigned => Some(None),
        }
    }

    pub fn title(&self) -> &str {
        match self {
            Scope::All => "all changelists",
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
    },
    Content {
        origin: char,
        text: String,
        foreign: bool,
    },
    /// Blank spacing between hunks.
    Spacer,
    Placeholder(String),
}

/// What a key press asks the main loop to do beyond mutating the App.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Quit,
    /// The manual refresh key (ADR 0005).
    Refresh,
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
    pub help_open: bool,
    /// When the in-flight refresh started; drives the deferred
    /// indicator.
    refresh_started: Option<Instant>,
    /// The last hard refresh failure, kept alongside the still-valid
    /// snapshot. Ticket #34 owns the full error vocabulary; until then
    /// it renders as one line in the Log placeholder.
    pub last_refresh_error: Option<String>,
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
            help_open: false,
            refresh_started: None,
            last_refresh_error: None,
        }
    }

    // ── engine events ───────────────────────────────────────────────

    pub fn on_refresh_started(&mut self, now: Instant) {
        self.refresh_started = Some(now);
    }

    pub fn on_refresh_failed(&mut self, error: String) {
        self.refresh_started = None;
        self.last_refresh_error = Some(error);
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
        self.last_refresh_error = None;

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
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Some(Action::Quit);
        }
        if self.help_open {
            match key.code {
                KeyCode::Esc | KeyCode::Char('?') => self.help_open = false,
                KeyCode::Char('q') => return Some(Action::Quit),
                _ => {}
            }
            return None;
        }
        match key.code {
            KeyCode::Char('q') => return Some(Action::Quit),
            KeyCode::Char('R') => return Some(Action::Refresh),
            KeyCode::Char('?') => self.help_open = true,
            KeyCode::Char('1') => self.focus = Panel::Status,
            KeyCode::Char('2') => self.focus = Panel::Changelists,
            KeyCode::Char('3') => self.focus_files(),
            KeyCode::Char('4') => self.focus = Panel::Commits,
            KeyCode::Char('5') => self.focus = Panel::Log,
            KeyCode::Char('0') => self.focus = Panel::Diff,
            KeyCode::Char('j') | KeyCode::Down => self.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_selection(-1),
            KeyCode::Enter => {
                // Drill-down: 2 → select changelist → 3 → files. Enter
                // in Files (hunk mode) is ticket #32.
                if self.focus == Panel::Changelists {
                    self.focus_files();
                }
            }
            KeyCode::Esc => match self.focus {
                Panel::Diff => self.focus = Panel::Files,
                Panel::Files => self.focus = Panel::Changelists,
                Panel::Changelists => self.select_changelist_row(0),
                _ => {}
            },
            _ => {}
        }
        None
    }

    fn focus_files(&mut self) {
        self.focus = Panel::Files;
        if self.file_sel.is_none() {
            self.file_sel = self.file_entries().first().cloned();
        }
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
                // Clamp to the content so `j` can't scroll into blank
                // space indefinitely.
                let max = self.diff_lines().len().saturating_sub(1) as u16;
                self.diff_scroll = if delta > 0 {
                    self.diff_scroll.saturating_add(1).min(max)
                } else {
                    self.diff_scroll.saturating_sub(1)
                };
            }
            Panel::Status | Panel::Log => {}
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
                for changelist in &snapshot.changelists {
                    let files = snapshot.files_in(Some(&changelist.name));
                    rows.push(FilesRow::Header {
                        label: changelist.name.clone(),
                        count: files.len(),
                        unassigned: false,
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
        if file.binary {
            // The one-line sized summary is ticket #35 (ADR 0009).
            lines.push(DiffLine::Placeholder("binary file".into()));
            return lines;
        }
        if file.hunks.is_empty() {
            lines.push(DiffLine::Placeholder("no hunks".into()));
            return lines;
        }
        let scope = self.scope();
        let owner = scope.owner();
        for (index, hunk) in file.hunks.iter().enumerate() {
            let foreign = owner.is_some_and(|owner| hunk.changelist.as_deref() != owner);
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
            });
            for line in &hunk.lines {
                lines.push(DiffLine::Content {
                    origin: line.origin,
                    text: line.content.trim_end_matches('\n').to_owned(),
                    foreign,
                });
            }
        }
        lines
    }

    /// Contextual keybar hints for the focused panel — only keys that do
    /// something in the read-only shell (ops keys arrive with #32/#33).
    pub fn key_hints(&self) -> Vec<(&'static str, &'static str)> {
        let mut hints: Vec<(&'static str, &'static str)> = match self.focus {
            Panel::Changelists => vec![("j/k", "move"), ("enter", "files")],
            Panel::Files => vec![("j/k", "move"), ("esc", "back")],
            Panel::Diff => vec![("j/k", "scroll"), ("esc", "back")],
            Panel::Commits => vec![("j/k", "move")],
            Panel::Status | Panel::Log => Vec::new(),
        };
        hints.extend([
            ("0-5", "panels"),
            ("R", "refresh"),
            ("?", "keybindings"),
            ("q", "quit"),
        ]);
        hints
    }
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

/// Step an index by `delta`, clamped to `[0, len)`.
fn step(index: usize, delta: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    index.saturating_add_signed(delta).min(len - 1)
}

#[cfg(test)]
mod tests {
    use gitchange_core::{Changelist, CommitInfo, Head, Hunk, HunkLine};

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
            changelist: changelist.map(str::to_owned),
        }
    }

    fn file(path: &str, hunks: Vec<Hunk>) -> ChangedFile {
        ChangedFile {
            path: path.into(),
            kind: ChangeKind::Modified,
            binary: false,
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
    fn a_failed_refresh_keeps_the_last_snapshot() {
        let mut app = app();
        app.on_refresh_started(Instant::now());
        app.on_refresh_failed("boom".into());
        assert!(app.snapshot.is_some());
        assert_eq!(app.last_refresh_error.as_deref(), Some("boom"));
        assert!(!app.indicator_visible(Instant::now() + Duration::from_secs(2)));
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
}
