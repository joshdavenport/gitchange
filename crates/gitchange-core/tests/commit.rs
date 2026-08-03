//! Commit mechanics (issue 28, ADR 0004): commit a changelist's staged
//! hunks from a temporary index, live index and worktree untouched, hooks
//! run natively — and the ADR 0012 record aftermath: consumed records
//! removed, surviving same-file records commuted, retained `◑` records
//! rewritten against the new HEAD, baseline stamped in the same locked
//! update so the external-move guard never fires on an own commit.

mod support;

use std::fs;

use gitchange_core::{CommitOptions, CommitOutcome, Error, HunkStage, Notice, Repo, Snapshot};
use support::RepoFixture;

/// Lines `line 1`..=`line count`, as a vec for splicing edits into.
fn numbered_lines(count: usize) -> Vec<String> {
    (1..=count).map(|n| format!("line {n}")).collect()
}

fn text(lines: &[String]) -> String {
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

fn repo(fixture: &RepoFixture) -> Repo {
    Repo::discover(fixture.path()).unwrap()
}

fn commit(repo: &Repo, changelist: Option<&str>, message: &str) -> CommitOutcome {
    repo.commit(changelist, message, &CommitOptions::default(), None)
        .unwrap()
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

fn stages(snapshot: &Snapshot, path: &str) -> Vec<HunkStage> {
    let file = snapshot
        .files
        .iter()
        .find(|file| file.path == path)
        .unwrap_or_else(|| panic!("{path} not in snapshot"));
    file.hunks.iter().map(|hunk| hunk.stage).collect()
}

fn state_json(fixture: &RepoFixture) -> serde_json::Value {
    let raw = fs::read_to_string(fixture.path().join(".git/gitchange/state.json"))
        .expect("state file exists");
    serde_json::from_str(&raw).unwrap()
}

/// The changelists of dormant records in the state file, in record order.
fn dormant_owners(fixture: &RepoFixture) -> Vec<serde_json::Value> {
    state_json(fixture)["records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|record| record["dormant_since"].is_u64())
        .map(|record| record["changelist"].clone())
        .collect()
}

#[test]
fn commit_writes_only_the_changelists_staged_hunks() {
    let fixture = RepoFixture::new();
    let head = numbered_lines(30);
    fixture.write("a.txt", &text(&head)).commit_all("init");
    let repo = repo(&fixture);

    // Changelist "one": edit line 10.
    repo.create_changelist("one").unwrap();
    let mut worktree = head.clone();
    worktree[9] = "ten-one".into();
    fixture.write("a.txt", &text(&worktree));
    repo.refresh().unwrap();

    // Changelist "two": edit line 20.
    repo.create_changelist("two").unwrap();
    repo.switch("two").unwrap();
    worktree[19] = "twenty-two".into();
    fixture.write("a.txt", &text(&worktree));
    repo.refresh().unwrap();

    // Both hunks staged; the payload filter, not staging, must scope the
    // commit to "one".
    fixture.stage("a.txt");
    let outcome = commit(&repo, Some("one"), "one: ten");
    assert!(matches!(outcome, CommitOutcome::Committed { .. }));

    // HEAD holds only "one"'s hunk.
    let mut committed = head.clone();
    committed[9] = "ten-one".into();
    assert_eq!(
        fixture.head_bytes("a.txt"),
        Some(text(&committed).into_bytes())
    );
    assert_eq!(fixture.head_message(), "one: ten\n");
    // The live index was never touched: it still holds both hunks.
    assert_eq!(fixture.index_content("a.txt"), Some(text(&worktree)));

    let snapshot = repo.refresh().unwrap();
    assert_eq!(
        owners(&snapshot, "a.txt"),
        vec![Some("two".into())],
        "changelist B's hunk stays in B"
    );
    assert_eq!(
        stages(&snapshot, "a.txt"),
        vec![HunkStage::Staged],
        "changelist B's staged hunk stays staged post-commit"
    );
    assert!(snapshot.notices.is_empty());
    // "one"'s record was fully consumed and removed, not left dormant.
    assert!(dormant_owners(&fixture).is_empty());
    // The emptied changelist is kept until explicitly deleted.
    assert!(snapshot.changelists.iter().any(|cl| cl.name == "one"));
}

#[cfg(unix)]
#[test]
fn a_hook_sees_the_commits_true_content() {
    let fixture = RepoFixture::new();
    let head = numbered_lines(30);
    fixture.write("a.txt", &text(&head)).commit_all("init");
    let repo = repo(&fixture);

    repo.create_changelist("one").unwrap();
    let mut worktree = head.clone();
    worktree[9] = "ten-one".into();
    fixture.write("a.txt", &text(&worktree));
    repo.refresh().unwrap();

    repo.create_changelist("two").unwrap();
    repo.switch("two").unwrap();
    worktree[19] = "twenty-two".into();
    fixture.write("a.txt", &text(&worktree));
    repo.refresh().unwrap();
    fixture.stage("a.txt");

    fixture.with_hook(
        "pre-commit",
        "#!/bin/sh\ngit diff --cached > \"$(git rev-parse --git-dir)/hook-saw.diff\"\n",
    );
    commit(&repo, Some("one"), "one: ten");

    let saw = fs::read_to_string(fixture.path().join(".git/hook-saw.diff")).unwrap();
    assert!(saw.contains("ten-one"), "hook sees the payload: {saw}");
    assert!(
        !saw.contains("twenty-two"),
        "hook does not see other changelists' staged hunks: {saw}"
    );
}

#[cfg(unix)]
#[test]
fn hook_rejection_changes_nothing() {
    let fixture = RepoFixture::new();
    let head = numbered_lines(20);
    fixture.write("a.txt", &text(&head)).commit_all("init");
    let repo = repo(&fixture);

    repo.create_changelist("one").unwrap();
    let mut worktree = head.clone();
    worktree[9] = "ten-one".into();
    fixture.write("a.txt", &text(&worktree)).stage("a.txt");
    repo.refresh().unwrap();

    let state_before =
        fs::read_to_string(fixture.path().join(".git/gitchange/state.json")).unwrap();
    let index_before = fixture.index_content("a.txt");

    fixture.with_hook("pre-commit", "#!/bin/sh\necho nope >&2\nexit 1\n");
    let err = repo
        .commit(Some("one"), "one: ten", &CommitOptions::default(), None)
        .unwrap_err();
    match err {
        Error::HookRejected { stderr } => {
            assert!(stderr.contains("nope"), "hook stderr surfaced: {stderr}")
        }
        other => panic!("expected HookRejected, got {other:?}"),
    }

    assert_eq!(fixture.commit_count(), 1, "no commit was created");
    assert_eq!(fixture.index_content("a.txt"), index_before);
    assert_eq!(
        fs::read_to_string(fixture.path().join("a.txt")).unwrap(),
        text(&worktree)
    );
    assert_eq!(
        fs::read_to_string(fixture.path().join(".git/gitchange/state.json")).unwrap(),
        state_before,
        "state file untouched by the failed commit"
    );
}

#[test]
fn committing_one_changelist_commutes_same_file_records() {
    // The own-commit half of ADR 0012: `commit()` shifts surviving
    // same-file records by the committed deltas, so a later anchor-broken
    // edit still inherits via tier-2 — the external-commit flavour of
    // this exact scenario goes dormant instead (tests/head_moves.rs).
    let fixture = RepoFixture::new();
    let head = numbered_lines(60);
    fixture.write("a.txt", &text(&head)).commit_all("init");
    let repo = repo(&fixture);

    // Changelist "two": replace lines 20..=31 with one line (delta -11).
    repo.create_changelist("two").unwrap();
    let mut worktree = head.clone();
    worktree.splice(19..31, ["twenty!".into()]);
    fixture.write("a.txt", &text(&worktree));
    repo.refresh().unwrap();

    // Changelist "one": edit original line 40, record old range [37, 44).
    repo.create_changelist("one").unwrap();
    repo.switch("one").unwrap();
    worktree[28] = "forty-v1".into();
    fixture.write("a.txt", &text(&worktree));
    let snapshot = repo.refresh().unwrap();
    assert_eq!(
        owners(&snapshot, "a.txt"),
        vec![Some("two".into()), Some("one".into())]
    );

    // Stage and commit only "two"'s hunk through gitchange.
    let file = &snapshot.files[0];
    repo.stage_hunk("a.txt", &file.hunks[0]).unwrap();
    // A third changelist is active, so re-attachment can't hide behind
    // active-capture landing on "one" by luck.
    repo.create_changelist("three").unwrap();
    repo.switch("three").unwrap();
    commit(&repo, Some("two"), "two: twenty");

    // Keep editing "one"'s hunk: anchor broken, tier-2 must inherit via
    // the commuted record.
    worktree[28] = "forty-v2".into();
    fixture.write("a.txt", &text(&worktree));

    let snapshot = repo.refresh().unwrap();
    assert_eq!(
        owners(&snapshot, "a.txt"),
        vec![Some("one".into())],
        "the commuted record keeps the shifted hunk in its changelist"
    );
    assert!(
        snapshot.notices.is_empty(),
        "no dormancy notice on an own commit"
    );
    assert!(dormant_owners(&fixture).is_empty());
}

#[test]
fn a_residual_stale_hunk_reattaches_after_an_own_commit() {
    // Committing a ◑ hunk as-is leaves a residual worktree diff; the
    // retained record is rewritten against the new HEAD so the residual
    // re-attaches to its changelist — the external flavour goes dormant
    // (tests/head_moves.rs pins that contrast).
    let fixture = RepoFixture::new();
    let head = numbered_lines(20);
    fixture.write("a.txt", &text(&head)).commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();

    let mut worktree = head.clone();
    worktree[9] = "ten-staged".into();
    fixture.write("a.txt", &text(&worktree)).stage("a.txt");
    // Edit further: the hunk is now staged-stale (◑).
    worktree[9] = "ten-final".into();
    fixture.write("a.txt", &text(&worktree));
    repo.refresh().unwrap();

    repo.create_changelist("two").unwrap();
    repo.switch("two").unwrap();
    commit(&repo, Some("one"), "one: ten (staged version)");

    let mut committed = head.clone();
    committed[9] = "ten-staged".into();
    assert_eq!(
        fixture.head_bytes("a.txt"),
        Some(text(&committed).into_bytes())
    );

    // Residual hunk: committed "ten-staged" ↔ worktree "ten-final".
    let snapshot = repo.refresh().unwrap();
    assert_eq!(
        owners(&snapshot, "a.txt"),
        vec![Some("one".into())],
        "the rewritten record re-attaches the residual to its changelist"
    );
    assert!(snapshot.notices.is_empty());
    assert!(dormant_owners(&fixture).is_empty());
}

#[test]
fn a_residual_stale_hunk_reattaches_when_the_payload_shifts_it() {
    // The hardest aftermath case (ADR 0012's "shifting alone is not
    // enough"): the same commit's other hunk shrinks the file above the
    // residual, so the retained record needs both new coordinates and a
    // re-derived anchor whose old side is the committed content.
    let fixture = RepoFixture::new();
    let head = numbered_lines(60);
    fixture.write("a.txt", &text(&head)).commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();

    // Changelist "one", two hunks: replace lines 10..=21 with one line
    // (delta -11), and edit original line 40 — both staged.
    let mut worktree = head.clone();
    worktree.splice(9..21, ["ten!".into()]);
    worktree[28] = "forty-staged".into();
    fixture.write("a.txt", &text(&worktree)).stage("a.txt");
    // Edit the second hunk further: staged-stale (◑).
    worktree[28] = "forty-final".into();
    fixture.write("a.txt", &text(&worktree));
    repo.refresh().unwrap();

    repo.create_changelist("two").unwrap();
    repo.switch("two").unwrap();
    commit(&repo, Some("one"), "one: both hunks, staged versions");

    let mut committed = head.clone();
    committed.splice(9..21, ["ten!".into()]);
    committed[28] = "forty-staged".into();
    assert_eq!(
        fixture.head_bytes("a.txt"),
        Some(text(&committed).into_bytes())
    );

    let snapshot = repo.refresh().unwrap();
    assert_eq!(
        owners(&snapshot, "a.txt"),
        vec![Some("one".into())],
        "the shifted residual still re-attaches to its changelist"
    );
    assert!(snapshot.notices.is_empty());
    // The consumed record is removed; the retained one stays live.
    assert!(dormant_owners(&fixture).is_empty());
}

#[test]
fn commit_stamps_the_baseline_in_the_same_update() {
    let fixture = RepoFixture::new();
    let head = numbered_lines(20);
    fixture.write("a.txt", &text(&head)).commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();

    let mut worktree = head.clone();
    worktree[9] = "ten-one".into();
    fixture.write("a.txt", &text(&worktree)).stage("a.txt");
    repo.refresh().unwrap();

    commit(&repo, Some("one"), "one: ten");
    // Before any follow-up refresh, the baseline already names the new
    // HEAD — the #39 guard can never arm on an own commit.
    assert_eq!(state_json(&fixture)["baseline_head"], fixture.head_oid());
}

#[test]
fn amend_reuses_the_temp_index_path() {
    let fixture = RepoFixture::new();
    let head = numbered_lines(40);
    fixture.write("a.txt", &text(&head)).commit_all("init");
    let repo = repo(&fixture);

    repo.create_changelist("one").unwrap();
    let mut worktree = head.clone();
    worktree[4] = "five-one".into();
    fixture.write("a.txt", &text(&worktree)).stage("a.txt");
    repo.refresh().unwrap();
    commit(&repo, Some("one"), "first");
    assert_eq!(fixture.commit_count(), 2);

    repo.create_changelist("two").unwrap();
    repo.switch("two").unwrap();
    worktree[9] = "ten-two".into();
    fixture.write("a.txt", &text(&worktree));
    repo.refresh().unwrap();

    // A third changelist's staged hunk (far enough from "two"'s that the
    // index diff keeps them separate atoms) proves the amend goes through
    // the same temp-index path: staged content outside the payload must
    // stay out of the amended tip and stay staged after it.
    repo.create_changelist("three").unwrap();
    repo.switch("three").unwrap();
    worktree[29] = "thirty-three".into();
    fixture.write("a.txt", &text(&worktree)).stage("a.txt");
    repo.refresh().unwrap();

    let outcome = repo
        .commit(
            Some("two"),
            "first and second",
            &CommitOptions {
                amend: true,
                ..CommitOptions::default()
            },
            None,
        )
        .unwrap();
    assert!(matches!(outcome, CommitOutcome::Committed { .. }));

    // Still two commits: the tip was replaced, holding "one"'s and
    // "two"'s edits — never "three"'s.
    assert_eq!(fixture.commit_count(), 2);
    assert_eq!(fixture.head_message(), "first and second\n");
    let mut amended = head.clone();
    amended[4] = "five-one".into();
    amended[9] = "ten-two".into();
    assert_eq!(
        fixture.head_bytes("a.txt"),
        Some(text(&amended).into_bytes())
    );

    let snapshot = repo.refresh().unwrap();
    assert!(snapshot.notices.is_empty());
    assert_eq!(owners(&snapshot, "a.txt"), vec![Some("three".into())]);
    assert_eq!(stages(&snapshot, "a.txt"), vec![HunkStage::Staged]);
}

#[test]
fn unassigned_commits_like_any_changelist() {
    // No changelists exist: the whole dirty tree is unassigned, and a
    // changelist-less repo must not grow a state file just to commit.
    let fixture = RepoFixture::new();
    let head = numbered_lines(10);
    fixture.write("a.txt", &text(&head)).commit_all("init");
    let repo = repo(&fixture);

    let mut worktree = head.clone();
    worktree[4] = "five-edited".into();
    fixture.write("a.txt", &text(&worktree)).stage("a.txt");

    let outcome = commit(&repo, None, "unassigned edit");
    assert!(matches!(outcome, CommitOutcome::Committed { .. }));
    assert_eq!(
        fixture.head_bytes("a.txt"),
        Some(text(&worktree).into_bytes())
    );
    assert!(
        !fixture.path().join(".git/gitchange/state.json").exists(),
        "no state file grows from committing an unmanaged tree"
    );
}

#[test]
fn unborn_branch_initial_commit_works() {
    let fixture = RepoFixture::new();
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();

    fixture.write("a.txt", "alpha\nbeta\n").stage("a.txt");
    repo.refresh().unwrap();

    let outcome = commit(&repo, Some("one"), "initial");
    assert!(matches!(outcome, CommitOutcome::Committed { .. }));
    assert_eq!(fixture.commit_count(), 1);
    assert_eq!(fixture.head_bytes("a.txt"), Some(b"alpha\nbeta\n".to_vec()));
    assert_eq!(state_json(&fixture)["baseline_head"], fixture.head_oid());
    let snapshot = repo.refresh().unwrap();
    assert!(snapshot.notices.is_empty());
}

#[test]
fn payload_drift_returns_to_the_confirm_step() {
    let fixture = RepoFixture::new();
    let head = numbered_lines(20);
    fixture.write("a.txt", &text(&head)).commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();

    let mut worktree = head.clone();
    worktree[9] = "ten-v1".into();
    fixture.write("a.txt", &text(&worktree)).stage("a.txt");
    let confirmed = repo.commit_payload(Some("one")).unwrap();

    // The staged payload changes between confirm and commit.
    worktree[9] = "ten-v2".into();
    fixture.write("a.txt", &text(&worktree)).stage("a.txt");

    let outcome = repo
        .commit(
            Some("one"),
            "one: ten",
            &CommitOptions::default(),
            Some(&confirmed),
        )
        .unwrap();
    let CommitOutcome::Drifted { payload } = outcome else {
        panic!("expected Drifted, got {outcome:?}");
    };
    assert_ne!(payload, confirmed);
    assert_eq!(fixture.commit_count(), 1, "nothing was committed");

    // Re-confirming with the fresh payload commits.
    let outcome = repo
        .commit(
            Some("one"),
            "one: ten",
            &CommitOptions::default(),
            Some(&payload),
        )
        .unwrap();
    assert!(matches!(outcome, CommitOutcome::Committed { .. }));
    assert_eq!(fixture.commit_count(), 2);
}

#[test]
fn zero_staged_hunks_is_an_error_not_a_commit() {
    let fixture = RepoFixture::new();
    let head = numbered_lines(20);
    fixture.write("a.txt", &text(&head)).commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();

    // An owned but unstaged hunk: nothing to commit.
    let mut worktree = head.clone();
    worktree[9] = "ten-unstaged".into();
    fixture.write("a.txt", &text(&worktree));
    repo.refresh().unwrap();

    let err = repo
        .commit(Some("one"), "one: ten", &CommitOptions::default(), None)
        .unwrap_err();
    assert!(matches!(err, Error::NothingStaged));
    assert_eq!(fixture.commit_count(), 1);

    let err = repo
        .commit(Some("missing"), "?", &CommitOptions::default(), None)
        .unwrap_err();
    assert!(matches!(err, Error::UnknownChangelist { .. }));
}

#[test]
fn payload_counts_cover_both_stale_flavours() {
    let fixture = RepoFixture::new();
    let head = numbered_lines(20);
    fixture
        .write("a.txt", &text(&head))
        .write("b.txt", &text(&head))
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();

    // a.txt: one cleanly staged hunk and one edited-flavour ◑ hunk.
    let mut worktree = head.clone();
    worktree[4] = "five-staged".into();
    worktree[14] = "fifteen-staged".into();
    fixture.write("a.txt", &text(&worktree)).stage("a.txt");
    worktree[14] = "fifteen-final".into();
    fixture.write("a.txt", &text(&worktree));

    // b.txt: an index-only ◑ hunk (staged then reverted in the worktree).
    let mut staged_b = head.clone();
    staged_b[9] = "ten-staged".into();
    fixture
        .write("b.txt", &text(&staged_b))
        .stage("b.txt")
        .write("b.txt", &text(&head));

    let payload = repo.commit_payload(Some("one")).unwrap();
    assert_eq!(payload.file_count(), 2);
    assert_eq!(payload.staged_hunks(), 1);
    assert_eq!(payload.stale_hunks(), 2, "both ◑ flavours count");
    assert!(!payload.is_empty());
}

#[test]
fn align_sets_index_to_worktree_for_the_changelists_stale_hunks() {
    let fixture = RepoFixture::new();
    let head = numbered_lines(20);
    fixture
        .write("a.txt", &text(&head))
        .write("b.txt", &text(&head))
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();

    // a.txt: edited-flavour ◑ — align re-stages the edited version.
    let mut worktree_a = head.clone();
    worktree_a[4] = "five-staged".into();
    fixture.write("a.txt", &text(&worktree_a)).stage("a.txt");
    worktree_a[4] = "five-final".into();
    fixture.write("a.txt", &text(&worktree_a));

    // b.txt: index-only ◑ — align discards the staged content.
    let mut staged_b = head.clone();
    staged_b[9] = "ten-staged".into();
    fixture
        .write("b.txt", &text(&staged_b))
        .stage("b.txt")
        .write("b.txt", &text(&head));

    let notices = repo.align(Some("one")).unwrap();
    assert_eq!(notices, Vec::<Notice>::new());

    assert_eq!(fixture.index_content("a.txt"), Some(text(&worktree_a)));
    assert_eq!(fixture.index_content("b.txt"), Some(text(&head)));

    let snapshot = repo.refresh().unwrap();
    assert_eq!(stages(&snapshot, "a.txt"), vec![HunkStage::Staged]);
    assert!(
        !snapshot.files.iter().any(|file| file.path == "b.txt"),
        "the discarded index-only hunk leaves no diff at all"
    );
}

#[test]
fn stage_all_stages_only_the_changelists_unstaged_hunks() {
    let fixture = RepoFixture::new();
    let head = numbered_lines(20);
    fixture
        .write("a.txt", &text(&head))
        .write("b.txt", &text(&head))
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();

    // Both edits land in the active changelist; b.txt's hunk is then
    // moved out to unassigned so stage_all must leave it behind.
    let mut worktree_a = head.clone();
    worktree_a[4] = "five-edited".into();
    fixture.write("a.txt", &text(&worktree_a));
    let mut worktree_b = head.clone();
    worktree_b[9] = "ten-edited".into();
    fixture.write("b.txt", &text(&worktree_b));
    let snapshot = repo.refresh().unwrap();
    let b_hunks = snapshot
        .files
        .iter()
        .find(|file| file.path == "b.txt")
        .unwrap()
        .hunks
        .clone();
    repo.assign_hunks("b.txt", &b_hunks, None).unwrap();

    let outcome = repo.stage_all(Some("one")).unwrap();
    assert_eq!(outcome.staged, 1);
    assert_eq!(outcome.notices, Vec::<Notice>::new());

    assert_eq!(fixture.index_content("a.txt"), Some(text(&worktree_a)));
    assert_eq!(
        fixture.index_content("b.txt"),
        Some(text(&head)),
        "unassigned hunk left unstaged"
    );

    let snapshot = repo.refresh().unwrap();
    assert_eq!(stages(&snapshot, "a.txt"), vec![HunkStage::Staged]);
    assert_eq!(stages(&snapshot, "b.txt"), vec![HunkStage::Unstaged]);

    assert!(matches!(
        repo.stage_all(Some("missing")),
        Err(Error::UnknownChangelist { .. })
    ));
}

#[test]
fn a_changelist_containing_a_binary_commits_it_whole() {
    // ADR 0009: the temp-index commit receives the staged blob
    // whole-file; other changelists' content stays behind.
    let fixture = RepoFixture::new();
    fixture
        .write("a.txt", &text(&numbered_lines(5)))
        .write_bytes("logo.png", &[0u8, 1, 2, 3])
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("art").unwrap();
    fixture
        .write_bytes("logo.png", &[0u8, 9, 9])
        .stage("logo.png");
    repo.refresh().unwrap();

    // A second changelist owns a staged text edit that must not commit.
    repo.create_changelist("other").unwrap();
    repo.switch("other").unwrap();
    let mut lines = numbered_lines(5);
    lines[0] = "edited".into();
    fixture.write("a.txt", &text(&lines)).stage("a.txt");
    repo.refresh().unwrap();

    let payload = repo.commit_payload(Some("art")).unwrap();
    assert_eq!(payload.file_count(), 1);
    assert_eq!(payload.staged_hunks(), 1, "the binary counts as one hunk");
    assert_eq!(payload.stale_hunks(), 0);

    commit(&repo, Some("art"), "binary whole-file");
    assert_eq!(fixture.head_bytes("logo.png").unwrap(), vec![0u8, 9, 9]);
    assert_eq!(
        fixture.head_bytes("a.txt").unwrap(),
        text(&numbered_lines(5)).into_bytes(),
        "the other changelist's staged edit stays out of the commit"
    );
    assert_eq!(
        fixture.index_content("a.txt").unwrap(),
        text(&lines),
        "the live index is untouched"
    );

    // The consumed record is removed; the path drops out of the diff.
    let snapshot = repo.refresh().unwrap();
    assert!(!snapshot.files.iter().any(|file| file.path == "logo.png"));
}

#[test]
fn a_stale_binary_warns_and_commits_the_staged_blob() {
    // ADR 0004's warn-and-confirm applies unchanged: the `◑` binary
    // counts as a stale hunk and the commit carries the staged blob,
    // not the worktree's newer bytes.
    let fixture = RepoFixture::new();
    fixture
        .write_bytes("logo.png", &[0u8, 1])
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("art").unwrap();
    fixture
        .write_bytes("logo.png", &[0u8, 2, 2])
        .stage("logo.png")
        .write_bytes("logo.png", &[0u8, 3, 3, 3]);

    let payload = repo.commit_payload(Some("art")).unwrap();
    assert_eq!(payload.staged_hunks(), 0);
    assert_eq!(payload.stale_hunks(), 1, "the ◑ binary is the warn count");

    commit(&repo, Some("art"), "stale binary");
    assert_eq!(fixture.head_bytes("logo.png").unwrap(), vec![0u8, 2, 2]);

    // The retained record was rewritten against the new HEAD: the
    // residual worktree change keeps its membership.
    let snapshot = repo.refresh().unwrap();
    assert_eq!(owners(&snapshot, "logo.png"), vec![Some("art".into())]);
    assert_eq!(stages(&snapshot, "logo.png"), vec![HunkStage::Unstaged]);
}

#[test]
fn a_staged_binary_deletion_commits_the_removal() {
    let fixture = RepoFixture::new();
    fixture
        .write("keep.txt", "content\n")
        .write_bytes("logo.png", &[0u8, 1])
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("art").unwrap();
    fs::remove_file(fixture.path().join("logo.png")).unwrap();
    fixture.stage_removal("logo.png");

    let payload = repo.commit_payload(Some("art")).unwrap();
    assert_eq!(payload.file_count(), 1);
    assert_eq!(payload.staged_hunks(), 1);

    commit(&repo, Some("art"), "drop binary");
    assert!(fixture.head_bytes("logo.png").is_none());
    assert!(fixture.head_bytes("keep.txt").is_some());
}

#[test]
fn a_restaged_binary_blob_drifts_the_confirmation() {
    // The payload's whole-file OID participates in drift equality
    // (ADR 0004's freshness guard): re-staging new bytes between
    // confirm and commit returns Drifted, nothing committed.
    let fixture = RepoFixture::new();
    fixture
        .write_bytes("logo.png", &[0u8, 1])
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("art").unwrap();
    fixture
        .write_bytes("logo.png", &[0u8, 2, 2])
        .stage("logo.png");

    let confirmed = repo.commit_payload(Some("art")).unwrap();
    fixture
        .write_bytes("logo.png", &[0u8, 4, 4, 4, 4])
        .stage("logo.png");

    let outcome = repo
        .commit(
            Some("art"),
            "drifted",
            &CommitOptions::default(),
            Some(&confirmed),
        )
        .unwrap();
    assert!(matches!(outcome, CommitOutcome::Drifted { .. }));
    assert_eq!(fixture.commit_count(), 1, "nothing was committed");
}

#[test]
fn a_binary_worktree_over_staged_text_commits_the_staged_text() {
    // The mixed case: text content staged, then the worktree file
    // replaced with binary bytes. The universe sees one `◑` whole-file
    // hunk; committing carries the staged text blob whole-file rather
    // than silently excluding the path.
    let fixture = RepoFixture::new();
    fixture.write("notes.txt", "original\n").commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("work").unwrap();
    fixture
        .write("notes.txt", "staged text\n")
        .stage("notes.txt");
    fixture.write_bytes("notes.txt", &[0u8, 1, 2, 3]);

    let payload = repo.commit_payload(Some("work")).unwrap();
    assert_eq!(payload.file_count(), 1);
    assert_eq!(payload.stale_hunks(), 1, "index and worktree differ: ◑");

    commit(&repo, Some("work"), "mixed staged text");
    assert_eq!(
        fixture.head_bytes("notes.txt").unwrap(),
        b"staged text\n".to_vec()
    );
    assert_eq!(
        std::fs::read(fixture.path().join("notes.txt")).unwrap(),
        vec![0u8, 1, 2, 3],
        "the worktree is untouched"
    );
}
