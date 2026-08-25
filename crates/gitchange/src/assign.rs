//! `assign`'s scope resolution (#147): what `assign <path>... (--to
//! <changelist> | --unassign)` turns one command line into, validated
//! against the snapshot the invocation's persisting refresh produced.
//!
//! Validation is **all-or-nothing**: every argument is checked against that
//! one snapshot, and any offender refuses the whole command naming every
//! offender, so a refused command assigned nothing and the retry is the same
//! command corrected (git's own model — `git add existing missing` is fatal
//! and adds nothing). Nothing here writes; the sweep the resolved scope
//! feeds is core's.
//!
//! The guard this verb has and the staging pair does not is **ownership**: a
//! staging sweep is ownership-scoped by construction (CONTEXT.md §Sweep), so
//! it can only ever move what the named changelist holds, where a membership
//! sweep takes the path's whole universe and could take another changelist's
//! work. Default-on, and the override is the named flag `--take-owned`
//! (ADR 0015's loud-CLI polarity: an opt-in guard is one forgotten argument
//! from silent cross-contamination). An addressed argument narrows the guard
//! to the hunk it names: a foreign hunk elsewhere in the file is irrelevant
//! to an exact address (#164).
//!
//! Every refusal here is exit `1`: whether a name is a changelist, who owns
//! a file's hunks, and whether a path has any changes at all are repo facts,
//! which clap cannot see. The exception is [`check_grammar`], which answers
//! before the repository is opened at all — its checks are grammar, and
//! grammar is exit `2`.

use std::path::Path;

use gitchange_core::{
    AssignTarget, ChangedFile, Hunk, Snapshot, UNASSIGNED, holder_label, target_named,
};

use crate::AssignScope;
use crate::scope;

/// One validated assignment, in the terms core's sweep takes.
pub struct Assignment<'a> {
    /// The changelist that will own the hunks; `None` is unassigned — a
    /// recordless release (ADR 0016), not a parking spot.
    pub target: Option<String>,
    /// What the arguments narrowed the assignment to, in argument order: a
    /// whole path per bare path argument, one hunk per address.
    pub targets: Vec<AssignTarget<'a>>,
}

impl Assignment<'_> {
    /// What this assignment did, in the word its receipts use: a release is
    /// not a placement, so the two directions never share a verb.
    pub fn past_tense(&self) -> &'static str {
        match self.target {
            Some(_) => "assigned",
            None => "released",
        }
    }
}

/// This verb's `--containing` grammar checks (#147): the shared three
/// ([`scope::check_containing`]) in this verb's own words. Only one side of
/// the arity arm is reachable here — the grammar requires at least one path,
/// so the rule is exactly one and the offender is always a second.
pub fn check_grammar(args: &AssignScope) -> anyhow::Result<()> {
    scope::check_containing(
        args.containing.containing.as_deref(),
        &args.paths,
        &scope::Forms {
            addressed: "gitchange assign <path>:<hunk-id> --to <changelist>".to_owned(),
            narrowed: "gitchange assign <path> --containing <line> --to <changelist>".to_owned(),
        },
    )
}

/// Resolve one assign invocation against `snapshot`, or refuse naming every
/// offender.
pub fn resolve<'a>(
    args: &AssignScope,
    snapshot: &'a Snapshot,
    workdir: &Path,
) -> anyhow::Result<Assignment<'a>> {
    let tokens = args.paths.iter().map(String::as_str);
    let (resolved, mut offenders) = scope::locate_paths(tokens, snapshot, workdir);
    let target = match target_of(args, snapshot) {
        Ok(target) => target,
        Err(offender) => {
            // The target is what every other class is judged against, so its
            // refusal leads; the path offenders still ride along, so one
            // round trip fixes the whole command line.
            offenders.insert(0, offender);
            anyhow::bail!(scope::refusal(&offenders));
        }
    };
    // Redundancy in the argument list is core's to absorb, not this
    // resolution's: a membership write is one locked cycle per path, so
    // `Repo::assign_sweep` groups one payload per distinct path anyway — and
    // a path named twice, a hunk addressed twice, and an address inside a
    // path swept whole all move once and count once as a consequence. The
    // staging pair dedupes on this side instead, its core traversal being
    // per-snapshot-file rather than per-payload.
    let containing = args.containing.containing.as_deref();
    let mut targets: Vec<AssignTarget<'a>> = Vec::new();
    for arg in &resolved {
        match narrow(arg, containing, target, args.take_owned, snapshot) {
            Ok(narrowed) => targets.push(narrowed),
            Err(offender) => offenders.push(offender),
        }
    }
    if !offenders.is_empty() {
        anyhow::bail!(scope::refusal(&offenders));
    }
    Ok(Assignment {
        target: target.map(str::to_owned),
        targets,
    })
}

