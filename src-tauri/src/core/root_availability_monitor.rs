//! Managed lifecycle for missing-index-root availability monitoring.

use std::fmt;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MonitorExit {
    Ready,
    Cancelled,
    ProbeFailed(String),
    DispatchFailed(String),
    ThreadPanicked,
}

#[derive(Default)]
struct MonitorCancellation {
    cancelled: Mutex<bool>,
    wake: Condvar,
}

impl MonitorCancellation {
    fn cancel(&self) {
        *self.cancelled.lock().expect("root monitor cancel poisoned") = true;
        self.wake.notify_all();
    }

    fn wait(&self, interval: Duration) -> bool {
        let cancelled = self.cancelled.lock().expect("root monitor cancel poisoned");
        if *cancelled {
            return true;
        }
        let (cancelled, _) = self
            .wake
            .wait_timeout(cancelled, interval)
            .expect("root monitor wait poisoned");
        *cancelled
    }

    fn is_cancelled(&self) -> bool {
        *self.cancelled.lock().expect("root monitor cancel poisoned")
    }
}

pub struct RootAvailabilityMonitorHandle {
    cancellation: Arc<MonitorCancellation>,
    join: Option<JoinHandle<MonitorExit>>,
    exit: Option<MonitorExit>,
}

impl fmt::Debug for RootAvailabilityMonitorHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RootAvailabilityMonitorHandle")
            .field("running", &self.join.is_some())
            .field("exit", &self.exit)
            .finish()
    }
}

impl RootAvailabilityMonitorHandle {
    pub fn cancel_and_join(&mut self) -> MonitorExit {
        self.cancellation.cancel();
        self.join()
    }

    pub fn join(&mut self) -> MonitorExit {
        if let Some(join) = self.join.take() {
            self.exit = Some(join.join().unwrap_or(MonitorExit::ThreadPanicked));
        }
        self.exit.clone().unwrap_or(MonitorExit::Cancelled)
    }
}

impl Drop for RootAvailabilityMonitorHandle {
    fn drop(&mut self) {
        if self.join.is_some() {
            let _ = self.cancel_and_join();
        }
    }
}

pub trait RootMonitorSpawner {
    fn spawn(
        &self,
        task: Box<dyn FnOnce() -> MonitorExit + Send>,
    ) -> Result<JoinHandle<MonitorExit>, String>;
}

pub struct SystemRootMonitorSpawner;

impl RootMonitorSpawner for SystemRootMonitorSpawner {
    fn spawn(
        &self,
        task: Box<dyn FnOnce() -> MonitorExit + Send>,
    ) -> Result<JoinHandle<MonitorExit>, String> {
        thread::Builder::new()
            .name("quickfox-index-root-recovery".to_owned())
            .spawn(task)
            .map_err(|error| error.to_string())
    }
}

pub fn spawn_root_availability_monitor(
    interval: Duration,
    probe: impl FnMut() -> Result<bool, String> + Send + 'static,
    on_ready: impl FnOnce() -> Result<(), String> + Send + 'static,
) -> Result<RootAvailabilityMonitorHandle, String> {
    spawn_root_availability_monitor_with(&SystemRootMonitorSpawner, interval, probe, on_ready)
}

pub fn spawn_root_availability_monitor_with(
    spawner: &dyn RootMonitorSpawner,
    interval: Duration,
    probe: impl FnMut() -> Result<bool, String> + Send + 'static,
    on_ready: impl FnOnce() -> Result<(), String> + Send + 'static,
) -> Result<RootAvailabilityMonitorHandle, String> {
    spawn_root_availability_monitor_with_completion(spawner, interval, probe, on_ready, |_| {})
}

