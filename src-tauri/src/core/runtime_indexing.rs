//! Tauri-neutral runtime incremental indexing service.

use crate::core::config::IndexConfigChange;
use crate::core::index_entry::{
    normalize_path_text_key, path_is_same_or_descendant_for_mode, PathComparisonMode,
};
use crate::core::index_entry::{IncrementalState, IndexDegradationCode, RuntimeIncrementalStatus};
use crate::core::index_journal::IndexJournalRepository;
use crate::core::index_update_coordinator::{
    CoordinatorPolicy, CoordinatorPushOutcome, CoordinatorShutdown, CoordinatorState,
};
use crate::core::index_watcher::{RuntimeIndexWatcher, WatchEventInbox, WatcherFailure};
use crate::core::layered_index::CommittedIndexDelta;
use crate::core::targeted_index_scanner::{
    DirectoryFingerprint, DirectoryManifestReader, KnownDirectoryEntriesReader, KnownIndexedChild,
    StdFileSystemProbe, TargetedIndexScanner, TargetedScanError, TargetedScanResult,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender, TryRecvError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Instant;

pub const MAX_DELTA_ENTRIES: usize = 50_000;
pub const MAX_DELTA_BYTES: usize = 64 * 1024 * 1024;
const MAX_INBOX_DRAIN_PER_TICK: usize = 256;

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
    CalibrationFailed,
    ManifestUnavailable,
    IndexConfigChanged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrustedIncrementalState {
    pub manifest_ready: bool,
    pub dirty_roots: usize,
}

impl TrustedIncrementalState {
    pub fn ready() -> Self {
        Self {
            manifest_ready: true,
            dirty_roots: 0,
        }
    }

    pub fn missing_manifest() -> Self {
        Self {
            manifest_ready: false,
            dirty_roots: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshDecision {
    FlushPendingThenCalibrateAllRoots,
    FullRefresh(BaselineRefreshReason),
}

pub fn refresh_decision(
    state: TrustedIncrementalState,
    config_change: IndexConfigChange,
) -> RefreshDecision {
    if config_change == IndexConfigChange::IndexSemantics {
        return RefreshDecision::FullRefresh(BaselineRefreshReason::IndexConfigChanged);
    }
    if !state.manifest_ready {
        return RefreshDecision::FullRefresh(BaselineRefreshReason::ManifestUnavailable);
    }
    if state.dirty_roots > 0 {
        return RefreshDecision::FullRefresh(BaselineRefreshReason::DirtyRoots);
    }
    RefreshDecision::FlushPendingThenCalibrateAllRoots
}

#[derive(Debug, Clone)]
pub enum RuntimeIndexingEvent {
    DeltaCommitted(CommittedIndexDelta),
    Status(RuntimeIncrementalStatus),
    BaselineRefreshRequired { reason: BaselineRefreshReason },
}

pub struct RuntimeIndexingHandle {
    command: SyncSender<RuntimeIndexingCommand>,
    shutdown: CoordinatorShutdown,
    join: Option<JoinHandle<()>>,
    recovery_required: Arc<AtomicBool>,
    last_committed_generation: Arc<AtomicU64>,
}

#[derive(Clone)]
pub struct RuntimeIndexingControl {
    command: SyncSender<RuntimeIndexingCommand>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeIndexingHandoffOutcome {
    Clean,
    RecoveryRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeIndexingHandoff {
    pub outcome: RuntimeIndexingHandoffOutcome,
    pub last_committed_generation: u64,
}

#[derive(Debug, Clone)]
enum RuntimeIndexingCommand {
    StopNow,
    Handoff,
    Fence(SyncSender<u64>),
    FlushPendingThenCalibrateAllRoots(SyncSender<Result<u64, String>>),
    Resume,
}

impl RuntimeIndexingHandle {
    pub fn control(&self) -> RuntimeIndexingControl {
        RuntimeIndexingControl {
            command: self.command.clone(),
        }
    }
    pub fn stop(mut self) {
        self.stop_and_join();
    }

    pub fn handoff(mut self) -> RuntimeIndexingHandoffOutcome {
        self.finish_handoff().outcome
    }

    pub fn handoff_with_generation(mut self) -> RuntimeIndexingHandoff {
        self.finish_handoff()
    }

    pub fn fence(&mut self) -> Result<u64, String> {
        let (reply, response) = mpsc::sync_channel(1);
        self.command
            .send(RuntimeIndexingCommand::Fence(reply))
            .map_err(|_| "runtime indexing worker is unavailable".to_owned())?;
        response
            .recv()
            .map_err(|_| "runtime indexing worker stopped before revision fence".to_owned())
    }

    pub fn resume(&mut self) -> Result<(), String> {
        self.command
            .send(RuntimeIndexingCommand::Resume)
            .map_err(|_| "runtime indexing worker is unavailable".to_owned())
    }

    pub fn flush_pending_then_calibrate_all_roots(&mut self) -> Result<u64, String> {
        self.control().flush_pending_then_calibrate_all_roots()
    }
}

impl RuntimeIndexingControl {
    pub fn flush_pending_then_calibrate_all_roots(&self) -> Result<u64, String> {
        let (reply, response) = mpsc::sync_channel(1);
        self.command
            .send(RuntimeIndexingCommand::FlushPendingThenCalibrateAllRoots(
                reply,
            ))
            .map_err(|_| "runtime indexing worker is unavailable".to_owned())?;
        response.recv().map_err(|_| {
            "runtime indexing worker stopped before manual calibration completed".to_owned()
        })?
    }
}

impl RuntimeIndexingHandle {
    fn finish_handoff(&mut self) -> RuntimeIndexingHandoff {
        if self.command.send(RuntimeIndexingCommand::Handoff).is_err() {
            self.recovery_required.store(true, Ordering::Release);
        }
        if !self.join_worker() {
            self.recovery_required.store(true, Ordering::Release);
        }
        let outcome = if self.recovery_required.load(Ordering::Acquire) {
            RuntimeIndexingHandoffOutcome::RecoveryRequired
        } else {
            RuntimeIndexingHandoffOutcome::Clean
        };
        RuntimeIndexingHandoff {
            outcome,
            last_committed_generation: self.last_committed_generation.load(Ordering::Acquire),
        }
    }

    fn stop_and_join(&mut self) {
        self.shutdown.request();
        let _ = self.command.try_send(RuntimeIndexingCommand::StopNow);
        self.join_worker();
    }

    fn join_worker(&mut self) -> bool {
        if let Some(join) = self.join.take() {
            return join.join().is_ok();
        }
        true
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
    start_runtime_indexing_with_scanner(
        watcher,
        inbox,
        Box::new(scanner),
        journal,
        options,
        publish,
    )
}

#[cfg(test)]
pub(crate) fn start_runtime_indexing_from_test_inbox(
    inbox: WatchEventInbox,
    scanner: TargetedIndexScanner,
    journal: Box<dyn IndexJournalRepository + Send>,
    options: RuntimeIndexingOptions,
    publish: impl Fn(RuntimeIndexingEvent) + Send + 'static,
) -> Result<RuntimeIndexingHandle, String> {
    start_runtime_indexing_from_parts(None, inbox, scanner, journal, options, publish)
}

trait RuntimeBatchScanner: Send {
    fn scan_batch_cancellable(
        &self,
        batch: crate::core::index_update_coordinator::CoordinatorBatch,
        journal: &dyn IndexJournalRepository,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<TargetedScanResult, TargetedScanError>;
}

struct JournalCalibrationReader<'a>(&'a dyn IndexJournalRepository);

impl DirectoryManifestReader for JournalCalibrationReader<'_> {
    fn directories_for_root(
        &self,
        root: &std::path::Path,
    ) -> Result<Vec<DirectoryFingerprint>, String> {
        self.0.directory_manifest_for_root(root)
    }
}

impl KnownDirectoryEntriesReader for JournalCalibrationReader<'_> {
    fn entries_for_directory(
        &self,
        root: &std::path::Path,
        directory: &std::path::Path,
    ) -> Result<Vec<KnownIndexedChild>, String> {
        self.0.known_direct_indexed_children(root, directory)
    }
}

impl RuntimeBatchScanner for TargetedIndexScanner {
    fn scan_batch_cancellable(
        &self,
        batch: crate::core::index_update_coordinator::CoordinatorBatch,
        journal: &dyn IndexJournalRepository,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<TargetedScanResult, TargetedScanError> {
        let dirty_roots = batch.dirty_roots.clone();
        let mut result = TargetedIndexScanner::scan_batch_cancellable(self, batch, is_cancelled)?;
        let reader = JournalCalibrationReader(journal);
        for root in dirty_roots {
            let calibration = self.calibrate_root_cancellable(
                &StdFileSystemProbe,
                &reader,
                &reader,
                &root,
                is_cancelled,
            )?;
            merge_targeted_scan_result(&mut result, calibration);
        }
        Ok(result)
    }
}

fn merge_targeted_scan_result(target: &mut TargetedScanResult, source: TargetedScanResult) {
    let _ = merge_targeted_scan_result_with_work(target, source);
}

fn merge_targeted_scan_result_with_work(
    target: &mut TargetedScanResult,
    source: TargetedScanResult,
) -> usize {
    let TargetedScanResult {
        upserts: source_upserts,
        removals: source_removals,
        manifest_upserts: source_manifest_upserts,
        manifest_removals: source_manifest_removals,
        mut failures,
    } = source;
    let mut work = 0;
    let mut entry_upserts: BTreeMap<String, _> = std::mem::take(&mut target.upserts)
        .into_iter()
        .map(|entry| (normalize_path_text_key(&entry.path), entry))
        .collect();
    let mut entry_removals: BTreeMap<String, _> = std::mem::take(&mut target.removals)
        .into_iter()
        .chain(source_removals)
        .map(|path| (normalized_path_key(&path), path))
        .collect();
    collapse_descendant_removals(&mut entry_removals, &mut work);
    entry_upserts.retain(|_, entry| {
        !has_removal_ancestor(&entry_removals, Path::new(&entry.path), &mut work)
    });
    for upsert in source_upserts {
        remove_ancestor_removals(&mut entry_removals, Path::new(&upsert.path), &mut work);
        entry_upserts.insert(normalize_path_text_key(&upsert.path), upsert);
    }

    let mut manifest_upserts: BTreeMap<String, _> = std::mem::take(&mut target.manifest_upserts)
        .into_iter()
        .map(|row| (normalize_path_text_key(&row.path), row))
        .collect();
    let mut manifest_removals: BTreeMap<String, _> = std::mem::take(&mut target.manifest_removals)
        .into_iter()
        .chain(source_manifest_removals)
        .map(|path| (normalized_path_key(&path), path))
        .collect();
    collapse_descendant_removals(&mut manifest_removals, &mut work);
    manifest_upserts.retain(|_, row| {
        !has_removal_ancestor(&manifest_removals, Path::new(&row.path), &mut work)
    });
    for upsert in source_manifest_upserts {
        remove_ancestor_removals(&mut manifest_removals, Path::new(&upsert.path), &mut work);
        manifest_upserts.insert(normalize_path_text_key(&upsert.path), upsert);
    }

    target.upserts = entry_upserts.into_values().collect();
    target.removals = entry_removals.into_values().collect();
    target.manifest_upserts = manifest_upserts.into_values().collect();
    target.manifest_removals = manifest_removals.into_values().collect();
    target.failures.append(&mut failures);
    work
}

fn normalized_path_key(path: &Path) -> String {
    normalize_path_text_key(&path.to_string_lossy())
}

fn has_removal_ancestor(
    removals: &BTreeMap<String, PathBuf>,
    path: &Path,
    work: &mut usize,
) -> bool {
    path.ancestors().any(|ancestor| {
        *work = work.saturating_add(1);
        removals.contains_key(&normalized_path_key(ancestor))
    })
}

fn remove_ancestor_removals(
    removals: &mut BTreeMap<String, PathBuf>,
    path: &Path,
    work: &mut usize,
) {
    for ancestor in path.ancestors() {
        *work = work.saturating_add(1);
        removals.remove(&normalized_path_key(ancestor));
    }
}

fn collapse_descendant_removals(removals: &mut BTreeMap<String, PathBuf>, work: &mut usize) {
    let keys: BTreeSet<_> = removals.keys().cloned().collect();
    removals.retain(|_, path| {
        !path.ancestors().skip(1).any(|ancestor| {
            *work = work.saturating_add(1);
            keys.contains(&normalized_path_key(ancestor))
        })
    });
}

fn start_runtime_indexing_with_scanner(
    watcher: Option<RuntimeIndexWatcher>,
    inbox: WatchEventInbox,
    scanner: Box<dyn RuntimeBatchScanner>,
    journal: Box<dyn IndexJournalRepository + Send>,
    options: RuntimeIndexingOptions,
    publish: impl Fn(RuntimeIndexingEvent) + Send + 'static,
) -> Result<RuntimeIndexingHandle, String> {
    let shutdown = CoordinatorShutdown::default();
    let worker_shutdown = shutdown.clone();
    let recovery_required = Arc::new(AtomicBool::new(false));
    let worker_recovery_required = Arc::clone(&recovery_required);
    let last_committed_generation = Arc::new(AtomicU64::new(options.initial_generation));
    let worker_last_committed_generation = Arc::clone(&last_committed_generation);
    let (command, command_receiver) = mpsc::sync_channel(1);
    let join = thread::Builder::new()
        .name("quickfox-runtime-indexing".to_owned())
        .spawn(move || {
            RuntimeIndexingService {
                watcher,
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
                command_receiver,
                roots: options.roots,
                degraded_roots: std::collections::BTreeSet::new(),
                baseline_refresh_requested: false,
                recovery_required: worker_recovery_required,
                last_committed_generation: worker_last_committed_generation,
            }
            .run();
        })
        .map_err(|error| error.to_string())?;

    Ok(RuntimeIndexingHandle {
        command,
        shutdown,
        join: Some(join),
        recovery_required,
        last_committed_generation,
    })
}

struct RuntimeIndexingService {
    watcher: Option<RuntimeIndexWatcher>,
    inbox: WatchEventInbox,
    scanner: Box<dyn RuntimeBatchScanner>,
    journal: Box<dyn IndexJournalRepository + Send>,
    coordinator: CoordinatorState,
    next_generation: u64,
    status: RuntimeIncrementalStatus,
    publish: Box<dyn Fn(RuntimeIndexingEvent) + Send>,
    shutdown: CoordinatorShutdown,
    command_receiver: mpsc::Receiver<RuntimeIndexingCommand>,
    roots: Vec<PathBuf>,
    degraded_roots: std::collections::BTreeSet<PathBuf>,
    baseline_refresh_requested: bool,
    recovery_required: Arc<AtomicBool>,
    last_committed_generation: Arc<AtomicU64>,
}

impl RuntimeIndexingService {
    fn run(&mut self) {
        self.publish_status();
        loop {
            match self.command_receiver.try_recv() {
                Ok(RuntimeIndexingCommand::Handoff) => {
                    self.finish_handoff();
                    return;
                }
                Ok(RuntimeIndexingCommand::Fence(reply)) => {
                    self.finish_fence();
                    let _ = reply.send(self.last_committed_generation.load(Ordering::Acquire));
                    if !self.wait_for_fence_release() {
                        return;
                    }
                }
                Ok(RuntimeIndexingCommand::FlushPendingThenCalibrateAllRoots(reply)) => {
                    let result = self.flush_pending_then_calibrate_all_roots();
                    let _ = reply.send(result);
                }
                Ok(RuntimeIndexingCommand::Resume) => {}
                Ok(RuntimeIndexingCommand::StopNow) | Err(TryRecvError::Disconnected) => return,
                Err(TryRecvError::Empty) => {}
            }
            if self.should_stop() {
                return;
            }
            let now = Instant::now();
            let wait = self.coordinator.next_wait(now);
            if let Ok(event) = self.inbox.recv_timeout(wait) {
                self.accept_event(event, Instant::now());
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
    }

    fn finish_handoff(&mut self) {
        self.watcher.take();
        self.finish_fence();
    }

    fn finish_fence(&mut self) {
        while !self.shutdown.is_requested() {
            let Ok(event) = self.inbox.try_recv() else {
                break;
            };
            self.accept_event(event, Instant::now());
        }
        self.consume_watcher_degradation();
        if !self.coordinator.is_empty() {
            self.commit_ready_batch();
        }
    }

    fn wait_for_fence_release(&mut self) -> bool {
        while !self.shutdown.is_requested() {
            match self
                .command_receiver
                .recv_timeout(std::time::Duration::from_millis(50))
            {
                Ok(RuntimeIndexingCommand::Resume) => return true,
                Ok(RuntimeIndexingCommand::Handoff) => {
                    self.finish_handoff();
                    return false;
                }
                Ok(RuntimeIndexingCommand::StopNow) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return false;
                }
                Ok(RuntimeIndexingCommand::Fence(reply)) => {
                    let _ = reply.send(self.last_committed_generation.load(Ordering::Acquire));
                }
                Ok(RuntimeIndexingCommand::FlushPendingThenCalibrateAllRoots(reply)) => {
                    let _ = reply.send(Err(
                        "runtime indexing worker is fenced for a configuration transition"
                            .to_owned(),
                    ));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
        false
    }

    fn drain_inbox(&mut self) {
        for _ in 0..MAX_INBOX_DRAIN_PER_TICK {
            if self.should_stop() {
                return;
            }
            let Ok(event) = self.inbox.try_recv() else {
                return;
            };
            self.accept_event(event, Instant::now());
        }
    }

    fn flush_pending_then_calibrate_all_roots(&mut self) -> Result<u64, String> {
        self.finish_fence();
        if self.recovery_required.load(Ordering::Acquire) {
            return Err("pending runtime indexing state requires full refresh".to_owned());
        }
        self.status.state = IncrementalState::Calibrating;
        self.status.dirty_roots = self.roots.len();
        self.publish_status();
        let now = Instant::now();
        for root in &self.roots {
            self.coordinator.mark_dirty_root(root.clone(), now);
        }
        self.commit_ready_batch();
        if self.recovery_required.load(Ordering::Acquire) {
            return Err("manual root calibration requires full refresh".to_owned());
        }
        if self.degraded_roots.is_empty() {
            self.status.state = IncrementalState::Watching;
            self.status.dirty_roots = 0;
            self.status.degradation_code = None;
        } else {
            self.status.state = IncrementalState::Degraded;
            self.status.dirty_roots = self.degraded_roots.len();
            self.status.degradation_code = Some(IndexDegradationCode::CalibrationFailed);
        }
        self.publish_status();
        Ok(self.last_committed_generation.load(Ordering::Acquire))
    }

    fn accept_event(&mut self, event: crate::core::index_watcher::IndexWatchEvent, now: Instant) {
        if self.coordinator.push_event(event, now) == CoordinatorPushOutcome::CapacityReached {
            self.coordinator.discard_individual_events();
            self.degraded_roots.extend(self.roots.iter().cloned());
            for root in &self.roots {
                self.coordinator.mark_dirty_root(root.clone(), now);
            }
            self.publish_degraded(IndexDegradationCode::ChannelOverflow);
            self.request_baseline_refresh(BaselineRefreshReason::DirtyRoots);
        }
    }

    fn consume_watcher_degradation(&mut self) {
        let dirty_roots = self.inbox.take_dirty_roots();
        let failure = self.inbox.take_failure();
        let degradation = self.inbox.take_degradation_code();
        if dirty_roots.is_empty() && failure.is_none() && degradation.is_none() {
            return;
        }

        if watcher_rescan_requires_calibration(failure.is_some(), degradation) {
            self.schedule_watcher_rescan_calibration(dirty_roots);
            return;
        }

        let now = Instant::now();
        for root in dirty_roots {
            self.degraded_roots.insert(root.clone());
            self.coordinator.mark_dirty_root(root, now);
        }
        let previous_status = self.status.clone();
        self.status.state = IncrementalState::Degraded;
        self.status.degradation_code = failure
            .as_ref()
            .map(|_| IndexDegradationCode::WatcherRuntimeFailed)
            .or(degradation)
            .or(Some(IndexDegradationCode::WatcherOverflow));
        self.status.pending_events = self.coordinator.pending_event_count();
        self.status.dirty_roots = self.degraded_roots.len();
        if self.status != previous_status {
            self.publish_status();
        }
        self.request_baseline_refresh(if failure.is_some() {
            BaselineRefreshReason::WatcherFailure
        } else {
            BaselineRefreshReason::DirtyRoots
        });
    }

    fn schedule_watcher_rescan_calibration(&mut self, dirty_roots: BTreeSet<PathBuf>) {
        let now = Instant::now();
        for root in dirty_roots {
            self.coordinator.mark_dirty_root(root, now);
        }
        self.status.state = IncrementalState::Calibrating;
        self.status.degradation_code = None;
        self.status.pending_events = self.coordinator.pending_event_count();
        self.status.dirty_roots = self.coordinator.dirty_root_count();
        self.publish_status();
    }

    fn commit_ready_batch(&mut self) {
        let batch = self.coordinator.drain();
        let attempted_dirty_roots: BTreeSet<_> = batch.dirty_roots.iter().cloned().collect();
        let mut dirty_roots = self.degraded_roots.clone();
        dirty_roots.extend(batch.dirty_roots.iter().cloned());
        let dirty_root_count = dirty_roots.len();
        let entry_count = batch
            .changed_paths
            .len()
            .saturating_add(batch.removed_paths.len());
        if entry_count == 0 && dirty_roots.is_empty() {
            self.status.pending_events = 0;
            self.status.dirty_roots = dirty_root_count;
            self.publish_status();
            return;
        }

        let started = Instant::now();
        let scanned =
            match self
                .scanner
                .scan_batch_cancellable(batch, self.journal.as_ref(), &|| {
                    self.shutdown.is_requested()
                }) {
                Ok(scanned) => scanned,
                Err(TargetedScanError::Cancelled) => return,
                Err(TargetedScanError::Io(_)) => {
                    self.degraded_roots.extend(self.roots.iter().cloned());
                    self.publish_calibration_failure(self.degraded_roots.len(), true);
                    return;
                }
            };
        let failed_roots = self.failed_configured_roots(&scanned.failures);
        let failed_root_count = failed_roots.len();
        for root in attempted_dirty_roots
            .iter()
            .filter(|root| !failed_roots.contains(*root))
        {
            self.degraded_roots.remove(root);
        }
        self.degraded_roots.extend(failed_roots.iter().cloned());
        dirty_roots.extend(failed_roots);
        let remaining_dirty_root_count = self.degraded_roots.len();
        let has_durable_changes = !scanned.upserts.is_empty()
            || !scanned.removals.is_empty()
            || !scanned.manifest_upserts.is_empty()
            || !scanned.manifest_removals.is_empty();
        if !has_durable_changes {
            if failed_root_count > 0 {
                self.publish_calibration_failure(dirty_roots.len(), false);
            } else {
                self.status.state = if remaining_dirty_root_count == 0 {
                    IncrementalState::Watching
                } else {
                    IncrementalState::Degraded
                };
                self.status.degradation_code = (remaining_dirty_root_count > 0)
                    .then_some(IndexDegradationCode::CalibrationFailed);
                self.status.pending_events = 0;
                self.status.dirty_roots = remaining_dirty_root_count;
                self.publish_status();
            }
            return;
        }
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
            self.degraded_roots.extend(self.roots.iter().cloned());
            self.publish_degraded(IndexDegradationCode::JournalWriteFailed);
            self.request_baseline_refresh(BaselineRefreshReason::DirtyRoots);
            if failed_root_count > 0 {
                self.publish_calibration_failure(dirty_roots.len(), false);
            }
            return;
        }

        self.status.state = if remaining_dirty_root_count == 0 {
            IncrementalState::Watching
        } else {
            IncrementalState::Degraded
        };
        self.status.pending_events = 0;
        self.status.dirty_roots = remaining_dirty_root_count;
        self.status.last_batch_entries = delta.upserts.len().saturating_add(delta.removals.len());
        self.status.last_batch_duration_ms = started.elapsed().as_millis() as u64;
        if remaining_dirty_root_count == 0 {
            self.status.degradation_code = None;
        }
        (self.publish)(RuntimeIndexingEvent::DeltaCommitted(delta));
        self.last_committed_generation
            .store(self.next_generation, Ordering::Release);
        self.next_generation = self.next_generation.saturating_add(1);
        self.publish_status();
        if failed_root_count > 0 {
            self.publish_calibration_failure(remaining_dirty_root_count, false);
        }
    }

    fn update_pending_status(&mut self) {
        let pending_events = self.coordinator.pending_event_count();
        let dirty_roots = self.degraded_roots.len();
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
        self.status.dirty_roots = self.degraded_roots.len();
        self.publish_status();
    }

    fn failed_configured_roots(
        &self,
        failures: &[crate::core::index_entry::IndexFailure],
    ) -> std::collections::BTreeSet<PathBuf> {
        let mode = PathComparisonMode::native();
        let mut roots: std::collections::BTreeSet<PathBuf> = failures
            .iter()
            .filter_map(|failure| {
                let failed_path = std::path::Path::new(&failure.root);
                self.roots
                    .iter()
                    .filter(|root| path_is_same_or_descendant_for_mode(root, failed_path, mode))
                    .max_by_key(|root| root.components().count())
            })
            .cloned()
            .collect();
        if !failures.is_empty() && roots.is_empty() {
            roots.extend(self.roots.iter().cloned());
        }
        roots
    }

    fn publish_calibration_failure(&mut self, dirty_root_count: usize, request_refresh: bool) {
        // A calibration failure means this service cannot prove that its handoff captured
        // every filesystem change. Keep that separate from whether another full refresh
        // should be requested; a refresh already in progress must not loop forever.
        self.recovery_required.store(true, Ordering::Release);
        self.status.state = IncrementalState::Degraded;
        self.status.degradation_code = Some(IndexDegradationCode::CalibrationFailed);
        self.status.pending_events = 0;
        self.status.dirty_roots = dirty_root_count;
        self.publish_status();
        if request_refresh {
            self.request_baseline_refresh(BaselineRefreshReason::CalibrationFailed);
        }
    }

    fn request_baseline_refresh(&mut self, reason: BaselineRefreshReason) {
        self.recovery_required.store(true, Ordering::Release);
        if self.baseline_refresh_requested {
            return;
        }
        self.baseline_refresh_requested = true;
        (self.publish)(RuntimeIndexingEvent::BaselineRefreshRequired { reason });
    }

    fn publish_status(&self) {
        (self.publish)(RuntimeIndexingEvent::Status(self.status.clone()));
    }
}

fn watcher_rescan_requires_calibration(
    has_failure: bool,
    degradation: Option<IndexDegradationCode>,
) -> bool {
    !has_failure && degradation == Some(IndexDegradationCode::WatcherOverflow)
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
    use crate::core::index::{FileSearchIndex, IndexScanOptions, IndexScanner};
    use crate::core::index_entry::{IndexFailure, IndexedEntry, IndexedEntryKind};
    use crate::core::index_journal::recover_layered_index;
    use crate::core::index_scanner::{IndexPathRules, IndexScanPlan};
    use crate::core::index_watcher::{IndexWatchEvent, WatchEventInbox, WatchEventSender};
    use crate::core::layered_index::LayeredSearchIndex;
    use crate::core::search::{QueryRequest, SearchMode, SearchResult};
    use crate::core::storage::SqliteStorage;
    use crate::core::targeted_index_scanner::baseline_manifest_from_entries;
    use rusqlite::Connection;
    use std::collections::VecDeque;
    use std::fs;
    use std::io;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tempfile::TempDir;

    fn wait_until(description: &str, condition: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !condition() {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {description}"
            );
            thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn manual_refresh_uses_manifest_calibration_when_state_is_trusted() {
        assert_eq!(
            refresh_decision(TrustedIncrementalState::ready(), IndexConfigChange::None),
            RefreshDecision::FlushPendingThenCalibrateAllRoots
        );
    }

    #[test]
    fn missing_dirty_or_semantic_state_uses_full_refresh_with_reason() {
        assert_eq!(
            refresh_decision(
                TrustedIncrementalState::missing_manifest(),
                IndexConfigChange::None
            ),
            RefreshDecision::FullRefresh(BaselineRefreshReason::ManifestUnavailable)
        );
        assert_eq!(
            refresh_decision(
                TrustedIncrementalState {
                    manifest_ready: true,
                    dirty_roots: 1,
                },
                IndexConfigChange::None
            ),
            RefreshDecision::FullRefresh(BaselineRefreshReason::DirtyRoots)
        );
        assert_eq!(
            refresh_decision(
                TrustedIncrementalState::ready(),
                IndexConfigChange::IndexSemantics
            ),
            RefreshDecision::FullRefresh(BaselineRefreshReason::IndexConfigChanged)
        );
    }

    #[test]
    fn watcher_rescan_is_calibrated_without_requesting_baseline_recovery() {
        assert!(watcher_rescan_requires_calibration(
            false,
            Some(IndexDegradationCode::WatcherOverflow),
        ));
        assert!(!watcher_rescan_requires_calibration(
            true,
            Some(IndexDegradationCode::WatcherOverflow),
        ));
        assert!(!watcher_rescan_requires_calibration(
            false,
            Some(IndexDegradationCode::ChannelOverflow),
        ));
    }

    struct FixedScanner(Mutex<VecDeque<Result<TargetedScanResult, TargetedScanError>>>);

    impl FixedScanner {
        fn returning(result: TargetedScanResult) -> Self {
            Self(Mutex::new(VecDeque::from([Ok(result)])))
        }

        fn returning_many(results: impl IntoIterator<Item = TargetedScanResult>) -> Self {
            Self(Mutex::new(
                results.into_iter().map(Ok).collect::<VecDeque<_>>(),
            ))
        }

        fn failing(error: io::Error) -> Self {
            Self(Mutex::new(VecDeque::from([Err(TargetedScanError::Io(
                error,
            ))])))
        }
    }

    impl RuntimeBatchScanner for FixedScanner {
        fn scan_batch_cancellable(
            &self,
            _batch: crate::core::index_update_coordinator::CoordinatorBatch,
            _journal: &dyn IndexJournalRepository,
            _is_cancelled: &dyn Fn() -> bool,
        ) -> Result<TargetedScanResult, TargetedScanError> {
            self.0.lock().unwrap().pop_front().expect("scripted scan")
        }
    }

    fn start_fixed_scanner(
        root: &TempDir,
        scanner: impl RuntimeBatchScanner + 'static,
    ) -> (
        WatchEventSender,
        RuntimeIndexingHandle,
        Arc<Mutex<Vec<RuntimeIndexingEvent>>>,
        PathBuf,
    ) {
        let database_path = root.path().join("fixed-scanner.sqlite");
        let storage = SqliteStorage::open(database_path.clone()).unwrap();
        let baseline_id = storage.save_completed_index_batch(1, &[]).unwrap();
        storage
            .activate_baseline_and_clear_incremental_state(baseline_id, 0)
            .unwrap();
        let roots = vec![root.path().to_path_buf()];
        let (sender, inbox) = WatchEventInbox::bounded(roots.clone(), 4);
        let events = Arc::new(Mutex::new(Vec::new()));
        let published = Arc::clone(&events);
        let handle = start_runtime_indexing_with_scanner(
            None,
            inbox,
            Box::new(scanner),
            Box::new(storage),
            RuntimeIndexingOptions {
                roots,
                policy: CoordinatorPolicy::new(Duration::ZERO, Duration::ZERO),
                initial_generation: 0,
            },
            move |event| published.lock().unwrap().push(event),
        )
        .unwrap();
        (sender, handle, events, database_path)
    }

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

        fn wait_until_processed(&self) {
            wait_until("runtime event processing", || {
                self.events.lock().unwrap().iter().any(|event| {
                    matches!(
                        event,
                        RuntimeIndexingEvent::DeltaCommitted(_)
                            | RuntimeIndexingEvent::BaselineRefreshRequired { .. }
                    )
                })
            });
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

        fn handoff(&self) -> RuntimeIndexingHandoffOutcome {
            self.handle
                .lock()
                .unwrap()
                .take()
                .expect("runtime indexing handle")
                .handoff()
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
        harness.wait_until_processed();

        let published = harness.take_published_delta().expect("delta published");
        assert_eq!(published.upserts[0].name, "new.md");
        assert_eq!(harness.journal_generations(), vec![1]);
        assert!(harness
            .search("new")
            .iter()
            .any(|result| result.title == "new.md"));
    }

    #[test]
    fn native_watcher_commits_an_empty_file_created_after_startup() {
        let root = tempfile::tempdir().unwrap();
        let root_path = root.path().canonicalize().unwrap();
        let storage = SqliteStorage::open(root.path().join("native-runtime.sqlite")).unwrap();
        let baseline_id = storage.save_completed_index_batch(1, &[]).unwrap();
        let manifest = baseline_manifest_from_entries(&[], std::slice::from_ref(&root_path));
        storage
            .activate_baseline_with_manifest_and_clear_incremental_state(baseline_id, 0, &manifest)
            .unwrap();
        let rules = IndexPathRules::from_plan(&IndexScanPlan {
            include_roots: vec![root_path.clone()],
            ..IndexScanPlan::default()
        })
        .unwrap();
        let watcher = RuntimeIndexWatcher::watch_roots(vec![root_path.clone()]).unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let published = Arc::clone(&events);
        let handle = start_runtime_indexing(
            watcher,
            TargetedIndexScanner::new(rules),
            Box::new(storage),
            RuntimeIndexingOptions {
                roots: vec![root_path.clone()],
                policy: CoordinatorPolicy::new(
                    Duration::from_millis(10),
                    Duration::from_millis(100),
                ),
                initial_generation: 0,
            },
            move |event| published.lock().unwrap().push(event),
        )
        .unwrap();
        let created = root_path.join("empty.txt");
        fs::write(&created, []).unwrap();

        wait_until("native empty-file delta", || {
            events.lock().unwrap().iter().any(|event| {
                matches!(
                    event,
                    RuntimeIndexingEvent::DeltaCommitted(delta)
                        if delta.upserts.iter().any(|entry| entry.path == created.to_string_lossy())
                )
            })
        });
        handle.stop();
    }

    #[test]
    fn watcher_failure_transition_publishes_when_pending_and_dirty_counts_are_unchanged() {
        let root = tempfile::tempdir().unwrap();
        let database_path = root.path().join("status-transition.sqlite");
        let storage = SqliteStorage::open(database_path).unwrap();
        let baseline_id = storage.save_completed_index_batch(1, &[]).unwrap();
        storage
            .activate_baseline_and_clear_incremental_state(baseline_id, 0)
            .unwrap();
        let roots = vec![root.path().to_path_buf()];
        let (sender, inbox) = WatchEventInbox::bounded(roots.clone(), 4);
        sender.record_failure(crate::core::index_watcher::WatcherFailure::new(
            root.path().to_path_buf(),
            "backend disconnected",
        ));
        assert_eq!(inbox.take_dirty_roots().len(), 1);
        let events = Arc::new(Mutex::new(Vec::new()));
        let published = Arc::clone(&events);
        let handle = start_runtime_indexing_with_scanner(
            None,
            inbox,
            Box::new(FixedScanner::returning(TargetedScanResult::default())),
            Box::new(storage),
            RuntimeIndexingOptions {
                roots,
                policy: CoordinatorPolicy::new(Duration::ZERO, Duration::ZERO),
                initial_generation: 0,
            },
            move |event| published.lock().unwrap().push(event),
        )
        .unwrap();

        wait_until("watcher failure status", || {
            events.lock().unwrap().iter().any(|event| {
                matches!(
                    event,
                    RuntimeIndexingEvent::Status(RuntimeIncrementalStatus {
                        degradation_code: Some(IndexDegradationCode::WatcherRuntimeFailed),
                        ..
                    })
                )
            })
        });
        handle.stop();

        let statuses: Vec<_> = events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
                RuntimeIndexingEvent::Status(status) => Some(status.clone()),
                _ => None,
            })
            .collect();
        assert!(statuses.iter().any(|status| {
            status.state == IncrementalState::Degraded
                && status.degradation_code == Some(IndexDegradationCode::WatcherRuntimeFailed)
        }));
    }

    #[test]
    fn manual_calibration_flushes_pending_events_and_scans_every_root() {
        let root = tempfile::tempdir().unwrap();
        let discovered = root.path().join("manual.md");
        fs::write(&discovered, "manual").unwrap();
        let scanner = FixedScanner::returning(TargetedScanResult {
            upserts: vec![IndexedEntry::from_path_metadata(
                &discovered,
                root.path(),
                IndexedEntryKind::File,
            )],
            ..TargetedScanResult::default()
        });
        let (_sender, handle, events, database_path) = start_fixed_scanner(&root, scanner);

        let generation = handle
            .control()
            .flush_pending_then_calibrate_all_roots()
            .expect("manual calibration succeeds");
        handle.stop();

        assert_eq!(generation, 1);
        assert!(events.lock().unwrap().iter().any(|event| matches!(
            event,
            RuntimeIndexingEvent::DeltaCommitted(delta)
                if delta.upserts.iter().any(|entry| entry.path == discovered.to_string_lossy())
        )));
        assert_eq!(
            SqliteStorage::open(database_path)
                .unwrap()
                .highest_committed_generation()
                .unwrap(),
            1
        );
    }

    #[test]
    fn dirty_root_calibration_folds_same_queued_path_without_duplicate_journal_rows() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("same.md");
        fs::write(&path, "old").unwrap();
        let queued = IndexedEntry::from_path_metadata(&path, root.path(), IndexedEntryKind::File);
        fs::write(&path, "new calibration value").unwrap();
        let calibrated =
            IndexedEntry::from_path_metadata(&path, root.path(), IndexedEntryKind::File);
        let root_text = root.path().to_string_lossy().into_owned();
        let fingerprint = DirectoryFingerprint {
            path: root_text.clone(),
            parent: None,
            root: root_text,
            modified_ms: Some(2),
        };
        let mut merged = TargetedScanResult {
            upserts: vec![queued],
            removals: vec![path.clone()],
            manifest_upserts: vec![DirectoryFingerprint {
                modified_ms: Some(1),
                ..fingerprint.clone()
            }],
            manifest_removals: vec![PathBuf::from(&fingerprint.path)],
            failures: Vec::new(),
        };

        merge_targeted_scan_result(
            &mut merged,
            TargetedScanResult {
                upserts: vec![calibrated.clone()],
                manifest_upserts: vec![fingerprint.clone()],
                ..TargetedScanResult::default()
            },
        );

        assert_eq!(merged.upserts, vec![calibrated]);
        assert!(merged.removals.is_empty());
        assert_eq!(merged.manifest_upserts, vec![fingerprint]);
        assert!(merged.manifest_removals.is_empty());
        let storage = SqliteStorage::open(root.path().join("folded.sqlite")).unwrap();
        let baseline_id = storage.save_completed_index_batch(1, &[]).unwrap();
        storage
            .activate_baseline_with_manifest_and_clear_incremental_state(
                baseline_id,
                0,
                &merged.manifest_upserts,
            )
            .unwrap();
        storage
            .commit_incremental_batch(
                &CommittedIndexDelta {
                    generation: 1,
                    upserts: merged.upserts,
                    removals: merged.removals,
                },
                &merged.manifest_upserts,
                &merged.manifest_removals,
            )
            .unwrap();
    }

    #[test]
    fn dirty_root_large_merge_uses_bounded_ancestor_work() {
        let count = 10_000;
        let mut target = TargetedScanResult {
            upserts: (0..count)
                .map(|index| {
                    IndexedEntry::legacy(
                        format!("/tmp/root/file-{index}.md"),
                        format!("file-{index}.md"),
                        IndexedEntryKind::File,
                    )
                })
                .collect(),
            ..TargetedScanResult::default()
        };
        let source = TargetedScanResult {
            upserts: (0..count)
                .map(|index| {
                    IndexedEntry::legacy(
                        format!("/tmp/root/file-{index}.md"),
                        format!("file-{index}.md"),
                        IndexedEntryKind::File,
                    )
                })
                .collect(),
            ..TargetedScanResult::default()
        };

        let ancestor_work = merge_targeted_scan_result_with_work(&mut target, source);

        assert_eq!(target.upserts.len(), count);
        assert!(ancestor_work <= count * 10);
    }

    #[test]
    fn manual_calibration_persists_manifest_only_changes() {
        let root = tempfile::tempdir().unwrap();
        let root_text = root.path().to_string_lossy().into_owned();
        let scanner = FixedScanner::returning(TargetedScanResult {
            manifest_upserts: vec![DirectoryFingerprint {
                path: root_text.clone(),
                parent: None,
                root: root_text,
                modified_ms: Some(42),
            }],
            ..TargetedScanResult::default()
        });
        let (_sender, mut handle, _events, database_path) = start_fixed_scanner(&root, scanner);

        let generation = handle
            .flush_pending_then_calibrate_all_roots()
            .expect("manifest-only calibration succeeds");
        handle.stop();

        assert_eq!(generation, 1);
        assert_eq!(
            SqliteStorage::open(database_path)
                .unwrap()
                .directory_manifest_for_root(root.path())
                .unwrap()[0]
                .modified_ms,
            Some(42)
        );
    }

    #[test]
    fn watcher_events_flow_into_runtime_index() {
        let harness = RuntimeIndexingHarness::new();
        let path = harness.create_file("visible.md");

        harness.push(IndexWatchEvent::Create(path));
        harness.wait_until_processed();

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
        harness.wait_until_processed();

        assert!(harness.committed_deltas().is_empty());
        assert!(harness.statuses().iter().any(|status| {
            status.degradation_code == Some(IndexDegradationCode::JournalWriteFailed)
        }));
        assert_eq!(
            harness
                .events
                .lock()
                .unwrap()
                .iter()
                .filter(|event| matches!(
                    event,
                    RuntimeIndexingEvent::BaselineRefreshRequired {
                        reason: BaselineRefreshReason::DirtyRoots
                    }
                ))
                .count(),
            1,
            "a drained batch must latch one recoverable fallback when its journal commit fails"
        );
        assert_eq!(harness.journal_generations(), Vec::<u64>::new());
        assert_eq!(
            harness.handoff(),
            RuntimeIndexingHandoffOutcome::RecoveryRequired,
            "a failed journal commit must make baseline handoff untrusted"
        );

        connection
            .execute_batch("DROP TRIGGER fail_runtime_commit;")
            .unwrap();

        harness.stop();
        let fallback_report = IndexScanner
            .scan(IndexScanOptions {
                include_dirs: vec![harness.root.path().to_path_buf()],
                ..IndexScanOptions::default()
            })
            .unwrap();
        let storage = SqliteStorage::open(harness.database_path.clone()).unwrap();
        let generation = storage.highest_committed_generation().unwrap();
        let baseline_id = storage
            .save_completed_index_batch(2, &fallback_report.entries)
            .unwrap();
        let manifest = baseline_manifest_from_entries(
            &fallback_report.entries,
            &[harness.root.path().to_path_buf()],
        );
        storage
            .activate_baseline_with_manifest_and_clear_incremental_state(
                baseline_id,
                generation,
                &manifest,
            )
            .unwrap();
        let recovered = recover_layered_index(&storage);
        assert!(recovered
            .index
            .search_files(&QueryRequest::new("failed", SearchMode::Normal), 20)
            .iter()
            .any(|result| result.title == "failed.md"));
        assert_eq!(recovered.index.generation(), generation);
    }

    #[test]
    fn committed_before_memory_apply_is_recovered_after_restart() {
        let harness = RuntimeIndexingHarness::new_with_apply(false);
        let path = harness.create_file("recover-after-crash.md");
        harness.push(IndexWatchEvent::Create(path));
        harness.wait_until_processed();
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
    fn worker_commit_queued_publish_refresh_handoff_clears_journal_without_duplicate_replay() {
        let harness = RuntimeIndexingHarness::new_with_apply(false);
        let path = harness.create_file("handoff.md");
        harness.push(IndexWatchEvent::Create(path));
        harness.wait_until_processed();
        let queued_delta = harness
            .take_published_delta()
            .expect("worker committed delta");
        assert!(harness.search("handoff").is_empty());
        harness.stop();

        let storage = SqliteStorage::open(harness.database_path.clone()).unwrap();
        let stable_generation = storage.highest_committed_generation().unwrap();
        assert_eq!(stable_generation, queued_delta.generation);
        let refreshed_entries = queued_delta.upserts.clone();
        let refreshed_id = storage
            .save_completed_index_batch(2, &refreshed_entries)
            .unwrap();
        let manifest = baseline_manifest_from_entries(
            &refreshed_entries,
            &[harness.root.path().to_path_buf()],
        );
        storage
            .activate_baseline_with_manifest_and_clear_incremental_state(
                refreshed_id,
                stable_generation,
                &manifest,
            )
            .unwrap();
        harness
            .index
            .lock()
            .unwrap()
            .replace_baseline(refreshed_entries, stable_generation);
        harness.index.lock().unwrap().apply_delta(queued_delta);

        assert!(storage.committed_index_deltas_after(0).unwrap().is_empty());
        assert_eq!(harness.search("handoff").len(), 1);
        let recovery = recover_layered_index(&storage);
        assert_eq!(recovery.index.generation(), stable_generation);
        assert_eq!(
            recovery
                .index
                .search_files(&QueryRequest::new("handoff", SearchMode::Normal), 20)
                .len(),
            1
        );
    }

    #[test]
    fn replay_after_memory_apply_is_idempotent() {
        let harness = RuntimeIndexingHarness::new();
        let path = harness.create_file("idempotent.md");
        harness.push(IndexWatchEvent::Create(path));
        harness.wait_until_processed();
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
        assert!(started.elapsed() <= Duration::from_millis(250));
    }

    #[test]
    fn partial_not_found_scan_commits_successes_and_requires_handoff_reconciliation() {
        let root = tempfile::tempdir().unwrap();
        let accepted = root.path().join("accepted.md");
        fs::write(&accepted, "accepted").unwrap();
        let failed = root.path().join("missing.md");
        let scanner = FixedScanner::returning(TargetedScanResult {
            upserts: vec![IndexedEntry::from_path_metadata(
                &accepted,
                root.path(),
                IndexedEntryKind::File,
            )],
            failures: vec![IndexFailure {
                root: failed.to_string_lossy().into_owned(),
                message: "not found".to_owned(),
            }],
            ..TargetedScanResult::default()
        });
        let (sender, handle, events, database_path) = start_fixed_scanner(&root, scanner);

        assert_eq!(
            sender.try_send(IndexWatchEvent::Write(accepted)),
            crate::core::index_watcher::WatchSendOutcome::Queued
        );
        wait_until("partial scan degradation", || {
            events.lock().unwrap().iter().any(|event| {
                matches!(
                    event,
                    RuntimeIndexingEvent::Status(RuntimeIncrementalStatus {
                        degradation_code: Some(IndexDegradationCode::CalibrationFailed),
                        ..
                    })
                )
            })
        });
        assert_eq!(
            handle.handoff(),
            RuntimeIndexingHandoffOutcome::RecoveryRequired
        );
        let events = events.lock().unwrap();

        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, RuntimeIndexingEvent::DeltaCommitted(_)))
                .count(),
            1
        );
        assert!(events.iter().any(|event| matches!(
            event,
            RuntimeIndexingEvent::Status(RuntimeIncrementalStatus {
                dirty_roots: 1,
                degradation_code: Some(IndexDegradationCode::CalibrationFailed),
                ..
            })
        )));
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    event,
                    RuntimeIndexingEvent::BaselineRefreshRequired {
                        reason: BaselineRefreshReason::CalibrationFailed
                    }
                ))
                .count(),
            0
        );
        assert_eq!(
            SqliteStorage::open(database_path)
                .unwrap()
                .committed_index_deltas_after(0)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn inaccessible_child_keeps_old_view_and_requires_handoff_reconciliation() {
        let root = tempfile::tempdir().unwrap();
        let old_path = root.path().join("old.md");
        fs::write(&old_path, "old").unwrap();
        let old_entry =
            IndexedEntry::from_path_metadata(&old_path, root.path(), IndexedEntryKind::File);
        let database_path = root.path().join("permission-failure.sqlite");
        let storage = SqliteStorage::open(database_path.clone()).unwrap();
        let baseline_id = storage
            .save_completed_index_batch(1, std::slice::from_ref(&old_entry))
            .unwrap();
        storage
            .activate_baseline_and_clear_incremental_state(baseline_id, 0)
            .unwrap();
        let scanner = FixedScanner::returning(TargetedScanResult {
            failures: vec![IndexFailure {
                root: root.path().join("locked").to_string_lossy().into_owned(),
                message: "permission denied".to_owned(),
            }],
            ..TargetedScanResult::default()
        });
        let roots = vec![root.path().to_path_buf()];
        let (sender, inbox) = WatchEventInbox::bounded(roots.clone(), 4);
        let view = Arc::new(Mutex::new(LayeredSearchIndex::from_baseline(vec![
            old_entry,
        ])));
        let published_view = Arc::clone(&view);
        let events = Arc::new(Mutex::new(Vec::new()));
        let published_events = Arc::clone(&events);
        let handle = start_runtime_indexing_with_scanner(
            None,
            inbox,
            Box::new(scanner),
            Box::new(storage),
            RuntimeIndexingOptions {
                roots,
                policy: CoordinatorPolicy::new(Duration::ZERO, Duration::ZERO),
                initial_generation: 0,
            },
            move |event| {
                if let RuntimeIndexingEvent::DeltaCommitted(delta) = &event {
                    published_view.lock().unwrap().apply_delta(delta.clone());
                }
                published_events.lock().unwrap().push(event);
            },
        )
        .unwrap();

        assert_eq!(
            sender.try_send(IndexWatchEvent::Write(root.path().join("locked"))),
            crate::core::index_watcher::WatchSendOutcome::Queued
        );
        wait_until("permission failure degradation", || {
            events.lock().unwrap().iter().any(|event| {
                matches!(
                    event,
                    RuntimeIndexingEvent::Status(RuntimeIncrementalStatus {
                        degradation_code: Some(IndexDegradationCode::CalibrationFailed),
                        ..
                    })
                )
            })
        });
        assert_eq!(
            handle.handoff(),
            RuntimeIndexingHandoffOutcome::RecoveryRequired
        );

        let events = events.lock().unwrap();
        assert!(events
            .iter()
            .all(|event| !matches!(event, RuntimeIndexingEvent::DeltaCommitted(_))));
        assert!(events.iter().any(|event| matches!(
            event,
            RuntimeIndexingEvent::Status(RuntimeIncrementalStatus {
                dirty_roots: 1,
                degradation_code: Some(IndexDegradationCode::CalibrationFailed),
                ..
            })
        )));
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    event,
                    RuntimeIndexingEvent::BaselineRefreshRequired {
                        reason: BaselineRefreshReason::CalibrationFailed
                    }
                ))
                .count(),
            0
        );
        assert!(SqliteStorage::open(database_path)
            .unwrap()
            .committed_index_deltas_after(0)
            .unwrap()
            .is_empty());
        let view = view.lock().unwrap();
        assert_eq!(view.generation(), 0);
        assert_eq!(
            view.search_files(&QueryRequest::new("old", SearchMode::Normal), 20)
                .len(),
            1
        );
    }

    #[test]
    fn repeated_scan_failures_keep_root_dirty_without_refresh_event_loop() {
        let root = tempfile::tempdir().unwrap();
        let failure_result = || TargetedScanResult {
            failures: vec![IndexFailure {
                root: root.path().join("locked").to_string_lossy().into_owned(),
                message: "permission denied".to_owned(),
            }],
            ..TargetedScanResult::default()
        };
        let scanner = FixedScanner::returning_many([failure_result(), failure_result()]);
        let (sender, handle, events, _database_path) = start_fixed_scanner(&root, scanner);

        for (expected_failures, name) in ["first", "second"].into_iter().enumerate() {
            assert_eq!(
                sender.try_send(IndexWatchEvent::Write(root.path().join(name))),
                crate::core::index_watcher::WatchSendOutcome::Queued
            );
            wait_until("scripted scan failure", || {
                events
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|event| {
                        matches!(
                            event,
                            RuntimeIndexingEvent::Status(RuntimeIncrementalStatus {
                                degradation_code: Some(IndexDegradationCode::CalibrationFailed),
                                ..
                            })
                        )
                    })
                    .count()
                    > expected_failures
            });
        }
        assert_eq!(
            handle.handoff(),
            RuntimeIndexingHandoffOutcome::RecoveryRequired
        );
        let events = events.lock().unwrap();

        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    event,
                    RuntimeIndexingEvent::BaselineRefreshRequired {
                        reason: BaselineRefreshReason::CalibrationFailed
                    }
                ))
                .count(),
            0
        );
        let final_status = events
            .iter()
            .filter_map(|event| match event {
                RuntimeIndexingEvent::Status(status) => Some(status),
                _ => None,
            })
            .next_back()
            .unwrap();
        assert_eq!(final_status.dirty_roots, 1);
        assert_eq!(
            final_status.degradation_code,
            Some(IndexDegradationCode::CalibrationFailed)
        );
    }

    #[test]
    fn scan_io_error_marks_all_roots_dirty_and_requests_fallback() {
        let root = tempfile::tempdir().unwrap();
        let scanner = FixedScanner::failing(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "permission denied",
        ));
        let (sender, handle, events, database_path) = start_fixed_scanner(&root, scanner);

        assert_eq!(
            sender.try_send(IndexWatchEvent::Write(root.path().join("locked"))),
            crate::core::index_watcher::WatchSendOutcome::Queued
        );
        wait_until("scan io fallback", || {
            events.lock().unwrap().iter().any(|event| {
                matches!(
                    event,
                    RuntimeIndexingEvent::BaselineRefreshRequired {
                        reason: BaselineRefreshReason::CalibrationFailed
                    }
                )
            })
        });
        handle.stop();
        let events = events.lock().unwrap();

        assert!(events.iter().any(|event| matches!(
            event,
            RuntimeIndexingEvent::Status(RuntimeIncrementalStatus {
                dirty_roots: 1,
                degradation_code: Some(IndexDegradationCode::CalibrationFailed),
                ..
            })
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            RuntimeIndexingEvent::BaselineRefreshRequired {
                reason: BaselineRefreshReason::CalibrationFailed
            }
        )));
        assert!(SqliteStorage::open(database_path)
            .unwrap()
            .committed_index_deltas_after(0)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn stopping_during_scan_cancels_and_joins_within_budget() {
        struct CancellableScanner {
            entered: Arc<AtomicBool>,
            cancelled: Arc<AtomicBool>,
        }

        impl RuntimeBatchScanner for CancellableScanner {
            fn scan_batch_cancellable(
                &self,
                _batch: crate::core::index_update_coordinator::CoordinatorBatch,
                _journal: &dyn IndexJournalRepository,
                is_cancelled: &dyn Fn() -> bool,
            ) -> Result<TargetedScanResult, TargetedScanError> {
                self.entered.store(true, Ordering::Release);
                while !is_cancelled() {
                    thread::yield_now();
                }
                self.cancelled.store(true, Ordering::Release);
                Err(TargetedScanError::Cancelled)
            }
        }

        let root = tempfile::tempdir().unwrap();
        let entered = Arc::new(AtomicBool::new(false));
        let cancelled = Arc::new(AtomicBool::new(false));
        let scanner = CancellableScanner {
            entered: Arc::clone(&entered),
            cancelled: Arc::clone(&cancelled),
        };
        let (sender, handle, events, database_path) = start_fixed_scanner(&root, scanner);
        assert_eq!(
            sender.try_send(IndexWatchEvent::Write(root.path().join("busy"))),
            crate::core::index_watcher::WatchSendOutcome::Queued
        );
        let wait_started = Instant::now();
        while !entered.load(Ordering::Acquire) && wait_started.elapsed() < Duration::from_secs(1) {
            thread::yield_now();
        }
        assert!(entered.load(Ordering::Acquire));

        let stop_started = Instant::now();
        handle.stop();

        assert!(cancelled.load(Ordering::Acquire));
        assert!(stop_started.elapsed() <= Duration::from_millis(250));
        assert!(events
            .lock()
            .unwrap()
            .iter()
            .all(|event| !matches!(event, RuntimeIndexingEvent::DeltaCommitted(_))));
        assert!(SqliteStorage::open(database_path)
            .unwrap()
            .committed_index_deltas_after(0)
            .unwrap()
            .is_empty());
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
        wait_until("watcher channel overflow fallback", || {
            events.lock().unwrap().iter().any(|event| {
                matches!(
                    event,
                    RuntimeIndexingEvent::BaselineRefreshRequired {
                        reason: BaselineRefreshReason::DirtyRoots
                    }
                )
            })
        });
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
    fn coordinator_capacity_overflow_clears_individual_events_and_latches_fallback() {
        let root = tempfile::tempdir().unwrap();
        let storage = SqliteStorage::open(root.path().join("coordinator-capacity.sqlite")).unwrap();
        let baseline_id = storage.save_completed_index_batch(1, &[]).unwrap();
        storage
            .activate_baseline_and_clear_incremental_state(baseline_id, 0)
            .unwrap();
        let roots = vec![root.path().to_path_buf()];
        let (sender, inbox) = WatchEventInbox::bounded(roots.clone(), 9_000);
        for index in 0..8_193 {
            assert_eq!(
                sender.try_send(IndexWatchEvent::Write(
                    root.path().join(format!("event-{index}.md")),
                )),
                crate::core::index_watcher::WatchSendOutcome::Queued
            );
        }
        let events = Arc::new(Mutex::new(Vec::new()));
        let published = Arc::clone(&events);
        let handle = start_runtime_indexing_with_scanner(
            None,
            inbox,
            Box::new(FixedScanner::returning(TargetedScanResult::default())),
            Box::new(storage),
            RuntimeIndexingOptions {
                roots,
                policy: CoordinatorPolicy::new(Duration::from_secs(60), Duration::from_secs(60)),
                initial_generation: 0,
            },
            move |event| published.lock().unwrap().push(event),
        )
        .unwrap();

        wait_until("coordinator capacity fallback", || {
            events.lock().unwrap().iter().any(|event| {
                matches!(
                    event,
                    RuntimeIndexingEvent::BaselineRefreshRequired {
                        reason: BaselineRefreshReason::DirtyRoots
                    }
                )
            })
        });
        handle.stop();
        let events = events.lock().unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    event,
                    RuntimeIndexingEvent::BaselineRefreshRequired {
                        reason: BaselineRefreshReason::DirtyRoots
                    }
                ))
                .count(),
            1
        );
        assert!(events.iter().any(|event| matches!(
            event,
            RuntimeIndexingEvent::Status(RuntimeIncrementalStatus {
                pending_events: 0,
                degradation_code: Some(IndexDegradationCode::ChannelOverflow),
                ..
            })
        )));
    }

    #[test]
    fn continuous_producer_cannot_starve_runtime_service_stop() {
        let root = tempfile::tempdir().unwrap();
        let storage = SqliteStorage::open(root.path().join("bounded-stop.sqlite")).unwrap();
        let baseline_id = storage.save_completed_index_batch(1, &[]).unwrap();
        storage
            .activate_baseline_and_clear_incremental_state(baseline_id, 0)
            .unwrap();
        let roots = vec![root.path().to_path_buf()];
        let (sender, inbox) = WatchEventInbox::bounded(roots.clone(), 8_192);
        let events = Arc::new(Mutex::new(Vec::new()));
        let published = Arc::clone(&events);
        let handle = start_runtime_indexing_with_scanner(
            None,
            inbox,
            Box::new(FixedScanner::returning(TargetedScanResult::default())),
            Box::new(storage),
            RuntimeIndexingOptions {
                roots,
                policy: CoordinatorPolicy::new(Duration::from_secs(60), Duration::from_secs(60)),
                initial_generation: 0,
            },
            move |event| published.lock().unwrap().push(event),
        )
        .unwrap();
        let producing = Arc::new(AtomicBool::new(true));
        let producer_running = Arc::clone(&producing);
        let producer_root = root.path().to_path_buf();
        let producer = thread::spawn(move || {
            let mut index = 0_u64;
            while producer_running.load(Ordering::Acquire) {
                let _ = sender.try_send(IndexWatchEvent::Write(
                    producer_root.join(format!("event-{}.md", index % 16_384)),
                ));
                index = index.wrapping_add(1);
            }
        });
        wait_until("continuous producer status", || {
            events.lock().unwrap().iter().any(|event| {
                matches!(
                    event,
                    RuntimeIndexingEvent::Status(RuntimeIncrementalStatus {
                        pending_events: 1..,
                        ..
                    })
                )
            })
        });

        let started = Instant::now();
        handle.stop();
        let elapsed = started.elapsed();
        producing.store(false, Ordering::Release);
        producer.join().unwrap();

        assert!(
            elapsed <= Duration::from_millis(250),
            "stop took {elapsed:?}"
        );
        assert!(events.lock().unwrap().iter().all(|event| match event {
            RuntimeIndexingEvent::Status(status) => status.pending_events <= 8_192,
            _ => true,
        }));
    }

    #[test]
    fn watcher_channel_event_storm_is_bounded_marks_dirty_and_keeps_old_view_searchable() {
        let root = tempfile::tempdir().unwrap();
        let old_path = root.path().join("last-known-good.md");
        fs::write(&old_path, "last known good").unwrap();
        let old_entry =
            IndexedEntry::from_path_metadata(&old_path, root.path(), IndexedEntryKind::File);
        let database_path = root.path().join("watcher-storm.sqlite");
        let storage = SqliteStorage::open(database_path).unwrap();
        let baseline_id = storage
            .save_completed_index_batch(1, std::slice::from_ref(&old_entry))
            .unwrap();
        storage
            .activate_baseline_and_clear_incremental_state(baseline_id, 0)
            .unwrap();
        let roots = vec![root.path().to_path_buf()];
        let (sender, inbox) = WatchEventInbox::bounded(roots.clone(), 8_192);
        for index in 0..8_192 {
            assert_eq!(
                sender.try_send(IndexWatchEvent::Write(
                    root.path().join(format!("storm-{index}.md")),
                )),
                crate::core::index_watcher::WatchSendOutcome::Queued
            );
        }
        assert_eq!(
            sender.try_send(IndexWatchEvent::Write(root.path().join("storm-8192.md"))),
            crate::core::index_watcher::WatchSendOutcome::Overflowed
        );

        let view = Arc::new(Mutex::new(LayeredSearchIndex::from_baseline(vec![
            old_entry,
        ])));
        let published_view = Arc::clone(&view);
        let events = Arc::new(Mutex::new(Vec::new()));
        let published_events = Arc::clone(&events);
        let handle = start_runtime_indexing_with_scanner(
            None,
            inbox,
            Box::new(FixedScanner::returning(TargetedScanResult::default())),
            Box::new(storage),
            RuntimeIndexingOptions {
                roots,
                policy: CoordinatorPolicy::new(Duration::from_secs(60), Duration::from_secs(60)),
                initial_generation: 0,
            },
            move |event| {
                if let RuntimeIndexingEvent::DeltaCommitted(delta) = &event {
                    published_view.lock().unwrap().apply_delta(delta.clone());
                }
                published_events.lock().unwrap().push(event);
            },
        )
        .unwrap();

        wait_until("8193-event watcher fallback", || {
            events.lock().unwrap().iter().any(|event| {
                matches!(
                    event,
                    RuntimeIndexingEvent::Status(RuntimeIncrementalStatus {
                        dirty_roots: 1,
                        degradation_code: Some(IndexDegradationCode::ChannelOverflow),
                        ..
                    })
                )
            })
        });
        handle.stop();

        let events = events.lock().unwrap();
        assert!(events.iter().all(|event| match event {
            RuntimeIndexingEvent::Status(status) => status.pending_events <= 8_192,
            _ => true,
        }));
        let status = events
            .iter()
            .filter_map(|event| match event {
                RuntimeIndexingEvent::Status(status)
                    if status.degradation_code == Some(IndexDegradationCode::ChannelOverflow) =>
                {
                    Some(status)
                }
                _ => None,
            })
            .next_back()
            .unwrap();
        let old_view_results = view
            .lock()
            .unwrap()
            .search_files(
                &QueryRequest::new("last-known-good", SearchMode::Normal),
                20,
            )
            .len();
        println!(
            "QUICKFOX_EVENT_STORM events=8193 channel_capacity=8192 pending_events={} dirty_roots={} old_view_results={}",
            status.pending_events, status.dirty_roots, old_view_results
        );
        assert_eq!(old_view_results, 1);
    }

    #[test]
    fn handoff_drains_received_events_before_old_watcher_service_joins() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("before-standby.md");
        fs::write(&path, "handoff").unwrap();
        let storage = SqliteStorage::open(root.path().join("handoff-drain.sqlite")).unwrap();
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
        let (sender, inbox) = WatchEventInbox::bounded(roots.clone(), 16);
        assert_eq!(
            sender.try_send(IndexWatchEvent::Create(path)),
            crate::core::index_watcher::WatchSendOutcome::Queued
        );
        let handle = start_runtime_indexing_from_parts(
            None,
            inbox,
            TargetedIndexScanner::new(rules),
            Box::new(storage),
            RuntimeIndexingOptions {
                roots,
                policy: CoordinatorPolicy::new(Duration::from_secs(60), Duration::from_secs(60)),
                initial_generation: 0,
            },
            |_| {},
        )
        .unwrap();

        let handoff = handle.handoff_with_generation();

        let storage = SqliteStorage::open(root.path().join("handoff-drain.sqlite")).unwrap();
        let deltas = storage.committed_index_deltas_after(0).unwrap();
        assert_eq!(deltas.len(), 1);
        assert_eq!(deltas[0].upserts[0].name, "before-standby.md");
        assert_eq!(handoff.outcome, RuntimeIndexingHandoffOutcome::Clean);
        assert_eq!(handoff.last_committed_generation, 1);
    }

    #[test]
    fn revision_fence_pauses_old_service_and_resume_drains_later_events() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("before-fence.md");
        let second = root.path().join("after-fence.md");
        fs::write(&first, "first").unwrap();
        fs::write(&second, "second").unwrap();
        let database_path = root.path().join("revision-pause.sqlite");
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
        let (sender, inbox) = WatchEventInbox::bounded(roots.clone(), 16);
        assert_eq!(
            sender.try_send(IndexWatchEvent::Create(first)),
            crate::core::index_watcher::WatchSendOutcome::Queued
        );
        let mut handle = start_runtime_indexing_from_parts(
            None,
            inbox,
            TargetedIndexScanner::new(rules),
            Box::new(storage),
            RuntimeIndexingOptions {
                roots,
                policy: CoordinatorPolicy::new(Duration::from_secs(60), Duration::from_secs(60)),
                initial_generation: 0,
            },
            |_| {},
        )
        .unwrap();

        assert_eq!(handle.fence().unwrap(), 1);
        assert_eq!(
            sender.try_send(IndexWatchEvent::Create(second)),
            crate::core::index_watcher::WatchSendOutcome::Queued
        );
        assert_eq!(
            SqliteStorage::open(database_path.clone())
                .unwrap()
                .highest_committed_generation()
                .unwrap(),
            1
        );
        handle.resume().unwrap();
        let handoff = handle.handoff_with_generation();
        assert_eq!(handoff.last_committed_generation, 2);
        assert_eq!(
            SqliteStorage::open(database_path)
                .unwrap()
                .highest_committed_generation()
                .unwrap(),
            2
        );
    }

    #[test]
    fn handoff_reports_worker_panic_as_recovery_required() {
        struct PanickingScanner;

        impl RuntimeBatchScanner for PanickingScanner {
            fn scan_batch_cancellable(
                &self,
                _batch: crate::core::index_update_coordinator::CoordinatorBatch,
                _journal: &dyn IndexJournalRepository,
                _is_cancelled: &dyn Fn() -> bool,
            ) -> Result<TargetedScanResult, TargetedScanError> {
                panic!("injected handoff worker panic")
            }
        }

        let root = tempfile::tempdir().unwrap();
        let storage = SqliteStorage::open(root.path().join("handoff-panic.sqlite")).unwrap();
        let baseline_id = storage.save_completed_index_batch(1, &[]).unwrap();
        storage
            .activate_baseline_and_clear_incremental_state(baseline_id, 0)
            .unwrap();
        let roots = vec![root.path().to_path_buf()];
        let (sender, inbox) = WatchEventInbox::bounded(roots.clone(), 1);
        assert_eq!(
            sender.try_send(IndexWatchEvent::Write(root.path().join("panic.md"))),
            crate::core::index_watcher::WatchSendOutcome::Queued
        );
        let handle = start_runtime_indexing_with_scanner(
            None,
            inbox,
            Box::new(PanickingScanner),
            Box::new(storage),
            RuntimeIndexingOptions {
                roots,
                policy: CoordinatorPolicy::new(Duration::from_secs(60), Duration::from_secs(60)),
                initial_generation: 0,
            },
            |_| {},
        )
        .unwrap();

        assert_eq!(
            handle.handoff(),
            RuntimeIndexingHandoffOutcome::RecoveryRequired
        );
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
