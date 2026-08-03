//! Snapshot application and selection survival (ADR 0005): swapping in
//! a fresh snapshot identity-first, and moving the per-panel cursors.

use gitchange_core::{ChangedFile, Snapshot, count_noun};

use super::status::COMMIT_DISABLED;
use super::{App, Panel, Severity};

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

        // The refresh echo (variant E): quiet when nothing moved, so a
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

    pub(super) fn move_selection(&mut self, delta: isize) {
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
