//! The one real-fs Engine smoke test (ADR 0008): touch a file, expect a
//! `RefreshComplete` carrying it within a generous timeout. It proves
//! the notify wiring on each CI OS — the decision logic (debounce,
//! self-loop filter, last-request-wins) is unit-tested deterministically
//! inside the engine module. No assertion touches real debounce timing.

mod support;

use std::time::{Duration, Instant};

use gitchange_core::{Engine, EngineEvent};
use support::RepoFixture;

#[test]
fn touching_a_file_produces_a_refresh_complete() {
    let fixture = RepoFixture::new();
    fixture.write("a.txt", "one\n").commit_all("init");

    let engine = Engine::spawn(fixture.path()).expect("spawn engine");
    // The initial unasked refresh sees a clean tree.
    let deadline = Instant::now() + Duration::from_secs(30);
    wait_for_snapshot(&engine, deadline, |files: &[String]| files.is_empty());

    fixture.write("a.txt", "one\ntwo\n");
    wait_for_snapshot(&engine, deadline, |files| {
        files.iter().any(|path| path == "a.txt")
    });
}

/// Drain engine events until a `RefreshComplete` whose file paths
/// satisfy `accept`, panicking past `deadline`. Watcher backends may
/// emit interim snapshots (and platform noise); only the awaited state
/// matters.
fn wait_for_snapshot(engine: &Engine, deadline: Instant, accept: impl Fn(&[String]) -> bool) {
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .expect("expected snapshot within the generous timeout");
        match engine.events().recv_timeout(remaining) {
            Ok(EngineEvent::RefreshComplete(snapshot)) => {
                let paths: Vec<String> = snapshot
                    .files
                    .iter()
                    .map(|file| file.path.clone())
                    .collect();
                if accept(&paths) {
                    return;
                }
            }
            Ok(EngineEvent::RefreshStarted) => {}
            Ok(other) => panic!("unexpected engine event: {other:?}"),
            Err(err) => panic!("engine events dried up: {err}"),
        }
    }
}
