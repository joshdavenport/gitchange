//! The modal layer: the overlay stack's types and key routing — text
//! inputs, delete confirmation, the assign popup, and the commit
//! dialog's whole warn/drift/restore lifecycle (ADR 0004).

use gitchange_core::{ChangeKind, CommitPayload, Hunk, UNASSIGNED, count_noun};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::selection::step;
use super::view::{FileEntry, owned_hunks, owns};
use super::{Action, App, CommitStep, Op, Panel, Severity};

/// Everything the commit dialog holds:
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
        self.changelist.as_deref().unwrap_or(UNASSIGNED)
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

/// A modal above the panel stack. At most one is open; it swallows every
/// key until it closes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Overlay {
    /// A one-line text input, text-on-border framing (the app-wide
    /// input convention).
    Input { kind: InputKind, value: String },
    /// Delete-changelist confirmation (brief §3-derived pattern): the
    /// changelist's hunks go to unassigned.
    ConfirmDelete { name: String },
    /// The centered assign popup.
    Assign { payload: AssignPayload, row: usize },
    /// The all-in-one commit dialog.
    Commit(CommitDraft),
    /// The ◑ staged-stale warn-and-confirm over the dialog (ADR 0004).
    CommitStale(CommitDraft),
    /// The zero-staged stage-all offer (ADR 0004 — core never
    /// auto-stages).
    CommitStageAll {
        changelist: Option<String>,
        /// The changelist's hunk/file counts, captured at open for the
        /// "stage all N hunks in M files" line.
        hunks: usize,
        files: usize,
    },
    /// Drift re-confirm keeping the message (the ADR 0004
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

impl App {
    /// Route a key into the open overlay. Overlays swallow everything;
    /// the ones producing an [`Op`] close as they return it.
    pub(super) fn on_overlay_key(&mut self, key: KeyEvent) -> Option<Action> {
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

    /// The commit dialog's keys: `enter` commit
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
                // ◑ in the payload → warn-and-confirm first;
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

    /// A derived payload arrived for `c`'s dialog. Empty
    /// payloads route through [`App::offer_stage_all`] instead.
    pub fn open_commit_dialog(&mut self, changelist: Option<String>, payload: CommitPayload) {
        self.overlay = Some(Overlay::Commit(CommitDraft::new(changelist, payload)));
    }

    /// The payload came back empty: offer stage-all-and-commit
    /// when the changelist has hunks at all, otherwise say
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
            let label = owner.unwrap_or(UNASSIGNED);
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
    /// against the fresh payload, message and flags kept.
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

    /// What `a` assigns: the Files row's group-owned hunks from the Files
    /// panel, the hunk-mode selection otherwise — including while that
    /// selection is blurred, the cross-panel reach issue #45 keeps its
    /// cursor visible for.
    pub(super) fn selected_assign_payload(&self) -> Option<AssignPayload> {
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
    /// what hunk mode is drilled into. Whether the assign keys are live
    /// at all is [`Capability::Assign`]'s answer (issue #45's blurred
    /// selection included) — dispatch consults it before landing here,
    /// so `None` means only "no file selected", a content no-op.
    pub(super) fn assign_file_path(&self) -> Option<String> {
        self.selected_file().map(|file| file.path.clone())
    }

    /// Open the assign popup when the payload has hunks in it. An empty
    /// payload — `A` on a file with nothing unassigned, most often — is a
    /// polite no-op with a Log line, never an error modal (ADR 0007).
    pub(super) fn open_assign(&mut self, payload: AssignPayload) {
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
            return gitchange_core::conflicted_hint(path);
        }
        match payload {
            AssignPayload::FileRow(entry) => format!(
                "no hunks in '{}' for {path} — nothing to assign",
                entry.group.label(),
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

    pub(super) fn resolve_payload(&self, payload: &AssignPayload) -> Option<(String, Vec<Hunk>)> {
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

    /// The assign popup's rows: every changelist,
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
            line.push_str(&format!(" @@ {} {}", hunk.old_coords(), hunk.new_coords()));
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
                None => UNASSIGNED.to_owned(),
            };
            match sources.iter_mut().find(|(seen, _)| *seen == label) {
                Some((_, count)) => *count += 1,
                None => sources.push((label, 1)),
            }
        }
        sources
    }
}
