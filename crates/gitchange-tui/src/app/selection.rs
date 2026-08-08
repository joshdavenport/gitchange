//! Snapshot application and selection survival (ADR 0005): swapping in
//! a fresh snapshot identity-first, and moving the per-panel cursors.

use gitchange_core::{ChangedFile, Snapshot, count_noun};

use super::status::COMMIT_DISABLED;
use super::{App, Panel, Severity};

/// How far one movement press moves (issues #84, #88).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Motion {
    /// One row, line or hunk — `j`/`k` and the arrows.
    Row,
    /// A screenful of the focused panel, less a row of overlap so the
    /// row that was last visible becomes the first — `.`/`,` and
    /// `PgDn`/`PgUp`.
    Page,
    /// The whole panel: as far as its content goes — `>` and `<`.
    Jump,
}

impl App {
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
                format!(
                    "{} conflicted, {COMMIT_DISABLED}",
                    count_noun(conflicted, "file")
                )
            } else {
                COMMIT_DISABLED.to_owned()
            };
            self.push_log(
                Severity::Info,
                format!("{} detected — {tail}", operation.label()),
            );
        }

        // The refresh echo: quiet when nothing moved, so a
        // degraded engine's polling ticks don't flood the stream.
        let hunks: usize = snapshot.files.iter().map(ChangedFile::total_hunks).sum();
        let conflicted = snapshot.conflicted_files().len();
        let counts = (snapshot.files.len(), hunks, conflicted);
        if self.last_refresh_echo != Some(counts) {
            self.last_refresh_echo = Some(counts);
            let mut echo = format!(
                "refresh — {} files {} {hunks} hunks",
                snapshot.files.len(),
                gitchange_core::SEPARATOR
            );
            if conflicted > 0 {
                echo.push_str(&format!(" ({conflicted} conflicted)"));
            }
            self.push_log(Severity::Info, echo);
        }

        // This refresh's automatic membership decisions, each exactly
        // once (a decision becomes a record; there is no replay).
        self.push_advisories(&snapshot.advisories);

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
        // The hunk-mode selection's identity, plus position for
        // tie-breaking. The same identity test core validates ops
        // against, so the cursor survives a refresh on exactly the hunks
        // an op would still apply to.
        let old_hunk = self.hunk_sel.and_then(|index| {
            self.selected_file()
                .and_then(|file| file.hunks.get(index))
                .map(|hunk| (hunk.identity.clone(), hunk.new_start))
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
                let survived = old_hunk.as_ref().and_then(|(identity, new_start)| {
                    hunks
                        .iter()
                        .enumerate()
                        .filter(|(_, hunk)| hunk.identity.same_hunk(identity))
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

    /// Move the focused panel's selection — or its scroll, where the
    /// panel has no selection — one row, one screenful or the whole
    /// panel, in `direction` (`1` down, `-1` up).
    ///
    /// One match over the panels serves every magnitude rather than a
    /// copy of it per key (issues #84, #88), so a panel added later
    /// cannot answer the movement keys without answering the page and
    /// jump keys too. Where a magnitude genuinely differs — hunk mode,
    /// whose page is not a count of hunks — it parts inside that panel's
    /// own arm.
    pub(super) fn move_selection(&mut self, motion: Motion, direction: isize) {
        // A page is measured against the panel the key acts on, as the
        // last frame drew it (issue #85) — never zero, so an App that
        // has yet to draw one pages by a single row. A jump moves
        // without bound: every arm below already clamps to its own
        // content, so both ends fall out of the clamps that are there
        // rather than out of a second set of them.
        let amount = match motion {
            Motion::Row => 1,
            Motion::Page => self.page(self.focus),
            Motion::Jump => usize::MAX,
        };
        let delta = isize::try_from(amount)
            .unwrap_or(isize::MAX)
            .saturating_mul(direction);
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
                // Hunk mode walks hunks (the renderer keeps the
                // selection visible); scroll mode moves lines, clamped
                // to the content so neither magnitude scrolls into blank
                // space.
                if let Some(index) = self.hunk_sel {
                    let next = match motion {
                        // A row and a jump are both counts of hunks —
                        // one step, or every step there is.
                        Motion::Row | Motion::Jump => {
                            let hunks = self.selected_file().map_or(0, |file| file.hunks.len());
                            step(index, delta, hunks)
                        }
                        Motion::Page => self.paged_hunk(index, direction, amount),
                    };
                    self.hunk_sel = Some(next);
                } else {
                    // The offset is a `u16`, so a diff longer than one
                    // can address ends where the offset does.
                    let max = u16::try_from(self.diff_lines().len().saturating_sub(1))
                        .unwrap_or(u16::MAX);
                    // A page came from a panel height, so it fits a
                    // `u16`; an unbounded jump saturates here and the
                    // clamps below take it to the content's end.
                    let lines = u16::try_from(amount).unwrap_or(u16::MAX);
                    self.diff_scroll = if direction > 0 {
                        self.diff_scroll.saturating_add(lines).min(max)
                    } else {
                        self.diff_scroll.saturating_sub(lines)
                    };
                }
            }
            Panel::Log => {
                // Offset from the stream's bottom: down goes back toward
                // the newest entry, up into history — and a jump ends
                // where that run of presses ends, so `>` is the newest
                // entry rather than the offset's maximum.
                self.log_scroll = if direction > 0 {
                    self.log_scroll.saturating_sub(amount)
                } else {
                    self.log_scroll
                        .saturating_add(amount)
                        .min(self.log.len().saturating_sub(1))
                };
            }
            Panel::Status => {}
        }
    }

    /// The hunk a page of `page` lines lands on, from the one at `index`.
    ///
    /// A screenful of *hunks* is not a fixed count — one hunk can be
    /// taller than the panel — so the step is measured in diff lines:
    /// downward it takes the first hunk whose header sits at or beyond a
    /// screenful past the current header. Three consequences follow, all
    /// intended: a header on the fold row is taken (the fold row is
    /// shared, matching the list panels' row of overlap), short hunks
    /// before it are skipped rather than walked, and a hunk taller than
    /// the panel is left in one press — reading it is down/up's job.
    /// Upward mirrors it. Neither end wraps.
    fn paged_hunk(&self, index: usize, direction: isize, page: usize) -> usize {
        let headers = self.hunk_header_lines();
        let Some(&current) = headers.get(index) else {
            return index;
        };
        if direction > 0 {
            let fold = current + page;
            headers
                .iter()
                .position(|&line| line >= fold)
                .unwrap_or(headers.len() - 1)
        } else {
            let fold = current.saturating_sub(page);
            headers.iter().rposition(|&line| line <= fold).unwrap_or(0)
        }
    }

    /// Select a Changelists row, rescoping the Files panel: selection
    /// moves to the new scope's first file.
    pub(super) fn select_changelist_row(&mut self, row: usize) {
        if row == self.changelist_row {
            return;
        }
        self.changelist_row = row;
        self.diff_scroll = 0;
        self.hunk_sel = None;
        self.file_sel = self.file_entries().first().cloned();
    }
}

/// Step an index by `delta`, clamped to `[0, len)`.
pub(super) fn step(index: usize, delta: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    index.saturating_add_signed(delta).min(len - 1)
}
