use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::matcher;
use crate::universe::Hunk;
use crate::vocabulary::{ALL, UNASSIGNED};

/// Names claimed by pseudo-views (`CONTEXT.md`): never valid for user
/// changelists. Built from the same constants frontends print as labels
/// (ADR 0006), so a reserved name and its printed label can't disagree.
pub const RESERVED_NAMES: [&str; 2] = [ALL, UNASSIGNED];

pub const SCHEMA_VERSION: u32 = 1;

/// A named set of uncommitted changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Changelist {
    pub name: String,
}

/// The persisted claim on one hunk (ADR 0001): path, line coordinates,
/// owner, and the verbatim content anchor the matcher re-derives
/// membership from on every refresh.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipRecord {
    /// Repo-relative path, as git reports it.
    pub path: String,
    /// HEAD-side coordinates — the space both fresh diffs share.
    pub old_start: u32,
    pub old_lines: u32,
    /// Worktree-side coordinates at record time.
    pub new_start: u32,
    pub new_lines: u32,
    /// Owning changelist; `None` claims the hunk for unassigned (orphans
    /// of deleted changelists, explicit assignment to it).
    pub changelist: Option<String>,
    /// Verbatim hunk lines (`origin` + content), context included — the
    /// identity evidence for tier-1 exact matching. Empty for binary
    /// records, whose identity lives in `oid_anchor` instead.
    pub anchor: Vec<String>,
    /// The blob-OID-pair anchor of a binary whole-file record (ADR 0009):
    /// present exactly when the record claims a binary file's degenerate
    /// hunk. `default` keeps pre-binary schema-1 files readable; omitted
    /// from text records to keep the file `cat`-debuggable (ADR 0002).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oid_anchor: Option<OidAnchor>,
    /// Unix epoch seconds since the hunk vanished from the diff; `None`
    /// while live. Dormant records revive only via tier-1 exact match
    /// (ADR 0002) and prune after 14 days.
    pub dormant_since: Option<u64>,
}

/// A binary record's identity evidence (ADR 0009): the HEAD-side blob
/// OID and the changed-side content hash. A `None` side doesn't exist
/// (no `head` for added files, no `changed` for deletions). Tier-1
/// matching and dormant revival compare the changed side only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OidAnchor {
    pub head: Option<String>,
    pub changed: Option<String>,
}

impl MembershipRecord {
    pub(crate) fn is_dormant(&self) -> bool {
        self.dormant_since.is_some()
    }
}

/// Everything the state file holds (ADR 0002): the changelists, in user
/// order, and the active marker. Exactly one changelist is active
/// whenever any exist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct State {
    pub version: u32,
    pub active: Option<String>,
    pub changelists: Vec<Changelist>,
    /// Membership records, live and dormant. `default` keeps pre-record
    /// schema-1 files readable.
    #[serde(default)]
    pub records: Vec<MembershipRecord>,
    /// The baseline HEAD (ADR 0012): the commit whose tree record
    /// coordinates address, stamped at each persisting refresh. `default`
    /// keeps pre-baseline files readable; `None` adopts the current HEAD
    /// on the next refresh, guard skipped once.
    #[serde(default)]
    pub baseline_head: Option<String>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            version: SCHEMA_VERSION,
            active: None,
            changelists: Vec::new(),
            records: Vec::new(),
            baseline_head: None,
        }
    }
}

impl State {
    fn contains(&self, name: &str) -> bool {
        self.changelists.iter().any(|cl| cl.name == name)
    }

    fn validate_new_name(&self, name: &str) -> Result<(), Error> {
        if name.trim().is_empty() {
            return Err(Error::InvalidName {
                reason: "name is empty".into(),
            });
        }
        if RESERVED_NAMES.contains(&name) {
            return Err(Error::ReservedName { name: name.into() });
        }
        if self.contains(name) {
            return Err(Error::ChangelistExists { name: name.into() });
        }
        Ok(())
    }

    /// Append a changelist. The first one created becomes active; later
    /// ones never steal the marker.
    pub fn create(&mut self, name: &str) -> Result<(), Error> {
        self.validate_new_name(name)?;
        self.changelists.push(Changelist { name: name.into() });
        if self.active.is_none() {
            self.active = Some(name.into());
        }
        Ok(())
    }

