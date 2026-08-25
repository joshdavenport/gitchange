//! ADR 0004's temporary index: what a commit writes through it — the
//! changelist's staged hunks and nothing else, live index and worktree
//! untouched — what the hooks see of it, and the three ways it refuses:
//! a rejecting hook, a refused apply, and an index entry a second holder
//! has content in (issue #106), which refuses before the temp index
//! exists at all.

use std::fs;

use crate::support::RepoFixture;
use gitchange_core::{CommitOptions, CommitOutcome, Error, HunkStage, Repo};

// Gated with its only user below, so Windows builds this file clean.
#[cfg(unix)]
use gitchange_core::ApplySite;

use super::helpers::{
    commit, dormant_owners, numbered_lines, owners, repo, stages, state_json, text,
};

#[test]
fn commit_writes_only_the_changelists_staged_hunks() {
    let fixture = RepoFixture::new();
    let head = numbered_lines(30);
    fixture.write("a.txt", &text(&head)).commit_all("init");
    let repo = repo(&fixture);

    // Changelist "one": edit line 10.
    repo.create_changelist("one").unwrap();
    repo.switch(Some("one")).unwrap();
    let mut worktree = head.clone();
    worktree[9] = "ten-one".into();
    fixture.write("a.txt", &text(&worktree));
    repo.refresh().unwrap();

    // Changelist "two": edit line 20.
    repo.create_changelist("two").unwrap();
    repo.switch(Some("two")).unwrap();
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

    let refreshed = repo.refresh().unwrap();
    let snapshot = &refreshed.snapshot;
    assert_eq!(
        owners(snapshot, "a.txt"),
        vec![Some("two".into())],
        "changelist B's hunk stays in B"
    );
    assert_eq!(
        stages(snapshot, "a.txt"),
        vec![HunkStage::Staged],
        "changelist B's staged hunk stays staged post-commit"
    );
    assert!(refreshed.advisories.is_empty());
    // "one"'s record was fully consumed and removed, not left dormant.
    assert!(dormant_owners(&fixture).is_empty());
    // The emptied changelist is kept until explicitly deleted.
    assert!(snapshot.changelists.iter().any(|cl| cl.name == "one"));
}

#[test]
fn a_hook_sees_the_commits_true_content() {
    let fixture = RepoFixture::new();
    let head = numbered_lines(30);
    fixture.write("a.txt", &text(&head)).commit_all("init");
    let repo = repo(&fixture);

    repo.create_changelist("one").unwrap();
    repo.switch(Some("one")).unwrap();
    let mut worktree = head.clone();
    worktree[9] = "ten-one".into();
    fixture.write("a.txt", &text(&worktree));
    repo.refresh().unwrap();

    repo.create_changelist("two").unwrap();
    repo.switch(Some("two")).unwrap();
    worktree[19] = "twenty-two".into();
    fixture.write("a.txt", &text(&worktree));
    repo.refresh().unwrap();
    fixture.stage("a.txt");

    fixture.with_hook(
        "pre-commit",
        "#!/bin/sh\ngit diff --cached > \"$(git rev-parse --git-dir)/hook-saw.diff\"\n",
    );
    commit(&repo, Some("one"), "one: ten");

    // A missing file means git never executed the hook at all, which is a
    // different failure from a hook that saw the wrong content — and the
    // one a platform without a usable `sh` would produce.
    let saw = fs::read_to_string(fixture.path().join(".git/hook-saw.diff"))
        .expect("the pre-commit hook ran and wrote its capture");
    assert!(saw.contains("ten-one"), "hook sees the payload: {saw}");
    assert!(
        !saw.contains("twenty-two"),
        "hook does not see other changelists' staged hunks: {saw}"
    );
}

