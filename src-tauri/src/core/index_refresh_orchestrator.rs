use std::thread;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RevisionCaptureFence {
    old_service_join_generation: u64,
}

impl RevisionCaptureFence {
    pub fn after_old_service_join(generation: u64) -> Self {
        Self {
            old_service_join_generation: generation,
        }
    }
}

pub fn compatible_tail_start_generation(
    scan_start_generation: u64,
    fence: Option<RevisionCaptureFence>,
) -> u64 {
    fence
        .map(|fence| fence.old_service_join_generation)
        .unwrap_or(scan_start_generation)
        .max(scan_start_generation)
}

pub trait RefreshWorkerSpawner {
    fn spawn(&self, task: Box<dyn FnOnce() + Send>) -> Result<(), String>;
}

pub struct SystemRefreshWorkerSpawner;

impl RefreshWorkerSpawner for SystemRefreshWorkerSpawner {
    fn spawn(&self, task: Box<dyn FnOnce() + Send>) -> Result<(), String> {
        thread::Builder::new()
            .name("quickfox-index-refresh".to_owned())
            .spawn(task)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshRequestReason {
    DeltaSafetyLimit,
    DirtyRoots,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshRequestDecision {
    Start,
    AbsorbedByActiveRefresh,
    QueueRerun,
}

pub fn refresh_request_decision(
    refresh_active: bool,
    reason: RefreshRequestReason,
) -> RefreshRequestDecision {
    match (refresh_active, reason) {
        (false, _) => RefreshRequestDecision::Start,
        (true, RefreshRequestReason::DeltaSafetyLimit) => {
            RefreshRequestDecision::AbsorbedByActiveRefresh
        }
        (true, RefreshRequestReason::DirtyRoots) => RefreshRequestDecision::QueueRerun,
    }
}

pub fn authoritative_install_generation(baseline_generation: u64, tail_generations: &[u64]) -> u64 {
    tail_generations
        .iter()
        .copied()
        .max()
        .unwrap_or(baseline_generation)
        .max(baseline_generation)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupReadiness {
    Ready,
    NeedsCalibration,
}

pub fn startup_readiness(
    required_roots_available: &[bool],
    manifest_covers_roots: bool,
) -> StartupReadiness {
    if manifest_covers_roots && required_roots_available.iter().all(|available| *available) {
        StartupReadiness::Ready
    } else {
        StartupReadiness::NeedsCalibration
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revision_fence_selects_tail_after_old_service_join() {
        let fence = RevisionCaptureFence::after_old_service_join(7);
        assert_eq!(compatible_tail_start_generation(3, Some(fence)), 7);
        assert_eq!(compatible_tail_start_generation(9, Some(fence)), 9);
    }

    #[test]
    fn delta_safety_during_refresh_is_absorbed_without_pending_rerun() {
        assert_eq!(
            refresh_request_decision(true, RefreshRequestReason::DeltaSafetyLimit),
            RefreshRequestDecision::AbsorbedByActiveRefresh
        );
        assert_eq!(
            refresh_request_decision(true, RefreshRequestReason::DirtyRoots),
            RefreshRequestDecision::QueueRerun
        );
    }

    #[test]
    fn baseline_install_uses_authoritative_tail_generation() {
        assert_eq!(authoritative_install_generation(3, &[4, 5, 8]), 8);
        assert_eq!(authoritative_install_generation(3, &[]), 3);
    }

    #[test]
    fn missing_required_root_keeps_startup_degraded() {
        assert_eq!(
            startup_readiness(&[true, false], true),
            StartupReadiness::NeedsCalibration
        );
        assert_eq!(
            startup_readiness(&[true, true], true),
            StartupReadiness::Ready
        );
    }
}
