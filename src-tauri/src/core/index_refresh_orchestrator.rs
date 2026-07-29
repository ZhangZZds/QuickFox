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

pub trait RefreshWorkerSpawner: Send + Sync {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalibrationPhase {
    Preparing,
    Capturing,
    Calibrated,
    Fenced,
    Watching,
    Degraded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalibrationTransitionError {
    CaptureNotRegistered,
    CalibrationIncomplete,
    FenceIncomplete,
    InvalidGeneration,
    Terminal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeFailureKind {
    Watcher,
    Calibration,
    Storage,
    WorkerSpawn,
    Dispatch,
    BaselinePersistence,
    Monitor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeFailureApplication {
    pub revision: u64,
    pub kind: RuntimeFailureKind,
    pub preserve_live_view: bool,
    pub schedule_recovery: bool,
}

impl RuntimeFailureApplication {
    pub fn degraded(revision: u64, kind: RuntimeFailureKind) -> Self {
        Self {
            revision,
            kind,
            preserve_live_view: true,
            schedule_recovery: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeCalibrationSession {
    revision: u64,
    starting_generation: u64,
    calibration_generation: Option<u64>,
    authoritative_generation: Option<u64>,
    phase: CalibrationPhase,
}

impl RuntimeCalibrationSession {
    pub fn new(revision: u64, starting_generation: u64) -> Self {
        Self {
            revision,
            starting_generation,
            calibration_generation: None,
            authoritative_generation: None,
            phase: CalibrationPhase::Preparing,
        }
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn phase(&self) -> CalibrationPhase {
        self.phase
    }

    pub fn authoritative_generation(&self) -> Option<u64> {
        self.authoritative_generation
    }

    pub fn mark_capture_registered(&mut self) -> Result<(), CalibrationTransitionError> {
        if matches!(
            self.phase,
            CalibrationPhase::Degraded | CalibrationPhase::Watching
        ) {
            return Err(CalibrationTransitionError::Terminal);
        }
        self.phase = CalibrationPhase::Capturing;
        Ok(())
    }

    pub fn mark_calibration_complete(
        &mut self,
        generation: u64,
    ) -> Result<(), CalibrationTransitionError> {
        if self.phase == CalibrationPhase::Degraded {
            return Err(CalibrationTransitionError::Terminal);
        }
        if self.phase != CalibrationPhase::Capturing {
            return Err(CalibrationTransitionError::CaptureNotRegistered);
        }
        if generation < self.starting_generation {
            return Err(CalibrationTransitionError::InvalidGeneration);
        }
        self.calibration_generation = Some(generation);
        self.phase = CalibrationPhase::Calibrated;
        Ok(())
    }

    pub fn mark_fenced(&mut self, generation: u64) -> Result<(), CalibrationTransitionError> {
        if self.phase == CalibrationPhase::Degraded {
            return Err(CalibrationTransitionError::Terminal);
        }
        let Some(calibration_generation) = self.calibration_generation else {
            return Err(CalibrationTransitionError::CalibrationIncomplete);
        };
        if self.phase != CalibrationPhase::Calibrated || generation < calibration_generation {
            return Err(CalibrationTransitionError::InvalidGeneration);
        }
        self.authoritative_generation = Some(generation);
        self.phase = CalibrationPhase::Fenced;
        Ok(())
    }

    pub fn mark_watching(&mut self) -> Result<(), CalibrationTransitionError> {
        match self.phase {
            CalibrationPhase::Preparing => Err(CalibrationTransitionError::CaptureNotRegistered),
            CalibrationPhase::Capturing => Err(CalibrationTransitionError::CalibrationIncomplete),
            CalibrationPhase::Calibrated => Err(CalibrationTransitionError::FenceIncomplete),
            CalibrationPhase::Fenced => {
                self.phase = CalibrationPhase::Watching;
                Ok(())
            }
            CalibrationPhase::Watching | CalibrationPhase::Degraded => {
                Err(CalibrationTransitionError::Terminal)
            }
        }
    }

    pub fn fail(&mut self, kind: RuntimeFailureKind) -> RuntimeFailureApplication {
        self.phase = CalibrationPhase::Degraded;
        RuntimeFailureApplication::degraded(self.revision, kind)
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RevisionRecoveryLatch {
    claimed_revision: Option<u64>,
}

impl RevisionRecoveryLatch {
    pub fn claim(&mut self, revision: u64) -> bool {
        if self.claimed_revision == Some(revision) {
            return false;
        }
        self.claimed_revision = Some(revision);
        true
    }

    pub fn clear(&mut self, revision: u64) -> bool {
        if self.claimed_revision != Some(revision) {
            return false;
        }
        self.claimed_revision = None;
        true
    }

    pub fn claimed_revision(&self) -> Option<u64> {
        self.claimed_revision
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

    #[test]
    fn calibration_session_requires_capture_calibration_and_fence_before_watching() {
        let mut session = RuntimeCalibrationSession::new(4, 11);
        assert_eq!(session.phase(), CalibrationPhase::Preparing);
        assert_eq!(
            session.mark_watching(),
            Err(CalibrationTransitionError::CaptureNotRegistered)
        );

        session.mark_capture_registered().unwrap();
        assert_eq!(session.phase(), CalibrationPhase::Capturing);
        assert_eq!(
            session.mark_watching(),
            Err(CalibrationTransitionError::CalibrationIncomplete)
        );

        session.mark_calibration_complete(13).unwrap();
        assert_eq!(session.phase(), CalibrationPhase::Calibrated);
        assert_eq!(
            session.mark_watching(),
            Err(CalibrationTransitionError::FenceIncomplete)
        );

        session.mark_fenced(15).unwrap();
        session.mark_watching().unwrap();
        assert_eq!(session.phase(), CalibrationPhase::Watching);
        assert_eq!(session.authoritative_generation(), Some(15));
    }

    #[test]
    fn calibration_session_failure_is_terminal_and_preserves_revision() {
        let mut session = RuntimeCalibrationSession::new(9, 21);
        let application = session.fail(RuntimeFailureKind::Storage);

        assert_eq!(session.phase(), CalibrationPhase::Degraded);
        assert_eq!(session.revision(), 9);
        assert_eq!(
            application,
            RuntimeFailureApplication::degraded(9, RuntimeFailureKind::Storage)
        );
        assert_eq!(
            session.mark_capture_registered(),
            Err(CalibrationTransitionError::Terminal)
        );
    }

    #[test]
    fn revision_recovery_latch_allows_at_most_one_claim_until_cleared() {
        let mut latch = RevisionRecoveryLatch::default();
        assert!(latch.claim(7));
        assert!(!latch.claim(7));
        assert!(latch.claim(8));
        assert_eq!(latch.claimed_revision(), Some(8));
        assert!(!latch.clear(7));
        assert!(latch.clear(8));
        assert_eq!(latch.claimed_revision(), None);
        assert!(latch.claim(8));
    }
}
