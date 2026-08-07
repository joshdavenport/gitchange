//! Presentation channels: the Log panel's stream, the error-modal
//! contract, condition pins, and the deferred refresh indicator
//! (ADR 0005/0007).

use std::time::{Duration, Instant};

use gitchange_core::Advisory;

use super::App;

/// The deferred-indicator threshold (ADR 0005): refreshes shorter than
/// this show nothing.
pub const INDICATOR_DELAY: Duration = Duration::from_millis(500);

/// Log entries kept — enough history for the panel plus scrollback.
const LOG_CAP: usize = 200;

/// The operation guard's "why" tail (ADR 0007): standalone when no path
/// is conflicted, the suffix of a longer clause when some are.
pub(super) const COMMIT_DISABLED: &str = "commit disabled";

/// The watcher-degraded condition's shared stem. ADR 0007 splits the
/// onset log from the live pin deliberately (the event marks the
/// moment, the pin names the state) — share only the stem, never the
/// two full strings.
const WATCHER_UNAVAILABLE: &str = "watcher unavailable";

/// A log event's severity (ADR 0007): three levels, fixed — new event
/// classes map onto these rather than growing the scale. Assigned here
/// in the presentation layer; core's [`Advisory`] carries none.
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

impl App {
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
            Severity::Info,
            format!("{WATCHER_UNAVAILABLE} — falling back to polling"),
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
        if let Some(snapshot) = &self.snapshot
            && let Some(operation) = snapshot.operation
        {
            let conflicted = snapshot.conflicted_files().len();
            let tail = if conflicted > 0 {
                format!("{conflicted} conflicted")
            } else {
                COMMIT_DISABLED.to_owned()
            };
            pins.push(format!("{} in progress — {tail}", operation.label()));
        }
        if self.watcher_degraded {
            pins.push(format!("{WATCHER_UNAVAILABLE} — polling"));
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

    /// Append core advisories in the Log vocabulary — the path every
    /// fail-soft op outcome takes.
    pub fn push_advisories<'a>(&mut self, advisories: impl IntoIterator<Item = &'a Advisory>) {
        for advisory in advisories {
            let entry = advisory_entry(advisory);
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
}

/// A core notice in the Log vocabulary. Text is core's canonical
/// phrasing (ADR 0006); severity is assigned here — the spec renders
/// automatic membership decisions at `!`, fail-soft stale-hunk outcomes
/// included; core keeps severity out (ADR 0007).
pub fn advisory_entry(advisory: &Advisory) -> LogEntry {
    LogEntry {
        severity: Severity::Notice,
        text: advisory.message(),
    }
}