/// What one path argument narrows the assignment to, in the shared
/// grammar's dispatch ([`scope::narrow`]): this verb supplies only its two
/// answers — a whole path past the sweep's guard, or one addressed hunk past
/// the same guard narrowed to it.
fn narrow<'a>(
    arg: &scope::PathArg,
    containing: Option<&str>,
    target: Option<&str>,
    take_owned: bool,
    snapshot: &'a Snapshot,
) -> Result<AssignTarget<'a>, String> {
    scope::narrow(
        arg,
        containing,
        snapshot,
        nothing_to_assign,
        |file| {
            sweepable(&arg.path, target, take_owned, file)
                .map(|()| AssignTarget::Path(arg.path.clone()))
        },
        |hunk, address| addressed(&arg.path, hunk, address, target, take_owned),
    )
}

/// One addressed hunk as a target, past **the ownership guard narrowed to
/// that hunk alone** (#147): a foreign hunk elsewhere in the file is
/// irrelevant to an exact address, so what the guard asks is only whether
/// this hunk is somebody else's. `--take-owned` composes here exactly as it
/// does over a sweep.
fn addressed<'a>(
    path: &str,
    hunk: &'a Hunk,
    address: &str,
    target: Option<&str>,
    take_owned: bool,
) -> Result<AssignTarget<'a>, String> {
    let owner = hunk.changelist.as_deref();
    // Unassigned is not owned, and the target's own is nobody's to take: what
    // is left is the cross-contamination the guard exists to refuse.
    if !take_owned && owner.is_some() && owner != target {
        return Err(format!(
            "hunk '{address}' is owned by {} — '--take-owned' takes it too",
            holder_label(owner)
        ));
    }
    Ok(AssignTarget::Hunk {
        path: path.to_owned(),
        hunk,
    })
}

/// A path that exists and simply has no changes: an answer, and the answer is
/// that there is nothing to assign — never a silent empty sweep.
fn nothing_to_assign(path: &str) -> String {
    format!("'{path}' has no changes — nothing to assign")
}

/// The target as core takes it (`None` is unassigned), or the refusal it
/// earns. `--unassign` is sugar for `--to unassigned` and arrives here as
/// the absent `--to`, the two spellings being one target by the grammar's
/// own exclusive group.
///
/// An unrecognised name refuses with the candidates listed (#122's gh
/// shape). Assign never creates: creation is `changelist <name>`'s op, and a
/// typo'd target that silently minted a changelist would misfile work with
/// no refusal anywhere.
fn target_of<'a>(args: &'a AssignScope, snapshot: &Snapshot) -> Result<Option<&'a str>, String> {
    let Some(name) = args.to.as_deref() else {
        debug_assert!(
            args.unassign,
            "the required target group leaves no third way"
        );
        return Ok(None);
    };
    let known = name == UNASSIGNED
        || snapshot
            .changelists
            .iter()
            .any(|changelist| changelist.name == name);
    match known {
        true => Ok(target_named(name)),
        false => Err(format!(
            "no changelist named '{name}' — {}",
            scope::changelist_scopes(snapshot)
        )),
    }
}

/// Whether this path may be swept into `target`, or the refusal it earns.
///
/// A path sweeps its **change universe** — every hunk of the file, staged or
/// not, membership and staging being separate axes (ADR 0003) — so the
/// questions are only whether there is anything there and whether taking all
/// of it takes somebody else's work.
///
/// Hunks `target` already owns are satisfied, not offenders: repeating an
/// assign is idempotent, and so is releasing what is already unassigned.
///
/// `file` is the path's entry in the change universe, `None` where it has
/// none — the caller has it in hand, and the conflicted case is its to have
/// answered already.
fn sweepable(
    path: &str,
    target: Option<&str>,
    take_owned: bool,
    file: Option<&ChangedFile>,
) -> Result<(), String> {
    let Some(file) = file else {
        // It resolved as a path and is absent from the change universe: a
        // clean file, or one that never changed.
        return Err(nothing_to_assign(path));
    };
    if take_owned {
        return Ok(());
    }
    // Unassigned hunks are not owned, and the target's own are not taken
    // from anyone: what is left is the cross-contamination the guard exists
    // to refuse.
    match scope::holders(file, |owner| owner.is_some() && owner != target) {
        owners if owners.is_empty() => Ok(()),
        owners => Err(format!(
            "'{path}' holds hunks owned by {} — '--take-owned' takes them too",
            owners.join(", ")
        )),
    }
}