pub fn spawn_root_availability_monitor_with_completion(
    spawner: &dyn RootMonitorSpawner,
    interval: Duration,
    mut probe: impl FnMut() -> Result<bool, String> + Send + 'static,
    on_ready: impl FnOnce() -> Result<(), String> + Send + 'static,
    on_exit: impl FnOnce(&MonitorExit) + Send + 'static,
) -> Result<RootAvailabilityMonitorHandle, String> {
    let cancellation = Arc::new(MonitorCancellation::default());
    let worker_cancellation = Arc::clone(&cancellation);
    let join = spawner.spawn(Box::new(move || {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut on_ready = Some(on_ready);
            loop {
                if worker_cancellation.is_cancelled() {
                    return MonitorExit::Cancelled;
                }
                match probe() {
                    Ok(true) => {
                        return match on_ready
                            .take()
                            .expect("root monitor ready callback missing")(
                        ) {
                            Ok(()) => MonitorExit::Ready,
                            Err(error) => MonitorExit::DispatchFailed(error),
                        };
                    }
                    Ok(false) => {}
                    Err(error) => return MonitorExit::ProbeFailed(error),
                }
                if worker_cancellation.wait(interval) {
                    return MonitorExit::Cancelled;
                }
            }
        }))
        .unwrap_or(MonitorExit::ThreadPanicked);
        on_exit(&outcome);
        outcome
    }))?;
    Ok(RootAvailabilityMonitorHandle {
        cancellation,
        join: Some(join),
        exit: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn cancellation_wakes_and_joins_monitor_without_waiting_for_poll_interval() {
        let mut handle =
            spawn_root_availability_monitor(Duration::from_secs(60), || Ok(false), || Ok(()))
                .unwrap();

        let outcome = handle.cancel_and_join();

        assert_eq!(outcome, MonitorExit::Cancelled);
    }

    #[test]
    fn ready_probe_dispatches_once_and_returns_joinable_outcome() {
        let dispatched = Arc::new(AtomicBool::new(false));
        let dispatched_from_worker = Arc::clone(&dispatched);
        let mut handle = spawn_root_availability_monitor(
            Duration::from_secs(60),
            || Ok(true),
            move || {
                dispatched_from_worker.store(true, Ordering::Release);
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(handle.join(), MonitorExit::Ready);
        assert!(dispatched.load(Ordering::Acquire));
    }

    #[test]
    fn probe_and_dispatch_failures_are_structured_monitor_exits() {
        let mut probe_failure = spawn_root_availability_monitor(
            Duration::from_secs(60),
            || Err("probe failed".to_owned()),
            || Ok(()),
        )
        .unwrap();
        assert_eq!(
            probe_failure.join(),
            MonitorExit::ProbeFailed("probe failed".to_owned())
        );

        let mut dispatch_failure = spawn_root_availability_monitor(
            Duration::from_secs(60),
            || Ok(true),
            || Err("dispatch failed".to_owned()),
        )
        .unwrap();
        assert_eq!(
            dispatch_failure.join(),
            MonitorExit::DispatchFailed("dispatch failed".to_owned())
        );
    }

    #[test]
    fn completion_observer_receives_probe_failure_and_worker_panic() {
        let probe_exit = Arc::new(Mutex::new(None));
        let observed_probe_exit = Arc::clone(&probe_exit);
        let mut probe_failure = spawn_root_availability_monitor_with_completion(
            &SystemRootMonitorSpawner,
            Duration::from_secs(60),
            || Err("probe failed".to_owned()),
            || Ok(()),
            move |outcome| *observed_probe_exit.lock().unwrap() = Some(outcome.clone()),
        )
        .unwrap();
        assert_eq!(
            probe_failure.join(),
            MonitorExit::ProbeFailed("probe failed".to_owned())
        );
        assert_eq!(
            *probe_exit.lock().unwrap(),
            Some(MonitorExit::ProbeFailed("probe failed".to_owned()))
        );

        let panic_exit = Arc::new(Mutex::new(None));
        let observed_panic_exit = Arc::clone(&panic_exit);
        let mut panicked = spawn_root_availability_monitor_with_completion(
            &SystemRootMonitorSpawner,
            Duration::from_secs(60),
            || -> Result<bool, String> { panic!("injected probe panic") },
            || Ok(()),
            move |outcome| *observed_panic_exit.lock().unwrap() = Some(outcome.clone()),
        )
        .unwrap();
        assert_eq!(panicked.join(), MonitorExit::ThreadPanicked);
        assert_eq!(
            *panic_exit.lock().unwrap(),
            Some(MonitorExit::ThreadPanicked)
        );
    }

    #[test]
    fn spawn_failure_is_returned_without_detaching_a_monitor_thread() {
        struct FailingSpawner;

        impl RootMonitorSpawner for FailingSpawner {
            fn spawn(
                &self,
                _task: Box<dyn FnOnce() -> MonitorExit + Send>,
            ) -> Result<JoinHandle<MonitorExit>, String> {
                Err("injected monitor spawn failure".to_owned())
            }
        }

        let result = spawn_root_availability_monitor_with(
            &FailingSpawner,
            Duration::from_secs(60),
            || Ok(false),
            || Ok(()),
        );

        assert_eq!(result.unwrap_err(), "injected monitor spawn failure");
    }
}
