use crate::support::{NON_UTF8_PATH, RepoFixture};
use gitchange_core::{ChangeKind, Error, Head, Repo};

#[test]
fn refresh_lists_worktree_changes_sorted_by_path() {
    let fixture = RepoFixture::new();
    fixture
        .write("tracked.txt", "one\n")
        .write("doomed.txt", "gone soon\n")
        .commit_all("init")
        .write("tracked.txt", "two\n")
        .write("untracked.txt", "hello\n");
    std::fs::remove_file(fixture.path().join("doomed.txt")).unwrap();

    let repo = Repo::discover(fixture.path()).unwrap();
    let snapshot = repo.refresh().unwrap();

    let entries: Vec<(&str, ChangeKind)> = snapshot
        .files
        .iter()
        .map(|file| (file.path.as_str(), file.kind))
        .collect();
    assert_eq!(
        entries,
        vec![
            ("doomed.txt", ChangeKind::Deleted),
            ("tracked.txt", ChangeKind::Modified),
            ("untracked.txt", ChangeKind::Untracked),
        ]
    );
}

#[test]
fn refresh_in_a_clean_repo_is_empty() {
    let fixture = RepoFixture::new();
    fixture.write("a.txt", "content\n").commit_all("init");

    let repo = Repo::discover(fixture.path()).unwrap();
    let snapshot = repo.refresh().unwrap();

    assert!(snapshot.files.is_empty());
}

#[test]
fn a_non_utf8_path_fails_refresh_loudly_never_lossily() {
    // ADR 0010: non-UTF-8 paths are unsupported — refresh errors rather
    // than persisting a mangled path that would break identity matching.
    let fixture = RepoFixture::new();
    fixture.write("a.txt", "content\n").commit_all("init");
    fixture.stage_blob_at_raw_path(NON_UTF8_PATH, "content\n");

    let repo = Repo::discover(fixture.path()).unwrap();
    let err = repo.refresh().unwrap_err();
    let Error::NonUtf8Path { path } = err else {
        panic!("expected a non-UTF-8 path failure, got {err:?}");
    };
    // ADR 0010 promises the error *names* the offending path, and names
    // it verbatim: it is the user's only handle on which file to rename,
    // and the lossy rendering the ADR forbids everywhere else would be
    // just as unusable here — two bad paths can render identically.
    assert_eq!(
        path,
        NON_UTF8_PATH.to_vec(),
        "the error carries the offending path byte for byte"
    );
}

#[test]
fn a_refresh_that_fails_on_a_non_utf8_path_persists_nothing() {
    // ADR 0010's other half: "nothing is persisted from that refresh".
    // The failure is loud *and* inert — no half-written records derived
    // from a universe the refresh could not finish reading.
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", "one\ntwo\nthree\n")
        .commit_all("init");
    let repo = Repo::discover(fixture.path()).unwrap();

    // Prior state worth protecting: a changelist owning a recorded hunk.
    repo.create_changelist("one").unwrap();
    fixture.write("a.txt", "one\ntwo edited\nthree\n");
    repo.refresh().unwrap();
    let state_path = fixture.path().join(".git/gitchange/state.json");
    let before = std::fs::read(&state_path).unwrap();

    // Two writes the next successful refresh would owe: reworked records
    // for the reworked hunk, and a fresh baseline stamp for the external
    // HEAD move (ADR 0012). Staged before the bad path so the external
    // commit can't carry it into HEAD's tree.
    fixture
        .write("a.txt", "one\ntwo committed\nthree\n")
        .stage("a.txt")
        .write("a.txt", "one\ntwo reworked\nthree\n")
        .commit_index("external");
    fixture.stage_blob_at_raw_path(NON_UTF8_PATH, "content\n");

    let err = repo.refresh().unwrap_err();
    assert!(matches!(err, Error::NonUtf8Path { .. }), "{err:?}");
    assert_eq!(
        std::fs::read(&state_path).unwrap(),
        before,
        "a failed refresh leaves the state file byte-identical"
    );
    assert_eq!(
        fixture.state_dir_entries(),
        vec!["state.json"],
        "and leaves nothing beside it — no half-written temp file, no lock"
    );
    // Not a vacuous pass: the stamp that refresh owed is still unpaid, so
    // a refresh that had got as far as writing would have rewritten the
    // file even if the records had matched.
    let state: serde_json::Value = serde_json::from_slice(&before).unwrap();
    assert_ne!(
        state["baseline_head"].as_str(),
        Some(fixture.head_oid().as_str()),
        "the baseline stamp is stale, so a write was genuinely due"
    );
}

#[test]
fn discover_outside_a_repo_is_not_a_repository() {
    let dir = tempfile::tempdir().unwrap();
    let Err(err) = Repo::discover(dir.path()).map(|_| ()) else {
        panic!("expected discover to fail outside a repository");
    };
    assert!(matches!(err, Error::NotARepository { .. }));
}

#[test]
fn snapshot_carries_branch_head_and_recent_commits_newest_first() {
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", "one\n")
        .commit_all("init")
        .write("a.txt", "two\n")
        .commit_all("second");

    let repo = Repo::discover(fixture.path()).unwrap();
    let snapshot = repo.refresh().unwrap();

    match &snapshot.head {
        Head::Branch { name } => assert!(!name.is_empty()),
        other => panic!("expected a branch head, got {other:?}"),
    }
    let summaries: Vec<&str> = snapshot
        .recent_commits
        .iter()
        .map(|commit| commit.summary.as_str())
        .collect();
    assert_eq!(summaries, vec!["second", "init"]);
    for commit in &snapshot.recent_commits {
        assert_eq!(commit.author, "gitchange-tests");
        assert!(!commit.short_id.is_empty());
        assert!(
            fixture
                .head_oid()
                .starts_with(&snapshot.recent_commits[0].short_id)
        );
    }
}

#[test]
fn snapshot_head_is_unborn_with_no_commits() {
    let fixture = RepoFixture::new();
    fixture.write("a.txt", "content\n");

    let repo = Repo::discover(fixture.path()).unwrap();
    let snapshot = repo.refresh().unwrap();

    match &snapshot.head {
        Head::Unborn { name } => assert!(!name.is_empty()),
        other => panic!("expected an unborn head, got {other:?}"),
    }
    assert!(snapshot.recent_commits.is_empty());
}

#[test]
fn snapshot_head_reports_detached_by_short_id() {
    let fixture = RepoFixture::new();
    fixture.write("a.txt", "content\n").commit_all("init");
    fixture.detach_head();

    let repo = Repo::discover(fixture.path()).unwrap();
    let snapshot = repo.refresh().unwrap();

    match &snapshot.head {
        Head::Detached { short_id } => {
            assert!(fixture.head_oid().starts_with(short_id.as_str()));
        }
        other => panic!("expected a detached head, got {other:?}"),
    }
}
