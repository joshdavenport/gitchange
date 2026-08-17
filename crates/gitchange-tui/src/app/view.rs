//! The row and diff view-models the renderer draws: Changelists rows,
//! Files rows, diff lines, panel titles and the keybar hints — pure
//! reads over the current snapshot and selections.

use gitchange_core::{
    CONFLICTS, ChangeKind, ChangedFile, FileSides, FileStage, GroupKind, Hunk, HunkStage,
    ModeDelta, SideInfo, UNASSIGNED, count_noun,
};

use super::keymap::{self, BindingId};
use super::{App, Panel};

/// One row of the Changelists panel — also the drill-down scope the
/// Files and Diff panels render. Deliberately *not* the same type as
/// [`Group`]: the two axes share two variants but neither is a subset of
/// the other, and merging them meant every match carrying an arm for a
/// state that axis can't reach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// The All pseudo-view: every changed file grouped by changelist.
    All,
    Changelist(String),
    /// The unassigned pseudo-changelist, pinned last.
    Unassigned,
}

impl Scope {
    /// The hunk owner this scope drills into; `None` for All — no owner,
    /// so nothing is foreign there.
    pub(super) fn owner(&self) -> Option<Option<&str>> {
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
            Scope::Unassigned => UNASSIGNED,
        }
    }

    /// The Files-row group a drilled scope renders as. `None` for All,
    /// which is a view over every group rather than one of them.
    fn as_group(&self) -> Option<Group> {
        match self {
            Scope::All => None,
            Scope::Changelist(name) => Some(Group::Changelist(name.clone())),
            Scope::Unassigned => Some(Group::Unassigned),
        }
    }
}

/// The group a Files row sits under — core's [`GroupKind`] as the TUI
/// needs it. Distinct from [`Scope`]: `Conflicts` is a group but never a
/// Changelists row, and `All` is a row but never a group.
///
/// Not `GroupKind` itself, whose `Changelist` variant carries `active` in
/// its `PartialEq`; [`FileEntry`] is compared by equality to survive a
/// refresh, so switching the active changelist would silently drop the
/// file selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Group {
    /// The quarantine group (ADR 0007).
    Conflicts,
    Changelist(String),
    /// The unassigned pseudo-changelist, pinned last.
    Unassigned,
}

impl Group {
    /// The hunk owner this group holds; `None` for Conflicts, which is
    /// quarantined and has no hunks at all — that's what makes assign
    /// refuse on a conflicted row (ADR 0007).
    pub(super) fn owner(&self) -> Option<Option<&str>> {
        match self {
            Group::Conflicts => None,
            Group::Changelist(name) => Some(Some(name)),
            Group::Unassigned => Some(None),
        }
    }

    /// The group as user-facing text, matching [`GroupKind::label`] —
    /// the same vocabulary the Files headers render (ADR 0006).
    pub fn label(&self) -> &str {
        match self {
            Group::Conflicts => CONFLICTS,
            Group::Changelist(name) => name,
            Group::Unassigned => UNASSIGNED,
        }
    }
}

