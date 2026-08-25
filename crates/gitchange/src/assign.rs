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
//! from silent cross-contamination).
//!
//! Every refusal here is exit `1`: whether a name is a changelist, who owns
//! a file's hunks, and whether a path has any changes at all are repo facts,
//! which clap cannot see.

use std::path::Path;

use gitchange_core::{ChangeKind, Snapshot, UNASSIGNED, conflicted_hint, target_named};

use crate::AssignScope;
use crate::scope;

/// One validated assignment, in the terms core's sweep takes.
pub struct Assignment {
    /// The changelist that will own the hunks; `None` is unassigned — a
    /// recordless release (ADR 0016), not a parking spot.
    pub target: Option<String>,
    /// The paths to sweep, repo-relative and in argument order, each named
    /// once.
    pub paths: Vec<String>,
}

impl Assignment {
    /// What this assignment did, in the word its receipts use: a release is
    /// not a placement, so the two directions never share a verb.
    pub fn past_tense(&self) -> &'static str {
        match self.target {
            Some(_) => "assigned",
            None => "released",
        }
    }
}

/// Resolve one assign invocation against `snapshot`, or refuse naming every
/// offender.
pub fn resolve(
    args: &AssignScope,
    snapshot: &Snapshot,
    workdir: &Path,
) -> anyhow::Result<Assignment> {
    unbuilt_forms(args)?;
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
    let mut paths: Vec<String> = Vec::new();
    for arg in &resolved {
        match sweepable(&arg.path, target, args.take_owned, snapshot) {
            // One path named twice is one sweep: it takes the file's whole
            // universe whatever the argument list says, so a repeat that
            // reached core would move nothing extra and count everything
            // twice.
            Ok(()) => {
                if !paths.contains(&arg.path) {
                    paths.push(arg.path.clone());
                }
            }
            Err(offender) => offenders.push(offender),
        }
    }
    if !offenders.is_empty() {
        anyhow::bail!(scope::refusal(&offenders));
    }
    Ok(Assignment {
        target: target.map(str::to_owned),
        paths,
    })
}

/// The two addressed forms the sweep does not answer yet (#164/#165). Both
/// parse — the grammar is whole from the skeleton — and both refuse rather
/// than falling back on the whole-path sweep: an address quietly widening
/// into a sweep is the one move this verb's rules exist to prevent, so the
/// interim answer has to be a refusal. Deleted by #164, which builds them.
fn unbuilt_forms(args: &AssignScope) -> anyhow::Result<()> {
    if let Some(token) = args
        .paths
        .iter()
        .find(|token| scope::carries_an_address(token))
    {
        anyhow::bail!("addressing one hunk ('{token}') is not implemented yet — name the path");
    }
    if args.containing.containing.is_some() {
        anyhow::bail!("'--containing' is not implemented yet — name the path");
    }
    Ok(())
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
fn sweepable(
    path: &str,
    target: Option<&str>,
    take_owned: bool,
    snapshot: &Snapshot,
) -> Result<(), String> {
    let file = scope::file_in(snapshot, path);
    // Quarantined (ADR 0007): a conflicted path holds no hunks to own, so
    // every answer below it would be a confusing "nobody".
    if file.is_some_and(|file| file.kind == ChangeKind::Conflicted) {
        return Err(conflicted_hint(path));
    }
    let Some(file) = file else {
        // It resolved as a path and is absent from the change universe: a
        // clean file, or one that never changed — an answer, and the answer
        // is that there is nothing to assign, never a silent empty sweep.
        return Err(format!("'{path}' has no changes — nothing to assign"));
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
