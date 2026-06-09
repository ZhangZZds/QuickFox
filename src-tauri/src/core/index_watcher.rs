//! Runtime index watcher boundary.

use notify::event::{ModifyKind, RenameMode};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexWatchEvent {
    Create(PathBuf),
    Write(PathBuf),
    Remove(PathBuf),
    Rename { from: PathBuf, to: PathBuf },
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
        let error = error.into();
        Self {
            root: root.clone(),
            message: format!(
                "watcher failed for {}: {error}; falling back to background refresh",
                root.display()
            ),
            requires_background_refresh: true,
        }
    }
}

#[derive(Debug)]
pub struct RuntimeIndexWatcher {
    _watcher: RecommendedWatcher,
    watched_roots: Vec<PathBuf>,
}

impl RuntimeIndexWatcher {
    pub fn watch_roots(
        roots: Vec<PathBuf>,
        sender: Sender<Result<IndexWatchEvent, WatcherFailure>>,
    ) -> Result<Self, WatcherFailure> {
        let callback_sender = sender.clone();
        let mut watcher =
            notify::recommended_watcher(move |result: notify::Result<Event>| match result {
                Ok(event) => {
                    for event in events_from_notify(event) {
                        let _ = callback_sender.send(Ok(event));
                    }
                }
                Err(error) => {
                    let _ = callback_sender
                        .send(Err(WatcherFailure::new(PathBuf::new(), error.to_string())));
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
        })
    }

    pub fn watched_roots(&self) -> &[PathBuf] {
        &self.watched_roots
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
        let failure = WatcherFailure::new(
            PathBuf::from("/tmp/quickfox-watch"),
            "too many open files".to_owned(),
        );

        assert!(failure.requires_background_refresh);
        assert!(failure.message.contains("watcher failed"));
        assert!(failure.message.contains("too many open files"));
    }

    fn old_path(root: PathBuf, name: &str) -> PathBuf {
        root.join(name)
    }
}
