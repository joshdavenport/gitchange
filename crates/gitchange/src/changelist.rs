//! The noun command (#149): `git branch`'s grammar wholesale — the bare
//! listing, create, delete, rename — of which this ticket (#166) builds
//! the listing and create.
//!
//! What lives here is what the argument list means ([`Mode`]) and how the
//! listing reads ([`print()`]); the repository work is main.rs's, as every
//! other verb's is.

use gitchange_core::{Roster, UNASSIGNED, target_line};

/// Which of the four modes an invocation is. Every other combination of
/// the flags is unrepresentable — the pairwise conflicts are clap's
/// (#140), so a mode is chosen by the parse, not validated here.
///
/// The two unbuilt modes carry nothing yet: their values are read by the
/// tickets that build them (#167's names and force flag, #168's pair),
/// and a mode is all this ticket's dispatch asks for.
pub enum Mode {
    /// Bare `changelist`: the listing, and the command's only read.
    List,
    /// `changelist <name>`.
    Create(String),
    /// `-d <name>...`, `-D <name>...`, `-d … -f` — one mode, since `-D`
    /// is sugar for `--delete --force` and `-f` beside it is
    /// legal-redundant.
    Delete,
    /// `-m <old> <new>`.
    Rename,
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
        rename: Option<Vec<String>>,
    ) -> Self {
        if let Some(name) = name {
            return Mode::Create(name);
        }
        if !delete.is_empty() || !force_delete.is_empty() {
            return Mode::Delete;
        }
        match rename {
            Some(_) => Mode::Rename,
            None => Mode::List,
        }
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
