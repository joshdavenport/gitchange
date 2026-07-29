mod support;

use gitchange_core::{ChangeKind, Error, Repo};
use support::RepoFixture;

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
fn discover_outside_a_repo_is_not_a_repository() {
    let dir = tempfile::tempdir().unwrap();
    let Err(err) = Repo::discover(dir.path()).map(|_| ()) else {
        panic!("expected discover to fail outside a repository");
    };
    assert!(matches!(err, Error::NotARepository { .. }));
}