/// One staged hunk in changelist "one" — the starting point the three
/// commit-failure tests below share. Shared rather than repeated so
/// "same fixture, one difference" is structural: the `--no-verify`
/// bypass is only evidence about the flag if the rejection it walks past
/// is demonstrably the same rejection. Returns the worktree lines.
fn one_staged_hunk_in_one(fixture: &RepoFixture) -> Vec<String> {
    let head = numbered_lines(20);
    fixture.write("a.txt", &text(&head)).commit_all("init");
    let repo = repo(fixture);
    repo.create_changelist("one").unwrap();
    repo.switch(Some("one")).unwrap();
    let mut worktree = head;
    worktree[9] = "ten-one".into();
    fixture.write("a.txt", &text(&worktree)).stage("a.txt");
    repo.refresh().unwrap();
    worktree
}

#[test]
fn hook_rejection_changes_nothing() {
    let fixture = RepoFixture::new();
    let worktree = one_staged_hunk_in_one(&fixture);
    let repo = repo(&fixture);

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
    // The rejection lands *after* the temp index and message file are
    // written, so this is where the discard-on-failure guarantee is
    // actually exercised (ADR 0004).
    assert_eq!(
        fixture.state_dir_entries(),
        vec!["state.json"],
        "the temp index and message file are discarded"
    );
}

#[test]
fn no_verify_commits_past_a_rejecting_hook() {
    // The same fixture, hook and payload as the rejection above, with
    // `no_verify` the only difference — the pair is what shows the flag
    // is doing the work. What it produces is an ordinary commit, not a
    // special case: HEAD moves, the payload lands, the record goes.
    let fixture = RepoFixture::new();
    let worktree = one_staged_hunk_in_one(&fixture);
    let repo = repo(&fixture);

    fixture.with_hook("pre-commit", "#!/bin/sh\necho nope >&2\nexit 1\n");
    let outcome = repo
        .commit(
            Some("one"),
            "one: ten",
            &CommitOptions {
                no_verify: true,
                amend: false,
            },
            None,
        )
        .unwrap();
    assert!(matches!(outcome, CommitOutcome::Committed { .. }));

    assert_eq!(fixture.commit_count(), 2, "the commit was created");
    assert_eq!(
        fixture.head_bytes("a.txt"),
        Some(text(&worktree).into_bytes()),
        "the payload landed in HEAD"
    );
    assert_eq!(fixture.head_message(), "one: ten\n");

    let refreshed = repo.refresh().unwrap();
    let snapshot = &refreshed.snapshot;
    assert!(
        !snapshot.files.iter().any(|file| file.path == "a.txt"),
        "the payload was fully consumed, so nothing is left changed"
    );
    assert!(refreshed.advisories.is_empty());
    // Fully consumed: the record is removed outright, not left dormant.
    assert!(
        state_json(&fixture)["records"]
            .as_array()
            .unwrap()
            .is_empty(),
        "the consumed record is gone: {}",
        state_json(&fixture)["records"]
    );
}

