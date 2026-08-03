//! Conflict quarantine and the git-operation guard (ticket #34,
//! ADR 0007): unmerged paths leave the hunk universe with their records
//! frozen, commit refuses while an operation is in progress, and
//! resolution re-lands membership — all through the public `Repo`
//! surface on real temp repos (ADR 0008).

mod support;

use gitchange_core::{ChangeKind, CommitOptions, Error, GitOperation, Advisory, Repo, Snapshot};
use support::RepoFixture;

fn repo(fixture: &RepoFixture) -> Repo {
    Repo::discover(fixture.path()).unwrap()
}

/// Each hunk's owning changelist for `path`, in file order.
fn owners(snapshot: &Snapshot, path: &str) -> Vec<Option<String>> {
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

fn state_json(fixture: &RepoFixture) -> serde_json::Value {
    let raw = std::fs::read_to_string(fixture.path().join(".git/gitchange/state.json"))
        .expect("state file exists");
    serde_json::from_str(&raw).unwrap()
}

/// A diverging edit to `a.txt` on a branch and on main, merged: the
/// canonical mid-merge conflict.
fn conflicted_merge() -> RepoFixture {
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", "base\n")
        .write("b.txt", "b base\n")
        .commit_all("init");
    fixture
        .branch("feature")
        .checkout("feature")
        .write("a.txt", "feature side\n")
        .commit_all("feature edit");
    fixture
        .checkout("main")
        .write("a.txt", "main side\n")
        .commit_all("main edit");
    fixture.merge_conflicting("feature");
    fixture
}

#[test]
fn a_merge_in_progress_is_reported_and_guards_commit() {
    let fixture = conflicted_merge();
    let repo = repo(&fixture);

    let snapshot = repo.refresh().unwrap();
    assert_eq!(snapshot.operation, Some(GitOperation::Merge));

    // The guard fires before anything else — nothing staged, no
    // changelist, doesn't matter: the next commit would conclude the
    // merge with one changelist's content (ADR 0007).
    let result = repo.commit(None, "msg", &CommitOptions::default(), None);
    assert!(
        matches!(
            result,
            Err(Error::OperationInProgress {
                operation: GitOperation::Merge
            })
        ),
        "commit mid-merge must refuse: {result:?}"
    );

    // Staging is never operation-guarded: the clean file still stages.
    fixture.write("b.txt", "b edited\n");
    repo.refresh().unwrap();
    repo.stage_file("b.txt").unwrap();
    assert_eq!(
        fixture.index_content("b.txt").as_deref(),
        Some("b edited\n")
    );
}

#[test]
fn an_unmerged_path_is_quarantined_from_the_universe() {
    let fixture = conflicted_merge();
    let repo = repo(&fixture);

    let snapshot = repo.refresh().unwrap();
    let conflicted: Vec<&str> = snapshot
        .conflicted_files()
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    assert_eq!(conflicted, vec!["a.txt"]);

    let file = snapshot
        .files
        .iter()
        .find(|file| file.path == "a.txt")
        .expect("conflicted file stays listed");
    assert_eq!(file.kind, ChangeKind::Conflicted);
    assert!(
        file.hunks.is_empty(),
        "conflict markers must not surface as stageable hunks"
    );
    // Quarantined, not unassigned: the hunk-less conflicted file must
    // not fall into the unassigned pseudo-changelist.
    assert!(
        !snapshot
            .files_in(None)
            .iter()
            .any(|file| file.path == "a.txt"),
        "a conflicted file is not unassigned"
    );

    // Mid-merge resolution: the file leaves the Conflicts group and
    // re-enters the live universe while the merge (and its commit
    // guard) is still in progress.
    fixture.write("a.txt", "resolved\n").stage("a.txt");
    let snapshot = repo.refresh().unwrap();
    assert!(snapshot.conflicted_files().is_empty());
    let file = snapshot
        .files
        .iter()
        .find(|file| file.path == "a.txt")
        .expect("the resolved file is live again");
    assert_ne!(file.kind, ChangeKind::Conflicted);
    assert!(!file.hunks.is_empty(), "hunks are back in the universe");
    assert_eq!(
        snapshot.operation,
        Some(GitOperation::Merge),
        "resolving files does not conclude the merge"
    );
    assert!(matches!(
        repo.commit(None, "msg", &CommitOptions::default(), None),
        Err(Error::OperationInProgress { .. })
    ));
}

#[test]
fn quarantine_freezes_records_and_resolution_relands_them() {
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", "line 1\nline 2\nline 3\n")
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("fixes").unwrap();

    // An owned live record for the worktree edit.
    fixture.write("a.txt", "line 1\nline 2 edited\nline 3\n");
    let snapshot = repo.refresh().unwrap();
    assert_eq!(owners(&snapshot, "a.txt"), vec![Some("fixes".into())]);
    let records_before = state_json(&fixture)["records"].clone();

    // The path becomes unmerged (stash-pop style: no operation state).
    fixture.add_index_conflict("a.txt");
    let snapshot = repo.refresh().unwrap();
    assert_eq!(snapshot.operation, None, "no operation, quarantine anyway");
    let file = snapshot
        .files
        .iter()
        .find(|file| file.path == "a.txt")
        .unwrap();
    assert_eq!(file.kind, ChangeKind::Conflicted);
    assert!(file.hunks.is_empty());

    // Frozen: the record survives verbatim — live, no dormancy clock —
    // even though its path presents no hunks this refresh.
    assert_eq!(
        state_json(&fixture)["records"],
        records_before,
        "a conflicted path's records freeze: no matching, no dormancy"
    );

    // A second conflicted refresh changes nothing either.
    repo.refresh().unwrap();
    assert_eq!(state_json(&fixture)["records"], records_before);

    // Resolution (worktree content unchanged, conflict staged away)
    // re-enters normal matching: the exact anchor re-lands the record.
    fixture.stage("a.txt");
    let snapshot = repo.refresh().unwrap();
    assert_eq!(
        owners(&snapshot, "a.txt"),
        vec![Some("fixes".into())],
        "resolution re-lands the frozen record"
    );
    assert!(
        !snapshot
            .advisories
            .iter()
            .any(|notice| matches!(notice, Advisory::DormantRevival { .. })),
        "the record was frozen live, never dormant — no revival fires"
    );
}
