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
//! The other guard this verb alone has is the **unit-subset refusal**
//! ([`subset_of_a_unit`], #165): core widens every assign payload to the
//! whole index-entry unit (ADR 0009), so an address naming part of one would
//! become a sweep without saying so. Staging inherits neither guard —
//! membership is independent of staging, so `add`/`unstage` never widen.
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
    // Resolution first, scope second — the order `--containing` already
    // follows (#122), extended to the whole command line because the
    // unit-subset refusal is a question about the *set* an invocation
    // addressed: what one argument scopes to cannot be judged until every
    // other argument has said which hunks it named. Kept per-argument, so
    // the offenders below still come out in argument order.
    let narrowed: Vec<Result<Narrowed<'a>, String>> = resolved
        .iter()
        .map(|arg| narrow(arg, containing, snapshot))
        .collect();
    let addressed_set: Vec<&Narrowed<'a>> = narrowed
        .iter()
        .filter_map(|one| one.as_ref().ok())
        .collect();
    let mut targets: Vec<AssignTarget<'a>> = Vec::new();
    // One unit, one refusal: a second member addressed beside the first
    // offends identically and would print the same list twice. It still
    // meets the ownership guard, so the owner it would have named is not
    // lost with the duplicate line.
    let mut refused_units: Vec<&str> = Vec::new();
    for one in &narrowed {
        let one = match one {
            Ok(one) => one,
            Err(offender) => {
                offenders.push(offender.clone());
                continue;
            }
        };
        let scoped = match &one.addressed {
            None => sweepable(&one.path, target, args.take_owned, one.file)
                .map(|()| AssignTarget::Path(one.path.clone())),
            Some((hunk, address)) => match subset_of_a_unit(one, hunk, address, &addressed_set) {
                Err(refusal) if !refused_units.contains(&one.path.as_str()) => {
                    refused_units.push(&one.path);
                    Err(refusal)
                }
                _ => addressed(&one.path, hunk, address, target, args.take_owned),
            },
        };
        match scoped {
            Ok(assignable) => targets.push(assignable),
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

/// One argument resolved against the snapshot, before any of this verb's
/// scope guards have run. Every narrowed argument reached a file of the
/// change universe — a path with nothing there refuses at resolution — and
/// some of them named one hunk of it.
struct Narrowed<'a> {
    /// Repo-relative, as every gitchange surface prints paths (#122).
    path: String,
    /// The path's entry in the change universe.
    file: &'a ChangedFile,
    /// The hunk an address named, with the composed address a refusal would
    /// name it by; `None` where the argument was a bare path and so sweeps
    /// the file whole.
    addressed: Option<(&'a Hunk, String)>,
}

/// What one path argument resolves to, in the shared grammar's dispatch
/// ([`scope::narrow`]): this verb supplies only its two answers — a whole
/// path that has something to sweep, or the single hunk an address named.
fn narrow<'a>(
    arg: &scope::PathArg,
    containing: Option<&str>,
    snapshot: &'a Snapshot,
) -> Result<Narrowed<'a>, String> {
    // The same lookup `scope::narrow` hands the whole-path arm, taken here
    // so the addressed arm carries the file too: the unit-subset guard needs
    // the entry an address sits in, not just the hunk it named.
    let entry = scope::file_in(snapshot, &arg.path);
    let of = |addressed| match entry {
        Some(file) => Ok(Narrowed {
            path: arg.path.clone(),
            file,
            addressed,
        }),
        // It resolved as a path and is absent from the change universe: a
        // clean file, or one that never changed. (An address can only ever
        // have resolved inside a file that is there, so this is the
        // whole-path arm's answer in practice.)
        None => Err(nothing_to_assign(&arg.path)),
    };
    scope::narrow(
        arg,
        containing,
        snapshot,
        nothing_to_assign,
        |_| of(None),
        |hunk, address| of(Some((hunk, address.to_owned()))),
    )
}

/// Whether `arg`'s address names only *part* of its file's index-entry unit
/// — the refusal that keeps an address from widening (#147/#165, ADR 0009).
///
/// Core widens every assign payload to the whole unit, so handing it one
/// member would move them all: an address becoming a sweep, which is exactly
/// the move `--containing`'s multi-match refusal already rejects, and which
/// in a split unit takes another changelist's hunks. So the CLI never hands
/// core a proper subset, and core's widening no-ops instead of being
/// overridden — ADR 0009 is untouched, and its "moves them together" holds
/// because what the caller names is the whole unit.
///
/// The addressed set is the whole command line's, not one argument's: naming
/// every member across several arguments is the retry the refusal asks for.
/// A whole-path argument beside them satisfies it outright — a sweep names
/// everything — and a single-member unit can never be a proper subset of
/// itself.
fn subset_of_a_unit(
    arg: &Narrowed<'_>,
    hunk: &Hunk,
    address: &str,
    narrowed: &[&Narrowed<'_>],
) -> Result<(), String> {
    // Membership is core's own predicate — the one `widen_to_entry_unit`
    // gates on — so the CLI cannot come to refuse a set core would not
    // widen. A mode hunk and a file presenting no whole-file hunk are both
    // "not in the unit" there (ADR 0017, ADR 0009).
    if !arg.file.in_entry_unit(hunk) {
        return Ok(());
    }
    let members = unit_members(arg.file);
    // The only member: naming it is naming the unit, so nothing widens.
    if members.len() < 2 {
        return Ok(());
    }
    let mut named: Vec<&str> = Vec::new();
    for one in narrowed.iter().filter(|one| one.path == arg.path) {
        match &one.addressed {
            Some((_, address)) => named.push(address),
            None => return Ok(()),
        }
    }
    if members
        .iter()
        .all(|member| named.contains(&member.as_str()))
    {
        return Ok(());
    }
    Err(format!(
        "'{address}' is one of {} hunks sharing an index entry, which assign moves as one \
         — name them all: {}, or sweep the path: '{}'",
        members.len(),
        members.join(", "),
        arg.path
    ))
}

/// The composed addresses of `file`'s index-entry unit, in file order — what
/// the refusal lists, and what a caller pastes back as the retry (#155).
fn unit_members(file: &ChangedFile) -> Vec<String> {
    file.hunks
        .iter()
        .zip(file.hunk_addresses())
        .filter(|(hunk, _)| file.in_entry_unit(hunk))
        .map(|(_, address)| address.abbreviated_at(&file.path))
        .collect()
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
/// `file` is the path's entry in the change universe, which resolution has
/// already established it has — a path with nothing to sweep never reaches
/// here, and neither does a conflicted one.
fn sweepable(
    path: &str,
    target: Option<&str>,
    take_owned: bool,
    file: &ChangedFile,
) -> Result<(), String> {
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
