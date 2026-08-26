//! The staging verbs' scope resolution (#145): what `add <changelist>
//! [<path>[:<hunk-id>]...] [--containing <line>]` — and, in the same
//! grammar, `unstage` — turns one command line into, validated against
//! the snapshot the invocation's persisting refresh produced.
//!
//! Validation is **all-or-nothing**: every argument is checked against
//! that one snapshot, and any offender refuses the whole command naming
//! every offender, so a refused command staged nothing and the retry is
//! the same command corrected (git's own model — `git add existing
//! missing` is fatal and adds nothing). Nothing here writes; the sweep the
//! resolved scope feeds is core's.
//!
//! The refusals are exit `1` throughout: whether a token is a changelist,
//! whose hunks live in a file, and whether a path has any changes at all
//! are repo facts, which clap cannot see. The exception is
//! [`check_grammar`], which answers before the repository is opened at
//! all — its three checks are grammar, and grammar is exit `2`.

use std::path::Path;

use gitchange_core::{ALL, ChangedFile, Hunk, Snapshot, StagingTarget, UNASSIGNED, holder_label};

use crate::StagingScope;
use crate::scope;

/// One validated staging scope, in the terms core's sweep takes.
pub struct Sweep<'a> {
    /// The changelist whose hunks move; `None` is unassigned, as
    /// everywhere.
    pub changelist: Option<String>,
    /// What the arguments narrowed the scope to, in argument order: a file
    /// row per path, one hunk per address. Empty is the bare form: every
    /// hunk the changelist owns, across all its files.
    pub targets: Vec<StagingTarget<'a>>,
}

/// This pair's `--containing` grammar checks (#145): the shared three
/// ([`scope::check_containing`]) in this pair's own words. Its arity arm is
/// reachable on both sides of one, `add`/`unstage`'s grammar allowing no
/// paths at all — a bare `add <changelist> --containing <line>` is a usage
/// error here where it cannot arise for `assign`.
pub fn check_grammar(verb: &str, args: &StagingScope) -> anyhow::Result<()> {
    scope::check_containing(
        args.containing.containing.as_deref(),
        &args.paths,
        &scope::Forms {
            addressed: format!("gitchange {verb} <changelist> <path>:<hunk-id>"),
            narrowed: format!("gitchange {verb} <changelist> <path> --containing <line>"),
        },
    )
}

/// Resolve one staging invocation against `snapshot`, or refuse naming
/// every offender. `verb` is the command's own name, which the refusals
/// that teach its grammar quote back.
pub fn resolve<'a>(
    verb: &str,
    args: &StagingScope,
    snapshot: &'a Snapshot,
    workdir: &Path,
) -> anyhow::Result<Sweep<'a>> {
    let tokens = args.paths.iter().map(String::as_str);
    let (resolved, mut offenders) = scope::locate_paths(tokens, snapshot, workdir);
    let changelist = match changelist_scope(verb, &args.changelist, snapshot, workdir) {
        Ok(changelist) => changelist,
        Err(bad_scope) => {
            // The changelist is the first argument, so its refusal leads;
            // the path offenders still ride along, so one round trip fixes
            // the whole command line.
            offenders.insert(0, bad_scope.offender);
            anyhow::bail!(refusal(&offenders, bad_scope.teach.as_deref()));
        }
    };
    // Ownership offenders come last because they are the only class that
    // needs a valid changelist to state: a scope nobody recognises cannot
    // say who owns a file's hunks instead.
    let mut targets: Vec<StagingTarget<'a>> = Vec::new();
    let value = args.containing.containing.as_deref();
    let narrowings: Vec<Result<StagingTarget<'a>, String>> = match resolved.as_slice() {
        // The bare form: the changelist itself is the scope, so a
        // changelist owning nothing is the offender.
        [] if snapshot.files_in(changelist).is_empty() => vec![Err(format!(
            "{} owns no hunks — nothing to {verb}",
            holder_label(changelist)
        ))],
        args => args
            .iter()
            .map(|arg| narrow(verb, changelist, arg, value, snapshot))
            .collect(),
    };
    for narrowing in narrowings {
        match narrowing {
            // One narrowing named twice is one narrowing: the sweep moves
            // a row's hunks once whatever the argument list says, so a
            // repeat that reached the echo would be the only place it
            // showed at all.
            Ok(target) => {
                if !already_named(&targets, &target) {
                    targets.push(target);
                }
            }
            Err(offender) => offenders.push(offender),
        }
    }
    if !offenders.is_empty() {
        anyhow::bail!(refusal(&offenders, None));
    }
    Ok(Sweep {
        changelist: changelist.map(str::to_owned),
        targets,
    })
}

/// Whether `target` is already in the list — the same file row, or the
/// same addressed hunk. Compared by what the caller named, path and
/// composed address, which is a hunk's unique name within its file: two
/// identical hunks named by their ordinals stay two targets, and one hunk
/// named twice is one.
fn already_named(targets: &[StagingTarget<'_>], target: &StagingTarget<'_>) -> bool {
    targets.iter().any(|named| match (named, target) {
        (StagingTarget::Row(named), StagingTarget::Row(row)) => named == row,
        (
            StagingTarget::Hunk {
                path: named,
                address: named_as,
                ..
            },
            StagingTarget::Hunk { path, address, .. },
        ) => named == path && named_as == address,
        _ => false,
    })
}

/// What one path argument narrows the scope to, in the shared grammar's
/// dispatch ([`scope::narrow`]): this pair supplies only its two answers —
/// the file row where the scope owns hunks in it, or one addressed hunk past
/// the consistency guard.
fn narrow<'a>(
    verb: &str,
    changelist: Option<&str>,
    arg: &scope::PathArg,
    containing: Option<&str>,
    snapshot: &'a Snapshot,
) -> Result<StagingTarget<'a>, String> {
    scope::narrow(
        arg,
        containing,
        snapshot,
        |path| nothing_to_do(verb, path),
        |file| match unowned(verb, changelist, &arg.path, file) {
            Some(offender) => Err(offender),
            None => Ok(StagingTarget::Row(arg.path.clone())),
        },
        |hunk, address| addressed(verb, changelist, &arg.path, hunk, address),
    )
}