impl From<&GroupKind> for Group {
    fn from(kind: &GroupKind) -> Self {
        match kind {
            GroupKind::Conflicts => Group::Conflicts,
            GroupKind::Changelist { name, .. } => Group::Changelist(name.clone()),
            GroupKind::Unassigned { .. } => Group::Unassigned,
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
    /// The group the row sits under.
    pub group: Group,
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
    /// Staging glyph next to the label; `None` on unassigned tags.
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
        /// Part of the hunk-mode selection.
        selected: bool,
    },
    Content {
        origin: char,
        text: String,
        foreign: bool,
        /// Part of the hunk-mode selection.
        selected: bool,
    },
    /// Blank spacing between hunks.
    Spacer,
    Placeholder(String),
    /// The quarantine placeholder (ADR 0007), error-tinted: no conflict
    /// diff rendering, ever.
    Conflict(String),
}

impl App {
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
            // Unassigned wears the `*` like any active target: capture
            // flows here while it holds the marker (ADR 0015).
            active: snapshot.active.is_none(),
        });
        rows
    }

    /// Files panel rows for the current scope. All: groups in changelist
    /// order with the unassigned group last when non-empty or active —
    /// `gitchange status`'s grouping semantics. Drilled: a flat list.
    pub fn files_rows(&self) -> Vec<FilesRow> {
        let Some(snapshot) = &self.snapshot else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        match self.scope() {
            Scope::All => {
                // Group order and membership are core's (ADR 0006):
                // Conflicts first (ADR 0007), changelists in user order,
                // unassigned last when non-empty or active.
                for group in snapshot.groups() {
                    rows.push(FilesRow::Header {
                        label: group.kind.label().to_owned(),
                        count: group.files.len(),
                        unassigned: matches!(group.kind, GroupKind::Unassigned { .. }),
                        conflicted: matches!(group.kind, GroupKind::Conflicts),
                        active: group.kind.active(),
                    });
                    let group_kind = Group::from(&group.kind);
                    for file in group.files {
                        rows.push(file_row(file, group_kind.clone(), true));
                    }
                }
            }
            scope @ (Scope::Changelist(_) | Scope::Unassigned) => {
                let group = scope.as_group().expect("a drilled scope is a group");
                for file in snapshot.files_in(group.owner().flatten()) {
                    rows.push(file_row(file, group.clone(), false));
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
        if file.kind == ChangeKind::Conflicted {
            return format!("{name} (conflicted)");
        }
        // Hunk mode replaces the staged counts with the selection
        // position.
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
                    title.push_str(&format!(
                        " {} {} elsewhere",
                        gitchange_core::SEPARATOR,
                        count_noun(elsewhere, "hunk")
                    ));
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
            lines.push(DiffLine::Conflict(format!(
                "conflicted — {}",
                gitchange_core::RESOLVE_OUTSIDE_GITCHANGE
            )));
            return lines;
        }
        if file.presents_lone_degenerate_hunk() {
            // The one-line placeholder (ADR 0009, ADR 0017) — the text is
            // what says binary, mode change or empty file; no new glyphs.
            lines.push(DiffLine::Placeholder(degenerate_placeholder(file)));
            return lines;
        }
        if file.hunks.is_empty() {
            lines.push(DiffLine::Placeholder("no hunks".into()));
            return lines;
        }
        let scope = self.scope();
        let owner = scope.owner();
        for (index, hunk) in file.hunks.iter().enumerate() {
            let foreign = owner.is_some_and(|owner| !hunk.owned_by(owner));
            let selected = self.hunk_sel == Some(index);
            let tag = if owner.is_none() || foreign {
                let unassigned = hunk.changelist.is_none();
                Some(HunkTag {
                    label: hunk.changelist.clone().unwrap_or_else(|| UNASSIGNED.into()),
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
                text: format!("@@ {} {} @@", hunk.old_coords(), hunk.new_coords()),
                tag,
                foreign,
                selected,
            });
            // Whole-file hunks left through the placeholder branch above,
            // so everything here is text; an empty slice would just
            // render no content rows anyway.
            for line in hunk.identity.text_lines().unwrap_or_default() {
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

    /// Where each hunk's header sits within [`App::diff_lines`], in hunk
    /// order — the map from a hunk index to its position on screen, which
    /// is what lets a page step in hunk mode be measured in lines
    /// (issue #84).
    pub(super) fn hunk_header_lines(&self) -> Vec<usize> {
        self.diff_lines()
            .iter()
            .enumerate()
            .filter_map(|(line, item)| matches!(item, DiffLine::HunkHeader { .. }).then_some(line))
            .collect()
    }

    /// The selected hunk's header position within [`App::diff_lines`],
    /// for the renderer's keep-visible scroll in hunk mode.
    pub fn selected_hunk_line(&self) -> Option<usize> {
        let index = self.hunk_sel?;
        self.hunk_header_lines().get(index).copied()
    }

    /// Contextual keybar hints for the focused panel: editorial arms —
    /// which bindings, in what order, under what short label — with
    /// spellings and liveness derived from the binding core (ADR 0014).
    /// A mention whose capability is disabled drops out, which is what
    /// keeps the bar from advertising a key the current context won't
    /// act on (ADR 0013 extended) and what lets every arm mention the
    /// assign trio for the blurred-hunk reach (issue #45) without a
    /// hand-written special case. The trio is one hint with its three
    /// scopes in key order — three separate hints crowd out the rest of
    /// the bar. Hunk mode swaps in its own arm.
    ///
    /// No arm mentions the movement, page or jump keys (issues #86, #84,
    /// #88): the bar has finite width, arrow-key users and vim users both
    /// already reach for a key that works, and the help overlay carries
    /// every spelling.
    pub fn key_hints(&self) -> Vec<(String, &'static str)> {
        use BindingId::*;
        const ASSIGN_TRIO: &[BindingId] = &[AssignSelected, AssignUnassigned, AssignAll];
        const ASSIGN_HUNK: &str = "assign hunk / unassigned / all";
        let arm: Vec<(&'static [BindingId], &'static str)> = if self.hunk_mode_focused() {
            vec![
                (&[StageToggle], "toggle stage hunk"),
                (ASSIGN_TRIO, ASSIGN_HUNK),
                (&[Commit], "commit changelist"),
                (&[Back], "back to files"),
                (&[Help], "keybindings"),
            ]
        } else {
            let mut arm: Vec<(&'static [BindingId], &'static str)> = match self.focus {
                Panel::Changelists => vec![
                    (&[DrillIn], "files"),
                    (&[StageToggle], "toggle stage changelist"),
                    (&[NewChangelist], "new"),
                    (&[DeleteChangelist], "delete"),
                    (&[RenameChangelist], "rename"),
                    (&[SwitchActive], "switch active"),
                    (&[Commit], "commit"),
                    (ASSIGN_TRIO, ASSIGN_HUNK),
                ],
                Panel::Files => vec![
                    (&[StageToggle], "toggle stage row"),
                    (&[DrillIn], "hunks"),
                    (ASSIGN_TRIO, "assign group / unassigned / all"),
                    (&[Commit], "commit"),
                    (&[NewChangelist], "new"),
                    (&[DeleteChangelist], "delete"),
                    (&[RenameChangelist], "rename"),
                    (&[SwitchActive], "switch active"),
                    (&[Back], "back"),
                ],
                Panel::Diff => vec![
                    (&[AssignUnassigned, AssignAll], "assign unassigned / all"),
                    (&[Back], "back"),
                ],
                Panel::Commits | Panel::Log | Panel::Status => vec![(ASSIGN_TRIO, ASSIGN_HUNK)],
            };
            arm.extend([
                (&[FocusPanel] as &[BindingId], "panels"),
                (&[Refresh], "refresh"),
                (&[Help], "keybindings"),
                (&[Quit], "quit"),
            ]);
            arm
        };
        arm.into_iter()
            .filter(|(ids, _)| {
                ids.iter().all(|&id| {
                    self.disabled_reason(keymap::binding(id).capability)
                        .is_none()
                })
            })
            .map(|(ids, label)| (keymap::bar_spelling(ids), label))
            .collect()
    }
}

/// A Files row is a (changelist, file) cell, so its marker and counts
/// read only the hunks its group owns (issue #97) — a file split across
/// changelists shows each row its own progress, and each row's `space`
/// acts on exactly what its glyph reported. The Conflicts group owns no
/// hunks and its files carry none either (ADR 0007), so its rows land on
/// the empty set that the renderer already draws without a marker.
fn file_row(file: &ChangedFile, group: Group, indent: bool) -> FilesRow {
    let owned: Vec<&Hunk> = match group.owner() {
        Some(owner) => file.owned_hunks(owner).collect(),
        None => Vec::new(),
    };
    FilesRow::File {
        entry: FileEntry {
            group,
            path: file.path.clone(),
        },
        stage: gitchange_core::file_stage(owned.iter().copied()),
        kind: file.kind,
        staged: owned
            .iter()
            .filter(|hunk| hunk.stage == HunkStage::Staged)
            .count(),
        total: owned.len(),
        indent,
    }
}

/// A file's hunks owned by `owner`, in file order — core's ownership
/// test ([`ChangedFile::owned_hunks`]) as the owned copies an assign
/// payload carries.
pub(super) fn owned_hunks(file: &ChangedFile, owner: Option<&str>) -> Vec<Hunk> {
    file.owned_hunks(owner).cloned().collect()
}

/// The Diff panel's one-line placeholder for a file whose whole change is
/// one degenerate hunk: sizes for a binary (ADR 0009) —
/// `Binary file changed (12.4 KB → 15.1 KB)` — and for a zero-hunk change
/// the shape its sides name (ADR 0017): `Mode changed (100644 → 100755)`,
/// `Empty file added`, `Empty file deleted`. Modes print octal, as git
/// prints them.
///
/// The hunk's flavour is asked first, and the file's binary flag only
/// after: a chmod'd binary whose bytes never moved is a mode change, and
/// naming it by size would report a change it doesn't have. Within the
/// whole-file flavours, side presence rather than change kind picks the
/// variant — an index-only change has no worktree kind to trust.
fn degenerate_placeholder(file: &ChangedFile) -> String {
    let sides = file.sides.as_ref();
    let delta = sides.and_then(FileSides::mode_delta);
    if file.hunks.iter().any(Hunk::is_mode_change) {
        // The mode hunk's placeholder text *is* its header: a mode delta
        // has no coordinates to frame. A mode hunk exists because the
        // modes differ, so the delta is always there; naming the change
        // without them still beats saying nothing.
        return delta
            .map(mode_delta_text)
            .unwrap_or_else(|| "Mode changed".into());
    }
    let head = sides.and_then(|sides| sides.head.as_ref());
    let changed = sides.and_then(|sides| sides.changed.as_ref());
    if file.binary {
        binary_placeholder(head, changed)
    } else {
        zero_hunk_placeholder(head, changed, delta)
    }
}

/// How a filemode delta reads as a placeholder: `Mode changed (100644 →
/// 100755)` for a chmod, `Type changed (100644 → 120000)` for a symlink
/// swap (ADR 0017, issue #100). Octal, as git prints them. One function
/// for both flavours, so which delta core reported decides the word —
/// reading the mode bits a second time here is how a symlink swap comes
/// to be called a chmod.
fn mode_delta_text(delta: ModeDelta) -> String {
    let (label, before, after) = match delta {
        ModeDelta::Mode { before, after } => ("Mode changed", before, after),
        ModeDelta::Type { before, after } => ("Type changed", before, after),
    };
    format!("{label} ({before:o} {} {after:o})", gitchange_core::ARROW)
}

/// The binary variants (ADR 0009), each showing the sizes it has.
fn binary_placeholder(head: Option<&SideInfo>, changed: Option<&SideInfo>) -> String {
    match (head, changed) {
        (Some(head), Some(changed)) => format!(
            "Binary file changed ({} {} {})",
            human_size(head.size),
            gitchange_core::ARROW,
            human_size(changed.size)
        ),
        (None, Some(changed)) => format!("Binary file added ({})", human_size(changed.size)),
        (Some(head), None) => format!("Binary file deleted ({})", human_size(head.size)),
        (None, None) => "Binary file changed".into(),
    }
}

/// The whole-file zero-hunk variants (ADR 0017). One side missing is the
/// file coming or going; both sides present with no line content means
/// the mode bits are the change, and `delta` says which kind it is — in
/// practice a type change, since a permission flip presents a mode hunk
/// and left through the branch above. Anything else is a change git
/// reports with neither content nor a mode to name — nothing to say
/// beyond that it exists.
fn zero_hunk_placeholder(
    head: Option<&SideInfo>,
    changed: Option<&SideInfo>,
    delta: Option<ModeDelta>,
) -> String {
    match (head, changed) {
        (None, Some(_)) => "Empty file added".into(),
        (Some(_), None) => "Empty file deleted".into(),
        (Some(_), Some(_)) => delta
            .map(mode_delta_text)
            .unwrap_or_else(|| "File changed".into()),
        (None, None) => "File changed".into(),
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
