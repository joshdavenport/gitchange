//! The threaded Engine (ADR 0005/0006): a `notify` watcher over the
//! worktree root including `.git`, ~500ms debounce, self-loop filter,
//! last-request-wins RefreshJob slot, immediate refresh requests
//! (mutations, terminal focus, manual key), a crossbeam event channel,
//! and a ~5s polling fallback when the watcher fails or dies.
//!
//! The watcher sits behind an injectable event source (a plain crossbeam
//! channel of [`SourceEvent`]s), so the decision logic below is
//! unit-tested deterministically with synthetic events (ADR 0008); one
//! real-fs smoke test per CI OS proves the notify wiring.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, at, never, select, unbounded};
use notify::Watcher;

use crate::error::Error;
use crate::repo::Repo;
use crate::snapshot::Snapshot;

/// An ongoing condition (ADR 0007's pinned vocabulary) the Engine
/// reports over its event channel — never an `Error` (ADR 0006).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Condition {
    /// The filesystem watcher failed to initialise or died; the Engine
    /// is refreshing on a polling tick instead (ADR 0005).
    WatcherDegraded,
}

/// What the Engine emits over its crossbeam channel.
#[derive(Debug)]
#[non_exhaustive]
pub enum EngineEvent {
    /// One atomic RefreshJob finished; panels swap whole snapshots
    /// (ADR 0005).
    RefreshComplete(Snapshot),
    /// A refresh failed hard (not lock contention, which is transient
    /// and retried internally). The last snapshot stays valid.
    RefreshFailed(Error),
    ConditionStarted(Condition),
    ConditionEnded(Condition),
}

/// What the event source (real watcher or synthetic test feed) delivers
/// to the control loop.
#[derive(Debug)]
enum SourceEvent {
    /// Filesystem activity at these paths — debounced, self-loop
    /// filtered.
    Paths(Vec<PathBuf>),
    /// The watcher reported an error mid-stream (e.g. queue overflow):
    /// events may have been missed, so refresh — debounced, unfiltered.
    Lost,
}

/// Timing knobs, injectable so unit tests run at synthetic speed without
/// asserting on the real constants (ADR 0008).
#[derive(Debug, Clone, Copy)]
struct Config {
    debounce: Duration,
    poll_interval: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            debounce: Duration::from_millis(500),
            poll_interval: Duration::from_secs(5),
        }
    }
}

/// Drops our own writes so they don't re-trigger refresh (ADR 0005):
/// everything under `$GIT_DIR/gitchange/` — the state file, its lock and
/// tmp, and the ADR 0004 temp index once the commit ticket places it
/// there. Roots are matched both as given and canonicalized, because
/// watcher backends may report real paths (macOS `/private/var` vs
/// `/var`) while the repo handle holds the symlinked spelling.
struct SelfLoopFilter {
    roots: Vec<PathBuf>,
}

impl SelfLoopFilter {
    fn new(state_dir: PathBuf) -> Self {
        let mut roots = vec![state_dir.clone()];
        if let Ok(canonical) = state_dir.canonicalize()
            && !roots.contains(&canonical)
        {
            roots.push(canonical);
        }
        Self { roots }
    }

    /// Whether an event at `path` is our own write, to be dropped.
    fn is_self_loop(&self, path: &Path) -> bool {
        self.roots.iter().any(|root| path.starts_with(root))
    }
}

/// The threaded runtime layered on the sync ops — the TUI's layer of
/// core's interface (ADR 0006). Construct with [`Engine::spawn`], read
/// [`EngineEvent`]s from [`Engine::events`], request refreshes with
/// [`Engine::request_refresh`]. Dropping the Engine shuts its threads
/// down.
pub struct Engine {
    requests: Sender<()>,
    events: Receiver<EngineEvent>,
}

