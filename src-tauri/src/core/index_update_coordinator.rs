//! Debounce and lifecycle state for runtime index updates.

use crate::core::index_watcher::{IndexEventBatcher, IndexWatchEvent};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const MAX_SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoordinatorPolicy {
    quiet_window: Duration,
    max_latency: Duration,
}

impl CoordinatorPolicy {
    pub const fn new(quiet_window: Duration, max_latency: Duration) -> Self {
        Self {
            quiet_window,
            max_latency,
        }
    }

    pub fn production() -> Self {
        Self::new(Duration::from_secs(5), Duration::from_secs(10))
    }

    pub fn quiet_window(self) -> Duration {
        self.quiet_window
    }

    pub fn max_latency(self) -> Duration {
        self.max_latency
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CoordinatorBatch {
    pub changed_paths: Vec<PathBuf>,
    pub removed_paths: Vec<PathBuf>,
    pub dirty_roots: Vec<PathBuf>,
}

#[derive(Debug)]
pub struct CoordinatorState {
    policy: CoordinatorPolicy,
    events: IndexEventBatcher,
    dirty_roots: BTreeSet<PathBuf>,
    first_event_at: Option<Instant>,
    last_event_at: Option<Instant>,
}

impl CoordinatorState {
    pub fn new(policy: CoordinatorPolicy) -> Self {
        Self {
            policy,
            events: IndexEventBatcher::default(),
            dirty_roots: BTreeSet::new(),
            first_event_at: None,
            last_event_at: None,
        }
    }

    pub fn push_event(&mut self, event: IndexWatchEvent, observed_at: Instant) {
        self.events.push(event);
        self.observe(observed_at);
    }

    pub fn mark_dirty_root(&mut self, root: PathBuf, observed_at: Instant) {
        self.dirty_roots.insert(root);
        self.observe(observed_at);
    }

    pub fn should_drain(&self, now: Instant) -> bool {
        self.next_deadline().is_some_and(|deadline| now >= deadline)
    }

    pub fn next_wait(&self, now: Instant) -> Duration {
        let until_deadline = self
            .next_deadline()
            .and_then(|deadline| deadline.checked_duration_since(now))
            .unwrap_or(Duration::ZERO);

        if self.next_deadline().is_none() {
            MAX_SHUTDOWN_POLL_INTERVAL
        } else {
            until_deadline.min(MAX_SHUTDOWN_POLL_INTERVAL)
        }
    }

    pub fn drain(&mut self) -> CoordinatorBatch {
        let event_batch = self.events.drain_batch();
        let batch = CoordinatorBatch {
            changed_paths: event_batch.changed_paths,
            removed_paths: event_batch.removed_paths,
            dirty_roots: std::mem::take(&mut self.dirty_roots).into_iter().collect(),
        };
        self.first_event_at = None;
        self.last_event_at = None;
        batch
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty() && self.dirty_roots.is_empty()
    }

    pub fn pending_event_count(&self) -> usize {
        self.events.len()
    }

    pub fn dirty_root_count(&self) -> usize {
        self.dirty_roots.len()
    }

    fn observe(&mut self, observed_at: Instant) {
        self.first_event_at = Some(
            self.first_event_at
                .map_or(observed_at, |current| current.min(observed_at)),
        );
        self.last_event_at = Some(
            self.last_event_at
                .map_or(observed_at, |current| current.max(observed_at)),
        );
    }

    fn next_deadline(&self) -> Option<Instant> {
        let quiet_deadline = self.last_event_at? + self.policy.quiet_window;
        let max_deadline = self.first_event_at? + self.policy.max_latency;
        Some(quiet_deadline.min(max_deadline))
    }
}

#[derive(Debug, Clone, Default)]
pub struct CoordinatorShutdown {
    requested: Arc<AtomicBool>,
}

impl CoordinatorShutdown {
    pub fn request(&self) {
        self.requested.store(true, Ordering::Release);
    }

    pub fn is_requested(&self) -> bool {
        self.requested.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::index_watcher::IndexWatchEvent;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    #[test]
    fn production_policy_uses_five_second_quiet_window_and_ten_second_max_latency() {
        let policy = CoordinatorPolicy::production();

        assert_eq!(policy.quiet_window(), Duration::from_secs(5));
        assert_eq!(policy.max_latency(), Duration::from_secs(10));
    }

    #[test]
    fn coordinator_policy_accepts_injected_test_deadlines() {
        let start = Instant::now();
        let policy = CoordinatorPolicy::new(Duration::from_millis(10), Duration::from_millis(20));
        let mut state = CoordinatorState::new(policy);
        state.push_event(IndexWatchEvent::Write(PathBuf::from("a.txt")), start);

        assert!(!state.should_drain(start + Duration::from_millis(9)));
        assert!(state.should_drain(start + Duration::from_millis(10)));
    }

    #[test]
    fn coordinator_drains_after_five_seconds_without_a_new_event() {
        let start = Instant::now();
        let mut state = CoordinatorState::new(CoordinatorPolicy::production());
        state.push_event(IndexWatchEvent::Write(PathBuf::from("b.txt")), start);

        assert!(!state.should_drain(start + Duration::from_millis(4_999)));
        assert!(state.should_drain(start + Duration::from_secs(5)));
    }

    #[test]
    fn coordinator_enforces_ten_second_max_latency_during_continuous_events() {
        let start = Instant::now();
        let mut state = CoordinatorState::new(CoordinatorPolicy::production());
        state.push_event(IndexWatchEvent::Write(PathBuf::from("a.txt")), start);
        state.push_event(
            IndexWatchEvent::Write(PathBuf::from("b.txt")),
            start + Duration::from_secs(4),
        );
        state.push_event(
            IndexWatchEvent::Write(PathBuf::from("c.txt")),
            start + Duration::from_secs(8),
        );

        assert!(!state.should_drain(start + Duration::from_millis(9_999)));
        assert!(state.should_drain(start + Duration::from_secs(10)));
    }

    #[test]
    fn coordinator_collapses_rename_write_and_dirty_roots_deterministically() {
        let start = Instant::now();
        let mut state = CoordinatorState::new(CoordinatorPolicy::production());
        let old_path = PathBuf::from("/workspace/old.txt");
        let new_path = PathBuf::from("/workspace/new.txt");

        state.push_event(
            IndexWatchEvent::Rename {
                from: old_path.clone(),
                to: new_path.clone(),
            },
            start,
        );
        state.push_event(IndexWatchEvent::Write(new_path.clone()), start);
        state.mark_dirty_root(PathBuf::from("/workspace/z-root"), start);
        state.mark_dirty_root(PathBuf::from("/workspace/a-root"), start);
        state.mark_dirty_root(PathBuf::from("/workspace/z-root"), start);

        let batch = state.drain();

        assert_eq!(batch.changed_paths, vec![new_path]);
        assert_eq!(batch.removed_paths, vec![old_path]);
        assert_eq!(
            batch.dirty_roots,
            vec![
                PathBuf::from("/workspace/a-root"),
                PathBuf::from("/workspace/z-root")
            ]
        );
        assert!(state.is_empty());
        assert!(!state.should_drain(start + Duration::from_secs(60)));
    }

    #[test]
    fn coordinator_shutdown_is_observable_within_bounded_poll_interval() {
        let start = Instant::now();
        let mut state = CoordinatorState::new(CoordinatorPolicy::production());
        state.push_event(IndexWatchEvent::Write(PathBuf::from("a.txt")), start);
        let shutdown = CoordinatorShutdown::default();
        let worker_shutdown = shutdown.clone();

        assert!(state.next_wait(start) <= Duration::from_millis(250));
        shutdown.request();
        assert!(worker_shutdown.is_requested());
    }
}
