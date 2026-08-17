//! Runtime index watcher boundary.

use crate::core::index_entry::IndexDegradationCode;
use notify::event::{ModifyKind, RenameMode};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const DEFAULT_WATCH_CHANNEL_CAPACITY: usize = 8_192;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexWatchEvent {
    Create(PathBuf),
    Write(PathBuf),
    Remove(PathBuf),
    Rename { from: PathBuf, to: PathBuf },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchSendOutcome {
    Queued,
    Filtered,
    Overflowed,
    Disconnected,
}

#[derive(Debug, Default)]
struct WatchInboxState {
    dirty_roots: BTreeSet<PathBuf>,
    latest_failure: Option<WatcherFailure>,
    latest_degradation: Option<IndexDegradationCode>,
}

#[derive(Clone)]
pub struct WatchEventSender {
    sender: SyncSender<IndexWatchEvent>,
    watched_roots: Arc<Vec<PathBuf>>,
    state: Arc<Mutex<WatchInboxState>>,
    path_filter: Arc<dyn Fn(&Path) -> bool + Send + Sync>,
}

impl WatchEventSender {
    pub fn try_send(&self, event: IndexWatchEvent) -> WatchSendOutcome {
        let accepted = event_paths(&event)
            .into_iter()
            .filter(|path| !path.as_os_str().is_empty())
            .any(|path| (self.path_filter)(path));
        if !accepted {
            return WatchSendOutcome::Filtered;
        }
        match self.sender.try_send(event) {
            Ok(()) => WatchSendOutcome::Queued,
            Err(TrySendError::Full(event)) => {
                self.mark_event_roots_dirty(&event);
                self.record_degradation(IndexDegradationCode::ChannelOverflow);
                WatchSendOutcome::Overflowed
            }
            Err(TrySendError::Disconnected(_)) => WatchSendOutcome::Disconnected,
        }
    }

    pub fn record_failure(&self, failure: WatcherFailure) {
        let mut state = self.lock_state();
        if failure.root.as_os_str().is_empty() {
            state.dirty_roots.extend(self.watched_roots.iter().cloned());
        } else {
            state.dirty_roots.insert(failure.root.clone());
        }
        state.latest_failure = Some(failure);
        state.latest_degradation = Some(IndexDegradationCode::WatcherRuntimeFailed);
    }

    fn record_degradation(&self, code: IndexDegradationCode) {
        self.lock_state().latest_degradation = Some(code);
    }

    fn mark_event_roots_dirty(&self, event: &IndexWatchEvent) {
        self.mark_paths_dirty(event_paths(event).into_iter(), false);
    }

    fn mark_paths_dirty<'a>(
        &self,
        paths: impl Iterator<Item = &'a Path>,
        dirty_all_if_unmatched: bool,
    ) {
        let mut state = self.lock_state();
        let mut matched_root = false;
        for path in paths.filter(|path| !path.as_os_str().is_empty()) {
            if let Some(root) = longest_matching_root(path, &self.watched_roots) {
                state.dirty_roots.insert(root.clone());
                matched_root = true;
            }
        }
        if !matched_root && dirty_all_if_unmatched {
            state.dirty_roots.extend(self.watched_roots.iter().cloned());
        }
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, WatchInboxState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[derive(Debug)]
pub struct WatchEventInbox {
    receiver: Receiver<IndexWatchEvent>,
    state: Arc<Mutex<WatchInboxState>>,
}

impl WatchEventInbox {
    pub fn bounded(watched_roots: Vec<PathBuf>, capacity: usize) -> (WatchEventSender, Self) {
        Self::bounded_filtered(watched_roots, capacity, Arc::new(|_| true))
    }

    pub fn bounded_filtered(
        watched_roots: Vec<PathBuf>,
        capacity: usize,
        path_filter: Arc<dyn Fn(&Path) -> bool + Send + Sync>,
    ) -> (WatchEventSender, Self) {
        let (sender, receiver) = mpsc::sync_channel(capacity);
        let state = Arc::new(Mutex::new(WatchInboxState::default()));
        (
            WatchEventSender {
                sender,
                watched_roots: Arc::new(watched_roots),
                state: Arc::clone(&state),
                path_filter,
            },
            Self { receiver, state },
        )
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<IndexWatchEvent, RecvTimeoutError> {
        self.receiver.recv_timeout(timeout)
    }

    pub fn try_recv(&self) -> Result<IndexWatchEvent, TryRecvError> {
        self.receiver.try_recv()
    }

    pub fn take_dirty_roots(&self) -> BTreeSet<PathBuf> {
        std::mem::take(&mut self.lock_state().dirty_roots)
    }

    pub fn take_failure(&self) -> Option<WatcherFailure> {
        self.lock_state().latest_failure.take()
    }

    pub fn take_degradation_code(&self) -> Option<IndexDegradationCode> {
        self.lock_state().latest_degradation.take()
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, WatchInboxState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn event_paths(event: &IndexWatchEvent) -> [&Path; 2] {
    match event {
        IndexWatchEvent::Create(path)
        | IndexWatchEvent::Write(path)
        | IndexWatchEvent::Remove(path) => [path.as_path(), Path::new("")],
        IndexWatchEvent::Rename { from, to } => [from.as_path(), to.as_path()],
    }
}

fn longest_matching_root<'a>(path: &Path, roots: &'a [PathBuf]) -> Option<&'a PathBuf> {
    roots
        .iter()
        .filter(|root| path.starts_with(root))
        .max_by_key(|root| root.components().count())
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IndexUpdateBatch {
    pub changed_paths: Vec<PathBuf>,
    pub removed_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, Default)]
pub struct IndexEventBatcher {
    changed_paths: BTreeSet<PathBuf>,
    removed_paths: BTreeSet<PathBuf>,
}

impl IndexEventBatcher {
    pub fn push(&mut self, event: IndexWatchEvent) {
        match event {
            IndexWatchEvent::Create(path) | IndexWatchEvent::Write(path) => {
                self.removed_paths.remove(&path);
                self.changed_paths.insert(path);
            }
            IndexWatchEvent::Remove(path) => {
                self.changed_paths.remove(&path);
                self.removed_paths.insert(path);
            }
            IndexWatchEvent::Rename { from, to } => {
                self.changed_paths.remove(&from);
                self.removed_paths.insert(from);
                self.removed_paths.remove(&to);
                self.changed_paths.insert(to);
            }
        }
    }

    pub fn drain_batch(&mut self) -> IndexUpdateBatch {
        IndexUpdateBatch {
            changed_paths: std::mem::take(&mut self.changed_paths)
                .into_iter()
                .collect(),
            removed_paths: std::mem::take(&mut self.removed_paths)
                .into_iter()
                .collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.changed_paths.is_empty() && self.removed_paths.is_empty()
    }

    pub fn len(&self) -> usize {
        self.changed_paths
            .len()
            .saturating_add(self.removed_paths.len())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatcherFailure {
    pub root: PathBuf,
    pub message: String,
    pub(crate) diagnostic: String,
    pub requires_background_refresh: bool,
}

impl WatcherFailure {
    pub fn new(root: PathBuf, diagnostic: impl Into<String>) -> Self {
        Self {
            root,
            message: "watcher failed; falling back to background refresh".to_owned(),
            diagnostic: diagnostic.into(),
            requires_background_refresh: true,
        }
    }
}

#[derive(Debug)]
pub struct RuntimeIndexWatcher {
    _watchers: Vec<RecommendedWatcher>,
    _registration_probe: tempfile::TempDir,
    watched_roots: Vec<PathBuf>,
    inbox: Option<WatchEventInbox>,
}

impl RuntimeIndexWatcher {
    pub fn watch_roots(roots: Vec<PathBuf>) -> Result<Self, WatcherFailure> {
        Self::watch_roots_with_filter(roots, Arc::new(|_| true))
    }

    pub fn watch_roots_with_filter(
        roots: Vec<PathBuf>,
        path_filter: Arc<dyn Fn(&Path) -> bool + Send + Sync>,
    ) -> Result<Self, WatcherFailure> {
        Self::watch_roots_with_probe_parent(roots, None, path_filter)
    }

    fn watch_roots_with_probe_parent(
        roots: Vec<PathBuf>,
        probe_parent: Option<&Path>,
        path_filter: Arc<dyn Fn(&Path) -> bool + Send + Sync>,
    ) -> Result<Self, WatcherFailure> {
        let (callback_sender, inbox) = WatchEventInbox::bounded_filtered(
            roots.clone(),
            DEFAULT_WATCH_CHANNEL_CAPACITY,
            path_filter,
        );
        let mut builder = tempfile::Builder::new();
        builder.prefix("quickfox-watcher-probe-");
        let registration_probe = match probe_parent {
            Some(parent) => builder.tempdir_in(parent),
            None => builder.tempdir(),
        }
        .map_err(|error| {
            WatcherFailure::new(
                probe_parent.unwrap_or_else(|| Path::new("")).to_path_buf(),
                error.to_string(),
            )
        })?;
        let mut watchers = Vec::with_capacity(roots.len());
        let mut latest_failure = None;
        for (root_index, root) in roots.iter().enumerate() {
            let owned_probe_path = registration_probe.path().join(format!("root-{root_index}"));
            if let Err(error) = fs::create_dir(&owned_probe_path) {
                let failure = WatcherFailure::new(root.clone(), error.to_string());
                callback_sender.record_failure(failure.clone());
                latest_failure = Some(failure);
                continue;
            }
            let probe_path = match owned_probe_path.canonicalize() {
                Ok(path) => path,
                Err(error) => {
                    let failure = WatcherFailure::new(root.clone(), error.to_string());
                    callback_sender.record_failure(failure.clone());
                    latest_failure = Some(failure);
                    continue;
                }
            };
            let (registration_ack, registration_ack_receiver) = mpsc::channel();
            let callback_probe = probe_path.clone();
            let root_sender = callback_sender.clone();
            let mut watcher =
                match notify::recommended_watcher(move |result: notify::Result<Event>| {
                    if result.as_ref().is_ok_and(|event| {
                        event
                            .paths
                            .iter()
                            .any(|path| path.starts_with(&callback_probe))
                    }) {
                        let _ = registration_ack.send(());
                        return;
                    }
                    dispatch_notify_result(result, &root_sender);
                }) {
                    Ok(watcher) => watcher,
                    Err(error) => {
                        let failure = WatcherFailure::new(root.clone(), error.to_string());
                        callback_sender.record_failure(failure.clone());
                        latest_failure = Some(failure);
                        continue;
                    }
                };

            // Each root owns an independent native watcher. A registration or runtime failure on
            // one volume therefore cannot tear down event delivery for another volume.
            if let Err(error) = watcher.watch(root, RecursiveMode::Recursive) {
                let failure = WatcherFailure::new(root.clone(), error.to_string());
                callback_sender.record_failure(failure.clone());
                latest_failure = Some(failure);
                continue;
            }
            if let Err(error) = watcher.watch(&probe_path, RecursiveMode::Recursive) {
                let failure = WatcherFailure::new(root.clone(), error.to_string());
                callback_sender.record_failure(failure.clone());
                latest_failure = Some(failure);
                continue;
            }

            // The app-owned probe is registered on the same native backend as this root. Returning
            // requires observing its event, so startup cannot silently lose immediate mutations.
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            let mut attempt = 0_u64;
            let acknowledged = loop {
                let probe_file = owned_probe_path.join(format!("ack-{attempt}"));
                if let Err(error) = fs::write(&probe_file, attempt.to_le_bytes()) {
                    let failure = WatcherFailure::new(root.clone(), error.to_string());
                    callback_sender.record_failure(failure.clone());
                    latest_failure = Some(failure);
                    break false;
                }
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() {
                    let failure = WatcherFailure::new(
                        root.clone(),
                        "native watcher registration acknowledgement timed out",
                    );
                    callback_sender.record_failure(failure.clone());
                    latest_failure = Some(failure);
                    break false;
                }
                match registration_ack_receiver
                    .recv_timeout(remaining.min(Duration::from_millis(50)))
                {
                    Ok(()) => break true,
                    Err(RecvTimeoutError::Timeout) => attempt = attempt.saturating_add(1),
                    Err(RecvTimeoutError::Disconnected) => {
                        let failure = WatcherFailure::new(
                            root.clone(),
                            "native watcher registration acknowledgement disconnected",
                        );
                        callback_sender.record_failure(failure.clone());
                        latest_failure = Some(failure);
                        break false;
                    }
                }
            };
            if acknowledged {
                watchers.push(watcher);
            }
        }

        if watchers.is_empty() && !roots.is_empty() {
            return Err(latest_failure.unwrap_or_else(|| {
                WatcherFailure::new(PathBuf::new(), "no native watcher could be initialized")
            }));
        }

        Ok(Self {
            _watchers: watchers,
            _registration_probe: registration_probe,
            watched_roots: roots,
            inbox: Some(inbox),
        })
    }

    pub fn watched_roots(&self) -> &[PathBuf] {
        &self.watched_roots
    }

    pub fn take_inbox(&mut self) -> Option<WatchEventInbox> {
        self.inbox.take()
    }
}

fn dispatch_notify_result(result: notify::Result<Event>, sender: &WatchEventSender) {
    match result {
        Ok(event) => dispatch_notify_event(event, sender),
        Err(error) => {
            let root = error
                .paths
                .iter()
                .filter_map(|path| longest_matching_root(path, &sender.watched_roots))
                .max_by_key(|root| root.components().count())
                .cloned()
                .unwrap_or_default();
            sender.mark_paths_dirty(error.paths.iter().map(PathBuf::as_path), true);
            sender.record_failure(WatcherFailure::new(root, error.to_string()));
        }
    }
}

fn dispatch_notify_event(event: Event, sender: &WatchEventSender) {
    let uncertain_rename = matches!(
        event.kind,
        EventKind::Modify(ModifyKind::Name(RenameMode::Any | RenameMode::Other))
    ) || matches!(
        event.kind,
        EventKind::Modify(ModifyKind::Name(RenameMode::Both)) if event.paths.len() < 2
    );
    if event.need_rescan() || uncertain_rename {
        sender.mark_paths_dirty(event.paths.iter().map(PathBuf::as_path), true);
        sender.record_degradation(IndexDegradationCode::WatcherOverflow);
        return;
    }

    for event in events_from_notify(event) {
        let _ = sender.try_send(event);
    }
}

pub fn events_from_notify(event: Event) -> Vec<IndexWatchEvent> {
    match event.kind {
        EventKind::Create(_) => event
            .paths
            .into_iter()
            .map(IndexWatchEvent::Create)
            .collect(),
        EventKind::Remove(_) => event
            .paths
            .into_iter()
            .map(IndexWatchEvent::Remove)
            .collect(),
        EventKind::Modify(ModifyKind::Name(RenameMode::Both)) if event.paths.len() >= 2 => {
            vec![IndexWatchEvent::Rename {
                from: event.paths[0].clone(),
                to: event.paths[1].clone(),
            }]
        }
        EventKind::Modify(ModifyKind::Name(RenameMode::From)) => event
            .paths
            .into_iter()
            .map(IndexWatchEvent::Remove)
            .collect(),
        EventKind::Modify(ModifyKind::Name(RenameMode::To)) => event
            .paths
            .into_iter()
            .map(IndexWatchEvent::Create)
            .collect(),
        EventKind::Modify(ModifyKind::Name(_)) => Vec::new(),
        EventKind::Modify(_) => event
            .paths
            .into_iter()
            .map(IndexWatchEvent::Write)
            .collect(),
        _ => Vec::new(),
    }
}

pub fn roots_from_entries(entries: &[crate::core::index_entry::IndexedEntry]) -> Vec<PathBuf> {
    entries
        .iter()
        .filter_map(|entry| {
            if entry.root.is_empty() {
                None
            } else {
                Some(Path::new(&entry.root).to_path_buf())
            }
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn debounce_queue_merges_write_create_remove_and_rename_into_one_batch() {
        let root = PathBuf::from("/tmp/quickfox-watch");
        let changed = root.join("notes.md");
        let removed = root.join("old.md");
        let renamed_from = root.join("draft.md");
        let renamed_to = root.join("final.md");
        let mut queue = IndexEventBatcher::default();

        queue.push(IndexWatchEvent::Write(changed.clone()));
        queue.push(IndexWatchEvent::Create(changed.clone()));
        queue.push(IndexWatchEvent::Remove(removed.clone()));
        queue.push(IndexWatchEvent::Rename {
            from: renamed_from.clone(),
            to: renamed_to.clone(),
        });

        let batch = queue.drain_batch();

        assert_eq!(batch.changed_paths, vec![renamed_to, changed]);
        assert_eq!(
            batch.removed_paths,
            vec![renamed_from, old_path(root, "old.md")]
        );
    }

    #[test]
    fn watcher_failure_status_requests_background_refresh_fallback() {
        let root = PathBuf::from("/tmp/quickfox-watch");
        let failure = WatcherFailure::new(root.clone(), "too many open files".to_owned());

        assert!(failure.requires_background_refresh);
        assert!(failure.message.contains("watcher failed"));
        assert!(failure.diagnostic.contains("too many open files"));
        assert!(!failure.message.contains(root.to_string_lossy().as_ref()));
    }

    #[test]
    fn runtime_notify_error_keeps_private_path_out_of_message_and_dirties_matching_root() {
        let private_root = PathBuf::from("/Users/private/QuickFox");
        let private_path = private_root.join("secret.txt");
        let unrelated_root = PathBuf::from("/Volumes/other");
        let (sender, inbox) =
            WatchEventInbox::bounded(vec![unrelated_root, private_root.clone()], 1);
        let error = notify::Error::generic("permission denied").add_path(private_path.clone());

        dispatch_notify_result(Err(error), &sender);

        let failure = inbox.take_failure().expect("runtime failure retained");
        assert!(!failure
            .message
            .contains(private_path.to_string_lossy().as_ref()));
        assert!(failure.diagnostic.contains("secret.txt"));
        assert_eq!(inbox.take_dirty_roots(), BTreeSet::from([private_root]));
        assert_eq!(
            inbox.take_degradation_code(),
            Some(IndexDegradationCode::WatcherRuntimeFailed)
        );
    }

    #[test]
    fn watcher_inbox_retains_receiver_and_marks_overflow_without_blocking() {
        let root = PathBuf::from("/tmp/quickfox-watch");
        let nested_root = root.join("workspace");
        let first_path = nested_root.join("first.txt");
        let second_path = nested_root.join("second.txt");
        let (sender, inbox) = WatchEventInbox::bounded(vec![root, nested_root.clone()], 1);

        assert_eq!(
            sender.try_send(IndexWatchEvent::Create(first_path.clone())),
            WatchSendOutcome::Queued
        );
        assert_eq!(
            sender.try_send(IndexWatchEvent::Write(second_path)),
            WatchSendOutcome::Overflowed
        );
        assert_eq!(
            inbox.recv_timeout(std::time::Duration::from_millis(10)),
            Ok(IndexWatchEvent::Create(first_path))
        );
        assert_eq!(inbox.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty));
        assert_eq!(
            inbox.take_dirty_roots(),
            BTreeSet::from([nested_root.clone()])
        );

        sender.record_failure(WatcherFailure::new(
            nested_root.clone(),
            "backend unavailable",
        ));
        let failure = inbox.take_failure().expect("failure should be retained");
        assert_eq!(failure.root, nested_root);
        assert!(inbox.take_failure().is_none());
    }

    #[test]
    fn filtered_events_are_dropped_before_the_bounded_channel_can_overflow() {
        let root = PathBuf::from("/Users/example");
        let excluded = root.join("Library/Application Support/QuickFox/quickfox.sqlite");
        let included = root.join("workspace/cann/new.txt");
        let (sender, inbox) = WatchEventInbox::bounded_filtered(
            vec![root],
            1,
            Arc::new(|path| !path.starts_with("/Users/example/Library")),
        );

        assert_eq!(
            sender.try_send(IndexWatchEvent::Write(excluded)),
            WatchSendOutcome::Filtered
        );
        assert_eq!(
            sender.try_send(IndexWatchEvent::Create(included.clone())),
            WatchSendOutcome::Queued
        );
        assert_eq!(
            inbox.recv_timeout(Duration::from_millis(10)),
            Ok(IndexWatchEvent::Create(included))
        );
        assert!(inbox.take_dirty_roots().is_empty());
        assert!(inbox.take_degradation_code().is_none());
    }

    #[test]
    fn runtime_watcher_owns_an_inbox_until_it_is_taken() {
        let root = tempfile::tempdir().unwrap();
        let mut watcher =
            RuntimeIndexWatcher::watch_roots(vec![root.path().to_path_buf()]).unwrap();

        assert!(watcher.take_inbox().is_some());
        assert!(watcher.take_inbox().is_none());
    }

    #[test]
    fn registration_probe_lifetime_matches_native_watcher() {
        let root = tempfile::tempdir().unwrap();
        let started = std::time::Instant::now();
        let watcher =
            RuntimeIndexWatcher::watch_roots(vec![root.path().canonicalize().unwrap()]).unwrap();
        let probe_path = watcher._registration_probe.path().to_path_buf();

        assert!(probe_path.is_dir());
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "quiet root registration exceeded its bounded acknowledgement window"
        );
        drop(watcher);
        assert!(!probe_path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn predictable_probe_symlink_cannot_redirect_cleanup_to_victim() {
        use std::os::unix::fs::symlink;

        let workspace = tempfile::tempdir().unwrap();
        let probe_parent = workspace.path().join("probes");
        let victim = workspace.path().join("victim");
        let watched_root = workspace.path().join("watched");
        fs::create_dir_all(&probe_parent).unwrap();
        fs::create_dir_all(&victim).unwrap();
        fs::create_dir_all(&watched_root).unwrap();
        let victim_marker = victim.join("must-survive.txt");
        fs::write(&victim_marker, "safe").unwrap();
        let predictable =
            probe_parent.join(format!("quickfox-watcher-probe-{}-0", std::process::id()));
        symlink(&victim, &predictable).unwrap();

        let watcher = RuntimeIndexWatcher::watch_roots_with_probe_parent(
            vec![watched_root.canonicalize().unwrap()],
            Some(&probe_parent),
            Arc::new(|_| true),
        )
        .unwrap();
        let owned_probe = watcher._registration_probe.path().to_path_buf();

        assert_ne!(owned_probe, predictable);
        drop(watcher);
        assert!(victim_marker.is_file());
        assert!(predictable.is_symlink());
    }

    #[test]
    fn watch_roots_returns_only_after_immediate_mutation_is_observable() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let created = root.join("immediate.txt");
        let mut watcher = RuntimeIndexWatcher::watch_roots(vec![root]).unwrap();
        fs::write(&created, "registered").unwrap();
        let inbox = watcher.take_inbox().unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let observed = loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                break false;
            }
            match inbox.recv_timeout(remaining) {
                Ok(IndexWatchEvent::Create(path) | IndexWatchEvent::Write(path))
                    if path == created || created.starts_with(&path) =>
                {
                    break true;
                }
                Ok(_) => {}
                Err(_) => break false,
            }
        };

        assert!(observed, "immediate post-registration mutation was missed");
    }

    #[test]
    fn one_unavailable_root_does_not_disable_another_native_watcher() {
        let available = tempfile::tempdir().unwrap();
        let available = available.path().canonicalize().unwrap();
        let unavailable = available.join("offline-volume");
        let created = available.join("still-observed.txt");
        let mut watcher =
            RuntimeIndexWatcher::watch_roots(vec![available.clone(), unavailable.clone()]).unwrap();
        let inbox = watcher.take_inbox().unwrap();
        let failure = inbox
            .take_failure()
            .expect("offline root should be reported");
        assert_eq!(failure.root, unavailable);

        fs::write(&created, "observed").unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let observed = loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                break false;
            }
            match inbox.recv_timeout(remaining) {
                Ok(IndexWatchEvent::Create(path) | IndexWatchEvent::Write(path))
                    if path == created || created.starts_with(&path) =>
                {
                    break true;
                }
                Ok(_) => {}
                Err(_) => break false,
            }
        };

        assert!(observed, "available root stopped after another root failed");
    }

    #[test]
    fn platform_rescan_marks_all_watched_roots_dirty() {
        let first_root = PathBuf::from("/tmp/quickfox-first");
        let second_root = PathBuf::from("/tmp/quickfox-second");
        let (sender, inbox) =
            WatchEventInbox::bounded(vec![first_root.clone(), second_root.clone()], 1);
        let event = Event::new(EventKind::Other).set_flag(notify::event::Flag::Rescan);

        dispatch_notify_event(event, &sender);

        assert_eq!(
            inbox.take_dirty_roots(),
            BTreeSet::from([first_root, second_root])
        );
        assert_eq!(
            inbox.take_degradation_code(),
            Some(IndexDegradationCode::WatcherOverflow)
        );
    }

    #[test]
    fn rename_mode_from_becomes_remove() {
        let path = PathBuf::from("/tmp/quickfox-watch/old.txt");
        let event = Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::From)))
            .add_path(path.clone());

        assert_eq!(
            events_from_notify(event),
            vec![IndexWatchEvent::Remove(path)]
        );
    }

    #[test]
    fn rename_mode_to_becomes_create() {
        let path = PathBuf::from("/tmp/quickfox-watch/new.txt");
        let event =
            Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::To))).add_path(path.clone());

        assert_eq!(
            events_from_notify(event),
            vec![IndexWatchEvent::Create(path)]
        );
    }

    #[test]
    fn rename_mode_any_marks_the_matching_root_dirty_without_queuing_a_write() {
        let root = PathBuf::from("/tmp/quickfox-watch");
        let path = root.join("uncertain.txt");
        let (sender, inbox) = WatchEventInbox::bounded(vec![root.clone()], 1);
        let event = Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::Any))).add_path(path);

        dispatch_notify_event(event, &sender);

        assert_eq!(inbox.try_recv(), Err(TryRecvError::Empty));
        assert_eq!(inbox.take_dirty_roots(), BTreeSet::from([root]));
    }

    #[test]
    fn overflowing_unwatched_path_does_not_dirty_unrelated_roots() {
        let root = PathBuf::from("/tmp/quickfox-watch");
        let (sender, inbox) = WatchEventInbox::bounded(vec![root], 1);
        assert_eq!(
            sender.try_send(IndexWatchEvent::Create(PathBuf::from("/tmp/queued.txt"))),
            WatchSendOutcome::Queued
        );

        assert_eq!(
            sender.try_send(IndexWatchEvent::Write(PathBuf::from("/other/file.txt"))),
            WatchSendOutcome::Overflowed
        );
        assert!(inbox.take_dirty_roots().is_empty());
    }

    fn old_path(root: PathBuf, name: &str) -> PathBuf {
        root.join(name)
    }
}
