//! HEAD-move staleness (issue 37): the matcher's tier-2 overlap runs on
//! HEAD-side old ranges, valid only while HEAD is unchanged. ADR 0012's
//! baseline HEAD guard (issue 39) covers the external-move half: when
//! refresh finds HEAD away from the stored baseline, tier-2 is disabled
//! for the paths the move changed — stale live records go dormant, and
//! anchor-broken hunks capture to active with a per-path notice. The
//! own-commit half (commutation, issue 28) lives in `Repo::commit`'s
//! record aftermath; `tests/commit.rs` pins the re-attachment these
//! same scenarios keep when the commit is gitchange's own.

mod support;

use std::fs;

use gitchange_core::{Notice, Repo, Snapshot};
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

/// Rewrite the state file's top-level fields in place — simulating
/// pre-baseline files and hand-edited or gc'd baselines.
fn patch_state(
    fixture: &RepoFixture,
    patch: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>),
) {
    let path = fixture.path().join(".git/gitchange/state.json");
    let mut json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    patch(json.as_object_mut().unwrap());
    fs::write(&path, serde_json::to_string_pretty(&json).unwrap()).unwrap();
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
fn a_shifted_neighbour_goes_dormant_loudly_instead_of_misfiling() {
    // Issue 37's "silent wrong-list assignment", closed by ADR 0012's
    // guard: the partial commit shifts the neighbour's fresh old range
    // down into the committed record's stale region, and a worktree edit
    // breaks the neighbour's anchor — but tier-2 is disabled for the
    // moved path, so the hunk captures to active instead of inheriting
    // from the wrong record, both stale records go dormant, and a notice
    // names the loss. (Restoring "one" itself is #38's re-baselining.)
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
    // [17, 35), clear of its own record's [37, 44). Without the guard
    // that overlap would silently misfile the hunk into "two".
    let snapshot = repo.refresh().unwrap();
    assert_eq!(
        owners(&snapshot, "a.txt"),
        vec![Some("three".into())],
        "the guarded tier captures to active, never a stale record's list"
    );
    assert_eq!(
        snapshot.notices,
        vec![Notice::HeadMoveDormancy {
            path: "a.txt".into(),
            changelists: vec!["one".into(), "two".into()],
        }]
    );
    assert_eq!(dormant_owners(&fixture), vec!["two", "one"]);
}

#[test]
fn a_shifted_neighbour_clear_of_stale_records_captures_to_active() {
    // Issue 37's "miss" flavour: same shift, but the hunk lands clear of
    // every stale record, so it reads as brand new and captures to the
    // active changelist — the same destination the guard picks. What the
    // guard adds is the notice: the membership loss is no longer silent.
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
        "clear of every stale record the hunk still reads as brand new"
    );
    assert_eq!(
        snapshot.notices,
        vec![Notice::HeadMoveDormancy {
            path: "a.txt".into(),
            changelists: vec!["one".into(), "two".into()],
        }],
        "the loss is loud now: dormancies alongside a guarded capture"
    );
    assert_eq!(dormant_owners(&fixture), vec!["two", "one"]);
}

#[test]
fn a_residual_staged_stale_hunk_goes_dormant_across_an_external_commit() {
    // The residual-◑ flow: stage a hunk, edit the worktree further,
    // commit the staged version. The residual hunk's anchor differs from
    // the record by construction (old side is now the committed
    // content), so tier-1 can never rescue it. Before ADR 0012 tier-2
    // happened to re-attach it here (no line-count change above), but an
    // external commit gives no proof of that, so the guard captures it
    // to active with a notice. gitchange's own commit rewrites retained
    // ◑ records instead and keeps the re-attachment (#28).
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
    // unchanged coordinates — but coordinates an external move can't
    // vouch for.
    let snapshot = repo.refresh().unwrap();
    assert_eq!(
        owners(&snapshot, "a.txt"),
        vec![Some("two".into())],
        "the guard never trusts stale coordinates, however plausible"
    );
    assert_eq!(
        snapshot.notices,
        vec![Notice::HeadMoveDormancy {
            path: "a.txt".into(),
            changelists: vec!["one".into()],
        }]
    );
    assert_eq!(dormant_owners(&fixture), vec!["one"]);
}

#[test]
fn a_residual_staged_stale_hunk_sheds_membership_when_the_commit_shifts_it() {
    // Issue 37, flavour "residual-◑": commit a changelist whose payload
    // also shrinks the file above the ◑ hunk — exactly what gitchange's
    // own commit of a two-hunk changelist will do (#28). The residual
    // hunk's fresh old range shifts by the payload's delta, its retained
    // record still holds old-HEAD coordinates, and it captures to
    // whatever is active. As an *external* commit that is the guard's
    // by-design outcome, now with a notice; #28's commutation rewrites
    // the retained record and keeps membership for own commits.
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
        "an external move sheds the shifted residual to active"
    );
    assert_eq!(
        snapshot.notices,
        vec![Notice::HeadMoveDormancy {
            path: "a.txt".into(),
            changelists: vec!["one".into()],
        }],
        "but no longer silently"
    );
    // Both of "one"'s records linger dormant, revivable exact-only —
    // which the residual's anchor can never satisfy.
    assert_eq!(dormant_owners(&fixture), vec!["one", "one"]);
}

