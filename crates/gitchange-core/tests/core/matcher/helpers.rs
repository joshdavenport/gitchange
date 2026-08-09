//! Fixtures the matcher modules share: the numbered-file builder, the
//! `Repo` opener, and the readers every assertion goes through — `owners`
//! off the snapshot, `state_json`/`record_at` off the persisted state
//! file (ADR 0002's shape).

use std::fs;

use crate::support::RepoFixture;
use gitchange_core::{Repo, Snapshot};

/// `count` numbered lines, with `edits` as (1-based line, replacement).
pub(super) fn numbered(count: usize, edits: &[(usize, &str)]) -> String {
    (1..=count)
        .map(|n| {
            edits
                .iter()
                .find(|(line, _)| *line == n)
                .map(|(_, text)| format!("{text}\n"))
                .unwrap_or_else(|| format!("line {n}\n"))
        })
        .collect()
}

pub(super) fn repo(fixture: &RepoFixture) -> Repo {
    Repo::discover(fixture.path()).unwrap()
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

pub(super) fn state_json(fixture: &RepoFixture) -> serde_json::Value {
    let raw = fs::read_to_string(fixture.path().join(".git/gitchange/state.json"))
        .expect("state file exists");
    serde_json::from_str(&raw).unwrap()
}

/// The single stored record for `path`, panicking unless there is exactly
/// one — read straight off the persisted shape, like `state_json`.
pub(super) fn record_at(json: &serde_json::Value, path: &str) -> serde_json::Value {
    let found: Vec<&serde_json::Value> = json["records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|record| record["path"] == path)
        .collect();
    assert_eq!(found.len(), 1, "expected exactly one record for {path}");
    found[0].clone()
}
