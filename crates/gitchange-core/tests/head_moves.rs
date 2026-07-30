//! HEAD-move staleness (issue 37): the matcher's tier-2 overlap runs on
//! HEAD-side old ranges, valid only while HEAD is unchanged. These tests
//! characterise what a same-file external partial commit does to
//! surviving membership records — including the silent wrong-list
//! assignment ADRs 0001/0002 promise never happens. They pin *current*
//! behaviour as the regression suite for the ticket's decision; tests
//! whose assertions document the defect say so inline and flip when the
//! fix lands.

mod support;

use std::fs;

use gitchange_core::{Repo, Snapshot};
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
fn an_untouched_neighbour_survives_an_external_partial_commit() {
    // The bounded blast radius: tier-1 anchors are position-independent,
    // so a neighbour whose content and context the commit didn't touch
    // keeps its membership across the HEAD move.
    let fixture = RepoFixture::new();
    let head = numbered_lines(60);
    fixture.write("a.txt", &text(&head)).commit_all("init");
    let repo = repo(&fixture);

    // Changelist "two": replace lines 20..=27 with one line (delta -7).
    repo.create_changelist("two").unwrap();
    let mut worktree = head.clone();
    worktree.splice(19..27, ["twenty!".into()]);
    fixture.write("a.txt", &text(&worktree));
    repo.refresh().unwrap();

    // Changelist "one": edit original line 40, well clear of "two"'s hunk.
    repo.create_changelist("one").unwrap();
    repo.switch("one").unwrap();
    worktree[32] = "forty-one-owned".into();
    fixture.write("a.txt", &text(&worktree));
    let snapshot = repo.refresh().unwrap();
    assert_eq!(
        owners(&snapshot, "a.txt"),
        vec![Some("two".into()), Some("one".into())]
    );

    // Externally partial-commit "two"'s hunk: stage the intermediate
    // content, restore the full worktree, commit the index.
    let mut intermediate = head.clone();
    intermediate.splice(19..27, ["twenty!".into()]);
    fixture
        .write("a.txt", &text(&intermediate))
        .stage("a.txt")
        .write("a.txt", &text(&worktree))
        .commit_index("two: twenty");

    let snapshot = repo.refresh().unwrap();
    assert_eq!(
        owners(&snapshot, "a.txt"),
        vec![Some("one".into())],
        "an exact anchor match keeps membership regardless of position"
    );
    assert!(snapshot.notices.is_empty());
    // The committed hunk's record is not consumed — an external commit
    // leaves it dormant (gitchange's own commit would remove it, ADR 0004).
    assert_eq!(dormant_owners(&fixture), vec!["two"]);
}

#[test]
fn a_shifted_neighbour_silently_misfiles_into_the_committed_changelist() {
    // DEFECT (issue 37, flavour "silent wrong-list assignment"): the
    // partial commit shifts the neighbour's fresh old range down into the
    // committed record's stale region; a worktree edit breaks the
    // neighbour's anchor, so tier-2 inherits from the wrong record — no
    // notice, no dormancy, "one"'s hunk lands in "two". These assertions
    // pin the wrong behaviour; the fix flips them to Some("one").
    let fixture = RepoFixture::new();
    let head = numbered_lines(60);
    fixture.write("a.txt", &text(&head)).commit_all("init");
    let repo = repo(&fixture);

    // Changelist "two": replace lines 20..=31 with one line (delta -11).
    // Record old range: [17, 35).
    repo.create_changelist("two").unwrap();
    let mut worktree = head.clone();
    worktree.splice(19..31, ["twenty!".into()]);
    fixture.write("a.txt", &text(&worktree));
    repo.refresh().unwrap();

    // Changelist "one": edit original line 40. Record old range: [37, 44).
    repo.create_changelist("one").unwrap();
    repo.switch("one").unwrap();
    worktree[28] = "forty-v1".into();
    fixture.write("a.txt", &text(&worktree));
    let snapshot = repo.refresh().unwrap();
    assert_eq!(
        owners(&snapshot, "a.txt"),
        vec![Some("two".into()), Some("one".into())]
    );

    // A third changelist is active, so neither outcome can hide behind
    // active-capture landing on "one" by luck.
    repo.create_changelist("three").unwrap();
    repo.switch("three").unwrap();

    // Externally partial-commit "two"'s hunk, then keep editing "one"'s
    // hunk — the ordinary commit-and-keep-working flow.
    let mut intermediate = head.clone();
    intermediate.splice(19..31, ["twenty!".into()]);
    fixture
        .write("a.txt", &text(&intermediate))
        .stage("a.txt")
        .write("a.txt", &text(&worktree))
        .commit_index("two: twenty");
    worktree[28] = "forty-v2".into();
    fixture.write("a.txt", &text(&worktree));

    // Fresh hunk old range vs new HEAD: [26, 33) — inside "two"'s stale
    // [17, 35), clear of its own record's [37, 44).
    let snapshot = repo.refresh().unwrap();
    assert_eq!(
        owners(&snapshot, "a.txt"),
        vec![Some("two".into())],
        "WRONG per issue 37: the hunk belongs to \"one\""
    );
    assert!(
        snapshot.notices.is_empty(),
        "the misfile is silent — a single stale candidate raises no notice"
    );
    // "two"'s stale record was superseded by the misfiled fresh record;
    // "one"'s goes dormant — revivable exact-only, so with the hunk now
    // recorded under "two" it never comes back.
    assert_eq!(dormant_owners(&fixture), vec!["one"]);
}

