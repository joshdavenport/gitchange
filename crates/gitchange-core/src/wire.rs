//! The machine surface (ADR 0018): the one place gitchange's JSON
//! dialect is composed. `status --json` and `diff --json` are serialised
//! here and nowhere else, so the two surfaces cannot drift — they share
//! [`SCHEMA_VERSION`], the change-kind and staging spellings, and the
//! ordering promise.
//!
//! The dialect: one envelope object per read, snake_case field names,
//! unions discriminated by an explicit `kind` string, bytes as plain
//! JSON strings, and no advisories field anywhere — a read-only refresh
//! decides nothing, so it has nothing to advise (ADR 0005).
//!
//! The wire types are private on purpose. Only the two serialised
//! documents leave this module — plus [`HunkContent`], which is an input
//! rather than a shape — so no frontend can assemble a machine-surface
//! field of its own, and every addition to the dialect is a change to
//! this file.

use serde::Serialize;

use crate::diff::{ChangeKind, FileSides, HunkLine, ModeDelta, SideInfo};
use crate::hunk_id::HunkAddress;
use crate::snapshot::{FileGroup, GitOperation, GroupKind, Head, Snapshot};
use crate::universe::{ChangedFile, FileStage, Hunk, HunkIdentity, HunkStage};

/// The version of the JSON dialect, carried by every read envelope as an
/// integer `schema_version` field. Global to the dialect: the two read
/// surfaces share types, so a breaking change to either bumps this one
/// number for both.
///
/// **The additive contract** published beside it (ADR 0018): adding a
/// field, adding a member to a string-typed enum, and populating a
/// previously-null field are *not* breaking and do not bump this;
/// renaming, removing, retyping, or changing the meaning of a field
/// does. Consumers must ignore unknown fields and unknown enum values.
///
/// Distinct from the state file's schema version
/// ([`crate::state::SCHEMA_VERSION`]), whose mismatch is a refusal —
/// two versions, two policies.
const SCHEMA_VERSION: u32 = 1;

/// `status --json`'s envelope: the repo context an agent orients by, plus
/// the All-view groups. One document, ready to print.
///
/// Both faces render `Snapshot::groups`, so the text and the JSON cannot
/// disagree about what is in a group or what order the groups come in.
pub fn status_envelope(snapshot: &Snapshot) -> String {
    // Bound rather than inlined: a group's name is the group's own
    // (`GroupKind` owns it), so the wire objects borrow from here.
    let groups = snapshot.groups();
    document(&StatusEnvelope {
        schema_version: SCHEMA_VERSION,
        head: HeadWire::from(&snapshot.head),
        operation: snapshot.operation.map(OperationWire::from),
        active: snapshot.active.as_deref(),
        groups: groups.iter().map(GroupWire::of).collect(),
    })
}

/// `diff --json`'s envelope: the addressed hunk detail behind the files
/// it is handed. One document, ready to print.
///
/// The files are serialised in the order given — the promise is "files in
/// path order", and every caller resolves its scope out of the snapshot,
/// which is path-sorted. Sorting again here would let the JSON reorder
/// what the text face printed, which is the one thing ADR 0018 forbids.
/// Hunks keep their file order for the same reason: it is already the
/// order `ChangedFile::hunks` carries (mode hunk first, ADR 0017).
pub fn diff_envelope(files: &[&ChangedFile], content: HunkContent) -> String {
    document(&DiffEnvelope {
        schema_version: SCHEMA_VERSION,
        files: files
            .iter()
            .map(|file| DiffFileWire::of(file, content))
            .collect(),
    })
}

/// Whether a diff envelope carries its text hunks' lines — `diff`'s
/// `--no-content`, which switches the content off and nothing else (#159):
/// the same envelope, the same file objects, the same IDs, so what a caller
/// addresses by never changes with it.
///
/// It reaches the serialiser rather than being applied to the output
/// afterwards because the dialect is composed in one place (ADR 0018): a
/// frontend stripping fields out of a finished document would be a second
/// author of the shape.
#[derive(Debug, Clone, Copy)]
pub enum HunkContent {
    Included,
    Omitted,
}

/// Render one envelope as the compact single-line document that goes to
/// stdout. Serialisation cannot fail: every type below is a plain struct
/// or enum of strings, integers, and booleans, with no map keys and no
/// floats to be non-finite.
fn document(envelope: &impl Serialize) -> String {
    serde_json::to_string(envelope).expect("the wire types serialise")
}