impl Engine {
    /// Open the repository containing `path` and start the Engine: the
    /// worker triggers an initial refresh immediately, so the first
    /// `RefreshComplete` arrives without a request. If the watcher can't
    /// be set up the Engine still runs, degraded to polling, and says so
    /// with `ConditionStarted(WatcherDegraded)`.
    pub fn spawn(path: &Path) -> Result<Self, Error> {
        let repo = Repo::discover(path)?;
        let workdir = repo
            .workdir()
            .ok_or_else(|| Error::Backend("bare repository has no worktree to watch".into()))?;
        let state_dir = repo.state_dir();
        let filter = SelfLoopFilter::new(state_dir.clone());

        let (source_tx, source_rx) = unbounded();
        let watcher = start_watcher(&workdir, &state_dir, source_tx);
        let degraded = watcher.is_none();

        Ok(spawn_loops(
            source_rx,
            filter,
            Box::new(move || repo.refresh()),
            Config::default(),
            degraded,
            watcher.map(|w| Box::new(w) as Box<dyn Send>),
        ))
    }

    /// The Engine's event channel, ready for the frontend's `Select`
    /// loop. Disconnects only if the Engine dies.
    pub fn events(&self) -> &Receiver<EngineEvent> {
        &self.events
    }

    /// Request an immediate refresh — no debounce. The path for
    /// gitchange's own mutations, terminal `FocusGained`, and the manual
    /// refresh key (ADR 0005). In-flight and queued requests collapse:
    /// at most one refresh runs at a time and at most one is pending.
    pub fn request_refresh(&self) {
        let _ = self.requests.send(());
    }
}

/// Start the notify watcher over the worktree root (which includes
/// `.git` in an ordinary repo) plus the private git dir when it lives
/// elsewhere (linked worktrees). `None` on any failure — the caller
/// degrades to polling; a watcher that dies later drops `source_tx`,
/// which the control loop sees as a disconnect.
fn start_watcher(
    workdir: &Path,
    state_dir: &Path,
    source_tx: Sender<SourceEvent>,
) -> Option<notify::RecommendedWatcher> {
    let mut watcher =
        notify::recommended_watcher(move |result: Result<notify::Event, notify::Error>| {
            let event = match result {
                Ok(event) => SourceEvent::Paths(event.paths),
                Err(_) => SourceEvent::Lost,
            };
            let _ = source_tx.send(event);
        })
        .ok()?;
    watcher
        .watch(workdir, notify::RecursiveMode::Recursive)
        .ok()?;
    // Linked worktree: the private git dir ($GIT_DIR, holding HEAD, the
    // index, and our state dir) sits under the main repo's .git, outside
    // the watched root — external staging/commits arrive through it.
    if let Some(git_dir) = state_dir.parent()
        && !git_dir.starts_with(workdir)
    {
        watcher
            .watch(git_dir, notify::RecursiveMode::Recursive)
            .ok()?;
    }
    Some(watcher)
}

/// The result of one RefreshJob, as the control loop needs it. The
/// worker sends `RefreshComplete`/`RefreshFailed` to the frontend
/// itself; contention is the one outcome the loop reacts to (retry).
enum RunOutcome {
    Done,
    Contended,
}

/// Wire up the control and worker threads. Separated from
/// [`Engine::spawn`] so unit tests inject a synthetic event source and
/// refresh function (ADR 0008).
fn spawn_loops(
    source: Receiver<SourceEvent>,
    filter: SelfLoopFilter,
    refresh: Box<dyn FnMut() -> Result<Snapshot, Error> + Send>,
    config: Config,
    initially_degraded: bool,
    // Dropped when the control loop exits — for the real Engine, the
    // notify watcher whose lifetime must track the loop's.
    watcher_keep_alive: Option<Box<dyn Send>>,
) -> Engine {
    let (requests_tx, requests_rx) = unbounded();
    let (events_tx, events_rx) = unbounded();
    let (run_tx, run_rx) = unbounded();
    let (done_tx, done_rx) = unbounded();

    let worker_events = events_tx.clone();
    std::thread::spawn(move || {
        worker_loop(run_rx, done_tx, refresh, worker_events);
    });
    std::thread::spawn(move || {
        control_loop(
            requests_rx,
            source,
            run_tx,
            done_rx,
            events_tx,
            filter,
            config,
            initially_degraded,
        );
        drop(watcher_keep_alive);
    });

    Engine {
        requests: requests_tx,
        events: events_rx,
    }
}