#[test]
fn a_head_move_touching_only_other_paths_leaves_tier_two_intact() {
    // The guard is path-scoped: an external commit that never touched
    // a.txt leaves its record coordinates addressing the new HEAD too,
    // so overlap inheritance still runs there — and the persisting
    // refresh advances the stored baseline.
    let fixture = RepoFixture::new();
    let head = numbered_lines(20);
    fixture.write("a.txt", &text(&head)).commit_all("init");
    let repo = repo(&fixture);

    repo.create_changelist("one").unwrap();
    let mut worktree = head.clone();
    worktree[9] = "ten-v1".into();
    fixture.write("a.txt", &text(&worktree));
    repo.refresh().unwrap();

    repo.create_changelist("two").unwrap();
    repo.switch("two").unwrap();
    // External commit touching only b.txt.
    fixture
        .write("b.txt", "hello\n")
        .stage("b.txt")
        .commit_index("b.txt only");
    // Rework a.txt's hunk: anchor broken, overlap must still inherit.
    worktree[9] = "ten-v2".into();
    fixture.write("a.txt", &text(&worktree));

    let snapshot = repo.refresh().unwrap();
    assert_eq!(
        owners(&snapshot, "a.txt"),
        vec![Some("one".into())],
        "a.txt's coordinates still address the moved-to HEAD"
    );
    assert!(snapshot.notices.is_empty());
    assert!(dormant_owners(&fixture).is_empty());
    assert_eq!(state_json(&fixture)["baseline_head"], fixture.head_oid());
}

#[test]
fn a_pre_baseline_state_file_adopts_the_head_move_silently() {
    // Upgrade path (ADR 0012): a state file written before the baseline
    // field can't prove which HEAD its coordinates address, so the first
    // refresh trusts them — tier-2 runs exactly as before, no mass
    // dormancy, no notice — and stamps the current HEAD, arming the
    // guard for the next move.
    let fixture = RepoFixture::new();
    let head = numbered_lines(40);
    fixture.write("a.txt", &text(&head)).commit_all("init");
    let repo = repo(&fixture);

    repo.create_changelist("one").unwrap();
    let mut worktree = head.clone();
    worktree[29] = "thirty-v1".into();
    fixture.write("a.txt", &text(&worktree));
    repo.refresh().unwrap();

    repo.create_changelist("two").unwrap();
    repo.switch("two").unwrap();
    // External commit replacing line 10 in place — a HEAD move that
    // shifts nothing below it.
    let mut committed = head.clone();
    committed[9] = "ten-committed".into();
    fixture.write("a.txt", &text(&committed)).stage("a.txt");
    let mut worktree = committed.clone();
    worktree[29] = "thirty-v2".into();
    fixture
        .write("a.txt", &text(&worktree))
        .commit_index("external: ten");
    patch_state(&fixture, |state| {
        state.remove("baseline_head");
    });

    let snapshot = repo.refresh().unwrap();
    assert_eq!(
        owners(&snapshot, "a.txt"),
        vec![Some("one".into())],
        "with no baseline to distrust, overlap inheritance runs as before"
    );
    assert!(snapshot.notices.is_empty());
    assert!(dormant_owners(&fixture).is_empty());
    assert_eq!(state_json(&fixture)["baseline_head"], fixture.head_oid());
}

#[test]
fn an_unresolvable_baseline_degrades_to_all_paths_affected() {
    // A rebase can gc the baseline commit away: with diff(baseline↔HEAD)
    // impossible the guard cannot scope itself, so every path counts as
    // moved — visible dormancy rather than silent trust of coordinates
    // nothing can vouch for.
    let fixture = RepoFixture::new();
    let head = numbered_lines(20);
    fixture.write("a.txt", &text(&head)).commit_all("init");
    let repo = repo(&fixture);

    repo.create_changelist("one").unwrap();
    let mut worktree = head.clone();
    worktree[9] = "ten-v1".into();
    fixture.write("a.txt", &text(&worktree));
    repo.refresh().unwrap();

    repo.create_changelist("two").unwrap();
    repo.switch("two").unwrap();
    patch_state(&fixture, |state| {
        state.insert(
            "baseline_head".into(),
            "0123456789abcdef0123456789abcdef01234567".into(),
        );
    });
    // Rework the hunk: anchor broken, and no tree diff can prove a.txt
    // unmoved, so overlap inheritance is off.
    worktree[9] = "ten-v2".into();
    fixture.write("a.txt", &text(&worktree));

    let snapshot = repo.refresh().unwrap();
    assert_eq!(owners(&snapshot, "a.txt"), vec![Some("two".into())]);
    assert_eq!(
        snapshot.notices,
        vec![Notice::HeadMoveDormancy {
            path: "a.txt".into(),
            changelists: vec!["one".into()],
        }]
    );
    assert_eq!(dormant_owners(&fixture), vec!["one"]);
    assert_eq!(
        state_json(&fixture)["baseline_head"],
        fixture.head_oid(),
        "the guarded refresh re-stamps a resolvable baseline"
    );
}