#[derive(Serialize)]
struct StatusEnvelope<'a> {
    schema_version: u32,
    head: HeadWire<'a>,
    operation: Option<OperationWire>,
    /// The active changelist, `null` for unassigned-active. The marker
    /// lives here alone; group objects never repeat it.
    active: Option<&'a str>,
    groups: Vec<GroupWire<'a>>,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum HeadWire<'a> {
    Branch { name: &'a str },
    Detached { short_id: &'a str },
    Unborn { name: &'a str },
}

impl<'a> From<&'a Head> for HeadWire<'a> {
    fn from(head: &'a Head) -> Self {
        match head {
            Head::Branch { name } => HeadWire::Branch { name },
            Head::Detached { short_id } => HeadWire::Detached { short_id },
            Head::Unborn { name } => HeadWire::Unborn { name },
        }
    }
}

/// The in-progress operation as a plain string — reporting only, and a
/// string rather than an object because it has nothing to carry beyond
/// its name.
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum OperationWire {
    Merge,
    Rebase,
    CherryPick,
    Revert,
    Am,
}

impl From<GitOperation> for OperationWire {
    fn from(operation: GitOperation) -> Self {
        match operation {
            GitOperation::Merge => OperationWire::Merge,
            GitOperation::Rebase => OperationWire::Rebase,
            GitOperation::CherryPick => OperationWire::CherryPick,
            GitOperation::Revert => OperationWire::Revert,
            GitOperation::Am => OperationWire::Am,
        }
    }
}

/// One All-view group. A conflicts group's files carry a path alone:
/// quarantined paths own no hunks (ADR 0007), so a stage mark or a hunk
/// count there would be a fabricated fact.
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum GroupWire<'a> {
    Conflicts {
        files: Vec<ConflictedFileWire<'a>>,
    },
    Changelist {
        name: &'a str,
        files: Vec<StatusFileWire<'a>>,
    },
    Unassigned {
        files: Vec<StatusFileWire<'a>>,
    },
}

impl<'a> GroupWire<'a> {
    /// One group as its wire object. The `active` flag is deliberately
    /// dropped: the marker lives in the envelope's `active` field alone,
    /// so a group object cannot disagree with it.
    fn of<'files: 'a>(group: &'a FileGroup<'files>) -> Self {
        match &group.kind {
            GroupKind::Conflicts => GroupWire::Conflicts {
                files: group
                    .files
                    .iter()
                    .map(|file| ConflictedFileWire { path: &file.path })
                    .collect(),
            },
            GroupKind::Changelist { name, .. } => GroupWire::Changelist {
                name,
                files: status_files(&group.files),
            },
            GroupKind::Unassigned { .. } => GroupWire::Unassigned {
                files: status_files(&group.files),
            },
        }
    }
}

fn status_files<'a>(files: &[&'a ChangedFile]) -> Vec<StatusFileWire<'a>> {
    files
        .iter()
        .map(|file| StatusFileWire::from(*file))
        .collect()
}

#[derive(Serialize)]
struct ConflictedFileWire<'a> {
    path: &'a str,
}

/// A status file row: whole-file facts, whatever group it appears under,
/// so a path two changelists own reads the same on both rows.
#[derive(Serialize)]
struct StatusFileWire<'a> {
    path: &'a str,
    change_kind: ChangeKindWire,
    stage: FileStageWire,
    /// Cleanly staged (`●`) hunks only: a staged-stale hunk surfaces
    /// through `partially_staged` and is never counted staged.
    staged_hunks: usize,
    total_hunks: usize,
}

impl<'a> From<&'a ChangedFile> for StatusFileWire<'a> {
    fn from(file: &'a ChangedFile) -> Self {
        Self {
            path: &file.path,
            change_kind: file.kind.into(),
            stage: file.stage().into(),
            staged_hunks: file.staged_hunks(),
            total_hunks: file.total_hunks(),
        }
    }
}

#[derive(Serialize)]
struct DiffEnvelope<'a> {
    schema_version: u32,
    files: Vec<DiffFileWire<'a>>,
}

/// A diff file object. `sides` is the single carrier of content facts —
/// present exactly when the file presents a whole-file hunk — and there
/// are no stored labels: the consumer derives binary / type change /
/// empty add-or-delete from the facts (ADR 0017 as amended). A
/// conflicted file states its quarantine rather than omitting it:
/// `hunks: []`, `sides: null`.
#[derive(Serialize)]
struct DiffFileWire<'a> {
    path: &'a str,
    change_kind: ChangeKindWire,
    binary: bool,
    sides: Option<SidesWire<'a>>,
    hunks: Vec<HunkWire<'a>>,
}

