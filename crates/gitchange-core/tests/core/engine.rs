//! The real-fs Engine smoke tests (ADR 0008): a worktree edit and a
//! sequence of external git operations, each expected to produce a
//! `RefreshComplete` reflecting it within a generous timeout. They prove
//! the notify wiring on each CI OS — the decision logic (debounce,
//! self-loop filter, last-request-wins) is unit-tested deterministically
//! inside the engine module. No assertion touches real debounce timing.

use std::time::{Duration, Instant};

use crate::support::RepoFixture;
use gitchange_core::{Engine, EngineEvent, FileStage, Snapshot};

/// Per-wait ceiling. Generous by design (ADR 0008): it exists to fail a
/// hung watcher, and is never a claim about how long a refresh takes.
const WAIT: Duration = Duration::from_secs(30);

#[test]
fn touching_a_file_produces_a_refresh_complete() {
    let fixture = RepoFixture::new();
    fixture.write("a.txt", "one\n").commit_all("init");

    let engine = Engine::spawn(fixture.path()).expect("spawn engine");
    // The initial unasked refresh sees a clean tree.
    wait_for_snapshot(&engine, |snapshot| snapshot.files.is_empty());

    fixture.write("a.txt", "one\ntwo\n");
    wait_for_snapshot(&engine, |snapshot| {
        snapshot.files.iter().any(|file| file.path == "a.txt")
    });
}

#[test]
fn external_git_operations_are_absorbed_through_the_real_watcher() {
    // ADR 0003's absorption arrives as writes under the git dir, which
    // the self-loop filter deliberately keeps (only `$GIT_DIR/gitchange/`
    // is ours). The filter's arms are unit-tested against literal paths;
    // what this adds is that a real git-dir write reaches a refresh
    // through the real watcher on each CI OS. Absorption semantics
    // themselves are covered watcher-independently in the core tests.
    //
    // Three operations, because they wake the watcher by different
    // writes: `add` writes a blob and the index, `reset` writes the index
    // and nothing else — the one step that can only have come from an
    // index event — and the commit moves HEAD and its ref.
    //
    // They go through libgit2 per ADR 0008's fixture rule: these builders
    // write the same index, HEAD and ref files real git writes, and paths
    // are all a watcher ever sees.
    let fixture = RepoFixture::new();
    fixture.write("a.txt", "one\n").commit_all("init");
    // Dirtied before spawn, so from here the worktree is never touched
    // again: every event the engine acts on below is a git-dir event.
    fixture.write("a.txt", "one\ntwo\n");

    let engine = Engine::spawn(fixture.path()).expect("spawn engine");
    wait_for_snapshot(&engine, |snapshot| {
        stage_of(snapshot, "a.txt") == Some(FileStage::Unstaged)
    });

    // `git add a.txt` in another terminal.
    fixture.stage("a.txt");
    wait_for_snapshot(&engine, |snapshot| {
        stage_of(snapshot, "a.txt") == Some(FileStage::Staged)
    });

    // `git reset -- a.txt`: the index reverts to HEAD's entry, whose blob
    // the odb already holds, so this write lands on the index alone —
    // asserted, not assumed, since it is the whole point of the step.
    let objects = fixture.odb_object_count();
    fixture.reset_path("a.txt");
    assert_eq!(
        fixture.odb_object_count(),
        objects,
        "the reset writes no object: only an index event can wake this one"
    );
    wait_for_snapshot(&engine, |snapshot| {
        stage_of(snapshot, "a.txt") == Some(FileStage::Unstaged)
    });

    // `git commit` in another terminal: HEAD moves and the tree goes
    // clean, which neither state above can be mistaken for.
    fixture.stage("a.txt").commit_index("external: a.txt");
    wait_for_snapshot(&engine, |snapshot| snapshot.files.is_empty());
}

/// `path`'s per-file stage marker in `snapshot`, `None` when the file
/// isn't in the universe at all.
fn stage_of(snapshot: &Snapshot, path: &str) -> Option<FileStage> {
    snapshot
        .files
        .iter()
        .find(|file| file.path == path)
        .map(|file| file.stage())
}

/// Drain engine events until a `RefreshComplete` whose snapshot satisfies
/// `accept`, panicking past [`WAIT`]. Watcher backends may emit interim
/// snapshots (and platform noise); only the awaited state matters, and
/// refreshes are serialized, so a snapshot arriving after one that
/// satisfied an earlier wait was computed after it too.
fn wait_for_snapshot(engine: &Engine, accept: impl Fn(&Snapshot) -> bool) {
    let deadline = Instant::now() + WAIT;
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .expect("expected snapshot within the generous timeout");
        match engine.events().recv_timeout(remaining) {
            Ok(EngineEvent::RefreshComplete(refreshed)) => {
                if accept(&refreshed.snapshot) {
                    return;
                }
            }
            Ok(EngineEvent::RefreshStarted) => {}
            Ok(other) => panic!("unexpected engine event: {other:?}"),
            Err(err) => panic!("engine events dried up: {err}"),
        }
    }
}