    /// Rename `from` to `to`, carrying the active marker with it.
    pub fn rename(&mut self, from: &str, to: &str) -> Result<(), Error> {
        if !self.contains(from) {
            return Err(Error::UnknownChangelist { name: from.into() });
        }
        if from == to {
            return Ok(());
        }
        self.validate_new_name(to)?;
        for cl in &mut self.changelists {
            if cl.name == from {
                cl.name = to.into();
            }
        }
        for record in &mut self.records {
            if record.changelist.as_deref() == Some(from) {
                record.changelist = Some(to.into());
            }
        }
        if self.active.as_deref() == Some(from) {
            self.active = Some(to.into());
        }
        Ok(())
    }

    /// Remove a changelist. Deleting the active one promotes the first
    /// remaining changelist, keeping the exactly-one-active invariant.
    /// The deleted changelist's live records become unassigned orphans —
    /// never captured by another changelist — and its dormant records are
    /// pruned (ADR 0002).
    pub fn delete(&mut self, name: &str) -> Result<(), Error> {
        if !self.contains(name) {
            return Err(Error::UnknownChangelist { name: name.into() });
        }
        self.changelists.retain(|cl| cl.name != name);
        self.records
            .retain(|record| !(record.changelist.as_deref() == Some(name) && record.is_dormant()));
        for record in &mut self.records {
            if record.changelist.as_deref() == Some(name) {
                record.changelist = None;
            }
        }
        if self.active.as_deref() == Some(name) {
            self.active = self.changelists.first().map(|cl| cl.name.clone());
        }
        Ok(())
    }

    /// Apply delete semantics to records naming a changelist that no
    /// longer exists (a hand-edited file, or a delete racing a refresh's
    /// unlocked read): live records orphan to unassigned, dormant ones
    /// are pruned — a deleted changelist must never claim hunks again.
    pub(crate) fn orphan_records_of_unknown_changelists(&mut self) {
        let known: Vec<String> = self.changelists.iter().map(|cl| cl.name.clone()).collect();
        let unknown = |record: &MembershipRecord| matches!(&record.changelist, Some(name) if !known.contains(name));
        self.records
            .retain(|record| !(unknown(record) && record.is_dormant()));
        for record in &mut self.records {
            if unknown(record) {
                record.changelist = None;
            }
        }
    }

    /// Point `path`'s records at `target` for the given fresh hunks —
    /// the explicit assign op (ticket #32). `target: None` claims them
    /// for unassigned, the same sticky claim delete-orphans carry.
    /// Competing claims are replaced: records anchor-matching an
    /// assigned hunk, and live records overlapping its HEAD-side range,
    /// are removed so neither matching tier re-claims the hunk for its
    /// old owner.
    pub(crate) fn assign_records(
        &mut self,
        path: &str,
        hunks: &[&Hunk],
        target: Option<&str>,
    ) -> Result<(), Error> {
        if let Some(name) = target
            && !self.contains(name)
        {
            return Err(Error::UnknownChangelist { name: name.into() });
        }
        let anchors: Vec<Vec<String>> = hunks.iter().map(|hunk| matcher::anchor_of(hunk)).collect();
        self.records.retain(|record| {
            record.path != path
                || !hunks.iter().zip(&anchors).any(|(hunk, anchor)| {
                    matcher::exact_anchor_match(record, hunk, anchor)
                        || (!record.is_dormant() && matcher::overlap_claim(record, hunk))
                })
        });
        for (hunk, anchor) in hunks.iter().zip(&anchors) {
            self.records.push(matcher::record_for(
                path,
                hunk,
                anchor,
                target.map(str::to_owned),
            ));
        }
        Ok(())
    }

    /// Set the active marker.
    pub fn switch(&mut self, name: &str) -> Result<(), Error> {
        if !self.contains(name) {
            return Err(Error::UnknownChangelist { name: name.into() });
        }
        self.active = Some(name.into());
        Ok(())
    }
}
