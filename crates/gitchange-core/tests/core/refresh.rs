use crate::support::{NON_UTF8_PATH, RepoFixture};
use gitchange_core::{ChangeKind, Error, GroupKind, Head, Repo};

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
    let snapshot = repo.refresh().unwrap().snapshot;

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
fn the_all_views_groups_mark_whichever_target_is_active() {
    // ADR 0015: the `*` belongs to exactly one of {the changelists,
    // unassigned}, so `groups()` answers for both — and an empty
    // unassigned group still renders when it holds the marker, since
    // that marker is capture-off's whole visible surface.
    let fixture = RepoFixture::new();
    fixture.write("a.txt", "one\n").commit_all("init");
    let repo = Repo::discover(fixture.path()).unwrap();
    repo.create_changelist("feature").unwrap();
    repo.switch(Some("feature")).unwrap();

    let snapshot = repo.refresh().unwrap().snapshot;
    assert_eq!(
        snapshot
            .groups()
            .iter()
            .map(|g| g.kind.clone())
            .collect::<Vec<_>>(),
        vec![GroupKind::Changelist {
            name: "feature".into(),
            active: true,
        }],
        "a clean tree has no unassigned group to show"
    );

    repo.switch(None).unwrap();
    let snapshot = repo.refresh().unwrap().snapshot;
    assert_eq!(
        snapshot
            .groups()
            .iter()
            .map(|g| g.kind.clone())
            .collect::<Vec<_>>(),
        vec![
            GroupKind::Changelist {
                name: "feature".into(),
                active: false,
            },
            GroupKind::Unassigned { active: true },
        ],
        "capture-off shows the unassigned group for its marker alone"
    );
    assert!(snapshot.groups()[1].files.is_empty());
}

#[test]
fn refresh_in_a_clean_repo_is_empty() {
    let fixture = RepoFixture::new();
    fixture.write("a.txt", "content\n").commit_all("init");

    let repo = Repo::discover(fixture.path()).unwrap();
    let snapshot = repo.refresh().unwrap().snapshot;

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
    let snapshot = repo.refresh().unwrap().snapshot;

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
    let snapshot = repo.refresh().unwrap().snapshot;

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
    let snapshot = repo.refresh().unwrap().snapshot;

    match &snapshot.head {
        Head::Detached { short_id } => {
            assert!(fixture.head_oid().starts_with(short_id.as_str()));
        }
        other => panic!("expected a detached head, got {other:?}"),
    }
}

// --- the op form ------------------------------------------------------------

/// A file of `count` numbered lines with `edits` applied — enough distance
/// between two edits that they diff as two hunks rather than one.
fn numbered(count: usize, edits: &[(usize, &str)]) -> String {
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

#[test]
fn the_refresh_op_counts_its_decisions_in_one_echo() {
    // The CLI's `refresh` receipt (#153): the echo is composed here, from
    // the same advisories the caller then prints — so a caller who kept
    // only stdout still learns that something moved. Fragments, since the
    // prose is a display: what is pinned is the count and the class.
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(20, &[]))
        .commit_all("init");
    let repo = Repo::discover(fixture.path()).unwrap();
    repo.create_changelist("one").unwrap();
    repo.switch(Some("one")).unwrap();
    fixture.write("a.txt", &numbered(20, &[(3, "three!"), (18, "eighteen!")]));

    let receipt = repo.refresh_op().unwrap();

    let echo = receipt.echo.expect("two captures are two decisions");
    assert!(echo.contains("captured 2 hunks"), "unexpected echo: {echo}");
    assert_eq!(
        receipt.advisories.len(),
        2,
        "each decision is named as well as counted: {:?}",
        receipt.advisories
    );
}

#[test]
fn the_refresh_op_says_nothing_when_it_decides_nothing() {
    // Nothing decided is `echo: None` (#122) — the silence the CLI's exit
    // 0 with empty output is made of. The second call is the subject: the
    // first one's captures are decisions, and its records are why the
    // second has nothing left to decide.
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(20, &[]))
        .commit_all("init");
    let repo = Repo::discover(fixture.path()).unwrap();
    repo.create_changelist("one").unwrap();
    repo.switch(Some("one")).unwrap();
    fixture.write("a.txt", &numbered(20, &[(3, "three!")]));
    assert!(repo.refresh_op().unwrap().echo.is_some());

    let receipt = repo.refresh_op().unwrap();

    assert_eq!(receipt.echo, None);
    assert!(receipt.advisories.is_empty());
}

#[test]
fn a_revival_is_counted_as_restored() {
    // The other decision class a refresh makes on its own (ADR 0002): the
    // hunks a dormant record reclaims, counted by the same echo — so the
    // receipt states what happened, not merely that capture ran.
    let mut fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(30, &[]))
        .commit_all("init");
    let repo = Repo::discover(fixture.path()).unwrap();
    repo.create_changelist("one").unwrap();
    repo.switch(Some("one")).unwrap();
    fixture.write("a.txt", &numbered(30, &[(5, "five!"), (20, "twenty!")]));
    repo.refresh().unwrap();
    fixture.stash();
    repo.refresh().unwrap();

    fixture.stash_pop();
    let receipt = repo.refresh_op().unwrap();

    let echo = receipt.echo.expect("a revival is a decision");
    assert!(echo.contains("restored 2 hunks"), "unexpected echo: {echo}");
}

#[test]
fn an_overlap_left_unassigned_is_counted_too() {
    // Capture off writes no records (ADR 0016), but an ambiguous overlap
    // is still a decision: two changelists' claims were dropped, so the
    // receipt has to say so — and an advisory with no echo beside it would
    // leave stdout claiming a silent refresh.
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &numbered(40, &[]))
        .commit_all("init");
    let repo = Repo::discover(fixture.path()).unwrap();

    repo.create_changelist("one").unwrap();
    repo.switch(Some("one")).unwrap();
    fixture.write("a.txt", &numbered(40, &[(10, "ten!")]));
    repo.refresh().unwrap();
    repo.create_changelist("two").unwrap();
    repo.switch(Some("two")).unwrap();
    fixture.write("a.txt", &numbered(40, &[(10, "ten!"), (20, "twenty!")]));
    repo.refresh().unwrap();

    // Capture off, with one fresh hunk bridging both records.
    repo.switch(None).unwrap();
    let bridged: Vec<(usize, String)> = (10..=20).map(|n| (n, format!("bridge {n}"))).collect();
    let bridged: Vec<(usize, &str)> = bridged.iter().map(|(n, at)| (*n, at.as_str())).collect();
    fixture.write("a.txt", &numbered(40, &bridged));

    let receipt = repo.refresh_op().unwrap();

    let echo = receipt.echo.expect("a dropped claim is a decision");
    assert!(
        echo.contains("left 1 hunk unassigned"),
        "unexpected echo: {echo}"
    );
    assert_eq!(receipt.advisories.len(), 1);
}