/// ADR 0004's apply-failure abort: a refused temp-index apply aborts
/// before any commit exists and changes nothing.
///
/// The refusal is forced by an unwritable object store, not by a
/// mismatching payload — that shape is unreachable through the public
/// ops, since the diff applied is computed from HEAD's tree against the
/// live index and applied straight back to that tree (issue #58). The
/// refusal maps to [`Error::ApplyFailed`] at the commit site — the
/// apply tripwire's second sensor (ADR 0003) — carrying libgit2's
/// message verbatim and stating ADR 0004's abort guarantee. Like the
/// staging twin, an unwritable odb is not the tripwire's trigger (#55);
/// what this certifies is the reporting path. The state dir stays clean
/// because the abort precedes the temp index being written, so it is
/// [`hook_rejection_changes_nothing`] that covers discarding files which
/// do exist.
// Unix-only by necessity: no Windows equivalent of a 0o500 directory, so
// the refusal has no way to happen there (ADR 0008).
#[cfg(unix)]
#[test]
fn a_refused_temp_index_apply_aborts_before_any_commit_exists() {
    let fixture = RepoFixture::new();
    let worktree = one_staged_hunk_in_one(&fixture);
    let repo = repo(&fixture);

    let state_before =
        fs::read_to_string(fixture.path().join(".git/gitchange/state.json")).unwrap();
    let index_before = fixture.index_content("a.txt");

    let _odb = fixture.unwritable_odb();
    let err = repo
        .commit(Some("one"), "one: ten", &CommitOptions::default(), None)
        .unwrap_err();
    // The variant pins the refusal to the apply itself: everything
    // else in the commit path the same mode breaks stays `Backend`.
    match &err {
        Error::ApplyFailed { path, detail, site } => {
            assert_eq!(path, "a.txt", "the error names the file");
            assert_eq!(*site, ApplySite::CommitTempIndex, "and the commit site");
            assert!(
                detail.contains("Permission denied"),
                "libgit2's own refusal, verbatim: {detail}"
            );
        }
        other => panic!("expected ApplyFailed, got {other:?}"),
    }
    assert!(
        err.to_string().contains("nothing was committed"),
        "the abort guarantee is stated: {err}"
    );

    assert_eq!(fixture.commit_count(), 1, "no commit was created");
    assert_eq!(fixture.index_content("a.txt"), index_before);
    assert_eq!(
        fs::read_to_string(fixture.path().join("a.txt")).unwrap(),
        text(&worktree),
        "the worktree is never touched"
    );
    assert_eq!(
        fs::read_to_string(fixture.path().join(".git/gitchange/state.json")).unwrap(),
        state_before,
        "state file untouched by the aborted commit"
    );
    assert_eq!(
        fixture.state_dir_entries(),
        vec!["state.json"],
        "no temp index left behind"
    );
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
    repo.switch(Some("art")).unwrap();
    fixture
        .write_bytes("logo.png", &[0u8, 9, 9])
        .stage("logo.png");
    repo.refresh().unwrap();

    // A second changelist owns a staged text edit that must not commit.
    repo.create_changelist("other").unwrap();
    repo.switch(Some("other")).unwrap();
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
    let snapshot = repo.refresh().unwrap().snapshot;
    assert!(!snapshot.files.iter().any(|file| file.path == "logo.png"));
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
    repo.switch(Some("art")).unwrap();
    fs::remove_file(fixture.path().join("logo.png")).unwrap();
    fixture.stage_removal("logo.png");

    let payload = repo.commit_payload(Some("art")).unwrap();
    assert_eq!(payload.file_count(), 1);
    assert_eq!(payload.staged_hunks(), 1);

    commit(&repo, Some("art"), "drop binary");
    assert!(fixture.head_bytes("logo.png").is_none());
    assert!(fixture.head_bytes("keep.txt").is_some());
}

#[cfg(unix)]
#[test]
fn a_whole_file_commit_leaves_another_changelists_mode_flip_behind() {
    // ADR 0017: the whole-file payload's temp-index entry takes its mode
    // from the changelist's own mode hunk, or HEAD's when none is owned —
    // never the live entry's. Here the flip is another changelist's, so
    // the commit lands HEAD's 644 over the new bytes.
    let fixture = RepoFixture::new();
    fixture
        .write_bytes("logo.png", &[0u8, 1, 2, 3])
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("art").unwrap();
    repo.switch(Some("art")).unwrap();
    fixture
        .write_bytes("logo.png", &[0u8, 9, 9])
        .stage("logo.png");
    repo.refresh().unwrap();

    // A second changelist owns the chmod, staged.
    repo.create_changelist("chores").unwrap();
    repo.switch(Some("chores")).unwrap();
    fixture.set_exec("logo.png").stage("logo.png");
    let snapshot = repo.refresh().unwrap().snapshot;
    assert_eq!(
        owners(&snapshot, "logo.png"),
        vec![Some("chores".to_string()), Some("art".to_string())],
        "the mode hunk sits first, owned by chores"
    );

    commit(&repo, Some("art"), "art: new bytes");
    assert_eq!(fixture.head_bytes("logo.png").unwrap(), vec![0u8, 9, 9]);
    assert_eq!(
        fixture.head_mode("logo.png"),
        Some(0o100644),
        "the mode hunk chores owns stays out of art's commit"
    );
    assert_eq!(
        fixture.index_mode("logo.png"),
        Some(0o100755),
        "the live index keeps the staged flip"
    );
}

