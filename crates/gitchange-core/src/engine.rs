//! The threaded Engine (ADR 0005/0006): a `notify` watcher over the
//! worktree root including `.git`, ~500ms debounce, self-loop filter,
//! last-request-wins RefreshJob slot, immediate refresh requests
//! (mutations, terminal focus, manual key), a crossbeam event channel,
//! and a ~5s polling fallback when the watcher fails or dies — which
//! re-subscribes on every tick, so degraded mode is a state the Engine
//! climbs out of rather than a one-way door.
//!
//! The watcher sits behind an injectable subscription (a factory
//! yielding a plain crossbeam channel of [`SourceEvent`]s), so the
//! decision logic below is unit-tested deterministically with synthetic
//! events (ADR 0008); one real-fs smoke test per CI OS proves the notify
//! wiring.

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
    /// is refreshing on a polling tick instead (ADR 0005). Ends when a
    /// re-subscription on one of those ticks succeeds.
    WatcherDegraded,
}

/// What the Engine emits over its crossbeam channel.
#[derive(Debug)]
#[non_exhaustive]
pub enum EngineEvent {
    /// A RefreshJob began. The frontend's hook for the deferred refresh
    /// indicator (ADR 0005): show nothing under ~500ms, an indicator
    /// past it, cleared by the matching `RefreshComplete`/`RefreshFailed`.
    RefreshStarted,
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
    /// with `ConditionStarted(WatcherDegraded)`; the control loop keeps
    /// re-subscribing while degraded and reports recovery with
    /// `ConditionEnded(WatcherDegraded)`.
    pub fn spawn(path: &Path) -> Result<Self, Error> {
        let repo = Repo::discover(path)?;
        let workdir = repo
            .workdir()
            .ok_or_else(|| Error::Backend("bare repository has no worktree to watch".into()))?;
        let state_dir = repo.state_dir();
        let filter = SelfLoopFilter::new(state_dir.clone());

        // Owned by the factory, not the refresh closure: `repo` moves
        // into the latter, and re-subscription needs these paths for the
        // whole life of the Engine.
        let subscribe = Box::new(move || {
            let (source_tx, source_rx) = unbounded();
            let watcher = start_watcher(&workdir, &state_dir, source_tx)?;
            Some((source_rx, Box::new(watcher) as Box<dyn Send>))
        });

        Ok(spawn_loops(
            subscribe,
            filter,
            Box::new(move || repo.refresh()),
            Config::default(),
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

/// Subscribes to filesystem events: builds a watcher and the source it
/// feeds, `None` when the subscription fails. Called once at startup and
/// again on each poll tick while degraded, so the healthy path can be
/// re-entered without restarting the Engine.
///
/// The `Box<dyn Send>` is the watcher itself. The control loop holds it
/// for exactly as long as the source is live — dropping it unsubscribes,
/// so replacing it is how a dead watcher is reaped.
type Subscribe = Box<dyn FnMut() -> Option<(Receiver<SourceEvent>, Box<dyn Send>)> + Send>;

/// Wire up the control and worker threads. Separated from
/// [`Engine::spawn`] so unit tests inject a synthetic subscription and
/// refresh function (ADR 0008).
fn spawn_loops(
    subscribe: Subscribe,
    filter: SelfLoopFilter,
    refresh: Box<dyn FnMut() -> Result<Snapshot, Error> + Send>,
    config: Config,
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
            subscribe,
            run_tx,
            done_rx,
            events_tx,
            filter,
            config,
        );
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
        let _ = events.send(EngineEvent::RefreshStarted);
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
fn control_loop(
    requests: Receiver<()>,
    subscribe: Subscribe,
    run_tx: Sender<()>,
    done_rx: Receiver<RunOutcome>,
    events: Sender<EngineEvent>,
    filter: SelfLoopFilter,
    config: Config,
) {
    let mut subscribe = subscribe;
    // The live subscription. `never()` while degraded, so a dead source
    // doesn't busy-loop the select; the watcher box lives exactly as
    // long as the source it feeds.
    let mut source: Receiver<SourceEvent> = never();
    let mut watcher: Option<Box<dyn Send>> = None;
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

    // Any triggered run subsumes an armed debounce: the job it dispatches
    // (or the pending follow-up it queues) starts after the debounced
    // filesystem change, so letting the deadline fire anyway would only
    // buy a redundant third refresh.
    let trigger =
        |job_running: &mut bool, pending: &mut bool, debounce_deadline: &mut Option<Instant>| {
            *debounce_deadline = None;
            if *job_running {
                *pending = true;
            } else if run_tx.send(()).is_ok() {
                *job_running = true;
            }
        };
    // Take a fresh subscription, replacing any current one — dropping
    // the old watcher box is what unsubscribes it. On Linux a recursive
    // watch adds one inotify watch per directory, so this can stall the
    // loop briefly on a large tree; it is the same cost already paid at
    // startup, and off the healthy path otherwise.
    let mut resubscribe = |source: &mut Receiver<SourceEvent>,
                           watcher: &mut Option<Box<dyn Send>>| {
        let Some((fresh, keep_alive)) = subscribe() else {
            return false;
        };
        *source = fresh;
        *watcher = Some(keep_alive);
        true
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
    // Back on the watcher: stand the poll down and let the frontend's
    // pin self-clear (ADR 0007).
    let leave_degraded = |degraded: &mut bool, poll_deadline: &mut Option<Instant>| {
        if !*degraded {
            return;
        }
        *degraded = false;
        *poll_deadline = None;
        let _ = events.send(EngineEvent::ConditionEnded(Condition::WatcherDegraded));
    };

    if !resubscribe(&mut source, &mut watcher) {
        enter_degraded(&mut degraded, &mut poll_deadline);
    }
    // Initial refresh: the frontend gets its first snapshot unasked.
    trigger(&mut job_running, &mut pending, &mut debounce_deadline);

    loop {
        let next_deadline = [debounce_deadline, poll_deadline]
            .into_iter()
            .flatten()
            .min();
        let timer = next_deadline.map_or_else(never, at);

        select! {
            recv(requests) -> msg => match msg {
                // Immediate: no debounce.
                Ok(()) => {
                    trigger(&mut job_running, &mut pending, &mut debounce_deadline);
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
                // source doesn't busy-loop the select, and drop the
                // defunct watcher — the poll tick re-subscribes.
                Err(_) => {
                    source = never();
                    watcher = None;
                    enter_degraded(&mut degraded, &mut poll_deadline);
                }
            },
            recv(done_rx) -> msg => match msg {
                Ok(outcome) => {
                    // Fold every request already queued into the single
                    // follow-up decision: `select!` picks ready channels
                    // in random order, so without this drain a request
                    // that preceded the completion could be seen after
                    // it and spawn a redundant extra run.
                    while requests.try_recv().is_ok() {
                        pending = true;
                        debounce_deadline = None;
                    }
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
                        trigger(&mut job_running, &mut pending, &mut debounce_deadline);
                    }
                }
                // Worker died (refresh panicked); nothing left to drive.
                Err(_) => return,
            },
            recv(timer) -> _ => {
                let now = Instant::now();
                if debounce_deadline.is_some_and(|deadline| deadline <= now) {
                    trigger(&mut job_running, &mut pending, &mut debounce_deadline);
                }
                if poll_deadline.is_some_and(|deadline| deadline <= now) {
                    // Re-subscribe before the tick's refresh, never
                    // after: that refresh then covers the window the
                    // dead watcher missed, and anything landing later
                    // arrives on the live source.
                    if resubscribe(&mut source, &mut watcher) {
                        leave_degraded(&mut degraded, &mut poll_deadline);
                    } else {
                        poll_deadline = Some(now + config.poll_interval);
                    }
                    trigger(&mut job_running, &mut pending, &mut debounce_deadline);
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

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
            advisories: Vec::new(),
            head: crate::snapshot::Head::Unborn {
                name: "main".into(),
            },
            recent_commits: Vec::new(),
            operation: None,
        }
    }

    /// The synthetic watcher standing in for notify: subscriptions the
    /// control loop has taken (newest last, so a test can feed the live
    /// source), a count of attempts, and a switch for whether the
    /// environment currently supports a watcher at all.
    #[derive(Clone)]
    struct Subscriptions {
        senders: Arc<Mutex<Vec<Sender<SourceEvent>>>>,
        attempts: Arc<AtomicUsize>,
        broken: Arc<AtomicBool>,
    }

    impl Subscriptions {
        fn healthy() -> Self {
            Self::new(false)
        }

        /// A watcher that cannot come up — every subscription fails
        /// until [`Subscriptions::repair`].
        fn broken() -> Self {
            Self::new(true)
        }

        fn new(broken: bool) -> Self {
            Self {
                senders: Arc::new(Mutex::new(Vec::new())),
                attempts: Arc::new(AtomicUsize::new(0)),
                broken: Arc::new(AtomicBool::new(broken)),
            }
        }

        fn subscribe(&self) -> Option<(Receiver<SourceEvent>, Box<dyn Send>)> {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            if self.broken.load(Ordering::SeqCst) {
                return None;
            }
            let (tx, rx) = unbounded();
            self.senders.lock().unwrap().push(tx);
            // The real keep-alive is the notify watcher; here its only
            // job is to be dropped when the loop replaces it.
            Some((rx, Box::new(())))
        }

        /// The most recent subscription's sender — where a live watcher
        /// would be delivering.
        fn live(&self) -> Sender<SourceEvent> {
            self.senders
                .lock()
                .unwrap()
                .last()
                .expect("a successful subscription")
                .clone()
        }

        /// Kill the watcher the way notify does — drop its forwarder,
        /// disconnecting the source — and keep it down, so the loop's
        /// re-subscription attempts fail until repaired.
        fn kill(&self) {
            self.broken.store(true, Ordering::SeqCst);
            self.senders.lock().unwrap().clear();
        }

        /// The environment recovers: the next attempt succeeds.
        fn repair(&self) {
            self.broken.store(false, Ordering::SeqCst);
        }

        fn attempts(&self) -> usize {
            self.attempts.load(Ordering::SeqCst)
        }
    }

    struct TestEngine {
        engine: Engine,
        watcher: Subscriptions,
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
            Self::spawn_with_watcher(Subscriptions::healthy(), refresh)
        }

        fn spawn_with_watcher(
            watcher: Subscriptions,
            refresh: impl FnMut(usize) -> Result<Snapshot, Error> + Send + 'static,
        ) -> Self {
            let refreshes = Arc::new(AtomicUsize::new(0));
            let counter = refreshes.clone();
            let mut refresh = refresh;
            let subscriptions = watcher.clone();
            let engine = spawn_loops(
                Box::new(move || subscriptions.subscribe()),
                SelfLoopFilter::new(PathBuf::from("/repo/.git/gitchange")),
                Box::new(move || {
                    let run = counter.fetch_add(1, Ordering::SeqCst) + 1;
                    refresh(run)
                }),
                TEST_CONFIG,
            );
            Self {
                engine,
                watcher,
                refreshes,
            }
        }

        fn send(&self, event: SourceEvent) {
            self.watcher.live().send(event).unwrap();
        }

        fn recv(&self) -> EngineEvent {
            self.engine
                .events()
                .recv_timeout(WAIT)
                .expect("engine event within the generous timeout")
        }

        /// The next `RefreshComplete`, skipping the `RefreshStarted`
        /// that precedes every run — tests here assert on outcomes, not
        /// the indicator hook.
        fn recv_complete(&self) -> Snapshot {
            loop {
                match self.recv() {
                    EngineEvent::RefreshComplete(snapshot) => return snapshot,
                    EngineEvent::RefreshStarted => {}
                    other => panic!("expected RefreshComplete, got {other:?}"),
                }
            }
        }

        /// The next condition transition, skipping the refresh traffic a
        /// degraded engine generates around it.
        fn recv_condition(&self) -> EngineEvent {
            loop {
                match self.recv() {
                    event @ (EngineEvent::ConditionStarted(_) | EngineEvent::ConditionEnded(_)) => {
                        return event;
                    }
                    EngineEvent::RefreshStarted | EngineEvent::RefreshComplete(_) => {}
                    other => panic!("expected a condition event, got {other:?}"),
                }
            }
        }

        /// Assert nothing more arrives for a window well past the
        /// synthetic poll interval — quiescence, never a timing claim
        /// about the real constants (ADR 0008).
        fn assert_quiet(&self) {
            assert!(
                self.engine
                    .events()
                    .recv_timeout(TEST_CONFIG.poll_interval * 8)
                    .is_err(),
                "expected no further engine events"
            );
        }
    }

    fn paths(list: &[&str]) -> SourceEvent {
        SourceEvent::Paths(list.iter().map(PathBuf::from).collect())
    }

    #[test]
    fn refresh_started_precedes_every_complete() {
        let engine = TestEngine::spawn();
        for _ in 0..2 {
            match engine.recv() {
                EngineEvent::RefreshStarted => {}
                other => panic!("expected RefreshStarted, got {other:?}"),
            }
            match engine.recv() {
                EngineEvent::RefreshComplete(_) => {}
                other => panic!("expected RefreshComplete, got {other:?}"),
            }
            engine.engine.request_refresh();
        }
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

        // The initial refresh is now blocked in flight. Every request
        // sent while it runs must collapse into a single follow-up.
        // Requests only — an fs event here would race its debounce
        // window against the gate releases (issue #48); coalescing of
        // fs events has its own test.
        for _ in 0..5 {
            engine.engine.request_refresh();
        }

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
        let engine = TestEngine::spawn();
        engine.recv_complete(); // initial

        // Kill the watcher: dropping the source sender is exactly what
        // a dying notify watcher does to its forwarder. It stays down,
        // so the loop's re-subscription attempts keep failing.
        engine.watcher.kill();

        match engine.recv_condition() {
            EngineEvent::ConditionStarted(Condition::WatcherDegraded) => {}
            other => panic!("expected WatcherDegraded, got {other:?}"),
        }
        // Polling refreshes keep arriving with no events or requests.
        engine.recv_complete();
        engine.recv_complete();
        // And each tick re-tries the watcher rather than settling.
        assert!(engine.watcher.attempts() >= 2);
    }

    #[test]
    fn a_watcher_that_never_comes_up_degrades_at_spawn() {
        let engine =
            TestEngine::spawn_with_watcher(Subscriptions::broken(), |_| Ok(empty_snapshot()));

        match engine.recv_condition() {
            EngineEvent::ConditionStarted(Condition::WatcherDegraded) => {}
            other => panic!("expected WatcherDegraded, got {other:?}"),
        }
        engine.recv_complete();
    }

    #[test]
    fn a_recovered_watcher_ends_the_condition_and_feeds_events_again() {
        let engine = TestEngine::spawn();
        engine.recv_complete(); // initial

        engine.watcher.kill();
        match engine.recv_condition() {
            EngineEvent::ConditionStarted(Condition::WatcherDegraded) => {}
            other => panic!("expected WatcherDegraded, got {other:?}"),
        }

        // The environment recovers; the next poll tick re-subscribes.
        engine.watcher.repair();
        match engine.recv_condition() {
            EngineEvent::ConditionEnded(Condition::WatcherDegraded) => {}
            other => panic!("expected the condition to end, got {other:?}"),
        }

        // Polling stood down with the condition — the engine is idle
        // again rather than ticking.
        engine.recv_complete(); // the recovering tick's own refresh
        engine.assert_quiet();

        // The new subscription is real, not just announced: an event on
        // it still drives a refresh.
        engine.send(paths(&["/repo/src/main.rs"]));
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
