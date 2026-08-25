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

use gitchange_core::{
    ALL, ChangeKind, ChangedFile, Hunk, Snapshot, StagingTarget, UNASSIGNED, conflicted_hint,
    holder_label, target_named,
};

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

/// The three non-declarative exit-`2` checks this pair owns (#145), asked
/// of the raw arguments alone: they are grammar violations, so they answer
/// before the repository is opened and cannot be confused with a refusal
/// about repo state. The skeleton could not declare them — clap sees
/// neither a value's newlines, nor how many positionals an option needs
/// beside it, nor whether one of them is ID-shaped.
pub fn check_grammar(verb: &str, args: &StagingScope) -> anyhow::Result<()> {
    let Some(value) = &args.containing.containing else {
        return Ok(());
    };
    if value.contains('\n') {
        return Err(crate::usage(format!(
            "'--containing' matches within one changed line, and this value spans \
             several — name a line, or address the hunk: 'gitchange {verb} \
             <changelist> <path>:<hunk-id>'"
        )));
    }
    // Exactly one path, this pair's own arity: `assign`'s grammar
    // guarantees at least one path, `add`/`unstage`'s does not, so a bare
    // `add <changelist> --containing <line>` is a usage error here where
    // it cannot arise there.
    if args.paths.len() != 1 {
        return Err(crate::usage(format!(
            "'--containing' narrows exactly one path, and {} were given — \
             'gitchange {verb} <changelist> <path> --containing <line>'",
            args.paths.len()
        )));
    }
    if let Some(token) = args
        .paths
        .iter()
        .find(|token| scope::carries_an_address(token))
    {
        return Err(crate::usage(format!(
            "'--containing' and '<path>:<hunk-id>' are two ways to address one \
             hunk — '{token}' already names one, so drop one of them"
        )));
    }
    Ok(())
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

/// What one path argument narrows the scope to: its file row, the single
/// hunk an address named, or — under `--containing <line>` — the hunk
/// holding that line. The three are one function because every one of them
/// is "this path, narrowed", and they share the quarantine check and the
/// consistency guard either side.
///
/// `--containing` resolves over the path's whole universe and only then
/// checks the verb's scope (#145), so the same value never means one hunk
/// under `add` and another under `unstage`.
///
/// An address's own refusals (not found, stale, ambiguous — #122/#158)
/// come back as offenders like any other, so an aged address joins the
/// all-or-nothing list rather than bailing ahead of the arguments beside
/// it. An explicit ID refusing here is what keeps a stale address from
/// ever acting.
fn narrow<'a>(
    verb: &str,
    changelist: Option<&str>,
    arg: &scope::PathArg,
    value: Option<&str>,
    snapshot: &'a Snapshot,
) -> Result<StagingTarget<'a>, String> {
    let file = scope::file_in(snapshot, &arg.path);
    // Quarantined (ADR 0007): a conflicted path holds no hunks to own or
    // address, so every answer below it would be a confusing "nobody".
    if file.is_some_and(|file| file.kind == ChangeKind::Conflicted) {
        return Err(conflicted_hint(&arg.path));
    }
    if let Some(value) = value {
        let file = file.ok_or_else(|| nothing_to_do(verb, &arg.path))?;
        let (hunk, address) = scope::resolve_containing(file, value)?;
        return addressed(verb, changelist, &arg.path, hunk, address);
    }
    match arg.resolve_hunk(snapshot) {
        Ok(None) => match unowned(verb, changelist, &arg.path, file) {
            Some(offender) => Err(offender),
            None => Ok(StagingTarget::Row(arg.path.clone())),
        },
        Ok(Some((hunk, address))) => addressed(verb, changelist, &arg.path, hunk, address),
        Err(refusal) => Err(format!("{refusal:#}")),
    }
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
    address: String,
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
        address,
    })
}

/// A path that exists and simply has no changes: an answer, and for a
/// staging verb the answer is that there is nothing to do — never a silent
/// empty sweep.
fn nothing_to_do(verb: &str, path: &str) -> String {
    format!("'{path}' has no changes — nothing to {verb}")
}

/// The all-or-nothing refusal: every offender in argument order, and the
/// grammar note where there is one — on its own line at the end, which is
/// where a reader looks for what to do rather than what went wrong. Inside
/// the `; `-joined list it would read as one more offender, and split the
/// list across a line break.
fn refusal(offenders: &[String], teach: Option<&str>) -> String {
    match teach {
        Some(teach) => format!("{}\n{teach}", offenders.join("; ")),
        None => offenders.join("; "),
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
    let known = name == UNASSIGNED
        || snapshot
            .changelists
            .iter()
            .any(|changelist| changelist.name == name);
    if known {
        return Ok(target_named(name));
    }
    Err(BadScope {
        offender: format!(
            "no changelist named '{name}' — {}",
            scope::changelist_scopes(snapshot)
        ),
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
        owners(file)
    ))
}

/// Who owns a file's hunks, each holder named once in file order, in the
/// spelling every holder line uses.
fn owners(file: &ChangedFile) -> String {
    let mut holders: Vec<String> = Vec::new();
    for hunk in &file.hunks {
        let label = holder_label(hunk.changelist.as_deref());
        if !holders.contains(&label) {
            holders.push(label);
        }
    }
    holders.join(", ")
}
