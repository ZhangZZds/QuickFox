//! Tauri-neutral runtime incremental indexing service.

use crate::core::index_entry::{IncrementalState, IndexDegradationCode, RuntimeIncrementalStatus};
use crate::core::index_journal::IndexJournalRepository;
use crate::core::index_update_coordinator::{
    CoordinatorPolicy, CoordinatorShutdown, CoordinatorState,
};
use crate::core::index_watcher::{RuntimeIndexWatcher, WatchEventInbox, WatcherFailure};
use crate::core::layered_index::CommittedIndexDelta;
use crate::core::targeted_index_scanner::{TargetedIndexScanner, TargetedScanError};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::mpsc::{self, SyncSender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::Instant;

pub const MAX_DELTA_ENTRIES: usize = 50_000;
pub const MAX_DELTA_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct RuntimeIndexingOptions {
    pub roots: Vec<PathBuf>,
    pub policy: CoordinatorPolicy,
    pub initial_generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BaselineRefreshReason {
    DeltaSafetyLimit,
    DirtyRoots,
    WatcherFailure,
}

#[derive(Debug, Clone)]
pub enum RuntimeIndexingEvent {
    DeltaCommitted(CommittedIndexDelta),
    Status(RuntimeIncrementalStatus),
    BaselineRefreshRequired { reason: BaselineRefreshReason },
}

pub struct RuntimeIndexingHandle {
    stop: SyncSender<()>,
    shutdown: CoordinatorShutdown,
    join: Option<JoinHandle<()>>,
}

impl RuntimeIndexingHandle {
    pub fn stop(mut self) {
        self.stop_and_join();
    }

    fn stop_and_join(&mut self) {
        self.shutdown.request();
        let _ = self.stop.try_send(());
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for RuntimeIndexingHandle {
    fn drop(&mut self) {
        self.stop_and_join();
    }
}

pub fn start_runtime_indexing(
    mut watcher: RuntimeIndexWatcher,
    scanner: TargetedIndexScanner,
    journal: Box<dyn IndexJournalRepository + Send>,
    options: RuntimeIndexingOptions,
    publish: impl Fn(RuntimeIndexingEvent) + Send + 'static,
) -> Result<RuntimeIndexingHandle, WatcherFailure> {
    let inbox = watcher.take_inbox().ok_or_else(|| {
        WatcherFailure::new(PathBuf::new(), "runtime watcher inbox was already consumed")
    })?;
    start_runtime_indexing_from_parts(Some(watcher), inbox, scanner, journal, options, publish)
        .map_err(|error| WatcherFailure::new(PathBuf::new(), error))
}

fn start_runtime_indexing_from_parts(
    watcher: Option<RuntimeIndexWatcher>,
    inbox: WatchEventInbox,
    scanner: TargetedIndexScanner,
    journal: Box<dyn IndexJournalRepository + Send>,
    options: RuntimeIndexingOptions,
    publish: impl Fn(RuntimeIndexingEvent) + Send + 'static,
) -> Result<RuntimeIndexingHandle, String> {
    let shutdown = CoordinatorShutdown::default();
    let worker_shutdown = shutdown.clone();
    let (stop, stop_receiver) = mpsc::sync_channel(1);
    let join = thread::Builder::new()
        .name("quickfox-runtime-indexing".to_owned())
        .spawn(move || {
            RuntimeIndexingService {
                _watcher: watcher,
                inbox,
                scanner,
                journal,
                coordinator: CoordinatorState::new(options.policy),
                next_generation: options.initial_generation.saturating_add(1),
                status: RuntimeIncrementalStatus {
                    enabled: true,
                    state: IncrementalState::Watching,
                    ..RuntimeIncrementalStatus::default()
                },
                publish: Box::new(publish),
                shutdown: worker_shutdown,
                stop_receiver,
                _roots: options.roots,
            }
            .run();
        })
        .map_err(|error| error.to_string())?;

    Ok(RuntimeIndexingHandle {
        stop,
        shutdown,
        join: Some(join),
    })
}

struct RuntimeIndexingService {
    _watcher: Option<RuntimeIndexWatcher>,
    inbox: WatchEventInbox,
    scanner: TargetedIndexScanner,
    journal: Box<dyn IndexJournalRepository + Send>,
    coordinator: CoordinatorState,
    next_generation: u64,
    status: RuntimeIncrementalStatus,
    publish: Box<dyn Fn(RuntimeIndexingEvent) + Send>,
    shutdown: CoordinatorShutdown,
    stop_receiver: mpsc::Receiver<()>,
    _roots: Vec<PathBuf>,
}

impl RuntimeIndexingService {
    fn run(&mut self) {
        self.publish_status();
        while !self.should_stop() {
            let now = Instant::now();
            let wait = self.coordinator.next_wait(now);
            if let Ok(event) = self.inbox.recv_timeout(wait) {
                self.coordinator.push_event(event, Instant::now());
            }
            self.drain_inbox();
            self.consume_watcher_degradation();
            if self.coordinator.should_drain(Instant::now()) {
                self.commit_ready_batch();
            } else {
                self.update_pending_status();
            }
        }
    }

    fn should_stop(&self) -> bool {
        self.shutdown.is_requested()
            || matches!(
                self.stop_receiver.try_recv(),
                Ok(()) | Err(TryRecvError::Disconnected)
            )
    }

    fn drain_inbox(&mut self) {
        while let Ok(event) = self.inbox.try_recv() {
            self.coordinator.push_event(event, Instant::now());
        }
    }

    fn consume_watcher_degradation(&mut self) {
        let dirty_roots = self.inbox.take_dirty_roots();
        let failure = self.inbox.take_failure();
        let degradation = self.inbox.take_degradation_code();
        if dirty_roots.is_empty() && failure.is_none() && degradation.is_none() {
            return;
        }

        let now = Instant::now();
        for root in dirty_roots {
            self.coordinator.mark_dirty_root(root, now);
        }
        self.status.state = IncrementalState::Degraded;
        self.status.degradation_code = failure
            .as_ref()
            .map(|_| IndexDegradationCode::WatcherRuntimeFailed)
            .or(degradation)
            .or(Some(IndexDegradationCode::WatcherOverflow));
        self.update_pending_status();
        (self.publish)(RuntimeIndexingEvent::BaselineRefreshRequired {
            reason: if failure.is_some() {
                BaselineRefreshReason::WatcherFailure
            } else {
                BaselineRefreshReason::DirtyRoots
            },
        });
    }

    fn commit_ready_batch(&mut self) {
        let batch = self.coordinator.drain();
        let dirty_root_count = batch.dirty_roots.len();
        let entry_count = batch
            .changed_paths
            .len()
            .saturating_add(batch.removed_paths.len());
        if entry_count == 0 {
            self.status.pending_events = 0;
            self.status.dirty_roots = dirty_root_count;
            self.publish_status();
            return;
        }

        let started = Instant::now();
        let scanned = match self
            .scanner
            .scan_batch_cancellable(batch, || self.shutdown.is_requested())
        {
            Ok(scanned) => scanned,
            Err(TargetedScanError::Cancelled) => return,
            Err(TargetedScanError::Io(_)) => {
                self.publish_degraded(IndexDegradationCode::CalibrationFailed);
                return;
            }
        };
        let delta = CommittedIndexDelta {
            generation: self.next_generation,
            upserts: scanned.upserts,
            removals: scanned.removals,
        };
        if self
            .journal
            .commit_incremental_batch(
                &delta,
                &scanned.manifest_upserts,
                &scanned.manifest_removals,
            )
            .is_err()
        {
            self.publish_degraded(IndexDegradationCode::JournalWriteFailed);
            return;
        }

        self.status.state = if dirty_root_count == 0 {
            IncrementalState::Watching
        } else {
            IncrementalState::Degraded
        };
        self.status.pending_events = 0;
        self.status.dirty_roots = dirty_root_count;
        self.status.last_batch_entries = delta.upserts.len().saturating_add(delta.removals.len());
        self.status.last_batch_duration_ms = started.elapsed().as_millis() as u64;
        if dirty_root_count == 0 {
            self.status.degradation_code = None;
        }
        (self.publish)(RuntimeIndexingEvent::DeltaCommitted(delta));
        self.next_generation = self.next_generation.saturating_add(1);
        self.publish_status();
    }

    fn update_pending_status(&mut self) {
        let pending_events = self.coordinator.pending_event_count();
        let dirty_roots = self.coordinator.dirty_root_count();
        if self.status.pending_events == pending_events && self.status.dirty_roots == dirty_roots {
            return;
        }
        self.status.pending_events = pending_events;
        self.status.dirty_roots = dirty_roots;
        self.publish_status();
    }

    fn publish_degraded(&mut self, code: IndexDegradationCode) {
        self.status.state = IncrementalState::Degraded;
        self.status.degradation_code = Some(code);
        self.status.pending_events = self.coordinator.pending_event_count();
        self.status.dirty_roots = self.coordinator.dirty_root_count();
        self.publish_status();
    }

    fn publish_status(&self) {
        (self.publish)(RuntimeIndexingEvent::Status(self.status.clone()));
    }
}

pub fn delta_safety_limit_reached(entry_count: usize, estimated_bytes: usize) -> bool {
    entry_count >= MAX_DELTA_ENTRIES || estimated_bytes >= MAX_DELTA_BYTES
}

pub fn baseline_refresh_event_for_delta_state(
    entry_count: usize,
    estimated_bytes: usize,
) -> Option<RuntimeIndexingEvent> {
    delta_safety_limit_reached(entry_count, estimated_bytes).then_some(
        RuntimeIndexingEvent::BaselineRefreshRequired {
            reason: BaselineRefreshReason::DeltaSafetyLimit,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::index::FileSearchIndex;
    use crate::core::index_journal::recover_layered_index;
    use crate::core::index_scanner::{IndexPathRules, IndexScanPlan};
    use crate::core::index_watcher::{IndexWatchEvent, WatchEventInbox, WatchEventSender};
    use crate::core::layered_index::LayeredSearchIndex;
    use crate::core::search::{QueryRequest, SearchMode, SearchResult};
    use crate::core::storage::SqliteStorage;
    use rusqlite::Connection;
    use std::fs;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tempfile::TempDir;

    struct RuntimeIndexingHarness {
        root: TempDir,
        database_path: PathBuf,
        sender: WatchEventSender,
        handle: Mutex<Option<RuntimeIndexingHandle>>,
        events: Arc<Mutex<Vec<RuntimeIndexingEvent>>>,
        index: Arc<Mutex<LayeredSearchIndex>>,
    }

    impl RuntimeIndexingHarness {
        fn new() -> Self {
            Self::new_with_apply(true)
        }

        fn new_with_apply(apply_delta: bool) -> Self {
            let root = tempfile::tempdir().unwrap();
            let database_path = root.path().join("runtime.sqlite");
            let storage = SqliteStorage::open(database_path.clone()).unwrap();
            let baseline_id = storage.save_completed_index_batch(1, &[]).unwrap();
            storage
                .activate_baseline_and_clear_incremental_state(baseline_id, 0)
                .unwrap();
            let roots = vec![root.path().to_path_buf()];
            let rules = IndexPathRules::from_plan(&IndexScanPlan {
                include_roots: roots.clone(),
                ..IndexScanPlan::default()
            })
            .unwrap();
            let scanner = TargetedIndexScanner::new(rules);
            let (sender, inbox) = WatchEventInbox::bounded(roots.clone(), 16);
            let events = Arc::new(Mutex::new(Vec::new()));
            let index = Arc::new(Mutex::new(LayeredSearchIndex::default()));
            let publish_events = Arc::clone(&events);
            let publish_index = Arc::clone(&index);
            let handle = start_runtime_indexing_from_parts(
                None,
                inbox,
                scanner,
                Box::new(storage),
                RuntimeIndexingOptions {
                    roots,
                    policy: CoordinatorPolicy::new(
                        Duration::from_millis(10),
                        Duration::from_millis(25),
                    ),
                    initial_generation: 0,
                },
                move |event| {
                    if apply_delta {
                        if let RuntimeIndexingEvent::DeltaCommitted(delta) = &event {
                            publish_index.lock().unwrap().apply_delta(delta.clone());
                        }
                    }
                    publish_events.lock().unwrap().push(event);
                },
            )
            .unwrap();
            Self {
                root,
                database_path,
                sender,
                handle: Mutex::new(Some(handle)),
                events,
                index,
            }
        }

        fn create_file(&self, name: &str) -> PathBuf {
            let path = self.root.path().join(name);
            fs::write(&path, "runtime indexing").unwrap();
            path
        }

        fn push(&self, event: IndexWatchEvent) {
            assert_eq!(
                self.sender.try_send(event),
                crate::core::index_watcher::WatchSendOutcome::Queued
            );
        }

        fn advance(&self, duration: Duration) {
            thread::sleep(duration);
        }

        fn take_published_delta(&self) -> Option<CommittedIndexDelta> {
            self.events
                .lock()
                .unwrap()
                .iter()
                .find_map(|event| match event {
                    RuntimeIndexingEvent::DeltaCommitted(delta) => Some(delta.clone()),
                    _ => None,
                })
        }

        fn committed_deltas(&self) -> Vec<CommittedIndexDelta> {
            self.events
                .lock()
                .unwrap()
                .iter()
                .filter_map(|event| match event {
                    RuntimeIndexingEvent::DeltaCommitted(delta) => Some(delta.clone()),
                    _ => None,
                })
                .collect()
        }

        fn statuses(&self) -> Vec<RuntimeIncrementalStatus> {
            self.events
                .lock()
                .unwrap()
                .iter()
                .filter_map(|event| match event {
                    RuntimeIndexingEvent::Status(status) => Some(status.clone()),
                    _ => None,
                })
                .collect()
        }

        fn stop(&self) {
            if let Some(handle) = self.handle.lock().unwrap().take() {
                handle.stop();
            }
        }

        fn journal_generations(&self) -> Vec<u64> {
            SqliteStorage::open(self.database_path.clone())
                .unwrap()
                .committed_index_deltas_after(0)
                .unwrap()
                .into_iter()
                .map(|delta| delta.generation)
                .collect()
        }

        fn search(&self, text: &str) -> Vec<SearchResult> {
            self.index
                .lock()
                .unwrap()
                .search_files(&QueryRequest::new(text, SearchMode::Normal), 20)
        }
    }

    impl Drop for RuntimeIndexingHarness {
        fn drop(&mut self) {
            if let Some(handle) = self.handle.lock().unwrap().take() {
                handle.stop();
            }
        }
    }

    #[test]
    fn watcher_event_is_committed_and_published_to_runtime_view() {
        let harness = RuntimeIndexingHarness::new();
        let path = harness.create_file("new.md");

        harness.push(IndexWatchEvent::Create(path));
        harness.advance(Duration::from_millis(100));

        let published = harness.take_published_delta().expect("delta published");
        assert_eq!(published.upserts[0].name, "new.md");
        assert_eq!(harness.journal_generations(), vec![1]);
        assert!(harness
            .search("new")
            .iter()
            .any(|result| result.title == "new.md"));
    }

    #[test]
    fn watcher_events_flow_into_runtime_index() {
        let harness = RuntimeIndexingHarness::new();
        let path = harness.create_file("visible.md");

        harness.push(IndexWatchEvent::Create(path));
        harness.advance(Duration::from_millis(100));

        assert_eq!(harness.journal_generations(), vec![1]);
        assert!(harness
            .search("visible")
            .iter()
            .any(|result| result.title == "visible.md"));
    }

    #[test]
    fn journal_failure_publishes_status_without_delta_or_generation_gap() {
        let harness = RuntimeIndexingHarness::new();
        let connection = Connection::open(&harness.database_path).unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER fail_runtime_commit BEFORE INSERT ON index_delta_batches BEGIN SELECT RAISE(ABORT, 'forced journal failure'); END;",
            )
            .unwrap();
        let failed_path = harness.create_file("failed.md");

        harness.push(IndexWatchEvent::Create(failed_path));
        harness.advance(Duration::from_millis(100));

        assert!(harness.committed_deltas().is_empty());
        assert!(harness.statuses().iter().any(|status| {
            status.degradation_code == Some(IndexDegradationCode::JournalWriteFailed)
        }));

        connection
            .execute_batch("DROP TRIGGER fail_runtime_commit;")
            .unwrap();
        let recovered_path = harness.create_file("recovered.md");
        harness.push(IndexWatchEvent::Create(recovered_path));
        harness.advance(Duration::from_millis(100));

        assert_eq!(
            harness
                .committed_deltas()
                .into_iter()
                .map(|delta| delta.generation)
                .collect::<Vec<_>>(),
            vec![1]
        );
    }

    #[test]
    fn committed_before_memory_apply_is_recovered_after_restart() {
        let harness = RuntimeIndexingHarness::new_with_apply(false);
        let path = harness.create_file("recover-after-crash.md");
        harness.push(IndexWatchEvent::Create(path));
        harness.advance(Duration::from_millis(100));
        assert!(harness.search("recover-after-crash").is_empty());
        harness.stop();

        let storage = SqliteStorage::open(harness.database_path.clone()).unwrap();
        let recovery = recover_layered_index(&storage);
        let results = recovery.index.search_files(
            &QueryRequest::new("recover-after-crash", SearchMode::Normal),
            20,
        );

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "recover-after-crash.md");
    }

    #[test]
    fn replay_after_memory_apply_is_idempotent() {
        let harness = RuntimeIndexingHarness::new();
        let path = harness.create_file("idempotent.md");
        harness.push(IndexWatchEvent::Create(path));
        harness.advance(Duration::from_millis(100));
        assert_eq!(harness.search("idempotent").len(), 1);
        harness.stop();

        let storage = SqliteStorage::open(harness.database_path.clone()).unwrap();
        let recovery = recover_layered_index(&storage);
        let results = recovery
            .index
            .search_files(&QueryRequest::new("idempotent", SearchMode::Normal), 20);

        assert_eq!(results.len(), 1);
    }

    #[test]
    fn stopping_runtime_indexing_joins_the_real_worker_thread() {
        struct ExitProbe(Arc<AtomicBool>);
        impl Drop for ExitProbe {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }

        let root = tempfile::tempdir().unwrap();
        let database_path = root.path().join("stop.sqlite");
        let storage = SqliteStorage::open(database_path).unwrap();
        let roots = vec![root.path().to_path_buf()];
        let rules = IndexPathRules::from_plan(&IndexScanPlan {
            include_roots: roots.clone(),
            ..IndexScanPlan::default()
        })
        .unwrap();
        let (_, inbox) = WatchEventInbox::bounded(roots.clone(), 1);
        let exited = Arc::new(AtomicBool::new(false));
        let probe = ExitProbe(Arc::clone(&exited));
        let handle = start_runtime_indexing_from_parts(
            None,
            inbox,
            TargetedIndexScanner::new(rules),
            Box::new(storage),
            RuntimeIndexingOptions {
                roots,
                policy: CoordinatorPolicy::production(),
                initial_generation: 0,
            },
            move |_| {
                let _ = &probe;
            },
        )
        .unwrap();

        let started = Instant::now();
        handle.stop();

        assert!(exited.load(Ordering::Acquire));
        assert!(started.elapsed() <= Duration::from_secs(1));
    }

    #[test]
    fn channel_overflow_marks_dirty_root_and_publishes_structured_status() {
        let root = tempfile::tempdir().unwrap();
        let roots = vec![root.path().to_path_buf()];
        let database_path = root.path().join("overflow.sqlite");
        let storage = SqliteStorage::open(database_path).unwrap();
        let rules = IndexPathRules::from_plan(&IndexScanPlan {
            include_roots: roots.clone(),
            ..IndexScanPlan::default()
        })
        .unwrap();
        let (sender, inbox) = WatchEventInbox::bounded(roots.clone(), 1);
        assert_eq!(
            sender.try_send(IndexWatchEvent::Write(root.path().join("first.md"))),
            crate::core::index_watcher::WatchSendOutcome::Queued
        );
        assert_eq!(
            sender.try_send(IndexWatchEvent::Write(root.path().join("overflow.md"))),
            crate::core::index_watcher::WatchSendOutcome::Overflowed
        );
        let events = Arc::new(Mutex::new(Vec::new()));
        let published = Arc::clone(&events);
        let handle = start_runtime_indexing_from_parts(
            None,
            inbox,
            TargetedIndexScanner::new(rules),
            Box::new(storage),
            RuntimeIndexingOptions {
                roots,
                policy: CoordinatorPolicy::new(
                    Duration::from_millis(10),
                    Duration::from_millis(25),
                ),
                initial_generation: 0,
            },
            move |event| published.lock().unwrap().push(event),
        )
        .unwrap();
        thread::sleep(Duration::from_millis(100));
        handle.stop();
        let events = events.lock().unwrap();

        assert!(events.iter().any(|event| matches!(
            event,
            RuntimeIndexingEvent::Status(RuntimeIncrementalStatus {
                dirty_roots: 1,
                degradation_code: Some(IndexDegradationCode::ChannelOverflow),
                ..
            })
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            RuntimeIndexingEvent::BaselineRefreshRequired {
                reason: BaselineRefreshReason::DirtyRoots
            }
        )));
    }

    #[test]
    fn delta_safety_limit_uses_inclusive_entry_and_memory_thresholds() {
        assert!(!delta_safety_limit_reached(
            MAX_DELTA_ENTRIES - 1,
            MAX_DELTA_BYTES - 1
        ));
        assert!(delta_safety_limit_reached(MAX_DELTA_ENTRIES, 0));
        assert!(delta_safety_limit_reached(0, MAX_DELTA_BYTES));
        assert!(matches!(
            baseline_refresh_event_for_delta_state(MAX_DELTA_ENTRIES, 0),
            Some(RuntimeIndexingEvent::BaselineRefreshRequired {
                reason: BaselineRefreshReason::DeltaSafetyLimit
            })
        ));
    }
}