impl<'a> DiffFileWire<'a> {
    fn of(file: &'a ChangedFile, content: HunkContent) -> Self {
        Self {
            path: &file.path,
            change_kind: file.kind.into(),
            binary: file.binary,
            sides: file.sides.as_ref().map(SidesWire::from),
            // Addresses are minted per file: the ordinal that tells
            // identical hunks apart is a fact about the whole file.
            hunks: file
                .hunks
                .iter()
                .zip(file.hunk_addresses())
                .map(|(hunk, address)| HunkWire::of(file, hunk, address, content))
                .collect(),
        }
    }
}

#[derive(Serialize)]
struct SidesWire<'a> {
    head: Option<SideWire<'a>>,
    changed: Option<SideWire<'a>>,
}

impl<'a> From<&'a FileSides> for SidesWire<'a> {
    fn from(sides: &'a FileSides) -> Self {
        Self {
            head: sides.head.as_ref().map(SideWire::from),
            changed: sides.changed.as_ref().map(SideWire::from),
        }
    }
}

#[derive(Serialize)]
struct SideWire<'a> {
    oid: &'a str,
    size: u64,
}

impl<'a> From<&'a SideInfo> for SideWire<'a> {
    fn from(side: &'a SideInfo) -> Self {
        Self {
            oid: &side.oid,
            size: side.size,
        }
    }
}

/// One hunk, discriminated by what it *is* — the flavours are mutually
/// exclusive by construction ([`HunkIdentity`]), and each carries only
/// the facts its flavour has: coordinates and lines for a text hunk, the
/// mode delta it owns for a degenerate one.
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum HunkWire<'a> {
    Text {
        #[serde(flatten)]
        common: HunkCommonWire<'a>,
        old_start: u32,
        old_lines: u32,
        new_start: u32,
        new_lines: u32,
        /// The hunk's content, absent under `--no-content` (#159) —
        /// omitted rather than `null`, since the lines were never nothing:
        /// the caller asked not to be sent them. The dialect's one
        /// licensed omission (ADR 0018 as amended), and the flag is what
        /// says so.
        #[serde(skip_serializing_if = "Option::is_none")]
        lines: Option<Vec<LineWire<'a>>>,
    },
    WholeFile {
        #[serde(flatten)]
        common: HunkCommonWire<'a>,
        /// The type delta — non-`null` exactly for a type change. A
        /// permission flip belongs to the mode hunk beside this one.
        mode_delta: Option<ModeDeltaWire>,
    },
    Mode {
        #[serde(flatten)]
        common: HunkCommonWire<'a>,
        /// The permission flip, which is why this hunk exists — so in
        /// practice never `null`. `Option` all the same: the delta is the
        /// file's snapshot data (#112), and inventing modes to fill a
        /// gap git left would be worse than saying nothing.
        mode_delta: Option<ModeDeltaWire>,
    },
}

impl<'a> HunkWire<'a> {
    /// `hunk` as its flavour, reading the mode facts its flavour owns off
    /// the file it belongs to — mode facts are hunk-attributed snapshot
    /// data (#112), and which delta a hunk owns follows from its flavour:
    /// the permission flip is the mode hunk's, the type delta the
    /// whole-file hunk's (ADR 0017).
    fn of(
        file: &'a ChangedFile,
        hunk: &'a Hunk,
        address: HunkAddress,
        content: HunkContent,
    ) -> Self {
        let common = HunkCommonWire::of(hunk, address);
        match &hunk.identity {
            HunkIdentity::Text { lines } => HunkWire::Text {
                common,
                old_start: hunk.old_start,
                old_lines: hunk.old_lines,
                new_start: hunk.new_start,
                new_lines: hunk.new_lines,
                // Only a text hunk has lines to withhold, so the switch
                // lands here and the degenerate flavours are untouched.
                lines: match content {
                    HunkContent::Included => Some(lines.iter().map(LineWire::from).collect()),
                    HunkContent::Omitted => None,
                },
            },
            HunkIdentity::WholeFile { .. } => HunkWire::WholeFile {
                common,
                mode_delta: mode_delta_of(file, ModeDelta::is_type_change),
            },
            HunkIdentity::ModeChange => HunkWire::Mode {
                common,
                mode_delta: mode_delta_of(file, ModeDelta::is_flip),
            },
        }
    }
}

/// The file's mode delta where it is the flavour `wanted` accepts — a
/// hunk asks for the one its flavour owns and gets nothing for the
/// other's.
fn mode_delta_of(
    file: &ChangedFile,
    wanted: impl FnOnce(&ModeDelta) -> bool,
) -> Option<ModeDeltaWire> {
    file.mode_delta.filter(wanted).map(ModeDeltaWire::from)
}

