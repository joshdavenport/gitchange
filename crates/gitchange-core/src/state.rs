use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::matcher;
use crate::universe::Hunk;
use crate::vocabulary::{ALL, UNASSIGNED, count_noun};

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
    /// Owning changelist — always a real name (ADR 0016): unassigned is
    /// the absence of a record, so nothing ever writes a nobody-owner.
    pub changelist: String,
    /// Verbatim hunk lines (`origin` + content), context included — the
    /// identity evidence for tier-1 exact matching. Empty for the
    /// degenerate records, whose identity lives in `oid_anchor` or
    /// `mode_change` instead.
    pub anchor: Vec<String>,
    /// The blob-OID-pair anchor of a whole-file record (ADR 0009):
    /// present exactly when the record claims a file's whole-file
    /// degenerate hunk. `default` keeps pre-binary schema-1 files
    /// readable; omitted from text records to keep the file
    /// `cat`-debuggable (ADR 0002).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oid_anchor: Option<OidAnchor>,
    /// Set on a mode hunk's record (ADR 0017). A mode delta carries no
    /// content evidence at all — no lines, no blob pair — so the flag is
    /// what tells such a record from a text record's empty anchor, and
    /// its identity is the path. Deliberately not the modes themselves:
    /// recording them would only buy refusing revival on a different
    /// flip at the same path, and revival there is right. `default`
    /// keeps pre-mode-hunk schema-1 files readable.
    #[serde(default, skip_serializing_if = "is_not_set")]
    pub mode_change: bool,
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

/// A stored record's identity evidence, borrowed — the record-side
/// mirror of [`HunkIdentity`]. The record keeps its evidence in
/// independent serde fields so the state file stays `cat`-debuggable
/// (ADR 0002); this reads them back as the sum type they have always
/// represented, so matching never tests one field's emptiness to infer
/// another's meaning.
///
/// [`HunkIdentity`]: crate::universe::HunkIdentity
pub(crate) enum RecordIdentity<'a> {
    Text {
        anchor: &'a [String],
    },
    WholeFile {
        oids: &'a OidAnchor,
    },
    /// A mode hunk's record: no evidence beyond the path (ADR 0017).
    ModeChange,
}

/// A hunk identity as [`MembershipRecord`]'s independent fields — what
/// [`Hunk::record_anchors`] projects and [`matcher::record_for`] writes.
/// The inverse of [`MembershipRecord::identity`], and the reason neither
/// direction infers a flavour from a field being empty.
///
/// [`Hunk::record_anchors`]: crate::universe::Hunk::record_anchors
#[derive(Debug)]
pub(crate) struct RecordAnchors {
    pub anchor: Vec<String>,
    pub oid_anchor: Option<OidAnchor>,
    pub mode_change: bool,
}

/// `skip_serializing_if` for a flag that is absent when unset — a text
/// record must not grow a `"mode_change": false` line (ADR 0002).
fn is_not_set(flag: &bool) -> bool {
    !flag
}

impl MembershipRecord {
    pub(crate) fn is_dormant(&self) -> bool {
        self.dormant_since.is_some()
    }

    /// What this record claims, per [`RecordIdentity`]. The mode flag is
    /// read first and alone: a mode record carries no other evidence, so
    /// a hand-edited file that sets it beside an `oid_anchor` still
    /// resolves to one flavour rather than half of two. Otherwise
    /// `oid_anchor` is present exactly for whole-file records, so it
    /// discriminates the rest.
    pub(crate) fn identity(&self) -> RecordIdentity<'_> {
        if self.mode_change {
            return RecordIdentity::ModeChange;
        }
        match &self.oid_anchor {
            Some(oids) => RecordIdentity::WholeFile { oids },
            None => RecordIdentity::Text {
                anchor: &self.anchor,
            },
        }
    }

    /// Whether this record stores exactly `anchors` — the field-by-field
    /// half of [`MembershipRecord::identity`], kept beside it so a fourth
    /// flavour cannot be added to the projection and forgotten here. The
    /// commit aftermath keys records this way (ADR 0004): a fresh live
    /// record mirrors its hunk exactly, so the identity fields plus
    /// coordinates pin one record.
    pub(crate) fn stores_identity(&self, anchors: &RecordAnchors) -> bool {
        self.anchor == anchors.anchor
            && self.oid_anchor == anchors.oid_anchor
            && self.mode_change == anchors.mode_change
    }
}

