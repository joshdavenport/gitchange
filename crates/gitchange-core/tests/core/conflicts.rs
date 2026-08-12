//! Conflict quarantine and the git-operation guard (ticket #34,
//! ADR 0007): unmerged paths leave the hunk universe with their records
//! frozen, commit refuses while an operation is in progress, and
//! resolution re-lands membership — all through the public `Repo`
//! surface on real temp repos (ADR 0008).

use crate::support::RepoFixture;
use gitchange_core::{ChangeKind, CommitOptions, Error, GitOperation, Repo, Snapshot};

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

/// How many lines the conflicted fixture's `a.txt` holds at HEAD. Named
/// so the resolution below can rebuild HEAD's content from the same
/// number the builder committed.
const CONFLICTED_FILE_LINES: usize = 20;

/// Lines `line 1`..=`line count`, as a vec for splicing edits into.
fn numbered_lines(count: usize) -> Vec<String> {
    (1..=count).map(|n| format!("line {n}")).collect()
}

fn text(lines: &[String]) -> String {
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// `a.txt`, twenty lines, with two live records far enough apart to be
/// separate hunks and owned by *different* changelists — "fixes" at line
/// 3, "other" at line 15 — the path then made unmerged (stash-pop style:
/// no operation in progress) and its records frozen by a refresh. Returns
/// the repo handle and the record set as it stood before the conflict,
/// which is what "frozen" is measured against; HEAD's content is not
/// returned because `numbered_lines(CONFLICTED_FILE_LINES)` rebuilds it
/// exactly.
///
/// Two changelists rather than one because that is what makes overlap
/// *positional* below: were overlap to claim regardless of range, each
/// hunk would see both records, and two candidate changelists is an
/// `AmbiguousOverlap` capture to active, not an inheritance. A third
/// changelist ("spare") holds the active marker so no re-land can be
/// confused with a capture either.
///
/// Shared by both re-entry tests so the *resolution content* is
/// structurally the only difference between them: tier-1's exact re-land
/// is only evidence about exact matching if the tier-2 case re-enters
/// from the same freeze. The integrity checks — a genuinely conflicted,
/// hunk-less path — sit here rather than in the tests, so a fixture that
/// failed to conflict can't make either re-land look like a re-land.
fn frozen_records_on_a_conflicted_path(fixture: &RepoFixture) -> (Repo, serde_json::Value) {
    let head = numbered_lines(CONFLICTED_FILE_LINES);
    fixture.write("a.txt", &text(&head)).commit_all("init");
    let repo = repo(fixture);

    // "fixes" owns line 3's edit. Record old range: [1, 7).
    repo.create_changelist("fixes").unwrap();
    repo.switch(Some("fixes")).unwrap();
    let mut worktree = head.clone();
    worktree[2] = "three-fixes".into();
    fixture.write("a.txt", &text(&worktree));
    repo.refresh().unwrap();

    // "other" owns line 15's edit. Record old range: [12, 19).
    repo.create_changelist("other").unwrap();
    repo.switch(Some("other")).unwrap();
    worktree[14] = "fifteen-other".into();
    fixture.write("a.txt", &text(&worktree));
    let snapshot = repo.refresh().unwrap();
    assert_eq!(
        owners(&snapshot, "a.txt"),
        vec![Some("fixes".into()), Some("other".into())]
    );
    let records_before = state_json(fixture)["records"].clone();

    // The active marker moves to a changelist that owns nothing, so a
    // re-land can never be confused with a capture: the tiers answer
    // "fixes" and "other", active capture answers "spare".
    repo.create_changelist("spare").unwrap();
    repo.switch(Some("spare")).unwrap();

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

    (repo, records_before)
}

#[test]
fn quarantine_freezes_records_and_resolution_relands_them() {
    let fixture = RepoFixture::new();
    let (repo, records_before) = frozen_records_on_a_conflicted_path(&fixture);

    // Frozen: the records survive verbatim — live, no dormancy clock —
    // even though their path presents no hunks this refresh.
    assert_eq!(
        state_json(&fixture)["records"],
        records_before,
        "a conflicted path's records freeze: no matching, no dormancy"
    );

    // A second conflicted refresh changes nothing either.
    repo.refresh().unwrap();
    assert_eq!(state_json(&fixture)["records"], records_before);

    // Resolution (worktree content unchanged, conflict staged away)
    // re-enters normal matching: the exact anchors re-land both records.
    fixture.stage("a.txt");
    let snapshot = repo.refresh().unwrap();
    assert_eq!(
        owners(&snapshot, "a.txt"),
        vec![Some("fixes".into()), Some("other".into())],
        "resolution re-lands the frozen records"
    );
    assert_eq!(
        snapshot.advisories,
        vec![],
        "nothing was captured or revived: the records were frozen live, so \
         neither an AutoCaptured to 'spare' nor a DormantRevival can fire"
    );
}

#[test]
fn a_shifted_resolution_relands_frozen_records_by_overlap() {
    // Re-entry is into *normal* matching (ADR 0007), which means both
    // tiers — and tier 2 is the one a real merge needs. Resolution
    // content that matches the pre-merge hunks line for line is the lucky
    // case; the ordinary one shifts them, and then only overlap on the
    // HEAD-side range can carry the membership across.
    let fixture = RepoFixture::new();
    let (repo, records_before) = frozen_records_on_a_conflicted_path(&fixture);
    assert_eq!(
        state_json(&fixture)["records"],
        records_before,
        "the records froze live — tier 2 claims live records only"
    );

    // The resolution keeps both edits but neither its content nor its
    // position: the merge's own line lands above them, so every anchor
    // line the records stored has moved or changed. HEAD-side ranges are
    // untouched by an insertion below them, so both records still address
    // the hunk that grew out of them — [1, 7) and [12, 19), each
    // overlapping one hunk and clear of the other.
    let mut resolved = numbered_lines(CONFLICTED_FILE_LINES);
    resolved[2] = "three-resolved".into();
    resolved[14] = "fifteen-resolved".into();
    resolved.insert(1, "inserted by the merge".into());
    fixture.write("a.txt", &text(&resolved)).stage("a.txt");
    let snapshot = repo.refresh().unwrap();

    assert_eq!(
        owners(&snapshot, "a.txt"),
        vec![Some("fixes".into()), Some("other".into())],
        "a shifted resolution re-lands each frozen record by overlap — and \
         each to its own changelist, which only a positional claim can do"
    );
    assert_eq!(
        snapshot.advisories,
        vec![],
        "inheritance: no capture to 'spare', no revival, and no ambiguity \
         (an overlap claim indifferent to range would see both records \
         from both hunks and report AmbiguousOverlap)"
    );

    // Which tier did it: tier 1 matches only when the stored anchor equals
    // the fresh hunk's, and a re-landed record stores that fresh anchor —
    // so a changed anchor is proof the exact tier could not have matched.
    let records = state_json(&fixture)["records"].clone();
    for (index, name) in ["fixes", "other"].iter().enumerate() {
        assert_ne!(
            records[index]["anchor"], records_before[index]["anchor"],
            "the resolved hunk's anchor differs, which is exactly what \
             tier 1 cannot match through"
        );
        assert_eq!(records[index]["changelist"], *name);
        assert_eq!(
            records[index].get("dormant_since"),
            Some(&serde_json::Value::Null),
            "and it re-lands live: {records}"
        );
    }
    assert_eq!(
        records.as_array().map(Vec::len),
        Some(2),
        "two records for the two resolved hunks — the frozen pair, not \
         fresh captures beside dormant leftovers"
    );
}