/// The fields every hunk flavour carries, flattened into each variant so
/// the document reads as one object per hunk.
#[derive(Serialize)]
struct HunkCommonWire<'a> {
    /// The hunk's ID with its `h` sigil and all 64 hex digits
    /// (`CONTEXT.md` §Hunk ID). The sigil travels: agents copy addresses
    /// out of this document, so the anti-misread device belongs on it
    /// (#122). Text faces abbreviate; the wire never does.
    id: String,
    /// The file-order ordinal among hunks sharing `id`, `null` for a
    /// unique hunk — non-`null` exactly when the composed address needs
    /// its `/<n>`.
    offset: Option<usize>,
    /// Owning changelist, `null` for unassigned.
    changelist: Option<&'a str>,
    stage: HunkStageWire,
    /// True for a hunk only the index diff sees — staged, then reverted
    /// in the worktree. Always reports `staged_stale`.
    index_only: bool,
}

impl<'a> HunkCommonWire<'a> {
    fn of(hunk: &'a Hunk, address: HunkAddress) -> Self {
        Self {
            id: address.id.to_string(),
            offset: address.offset,
            changelist: hunk.changelist.as_deref(),
            stage: hunk.stage.into(),
            index_only: hunk.index_only,
        }
    }
}

/// One diff line: `origin` follows git's convention — `" "`, `"+"`,
/// `"-"`, plus the no-newline markers `"="`, `">"`, `"<"` — and content
/// is verbatim, its trailing newline included. Bytes are plain JSON
/// strings: diff content is lossily UTF-8 before it reaches here, so a
/// `{text|bytes}` union would advertise fidelity gitchange does not have
/// (ADR 0010, ADR 0018).
#[derive(Serialize)]
struct LineWire<'a> {
    origin: char,
    content: &'a str,
}

impl<'a> From<&'a HunkLine> for LineWire<'a> {
    fn from(line: &'a HunkLine) -> Self {
        Self {
            origin: line.origin,
            content: &line.content,
        }
    }
}

/// A filemode delta as octal strings, exactly as git prints modes.
/// Strings rather than numbers: `100644` read as decimal is a different
/// mode, and a consumer that never parses them still prints them right.
#[derive(Serialize)]
struct ModeDeltaWire {
    before: String,
    after: String,
}

impl From<ModeDelta> for ModeDeltaWire {
    fn from(delta: ModeDelta) -> Self {
        let (before, after) = match delta {
            ModeDelta::Mode { before, after } | ModeDelta::Type { before, after } => {
                (before, after)
            }
        };
        Self {
            before: format!("{before:o}"),
            after: format!("{after:o}"),
        }
    }
}

/// The six change kinds, shared by both read surfaces. There is no
/// `renamed` member: rename detection is off, so a rename presents as a
/// delete plus an add (ADR 0011).
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum ChangeKindWire {
    Added,
    Modified,
    Deleted,
    TypeChanged,
    Untracked,
    Conflicted,
}

impl From<ChangeKind> for ChangeKindWire {
    fn from(kind: ChangeKind) -> Self {
        match kind {
            ChangeKind::Added => ChangeKindWire::Added,
            ChangeKind::Modified => ChangeKindWire::Modified,
            ChangeKind::Deleted => ChangeKindWire::Deleted,
            ChangeKind::TypeChanged => ChangeKindWire::TypeChanged,
            ChangeKind::Untracked => ChangeKindWire::Untracked,
            ChangeKind::Conflicted => ChangeKindWire::Conflicted,
        }
    }
}

/// The staging set's file-level members (`CONTEXT.md` §Staging set), spelled
/// out rather than glyphed: `◐` is per-file alone.
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum FileStageWire {
    Staged,
    PartiallyStaged,
    Unstaged,
}

impl From<FileStage> for FileStageWire {
    fn from(stage: FileStage) -> Self {
        match stage {
            FileStage::Staged => FileStageWire::Staged,
            FileStage::PartiallyStaged => FileStageWire::PartiallyStaged,
            FileStage::Unstaged => FileStageWire::Unstaged,
        }
    }
}

/// The staging set's hunk-level members: `◑` is per-hunk alone, and it
/// rolls up into a file's `partially_staged`.
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum HunkStageWire {
    Staged,
    Unstaged,
    StagedStale,
}

impl From<HunkStage> for HunkStageWire {
    fn from(stage: HunkStage) -> Self {
        match stage {
            HunkStage::Staged => HunkStageWire::Staged,
            HunkStage::Unstaged => HunkStageWire::Unstaged,
            HunkStage::StagedStale => HunkStageWire::StagedStale,
        }
    }
}