/// Run RefreshJobs one at a time. Lock contention is transient — a
/// concurrent CLI invocation holds the fail-fast lock only briefly — so
/// it is reported to the control loop for a single short retry, never
/// surfaced to the frontend as a failure.
fn worker_loop(
    run_rx: Receiver<()>,
    done_tx: Sender<RunOutcome>,
    mut refresh: Box<dyn FnMut() -> Result<Snapshot, Error> + Send>,
    events: Sender<EngineEvent>,
) {
    for () in run_rx {
        let outcome = match refresh() {
            Ok(snapshot) => {
                let _ = events.send(EngineEvent::RefreshComplete(snapshot));
                RunOutcome::Done
            }
            Err(Error::LockContention { .. }) => RunOutcome::Contended,
            Err(err) => {
                let _ = events.send(EngineEvent::RefreshFailed(err));
                RunOutcome::Done
            }
        };
        if done_tx.send(outcome).is_err() {
            return;
        }
    }
}

/// The Engine's decision logic — the part ADR 0006 calls domain rules,
/// tested with synthetic events. Owns the debounce and poll deadlines
/// and the last-request-wins slot state.
#[allow(clippy::too_many_arguments)]
fn control_loop(
    requests: Receiver<()>,
    source: Receiver<SourceEvent>,
    run_tx: Sender<()>,
    done_rx: Receiver<RunOutcome>,
    events: Sender<EngineEvent>,
    filter: SelfLoopFilter,
    config: Config,
    initially_degraded: bool,
) {
    let mut source = source;
    // Last-request-wins slot: one job in flight, at most one pending.
    // Refresh takes no parameters and the matcher is pure, so collapsing
    // any burst to "run once more after this one" loses nothing.
    let mut job_running = false;
    let mut pending = false;
    let mut debounce_deadline: Option<Instant> = None;
    let mut poll_deadline: Option<Instant> = None;
    let mut degraded = false;
    // One short retry after a contended refresh; further contention
    // waits for the next natural trigger rather than spinning on a
    // leaked lockfile.
    let mut retrying_contention = false;

    let trigger = |job_running: &mut bool, pending: &mut bool| {
        if *job_running {
            *pending = true;
        } else if run_tx.send(()).is_ok() {
            *job_running = true;
        }
    };
    // Notify once, poll from here on (ADR 0005) — entered at spawn when
    // the watcher never came up, or later when it dies.
    let enter_degraded = |degraded: &mut bool, poll_deadline: &mut Option<Instant>| {
        if *degraded {
            return;
        }
        *degraded = true;
        *poll_deadline = Some(Instant::now() + config.poll_interval);
        let _ = events.send(EngineEvent::ConditionStarted(Condition::WatcherDegraded));
    };

    if initially_degraded {
        enter_degraded(&mut degraded, &mut poll_deadline);
    }
    // Initial refresh: the frontend gets its first snapshot unasked.
    trigger(&mut job_running, &mut pending);

    loop {
        let next_deadline = [debounce_deadline, poll_deadline]
            .into_iter()
            .flatten()
            .min();
        let timer = next_deadline.map_or_else(never, at);

        select! {
            recv(requests) -> msg => match msg {
                // Immediate: no debounce, and any armed debounce is
                // subsumed by the refresh that's about to run.
                Ok(()) => {
                    debounce_deadline = None;
                    trigger(&mut job_running, &mut pending);
                }
                // Engine dropped: shut down. run_tx drops with this
                // frame, ending the worker.
                Err(_) => return,
            },
            recv(source) -> msg => match msg {
                Ok(SourceEvent::Paths(paths)) => {
                    if paths.iter().any(|path| !filter.is_self_loop(path)) {
                        debounce_deadline = Some(Instant::now() + config.debounce);
                    }
                }
                Ok(SourceEvent::Lost) => {
                    debounce_deadline = Some(Instant::now() + config.debounce);
                }
                // Watcher died. Swap in a never-channel so a dead
                // source doesn't busy-loop the select.
                Err(_) => {
                    source = never();
                    enter_degraded(&mut degraded, &mut poll_deadline);
                }
            },
            recv(done_rx) -> msg => match msg {
                Ok(outcome) => {
                    job_running = false;
                    match outcome {
                        RunOutcome::Done => retrying_contention = false,
                        RunOutcome::Contended if !retrying_contention => {
                            retrying_contention = true;
                            debounce_deadline =
                                Some(Instant::now() + config.debounce);
                        }
                        RunOutcome::Contended => {}
                    }
                    if pending {
                        pending = false;
                        trigger(&mut job_running, &mut pending);
                    }
                }
                // Worker died (refresh panicked); nothing left to drive.
                Err(_) => return,
            },
            recv(timer) -> _ => {
                let now = Instant::now();
                if debounce_deadline.is_some_and(|deadline| deadline <= now) {
                    debounce_deadline = None;
                    trigger(&mut job_running, &mut pending);
                }
                if poll_deadline.is_some_and(|deadline| deadline <= now) {
                    poll_deadline = Some(now + config.poll_interval);
                    trigger(&mut job_running, &mut pending);
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// Generous ceiling for waiting on an event that must arrive; never
    /// asserts real debounce timing (ADR 0008), only that things happen.
    const WAIT: Duration = Duration::from_secs(10);
    /// Synthetic timing: fast enough for tests, slow enough that a
    /// burst sent in microseconds lands inside one debounce window.
    const TEST_CONFIG: Config = Config {
        debounce: Duration::from_millis(25),
        poll_interval: Duration::from_millis(25),
    };

    fn empty_snapshot() -> Snapshot {
        Snapshot {
            files: Vec::new(),
            changelists: Vec::new(),
            active: None,
            notices: Vec::new(),
        }
    }

    struct TestEngine {
        engine: Engine,
        source: Option<Sender<SourceEvent>>,
        refreshes: Arc<AtomicUsize>,
    }

    impl TestEngine {
        fn spawn() -> Self {
            Self::spawn_with(|_| Ok(empty_snapshot()))
        }

        /// `refresh` receives the 1-based run count and returns what the
        /// RefreshJob should produce.
        fn spawn_with(
            refresh: impl FnMut(usize) -> Result<Snapshot, Error> + Send + 'static,
        ) -> Self {
            let (source_tx, source_rx) = unbounded();
            let refreshes = Arc::new(AtomicUsize::new(0));
            let counter = refreshes.clone();
            let mut refresh = refresh;
            let engine = spawn_loops(
                source_rx,
                SelfLoopFilter::new(PathBuf::from("/repo/.git/gitchange")),
                Box::new(move || {
                    let run = counter.fetch_add(1, Ordering::SeqCst) + 1;
                    refresh(run)
                }),
                TEST_CONFIG,
                false,
                None,
            );
            Self {
                engine,
                source: Some(source_tx),
                refreshes,
            }
        }

        fn send(&self, event: SourceEvent) {
            self.source.as_ref().unwrap().send(event).unwrap();
        }

        fn recv(&self) -> EngineEvent {
            self.engine
                .events()
                .recv_timeout(WAIT)
                .expect("engine event within the generous timeout")
        }

        fn recv_complete(&self) -> Snapshot {
            match self.recv() {
                EngineEvent::RefreshComplete(snapshot) => snapshot,
                other => panic!("expected RefreshComplete, got {other:?}"),
            }
        }
    }

    fn paths(list: &[&str]) -> SourceEvent {
        SourceEvent::Paths(list.iter().map(PathBuf::from).collect())
    }

    #[test]
    fn initial_refresh_arrives_unasked() {
        let engine = TestEngine::spawn();
        engine.recv_complete();
        assert_eq!(engine.refreshes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn debounce_coalesces_a_burst_into_one_refresh() {
        let engine = TestEngine::spawn();
        engine.recv_complete(); // initial

        // A save burst: many events inside one debounce window.
        for _ in 0..10 {
            engine.send(paths(&["/repo/src/main.rs"]));
        }
        engine.recv_complete(); // the burst's single refresh

        // A marker refresh sequences the assertion: if the burst had
        // produced more than one run, the counter would exceed 3.
        engine.engine.request_refresh();
        engine.recv_complete();
        assert_eq!(engine.refreshes.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn self_loop_events_are_dropped() {
        let engine = TestEngine::spawn();
        engine.recv_complete(); // initial

        // Our own state writes: state file, lock, tmp (ADR 0005).
        engine.send(paths(&["/repo/.git/gitchange/state.json"]));
        engine.send(paths(&["/repo/.git/gitchange/state.json.lock"]));
        engine.send(paths(&["/repo/.git/gitchange/state.json.tmp"]));

        // Quiet window well past the synthetic debounce: no refresh may
        // have started from the filtered events.
        assert!(
            engine
                .engine
                .events()
                .recv_timeout(TEST_CONFIG.debounce * 8)
                .is_err()
        );
        assert_eq!(engine.refreshes.load(Ordering::SeqCst), 1);

        // A mixed batch (one real path among our own) must still
        // trigger.
        engine.send(SourceEvent::Paths(vec![
            PathBuf::from("/repo/.git/gitchange/state.json"),
            PathBuf::from("/repo/src/lib.rs"),
        ]));
        engine.recv_complete();
        assert_eq!(engine.refreshes.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn last_request_wins_collapses_requests_behind_a_running_job() {
        // Refresh blocks until released, so requests pile up behind a
        // deliberately in-flight job.
        let (gate_tx, gate_rx) = unbounded::<()>();
        let engine = TestEngine::spawn_with(move |_| {
            gate_rx.recv().unwrap();
            Ok(empty_snapshot())
        });

        // The initial refresh is now blocked in flight. Everything sent
        // while it runs must collapse into a single follow-up.
        for _ in 0..5 {
            engine.engine.request_refresh();
        }
        engine.send(paths(&["/repo/src/main.rs"]));

        gate_tx.send(()).unwrap(); // release the initial job
        engine.recv_complete();
        gate_tx.send(()).unwrap(); // release the one collapsed follow-up
        engine.recv_complete();

        // No third job may exist: the gate has no further waiter.
        assert_eq!(engine.refreshes.load(Ordering::SeqCst), 2);
        assert!(
            engine
                .engine
                .events()
                .recv_timeout(TEST_CONFIG.debounce * 8)
                .is_err()
        );
    }

    #[test]
    fn watcher_death_degrades_and_polling_keeps_refreshing() {
        let mut engine = TestEngine::spawn();
        engine.recv_complete(); // initial

        // Kill the watcher: dropping the source sender is exactly what
        // a dying notify watcher does to its forwarder.
        drop(engine.source.take());

        match engine.recv() {
            EngineEvent::ConditionStarted(Condition::WatcherDegraded) => {}
            other => panic!("expected WatcherDegraded, got {other:?}"),
        }
        // Polling refreshes keep arriving with no events or requests.
        engine.recv_complete();
        engine.recv_complete();
    }

    #[test]
    fn lock_contention_is_retried_not_surfaced() {
        let engine = TestEngine::spawn_with(|run| {
            if run == 1 {
                Err(Error::LockContention {
                    path: PathBuf::from("/repo/.git/gitchange/state.json.lock"),
                })
            } else {
                Ok(empty_snapshot())
            }
        });

        // The only event is the retry's success — contention itself
        // never reaches the frontend.
        engine.recv_complete();
        assert_eq!(engine.refreshes.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn self_loop_filter_matches_the_state_dir_only() {
        let filter = SelfLoopFilter::new(PathBuf::from("/repo/.git/gitchange"));
        assert!(filter.is_self_loop(Path::new("/repo/.git/gitchange/state.json")));
        assert!(filter.is_self_loop(Path::new("/repo/.git/gitchange/state.json.tmp")));
        assert!(!filter.is_self_loop(Path::new("/repo/.git/index")));
        assert!(!filter.is_self_loop(Path::new("/repo/.git/HEAD")));
        assert!(!filter.is_self_loop(Path::new("/repo/src/gitchange/file.rs")));
        // Prefix match is per-component, not textual.
        assert!(!filter.is_self_loop(Path::new("/repo/.git/gitchange-other/x")));
    }
}