#[test]
fn a_shifted_neighbour_clear_of_stale_records_captures_to_active() {
    // DEFECT (issue 37, the "miss" flavour): same shift, but the hunk
    // lands clear of every stale record, so it reads as brand new and
    // captures to the active changelist. Issue 37 calls this the
    // acceptable outcome — but note it is just as silent as the misfile:
    // no notice marks the membership loss.
    let fixture = RepoFixture::new();
    let head = numbered_lines(80);
    fixture.write("a.txt", &text(&head)).commit_all("init");
    let repo = repo(&fixture);

    // Changelist "two": replace lines 20..=31 with one line (delta -11).
    repo.create_changelist("two").unwrap();
    let mut worktree = head.clone();
    worktree.splice(19..31, ["twenty!".into()]);
    fixture.write("a.txt", &text(&worktree));
    repo.refresh().unwrap();

    // Changelist "one": edit original line 60, far enough down that the
    // -11 shift clears both stale records. Record old range: [57, 64).
    repo.create_changelist("one").unwrap();
    repo.switch("one").unwrap();
    worktree[48] = "sixty-v1".into();
    fixture.write("a.txt", &text(&worktree));
    repo.refresh().unwrap();

    repo.create_changelist("three").unwrap();
    repo.switch("three").unwrap();

    let mut intermediate = head.clone();
    intermediate.splice(19..31, ["twenty!".into()]);
    fixture
        .write("a.txt", &text(&intermediate))
        .stage("a.txt")
        .write("a.txt", &text(&worktree))
        .commit_index("two: twenty");
    worktree[48] = "sixty-v2".into();
    fixture.write("a.txt", &text(&worktree));

    // Fresh hunk old range vs new HEAD: [46, 53) — overlaps nothing.
    let snapshot = repo.refresh().unwrap();
    assert_eq!(
        owners(&snapshot, "a.txt"),
        vec![Some("three".into())],
        "WRONG per issue 37: the hunk belongs to \"one\"; it was captured \
         as if brand new"
    );
    assert!(
        snapshot.notices.is_empty(),
        "the membership loss is silent too"
    );
    assert_eq!(dormant_owners(&fixture), vec!["two", "one"]);
}

#[test]
fn a_residual_staged_stale_hunk_reattaches_when_nothing_shifts() {
    // The residual-◑ flow ADR 0004 banks on: stage a hunk, edit the
    // worktree further, commit the staged version. The residual hunk's
    // anchor differs from the record by construction (old side is now
    // the committed content), so tier-1 can never rescue it — but with
    // no line-count change above it, the stale coordinates happen to
    // still be right and tier-2 re-attaches it.
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
    fixture.commit_index("one: ten (staged version)");

    // Residual hunk: committed "ten-staged" ↔ worktree "ten-final", at
    // unchanged coordinates.
    let snapshot = repo.refresh().unwrap();
    assert_eq!(
        owners(&snapshot, "a.txt"),
        vec![Some("one".into())],
        "stale-but-unshifted coordinates re-attach the residual by overlap"
    );
    assert!(snapshot.notices.is_empty());
    assert!(dormant_owners(&fixture).is_empty());
}

#[test]
fn a_residual_staged_stale_hunk_sheds_membership_when_the_commit_shifts_it() {
    // DEFECT (issue 37, flavour "residual-◑"): commit a changelist whose
    // payload also shrinks the file above the ◑ hunk — exactly what
    // gitchange's own commit of a two-hunk changelist will do (#28). The
    // residual hunk's fresh old range shifts by the payload's delta, its
    // retained record still holds old-HEAD coordinates, and ADR 0004's
    // "retained for overlap re-attachment" misses exactly when it is
    // needed. The residual captures to whatever is active instead.
    let fixture = RepoFixture::new();
    let head = numbered_lines(60);
    fixture.write("a.txt", &text(&head)).commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("one").unwrap();

    // Changelist "one", two hunks: replace lines 10..=21 with one line
    // (delta -11), and edit original line 40.
    let mut worktree = head.clone();
    worktree.splice(9..21, ["ten!".into()]);
    worktree[28] = "forty-staged".into();
    fixture.write("a.txt", &text(&worktree)).stage("a.txt");
    // Edit the second hunk further: staged-stale (◑). Record old range
    // stays [37, 44) — HEAD hasn't moved yet.
    worktree[28] = "forty-final".into();
    fixture.write("a.txt", &text(&worktree));
    repo.refresh().unwrap();

    repo.create_changelist("two").unwrap();
    repo.switch("two").unwrap();
    // Commit the staged payload: both hunks, the ◑ one as-is.
    fixture.commit_index("one: both hunks, staged versions");

    // Residual hunk old range vs new HEAD: [26, 33) — clear of the
    // retained record's stale [37, 44).
    let snapshot = repo.refresh().unwrap();
    assert_eq!(
        owners(&snapshot, "a.txt"),
        vec![Some("two".into())],
        "WRONG per issue 37: the residual ◑ hunk belongs to \"one\""
    );
    assert!(snapshot.notices.is_empty(), "the loss is silent");
    // Both of "one"'s records linger dormant, revivable exact-only —
    // which the residual's anchor can never satisfy.
    assert_eq!(dormant_owners(&fixture), vec!["one", "one"]);
}
