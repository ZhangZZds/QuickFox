//! Runtime index watcher boundary.

use notify::event::{ModifyKind, RenameMode};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{
    self, Receiver, RecvTimeoutError, Sender, SyncSender, TryRecvError, TrySendError,
};
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
    Overflowed,
    Disconnected,
}

#[derive(Debug, Default)]
struct WatchInboxState {
    dirty_roots: BTreeSet<PathBuf>,
    latest_failure: Option<WatcherFailure>,
}

#[derive(Debug, Clone)]
pub struct WatchEventSender {
    sender: SyncSender<IndexWatchEvent>,
    watched_roots: Arc<Vec<PathBuf>>,
    state: Arc<Mutex<WatchInboxState>>,
}

impl WatchEventSender {
    pub fn try_send(&self, event: IndexWatchEvent) -> WatchSendOutcome {
        match self.sender.try_send(event) {
            Ok(()) => WatchSendOutcome::Queued,
            Err(TrySendError::Full(event)) => {
                self.mark_event_roots_dirty(&event);
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
        let (sender, receiver) = mpsc::sync_channel(capacity);
        let state = Arc::new(Mutex::new(WatchInboxState::default()));
        (
            WatchEventSender {
                sender,
                watched_roots: Arc::new(watched_roots),
                state: Arc::clone(&state),
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatcherFailure {
    pub root: PathBuf,
    pub message: String,
    pub requires_background_refresh: bool,
}

impl WatcherFailure {
    pub fn new(root: PathBuf, error: impl Into<String>) -> Self {
        let mut error = error.into();
        if !root.as_os_str().is_empty() {
            error = error.replace(root.to_string_lossy().as_ref(), "watched root");
        }
        Self {
            root,
            message: format!("watcher failed: {error}; falling back to background refresh"),
            requires_background_refresh: true,
        }
    }
}

#[derive(Debug)]
pub struct RuntimeIndexWatcher {
    _watcher: RecommendedWatcher,
    watched_roots: Vec<PathBuf>,
    inbox: Option<WatchEventInbox>,
}

impl RuntimeIndexWatcher {
    pub fn watch_roots(
        roots: Vec<PathBuf>,
        _legacy_sender: Sender<Result<IndexWatchEvent, WatcherFailure>>,
    ) -> Result<Self, WatcherFailure> {
        let (callback_sender, inbox) =
            WatchEventInbox::bounded(roots.clone(), DEFAULT_WATCH_CHANNEL_CAPACITY);
        let mut watcher =
            notify::recommended_watcher(move |result: notify::Result<Event>| match result {
                Ok(event) => dispatch_notify_event(event, &callback_sender),
                Err(error) => {
                    callback_sender
                        .record_failure(WatcherFailure::new(PathBuf::new(), error.to_string()));
                }
            })
            .map_err(|error| WatcherFailure::new(PathBuf::new(), error.to_string()))?;

        for root in &roots {
            watcher
                .watch(root, RecursiveMode::Recursive)
                .map_err(|error| WatcherFailure::new(root.clone(), error.to_string()))?;
        }

        Ok(Self {
            _watcher: watcher,
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

fn dispatch_notify_event(event: Event, sender: &WatchEventSender) {
    if event.need_rescan() {
        sender.mark_paths_dirty(event.paths.iter().map(PathBuf::as_path), true);
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
        assert!(failure.message.contains("too many open files"));
        assert!(!failure.message.contains(root.to_string_lossy().as_ref()));
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
    fn runtime_watcher_owns_an_inbox_until_it_is_taken() {
        let root = tempfile::tempdir().unwrap();
        let (legacy_sender, _legacy_receiver) = std::sync::mpsc::channel();
        let mut watcher =
            RuntimeIndexWatcher::watch_roots(vec![root.path().to_path_buf()], legacy_sender)
                .unwrap();

        assert!(watcher.take_inbox().is_some());
        assert!(watcher.take_inbox().is_none());
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