#[cfg(unix)]
#[test]
fn a_whole_file_commit_lands_the_mode_flip_it_owns() {
    // The mirror: one changelist owns both hunks, so its payload carries
    // the mode hunk and HEAD gets the flip.
    let fixture = RepoFixture::new();
    fixture
        .write_bytes("logo.png", &[0u8, 1, 2, 3])
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("art").unwrap();
    repo.switch(Some("art")).unwrap();
    fixture
        .write_bytes("logo.png", &[0u8, 9, 9])
        .set_exec("logo.png")
        .stage("logo.png");

    let payload = repo.commit_payload(Some("art")).unwrap();
    assert_eq!(payload.staged_hunks(), 2, "the mode hunk and the bytes");

    commit(&repo, Some("art"), "art: new bytes, executable");
    assert_eq!(fixture.head_bytes("logo.png").unwrap(), vec![0u8, 9, 9]);
    assert_eq!(fixture.head_mode("logo.png"), Some(0o100755));
}

#[test]
fn a_binary_worktree_over_staged_text_commits_the_staged_text() {
    // The mixed case: text content staged, then the worktree file
    // replaced with binary bytes. Both deltas are the universe's, each
    // `◑` — the worktree's binary rewrite as a whole-file hunk, the
    // staged edit as an index-only text hunk (ADR 0017: no side's delta
    // is dropped because the other side's hunks survived pairing).
    // Committing carries the staged text blob whole-file rather than
    // silently excluding the path.
    let fixture = RepoFixture::new();
    fixture.write("notes.txt", "original\n").commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("work").unwrap();
    repo.switch(Some("work")).unwrap();
    fixture
        .write("notes.txt", "staged text\n")
        .stage("notes.txt");
    fixture.write_bytes("notes.txt", &[0u8, 1, 2, 3]);

    let payload = repo.commit_payload(Some("work")).unwrap();
    assert_eq!(payload.file_count(), 1);
    assert_eq!(payload.stale_hunks(), 2, "index and worktree differ: ◑");

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

/// The #106 content shape with two holders in one index entry, built out
/// of nothing but real ops: a staged text edit split between two holders
/// *before* the worktree turns binary. The whole-file hunk then arrives at
/// an entry ADR 0009's unit rule cannot unify — two owners are already
/// established in it — and captures normally, which is the state ADR 0004's
/// refusal exists to backstop. `second` is the other holder, `None` being
/// unassigned, which counts as a holder like any changelist.
///
/// Returns the staged lines too: the live index must still hold them after
/// a refusal.
fn split_entry(second: Option<&str>) -> (RepoFixture, Repo, Vec<String>) {
    let fixture = RepoFixture::new();
    fixture
        .write("notes.txt", &text(&numbered_lines(30)))
        .commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("art").unwrap();
    if let Some(name) = second {
        repo.create_changelist(name).unwrap();
    }
    repo.switch(Some("art")).unwrap();
    let mut lines = numbered_lines(30);
    lines[4] = "five!".into();
    lines[24] = "twentyfive!".into();
    fixture.write("notes.txt", &text(&lines)).stage("notes.txt");
    let snapshot = repo.refresh().unwrap().snapshot;
    let far = snapshot.files[0].hunks[1].clone();
    // Capture-off before a release, or the uniform rule captures the
    // released hunk straight back (ADR 0016).
    if second.is_none() {
        repo.switch(None).unwrap();
    }
    repo.assign_hunks("notes.txt", &[far], second).unwrap();
    let snapshot = repo.refresh().unwrap().snapshot;
    assert_eq!(
        owners(&snapshot, "notes.txt"),
        vec![Some("art".to_owned()), second.map(str::to_owned)],
        "precondition: two holders in the entry, before any binary appears"
    );

    // The worktree turns binary: one entry, a whole-file hunk over content
    // two holders own.
    fixture.write_bytes("notes.txt", &[0u8, 1, 2, 3]);
    let snapshot = repo.refresh().unwrap().snapshot;
    assert!(
        snapshot.files[0].hunks[0].is_whole_file(),
        "the rewrite presents a whole-file hunk"
    );
    assert_eq!(
        owners(&snapshot, "notes.txt")[1..],
        [Some("art".to_owned()), second.map(str::to_owned)],
        "precondition: the content hunks keep their two holders"
    );
    (fixture, repo, lines)
}

#[test]
fn a_whole_file_payload_refuses_an_entry_a_second_changelist_holds_content_in() {
    // ADR 0004's foreign-content refusal (issue #106): the entry commits
    // whole, so committing either holder would carry the other's staged
    // content. Both directions refuse, naming who else is in the entry, and
    // the refusal precedes every write — no temp index, no commit, and the
    // live index still holds what it held.
    let (fixture, repo, lines) = split_entry(Some("other"));

    for (committing, holder) in [(Some("art"), "other"), (Some("other"), "art")] {
        let err = repo.commit_payload(committing).unwrap_err();
        let Error::ForeignEntryContent { path, holders } = &err else {
            panic!("expected a foreign-content refusal, got {err:?}");
        };
        assert_eq!(path, "notes.txt");
        assert_eq!(*holders, vec![Some(holder.to_owned())]);
        assert!(
            err.to_string()
                .contains(&format!("content held by '{holder}'")),
            "the advisory names the other holder: {err}"
        );

        // The commit itself refuses the same way, before any temp-index
        // work.
        let err = repo
            .commit(
                committing,
                "should not land",
                &CommitOptions::default(),
                None,
            )
            .unwrap_err();
        assert!(matches!(err, Error::ForeignEntryContent { .. }), "{err:?}");
    }

    assert_eq!(fixture.commit_count(), 1, "nothing was committed");
    assert_eq!(
        fixture.state_dir_entries(),
        vec!["state.json"],
        "no temp index or message file was created"
    );
    assert_eq!(
        fixture.index_content("notes.txt").unwrap(),
        text(&lines),
        "the live index is untouched"
    );
}

#[test]
fn unassigned_counts_as_a_holder_of_a_shared_index_entry() {
    // Unassigned is a holder like any changelist (ADR 0004). The split is
    // built under capture-off, where the unit rule claims nothing
    // (ADR 0015): the whole-file hunk and the released content hunk are
    // both unassigned, 'art' holds the other content hunk, and each side
    // refuses on the other's content.
    let (fixture, repo, _lines) = split_entry(None);
    let snapshot = repo.refresh().unwrap().snapshot;
    assert_eq!(
        owners(&snapshot, "notes.txt"),
        vec![None, Some("art".to_owned()), None],
        "capture-off leaves the newcomer unassigned, split and all"
    );

    let err = repo.commit_payload(Some("art")).unwrap_err();
    assert!(
        matches!(&err, Error::ForeignEntryContent { holders, .. } if *holders == vec![None]),
        "{err:?}"
    );
    assert!(
        err.to_string().contains("content held by unassigned"),
        "the advisory names unassigned as the holder: {err}"
    );

    let err = repo.commit_payload(None).unwrap_err();
    assert!(
        matches!(&err, Error::ForeignEntryContent { holders, .. }
            if *holders == vec![Some("art".to_owned())]),
        "{err:?}"
    );
    assert_eq!(fixture.commit_count(), 1, "nothing was committed");
}

#[cfg(unix)]
#[test]
fn a_mode_only_payload_commits_past_a_split_entry() {
    // The carve-out that keeps the refusal from over-firing: a mode hunk
    // holds no content and takes nothing out of the entry (ADR 0017), so
    // the changelist owning it commits while the split below is still
    // unresolved.
    let (fixture, repo, lines) = split_entry(Some("other"));
    repo.create_changelist("chores").unwrap();
    repo.switch(Some("chores")).unwrap();
    fixture.set_exec("notes.txt");
    let snapshot = repo.refresh().unwrap().snapshot;
    let mode = snapshot.files[0].hunks[0].clone();
    assert!(mode.is_mode_change(), "the mode hunk sits first");
    assert_eq!(
        owners(&snapshot, "notes.txt")[0],
        Some("chores".to_owned()),
        "the mode hunk captures to active: it is outside the entry's unit"
    );
    // Staged through its own hunk, which keeps the entry's staged blob.
    repo.stage_hunk("notes.txt", &mode).unwrap();

    commit(&repo, Some("chores"), "chores: make it executable");
    assert_eq!(fixture.head_mode("notes.txt"), Some(0o100755));
    assert_eq!(
        fixture.head_bytes("notes.txt").unwrap(),
        text(&numbered_lines(30)).into_bytes(),
        "the mode hunk commits no content, split or not"
    );
    assert_eq!(
        fixture.index_content("notes.txt").unwrap(),
        text(&lines),
        "the live index keeps the contested staged text"
    );
}

#[test]
fn assigning_the_unit_clears_a_refusal() {
    // The advisory's instruction has to work: assigning the file's hunks to
    // one changelist is a single op, because they assign as a unit
    // (ADR 0009), and the entry then has one holder and commits. Without
    // this the refusal would be a dead end.
    let (fixture, repo, lines) = split_entry(Some("other"));
    let snapshot = repo.refresh().unwrap().snapshot;
    let whole_file = snapshot.files[0].hunks[0].clone();
    let outcome = repo
        .assign_hunks("notes.txt", &[whole_file], Some("art"))
        .unwrap();
    assert_eq!(
        outcome.echo.as_deref(),
        Some("assigned 3 hunks — notes.txt → 'art'"),
        "one op moves the whole entry, the other holder's hunk included"
    );

    let snapshot = repo.refresh().unwrap().snapshot;
    assert_eq!(
        owners(&snapshot, "notes.txt"),
        vec![Some("art".to_owned()); 3],
        "one holder in the entry"
    );
    commit(&repo, Some("art"), "art: the staged text");
    assert_eq!(
        fixture.head_bytes("notes.txt").unwrap(),
        text(&lines).into_bytes(),
        "the entry commits whole, as ADR 0009 has it"
    );
}

#[test]
fn an_unstaged_hunk_of_a_shared_entry_refuses_nothing() {
    // The refusal's other carve-out (ADR 0004): an unstaged hunk is in the
    // worktree alone, so a commit of the entry neither carries it nor
    // broadens it — and refusing over one would be a dead end with no
    // cause. Here 'text' owns the worktree's edit while unassigned holds
    // the staged binary the entry actually carries.
    let fixture = RepoFixture::new();
    fixture.write("notes.txt", "original\n").commit_all("init");
    let repo = repo(&fixture);
    repo.create_changelist("text").unwrap();
    repo.switch(Some("text")).unwrap();
    fixture.write("notes.txt", "edited text\n");
    repo.refresh().unwrap();

    // Capture-off, so the whole-file hunk the staged rewrite adds stays
    // unassigned instead of joining 'text' (ADR 0009): a split, but only
    // across an unstaged hunk.
    repo.switch(None).unwrap();
    fixture
        .write_bytes("notes.txt", &[0u8, 1, 2, 3])
        .stage("notes.txt");
    fixture.write("notes.txt", "edited text\n");
    let snapshot = repo.refresh().unwrap().snapshot;
    assert_eq!(
        owners(&snapshot, "notes.txt"),
        vec![None, Some("text".to_owned())],
    );
    assert_eq!(
        stages(&snapshot, "notes.txt")[1],
        HunkStage::Unstaged,
        "'text' owns a worktree-only hunk: nothing of it is in the entry"
    );

    commit(&repo, None, "the staged rewrite");
    assert_eq!(
        fixture.head_bytes("notes.txt").unwrap(),
        vec![0u8, 1, 2, 3],
        "the entry's one holder commits it"
    );
    assert_eq!(
        fs::read_to_string(fixture.path().join("notes.txt")).unwrap(),
        "edited text\n",
        "the worktree, and 'text''s hunk with it, is untouched"
    );
}
