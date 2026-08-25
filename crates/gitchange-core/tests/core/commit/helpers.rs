//! Fixtures the commit modules share: the numbered-file builder, the
//! `Repo` opener, the unwrapping `commit` shorthand, and the readers the
//! assertions go through — `owners`/`stages` off the snapshot,
//! `state_json`/`dormant_owners` off the persisted state file.

use std::fs;

use crate::support::RepoFixture;
use gitchange_core::{CommitMessage, CommitOptions, CommitOutcome, HunkStage, Repo, Snapshot};

/// Lines `line 1`..=`line count`, as a vec for splicing edits into.
pub(super) fn numbered_lines(count: usize) -> Vec<String> {
    (1..=count).map(|n| format!("line {n}")).collect()
}

pub(super) fn text(lines: &[String]) -> String {
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

pub(super) fn repo(fixture: &RepoFixture) -> Repo {
    Repo::discover(fixture.path()).unwrap()
}

pub(super) fn commit(repo: &Repo, changelist: Option<&str>, message: &str) -> CommitOutcome {
    repo.commit(
        changelist,
        CommitMessage::Given(message),
        &CommitOptions::default(),
        None,
    )
    .unwrap()
}

/// `commit` with `--amend` and a fresh message. The message-keeping mode
/// (`CommitMessage::Kept`) has one test of its own, which calls
/// [`Repo::commit`] directly rather than widen this.
pub(super) fn amend(repo: &Repo, changelist: Option<&str>, message: &str) -> CommitOutcome {
    repo.commit(
        changelist,
        CommitMessage::Given(message),
        &CommitOptions {
            amend: true,
            ..CommitOptions::default()
        },
        None,
    )
    .unwrap()
}

/// Each hunk's owning changelist for `path`, in file order.
pub(super) fn owners(snapshot: &Snapshot, path: &str) -> Vec<Option<String>> {
    let file = snapshot
        .files
        .iter()
        .find(|file| file.path == path)
        .unwrap_or_else(|| panic!("{path} not in snapshot"));
    file.hunks
        .iter()
        .map(|hunk| hunk.changelist.clone())
        .collect()
}

pub(super) fn stages(snapshot: &Snapshot, path: &str) -> Vec<HunkStage> {
    let file = snapshot
        .files
        .iter()
        .find(|file| file.path == path)
        .unwrap_or_else(|| panic!("{path} not in snapshot"));
    file.hunks.iter().map(|hunk| hunk.stage).collect()
}

pub(super) fn state_json(fixture: &RepoFixture) -> serde_json::Value {
    let raw = fs::read_to_string(fixture.path().join(".git/gitchange/state.json"))
        .expect("state file exists");
    serde_json::from_str(&raw).unwrap()
}

/// The changelists of dormant records in the state file, in record order.
pub(super) fn dormant_owners(fixture: &RepoFixture) -> Vec<serde_json::Value> {
    state_json(fixture)["records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|record| record["dormant_since"].is_u64())
        .map(|record| record["changelist"].clone())
        .collect()
}