/// The last commit gitchange made, whatever the frontend
/// (ADR 0004 §Aftermath): one record for the repo, not one per
/// changelist, replaced by every commit and every amend. Commits carry no
/// provenance, so this is the only thing that tells an own commit from
/// another actor's — the reference the CLI's foreign-head guard reads
/// before an amend (#151).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LastCommit {
    /// The commit the aftermath found at HEAD.
    pub oid: String,
    /// The scope committed. Unassigned is spelled with its label rather
    /// than left out: this names what a commit took, not who owns a hunk,
    /// so ADR 0016's no-record-is-unassigned rule has nothing to say here
    /// — and a reader comparing names must not have to read an absent
    /// field as either "unassigned" or "never committed".
    pub changelist: String,
}

/// Everything the state file holds (ADR 0002), field by field below —
/// enumerated once, here, so a field added cannot be missing from its own
/// description. Exactly one of {the changelists, unassigned} is active.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct State {
    pub version: u32,
    /// The active changelist; `None` is the unassigned pseudo-changelist
    /// — ADR 0015's capture-off, where capture and ambiguous-edit
    /// routing flow to unassigned. A state file written before
    /// unassigned was switchable reads the same way: no marker meant
    /// unassigned then too.
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
    /// The last commit gitchange made (ADR 0004 §Aftermath). `default`
    /// keeps files written before it readable, and `None` is
    /// record-absent — no commit gitchange knows of, which is exactly
    /// what an amend guard must treat as foreign.
    #[serde(default)]
    pub last_commit: Option<LastCommit>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            version: SCHEMA_VERSION,
            active: None,
            changelists: Vec::new(),
            records: Vec::new(),
            baseline_head: None,
            last_commit: None,
        }
    }
}

/// A changelist's membership records, counted by liveness — what the
/// delete guard names and the forced release reports (#149). Live and
/// dormant are counted apart because they promise different things: a
/// live record claims a hunk that is in the diff now, a dormant one a
/// hunk that would come back (ADR 0002), and a delete prunes both
/// (ADR 0016).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordCounts {
    /// Records claiming a hunk that is in the diff now: the hunks a
    /// delete would release.
    pub live: usize,
    /// Records whose hunk has left the diff and would be restored if it
    /// came back (ADR 0002): nothing to release, a revival to lose.
    pub dormant: usize,
}

impl RecordCounts {
    /// Whether there are any records at all — the records guard's
    /// condition, live or dormant alike.
    pub fn any(&self) -> bool {
        self.live + self.dormant > 0
    }

    /// These counts as prose, in the one phrasing every line that names
    /// them uses (ADR 0006's one-home rule): both numbers when both are
    /// nonzero, and only the nonzero one otherwise — "1 live record and 0
    /// dormant records" states a fact nobody asked about, and the guard
    /// fires on the total anyway.
    pub fn counted(&self) -> String {
        match (self.live, self.dormant) {
            (0, dormant) => count_noun(dormant, "dormant record"),
            (live, 0) => count_noun(live, "live record"),
            (live, dormant) => format!(
                "{} and {}",
                count_noun(live, "live record"),
                count_noun(dormant, "dormant record")
            ),
        }
    }
}

impl State {
    pub(crate) fn contains(&self, name: &str) -> bool {
        self.changelists.iter().any(|cl| cl.name == name)
    }

    /// The changelist names in user order — the candidates an
    /// unrecognised-name refusal quotes back. Real changelists only:
    /// unassigned is the absence of membership (ADR 0016), so no mode of
    /// the noun command has anything to do with it.
    pub(crate) fn changelist_names(&self) -> Vec<String> {
        self.changelists.iter().map(|cl| cl.name.clone()).collect()
    }

    /// Record `oid` as the last commit gitchange made, for `changelist`
    /// (`None` = unassigned) — every commit records itself, an amend
    /// included (ADR 0004 §Aftermath), so the previous record is simply
    /// replaced.
    pub(crate) fn record_commit(&mut self, oid: &str, changelist: Option<&str>) {
        self.last_commit = Some(LastCommit {
            oid: oid.to_owned(),
            changelist: changelist.unwrap_or(UNASSIGNED).to_owned(),
        });
    }

    /// Whether `oid` is the commit this state last recorded for
    /// `changelist` (`None` = unassigned) — the foreign-head fact
    /// (ADR 0004 §Amend). No record, another scope's record, or a record
    /// naming an older commit all answer `false`: each is a HEAD an amend
    /// would fold this payload into blind.
    pub(crate) fn is_last_commit(&self, oid: &str, changelist: Option<&str>) -> bool {
        self.last_commit.as_ref().is_some_and(|last| {
            last.oid == oid && last.changelist == changelist.unwrap_or(UNASSIGNED)
        })
    }

