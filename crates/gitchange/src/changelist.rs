//! The noun command (#149): `git branch`'s grammar wholesale — the bare
//! listing and create (#166), delete (#167) and rename (#168).
//!
//! What lives here is what the argument list means ([`Mode`]), how the
//! listing reads ([`print()`]), and how a refused delete or rename reads
//! ([`refusal()`], [`rename_refusal()`]); the repository work is main.rs's,
//! as every other verb's is.

use gitchange_core::{Error, Release, Roster, UNASSIGNED, Undeletable, target_line};

/// Which of the four modes an invocation is. Every other combination of
/// the flags is unrepresentable — the pairwise conflicts are clap's
/// (#140), so a mode is chosen by the parse, not validated here.
pub enum Mode {
    /// Bare `changelist`: the listing, and the command's only read.
    List,
    /// `changelist <name>`.
    Create(String),
    /// `-d <name>...`, `-D <name>...`, `-d … -f` — one mode, since `-D`
    /// is sugar for `--delete --force` and `-f` beside it is
    /// legal-redundant. Which spelling arrived survives only as the
    /// [`Release`] it means.
    Delete {
        names: Vec<String>,
        release: Release,
    },
    /// `-m <old> <new>`.
    Rename { from: String, to: String },
}

impl Mode {
    /// The mode the parsed arguments name. The grammar's conflicts are
    /// what make this a sequence of independent questions rather than a
    /// precedence order: at most one mode-bearing slot is ever filled,
    /// so whichever is found is the mode, and the empty parse is the
    /// listing.
    pub fn of(
        name: Option<String>,
        delete: Vec<String>,
        force_delete: Vec<String>,
        force: bool,
        rename: Option<Vec<String>>,
    ) -> Self {
        if let Some(name) = name {
            return Mode::Create(name);
        }
        // `-D` is `--delete --force`, so its own list forces on its own;
        // `-f` cannot arrive without one of the two lists filled (#140),
        // which is why the flag needs no mode of its own here.
        if !delete.is_empty() || !force_delete.is_empty() {
            let forced = force || !force_delete.is_empty();
            return Mode::Delete {
                names: match force_delete.is_empty() {
                    true => delete,
                    false => force_delete,
                },
                release: match forced {
                    true => Release::Forced,
                    false => Release::Guarded,
                },
            };
        }
        match rename {
            // Exactly two values or no parse at all (#140), so the pair is
            // whole by the time it arrives: the arity clap enforced is the
            // one this destructuring relies on.
            Some(pair) => match <[String; 2]>::try_from(pair) {
                Ok([from, to]) => Mode::Rename { from, to },
                Err(_) => unreachable!("'-m' parses with num_args = 2"),
            },
            None => Mode::List,
        }
    }
}

/// A refused delete, in the all-or-nothing shape every mutating verb
/// refuses in (#122/#145): every offender in argument order, `; `-joined,
/// each sentence core's own (ADR 0006) — and where the records guard
/// fired, the override on its own line at the end.
///
/// The override is this surface's to spell and no one else's: core states
/// the mechanism, the CLI names the flag that accepts it. It goes after
/// the list rather than into it for the reason the staging refusals put
/// their grammar note there — inside, it would read as one more offender.
pub fn refusal(offenders: &[Undeletable]) -> String {
    let guarded = offenders
        .iter()
        .any(|offender| matches!(offender, Undeletable::HoldsRecords { .. }));
    let listed = crate::scope::refusal(
        &offenders
            .iter()
            .map(Undeletable::message)
            .collect::<Vec<String>>(),
    );
    match guarded {
        true => {
            format!("{listed}\nrelease them deliberately with 'gitchange changelist -D <name>...'")
        }
        false => listed,
    }
}

/// A refused rename, or `None` where core's own error is the whole answer
/// (#149). Two of the three refusals want more than the error alone says;
/// the rest — lock contention above all — must reach
/// [`crate::report_failure`] as the error they are, since that is what the
/// exit-code split reads.
///
/// - an unrecognised `<old>` refuses with the candidates listed. The
///   sentence is core's, the same one a delete's offender carries (so one
///   repository answers one list); what this surface adds is the list,
///   which the error does not carry;
/// - an existing `<new>` refuses **naming the composition**: there is no
///   `-M`, because core has no clobbering rename and a CLI-side
///   delete-then-rename would pose as one op. So the destructive half is
///   spelled as the op it is, and the retry is two named commands.
///
/// A reserved `<new>` needs nothing added: core's sentence names the name
/// and says why, and the fix is to pick another one.
///
/// `candidates` is a thunk answering `None` where the roster could not be
/// read: only the first arm has any use for one — the retry's advice, read
/// when there is a retry to advise — and an unread roster must fall through
/// to core's bare sentence rather than be dressed as an empty one, which
/// would state that the repository has no changelists.
pub fn rename_refusal(
    from: &str,
    to: &str,
    error: &Error,
    candidates: impl FnOnce() -> Option<Vec<String>>,
) -> Option<String> {
    match error {
        Error::UnknownChangelist { name } => {
            candidates().map(|candidates| gitchange_core::unknown_changelist(name, &candidates))
        }
        Error::ChangelistExists { .. } => Some(format!(
            "{error}\nthere is no clobbering rename — delete it deliberately with \
             'gitchange changelist -D {to}', then 'gitchange changelist -m {from} {to}'"
        )),
        _ => None,
    }
}

/// The listing: one name per line in user order (creation-append), `*` on
/// the active one — `git branch`'s shape, and the same order and marker
/// `status`'s text face prints, so the surfaces cannot disagree.
///
/// `unassigned` is not a changelist but the absence of membership
/// (ADR 0016), so it appears only while it holds the marker — last, as
/// `* unassigned`, which is git's `* (HEAD detached)` move. A repo with
/// no changelists therefore lists exactly that one line: the listing is
/// never empty, because the marker is always on something.
pub fn print(roster: &Roster) {
    for changelist in &roster.changelists {
        let active = roster.active.as_deref() == Some(changelist.name.as_str());
        println!("{}", target_line(active, &changelist.name));
    }
    if roster.active.is_none() {
        println!("{}", target_line(true, UNASSIGNED));
    }
}
