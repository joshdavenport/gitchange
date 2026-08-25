//! The staging verbs' scope resolution (#145): what `add <changelist>
//! [<path>...]` — and, in the same grammar, `unstage` — turns one command
//! line into, validated against the snapshot the invocation's persisting
//! refresh produced.
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
//! are repo facts, which clap cannot see.

use std::path::Path;

use gitchange_core::{
    ALL, ChangeKind, ChangedFile, Snapshot, UNASSIGNED, conflicted_hint, holder_label, target_named,
};

use crate::StagingScope;
use crate::scope;

/// One validated staging scope, in the terms core's sweep takes.
pub struct Sweep {
    /// The changelist whose hunks move; `None` is unassigned, as
    /// everywhere.
    pub changelist: Option<String>,
    /// The file rows the path arguments narrowed the scope to,
    /// repo-relative and in argument order. Empty is the bare form: every
    /// hunk the changelist owns, across all its files.
    pub paths: Vec<String>,
}

impl Sweep {
    /// The rows as core's sweep borrows them.
    pub fn rows(&self) -> Vec<&str> {
        self.paths.iter().map(String::as_str).collect()
    }
}

/// Resolve one staging invocation against `snapshot`, or refuse naming
/// every offender. `verb` is the command's own name, which the refusals
/// that teach its grammar quote back.
pub fn resolve(
    verb: &str,
    args: &StagingScope,
    snapshot: &Snapshot,
    workdir: &Path,
) -> anyhow::Result<Sweep> {
    // The addressed forms are the staging batch's third ticket (#162):
    // refused rather than quietly swept, since either would act on a
    // wider scope than the caller named. That ticket resolves them here
    // instead and deletes this guard.
    if args.containing.containing.is_some() {
        crate::not_implemented(&format!("{verb} --containing"))?;
    }
    let tokens = args.paths.iter().map(String::as_str);
    let (resolved, mut offenders) = scope::locate_paths(tokens, snapshot, workdir);
    if resolved.iter().any(scope::PathArg::addresses_a_hunk) {
        crate::not_implemented(&format!("{verb} <path>:<hunk-id>"))?;
    }
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
    match resolved.as_slice() {
        // The bare form: the changelist itself is the scope, so a
        // changelist owning nothing is the offender.
        [] if snapshot.files_in(changelist).is_empty() => offenders.push(format!(
            "{} owns no hunks — nothing to {verb}",
            holder_label(changelist)
        )),
        [] => {}
        args => offenders.extend(
            args.iter()
                .filter_map(|arg| unowned(verb, changelist, &arg.path, snapshot)),
        ),
    }
    if !offenders.is_empty() {
        anyhow::bail!(refusal(&offenders, None));
    }
    let mut paths: Vec<String> = Vec::new();
    for arg in resolved {
        // One row named twice is one row: the sweep visits a file once
        // whatever the argument list says, so a repeat that reached the
        // echo would be the only place it showed at all.
        if !paths.contains(&arg.path) {
            paths.push(arg.path);
        }
    }
    Ok(Sweep {
        changelist: changelist.map(str::to_owned),
        paths,
    })
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
                "'{ALL}' is a view, not a {verb} scope — name one changelist, or '{UNASSIGNED}'"
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
                "'{verb}' names the changelist first: 'gitchange {verb} <changelist> [<path>...]'"
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
fn unowned(
    verb: &str,
    changelist: Option<&str>,
    path: &str,
    snapshot: &Snapshot,
) -> Option<String> {
    let Some(file) = snapshot.files.iter().find(|file| file.path == path) else {
        // It resolved as a path and is absent from the change universe: a
        // clean file. An answer, and for a staging verb the answer is that
        // there is nothing to do — never a silent empty sweep.
        return Some(format!("'{path}' has no changes — nothing to {verb}"));
    };
    if file.kind == ChangeKind::Conflicted {
        // Quarantined (ADR 0007): the path holds no hunks to own, so the
        // ownership answer would be a confusing "nobody".
        return Some(conflicted_hint(path));
    }
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