/// One addressed hunk as a target, past **the scope consistency guard**:
/// a hunk the named changelist does not own refuses (#145) naming the
/// actual owner, which makes the retry a one-word edit of a command line
/// whose address is already right.
///
/// Deliberately stricter than the TUI, whose per-hunk `space` ignores
/// ownership: that selection is one human's statement of intent, where the
/// CLI's caller is an agent under ADR 0015's explicit-mode contract, and a
/// silent cross-ownership stage is exactly what the invariant rules out.
fn addressed<'a>(
    verb: &str,
    changelist: Option<&str>,
    path: &str,
    hunk: &'a Hunk,
    address: &str,
) -> Result<StagingTarget<'a>, String> {
    if !hunk.owned_by(changelist) {
        let owner = hunk.changelist.as_deref();
        return Err(format!(
            "hunk '{address}' belongs to {}, not {} — retry as 'gitchange {verb} {} {address}'",
            holder_label(owner),
            holder_label(changelist),
            owner.unwrap_or(UNASSIGNED)
        ));
    }
    Ok(StagingTarget::Hunk {
        path: path.to_owned(),
        hunk,
        address: address.to_owned(),
    })
}

/// A path that exists and simply has no changes: an answer, and for a
/// staging verb the answer is that there is nothing to do — never a silent
/// empty sweep.
fn nothing_to_do(verb: &str, path: &str) -> String {
    format!("'{path}' has no changes — nothing to {verb}")
}

/// The shared all-or-nothing refusal plus the grammar note where there is
/// one — on its own line at the end, which is where a reader looks for what
/// to do rather than what went wrong. Inside the `; `-joined list it would
/// read as one more offender, and split the list across a line break.
fn refusal(offenders: &[String], teach: Option<&str>) -> String {
    let offenders = scope::refusal(offenders);
    match teach {
        Some(teach) => format!("{offenders}\n{teach}"),
        None => offenders,
    }
}

/// An unusable changelist argument: the offender line for the all-or-
/// nothing list, plus the grammar note that belongs after it rather than
/// in it.
struct BadScope {
    offender: String,
    teach: Option<String>,
}

/// The changelist argument as core takes it (`None` is unassigned), or the
/// refusal it earns.
///
/// `all` is a view, not a scope — a sweep across changelist boundaries is
/// the op gitchange deliberately does not have. Anything else unrecognised
/// refuses with the candidates listed, and where the token also names a
/// path the refusal teaches the changelist-first grammar: `gitchange add
/// src/main.rs` is the git habit, and one round trip is what it should
/// cost.
fn changelist_scope<'a>(
    verb: &str,
    name: &'a str,
    snapshot: &Snapshot,
    workdir: &Path,
) -> Result<Option<&'a str>, BadScope> {
    if name == ALL {
        return Err(BadScope {
            offender: format!(
                // The verb is quoted rather than used as an adjective:
                // "a {verb} scope" reads as "a unstage scope" for one half
                // of the pair, and an article that depends on the caller's
                // verb is a wart the mirror would only spread.
                "'{ALL}' is a view, not a scope for '{verb}' — name one changelist, \
                 or '{UNASSIGNED}'"
            ),
            teach: None,
        });
    }
    let offender = match scope::recognised(name, &snapshot.changelists) {
        Ok(target) => return Ok(target),
        Err(offender) => offender,
    };
    Err(BadScope {
        offender,
        teach: scope::names_a_path(name, snapshot, workdir).then(|| {
            format!(
                "'{verb}' names the changelist first: 'gitchange {verb} <changelist> \
                 [<path>[:<hunk-id>]...]'"
            )
        }),
    })
}

/// The refusal a named path earns when the scope has nothing to move in
/// it, or `None` when it does.
///
/// A path narrows to the **file row** (changelist, path), so a file the
/// scope owns no hunks in is an offender rather than an empty sweep — and
/// the refusal names who owns them instead, which makes the retry a
/// one-word edit. That is also what keeps a co-owned file from leaking:
/// naming the path under one changelist can only ever move that
/// changelist's hunks.
///
/// `file` is the path's entry in the change universe, `None` where it has
/// none — the caller has it in hand, and the conflicted case is its to have
/// answered already.
fn unowned(
    verb: &str,
    changelist: Option<&str>,
    path: &str,
    file: Option<&ChangedFile>,
) -> Option<String> {
    let Some(file) = file else {
        // It resolved as a path and is absent from the change universe: a
        // clean file.
        return Some(nothing_to_do(verb, path));
    };
    if file.owned_hunks(changelist).next().is_some() {
        return None;
    }
    Some(format!(
        "no hunks of '{path}' belong to {} — they belong to {}",
        holder_label(changelist),
        // Every holder, unassigned included: whoever holds them is who the
        // retry has to name.
        scope::holders(file, |_| true).join(", ")
    ))
}