    /// How many records `name` holds, live and dormant.
    pub(crate) fn record_counts(&self, name: &str) -> RecordCounts {
        let mut counts = RecordCounts {
            live: 0,
            dormant: 0,
        };
        for record in self.records.iter().filter(|r| r.changelist == name) {
            match record.is_dormant() {
                true => counts.dormant += 1,
                false => counts.live += 1,
            }
        }
        counts
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

    /// Append a changelist. Creation never moves the active marker
    /// (ADR 0015): `switch` is the only thing that does, so creating a
    /// changelist while unassigned is active cannot turn capture back on
    /// under a concurrent actor. A caller who does want the new
    /// changelist active — the TUI's `n` — says so with its own switch.
    pub fn create(&mut self, name: &str) -> Result<(), Error> {
        self.validate_new_name(name)?;
        self.changelists.push(Changelist { name: name.into() });
        Ok(())
    }

    /// Rename `from` to `to`, carrying the active marker with it. Every
    /// field that stores the name is rewritten in this one walk — the
    /// changelist, its records live and dormant, the marker, and the
    /// last-commit record (ADR 0004 §Aftermath) — so a rename stays the
    /// total, atomic bookkeeping it promises. Each field left behind
    /// fails differently, which is why none may be missed: a stale
    /// record is pruned as an unknown changelist's, and a stale
    /// last-commit record stops an amend recognising its own commit.
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
            if record.changelist == from {
                record.changelist = to.into();
            }
        }
        if self.active.as_deref() == Some(from) {
            self.active = Some(to.into());
        }
        if let Some(last) = &mut self.last_commit
            && last.changelist == from
        {
            last.changelist = to.into();
        }
        Ok(())
    }

    /// Remove a changelist. Deleting the active one leaves unassigned
    /// active (ADR 0015): promoting a neighbour would point capture at a
    /// changelist nobody named — in a shared tree, quite possibly
    /// another actor's. All of the changelist's records are pruned, live
    /// and dormant (ADR 0016): its hunks are released recordless, to
    /// flow under ADR 0001's uniform rule on the next refresh.
    ///
    /// A last-commit record naming it goes too (ADR 0004 §Aftermath). The
    /// name can be recreated, and the changelist that comes back is not
    /// the one that made that commit — so a surviving record would let its
    /// first amend fold a payload into a stranger's commit with the
    /// foreign-head guard satisfied. Nothing else is lost with it: the
    /// record is only ever read for a name an amend can still target.
    pub fn delete(&mut self, name: &str) -> Result<(), Error> {
        if !self.contains(name) {
            return Err(Error::UnknownChangelist { name: name.into() });
        }
        self.changelists.retain(|cl| cl.name != name);
        self.records.retain(|record| record.changelist != name);
        if self.active.as_deref() == Some(name) {
            self.active = None;
        }
        if self
            .last_commit
            .as_ref()
            .is_some_and(|last| last.changelist == name)
        {
            self.last_commit = None;
        }
        Ok(())
    }

    /// Apply delete semantics to records naming a changelist that no
    /// longer exists (a hand-edited file, or a delete racing a refresh's
    /// unlocked read): pruned wholesale, like [`State::delete`] — a
    /// deleted changelist must never claim hunks again (ADR 0016).
    ///
    /// Membership records only, deliberately, where [`State::delete`]
    /// clears the last-commit record too: this runs on a refresh's own
    /// copy of the state, which persists only when records or the
    /// baseline stamp move (ADR 0005's self-loop filter, ADR 0012) — and
    /// a moved stamp means a moved HEAD, which the record's oid already
    /// fails against. So clearing it here would be a branch nothing can
    /// observe; the rule lives at the delete, which always writes.
    pub(crate) fn prune_records_of_unknown_changelists(&mut self) {
        let known: Vec<String> = self.changelists.iter().map(|cl| cl.name.clone()).collect();
        self.records
            .retain(|record| known.contains(&record.changelist));
    }

    /// Point `path`'s records at `target` for the given fresh hunks —
    /// the explicit assign op (ticket #32). Competing claims are
    /// replaced: records anchor-matching an assigned hunk, and live
    /// records overlapping its HEAD-side range, are removed so neither
    /// matching tier re-claims the hunk for its old owner. `target:
    /// None` is a release (ADR 0016): the deletion happens and nothing
    /// is written back — the hunks are recordless afterwards, so
    /// releasing already-recordless hunks changes nothing.
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
        if let Some(name) = target {
            for (hunk, anchor) in hunks.iter().zip(&anchors) {
                self.records
                    .push(matcher::record_for(path, hunk, anchor, name.to_owned()));
            }
        }
        Ok(())
    }

    /// Set the active marker. `target: None` is unassigned (ADR 0015),
    /// which always exists — the target that turns capture off.
    pub fn switch(&mut self, target: Option<&str>) -> Result<(), Error> {
        if let Some(name) = target
            && !self.contains(name)
        {
            return Err(Error::UnknownChangelist { name: name.into() });
        }
        self.active = target.map(str::to_owned);
        Ok(())
    }
}
