//! Shim onto the shared fixture crate. The builder itself lives in
//! `gitchange-test-support` (ADR 0006/0008, issue #59) so the TUI crate's
//! run-loop tests build repos through the same one; core's test modules
//! say `use crate::support::RepoFixture;` and are otherwise untouched.

pub use gitchange_test_support::{NON_UTF8_PATH, RepoFixture};

use gitchange_core::{Deletion, OpOutcome, Release, Repo, Undeletable};

/// Delete one changelist, releasing whatever records it holds — the
/// unconditional delete a test wants when the records guard (#149) is not
/// the thing it is asserting. A refusal panics: forced, the only offender
/// left is an unrecognised name, which in these fixtures is a bug in the
/// test.
pub fn delete(repo: &Repo, name: &str) -> OpOutcome {
    match repo.delete_changelists(&[name], Release::Forced).unwrap() {
        Deletion::Done(outcome) => outcome,
        Deletion::Refused(offenders) => panic!("delete of '{name}' refused: {offenders:?}"),
    }
}

/// The offenders of a delete that refused; a delete that ran instead is
/// the test's own bug.
pub fn offenders(deletion: Deletion) -> Vec<Undeletable> {
    match deletion {
        Deletion::Refused(offenders) => offenders,
        Deletion::Done(receipt) => panic!("the delete ran: {receipt:?}"),
    }
}

/// The receipt of a delete that ran.
pub fn done(deletion: Deletion) -> OpOutcome {
    match deletion {
        Deletion::Done(receipt) => receipt,
        Deletion::Refused(offenders) => panic!("the delete refused: {offenders:?}"),
    }
}
