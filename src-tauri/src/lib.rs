pub mod core;

use crate::core::actions::{Action, OpenApplication};
use crate::core::config::{ConfigStore, IndexPerformanceMode, QuickFoxConfig};
use crate::core::content_index::{ContentIndex, ContentIndexOptions};
use crate::core::index::{
    FileSearchIndex, IndexLifecycle, IndexReport, IndexScanOptions, IndexScanner, IndexStatus,
    SearchIndex,
};
use crate::core::index_entry::{
    normalize_path_key, normalize_path_text_key, path_is_same_or_descendant_for_mode,
    ContentIndexState, IncrementalState, IndexDegradationCode, IndexScanStats, IndexedEntry,
    IndexedEntryKind, PathComparisonMode, RuntimeIncrementalStatus, ScanEvent,
};
use crate::core::index_journal::recover_layered_index;
use crate::core::index_refresh_orchestrator::{
    authoritative_install_generation, compatible_tail_start_generation, refresh_request_decision,
    RefreshRequestDecision, RefreshRequestReason, RefreshWorkerSpawner, RevisionCaptureFence,
    RevisionRecoveryLatch, RuntimeCalibrationSession,
    RuntimeFailureApplication as CoreRuntimeFailureApplication, RuntimeFailureKind,
    SystemRefreshWorkerSpawner,
};
use crate::core::index_scanner::{IndexPathRules, IndexScanPlan, IndexScanStage};
use crate::core::index_watcher::RuntimeIndexWatcher;
use crate::core::layered_index::{CommittedIndexDelta, LayeredSearchIndex};
use crate::core::platform::{
    CommandSafetyChecker, CommandSafetyDecision, DevelopmentToolAdapter, HotkeyKey, HotkeyState,
    KeyPress, LauncherWindowEffect, LauncherWindowState, ProcessCommand, WakeShortcut,
};
use crate::core::providers::{
    CalculatorProvider, CommandProvider, CommandProviderConfig, FileProvider, ProviderRegistry,
    WebSearchEngine, WebSearchProvider,
};
use crate::core::root_availability_monitor::{
    spawn_root_availability_monitor_with_completion_gate, MonitorExit,
    RootAvailabilityMonitorHandle, RootMonitorSpawner, SystemRootMonitorSpawner,
};
use crate::core::runtime_indexing::{
    baseline_refresh_event_for_delta_state, start_runtime_indexing, BaselineRefreshReason,
    RuntimeIndexingEvent, RuntimeIndexingHandle, RuntimeIndexingHandoffOutcome,
    RuntimeIndexingOptions,
};
use crate::core::search::{HistoryScores, QueryParser, QueryParserConfig, Ranker, SearchResult};
use crate::core::storage::SqliteStorage;
use crate::core::targeted_index_scanner::{
    baseline_manifest_from_entries, DirectoryFingerprint, TargetedIndexScanner,
};
use keytap::{EventKind, Key, Tap};
use serde::Serialize;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Instant;
use tauri::image::Image;
use tauri::{Emitter, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_opener::OpenerExt;

#[cfg(target_os = "linux")]
use crate::core::platform::LinuxTerminalAdapter;
#[cfg(target_os = "macos")]
use crate::core::platform::MacosTerminalAdapter;
#[cfg(target_os = "windows")]
use crate::core::platform::WindowsTerminalAdapter;

struct QuickFoxRuntime {
    config: QuickFoxConfig,
    index: LayeredSearchIndex,
    last_report: IndexReport,
    index_lifecycle: IndexLifecycle,
    runtime_indexing: Option<RuntimeIndexingHandle>,
    incremental_status: RuntimeIncrementalStatus,
    manifest_ready: bool,
    index_refresh: IndexRefreshControl,
}

struct QuickFoxAppState {
    runtime: Mutex<QuickFoxRuntime>,
    index_refresh_fence: Mutex<()>,
    window_state: Mutex<LauncherWindowState>,
    global_hotkey_status: Mutex<GlobalHotkeyStatus>,
}

#[derive(Default)]
struct StartupIndexingGate {
    setup_returned: Mutex<bool>,
    wake: Condvar,
    worker_scheduled: AtomicBool,
    retry_attempted: AtomicBool,
}

impl StartupIndexingGate {
    fn wait_for_setup_return(&self) {
        let mut returned = self
            .setup_returned
            .lock()
            .expect("startup indexing gate poisoned");
        while !*returned {
            returned = self
                .wake
                .wait(returned)
                .expect("startup indexing gate poisoned");
        }
    }

    fn release_after_setup(&self) {
        *self
            .setup_returned
            .lock()
            .expect("startup indexing gate poisoned") = true;
        self.wake.notify_all();
    }

    fn claim_retry(&self) -> bool {
        !self.worker_scheduled.load(Ordering::Acquire)
            && !self.retry_attempted.swap(true, Ordering::AcqRel)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct IndexRefreshIdentity {
    lifecycle_generation: u64,
    config_revision: u64,
    config_fingerprint: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RuntimeServiceIdentity {
    epoch: u64,
    config_revision: u64,
}

#[derive(Debug, Clone, Copy)]
enum RuntimeRestartFailureKind {
    Watcher,
    Rules,
    Storage,
    WorkerSpawn,
    Handoff,
    Dispatch,
}

struct RuntimeRestartFailureApplication {
    status: IndexStatus,
    request_recovery: bool,
    handle_to_stop: Option<RuntimeIndexingHandle>,
}

#[derive(Debug, Default)]
struct IndexRefreshControl {
    config_revision: u64,
    config_fingerprint: String,
    active: Option<IndexRefreshIdentity>,
    pending: bool,
    next_service_epoch: u64,
    active_service: Option<RuntimeServiceIdentity>,
    standby_watcher: Option<RuntimeIndexWatcher>,
    revision_capture_fence: Option<RevisionCaptureFence>,
    restart_recovery_revision: Option<u64>,
    root_recovery_latch: RevisionRecoveryLatch,
    root_monitor_failure_revision: Option<u64>,
    root_monitor: Option<RootAvailabilityMonitorHandle>,
}

impl IndexRefreshControl {
    fn for_config(config: &QuickFoxConfig) -> Self {
        Self {
            config_revision: 0,
            config_fingerprint: index_semantic_config_fingerprint(config),
            active: None,
            pending: false,
            next_service_epoch: 0,
            active_service: None,
            standby_watcher: None,
            revision_capture_fence: None,
            restart_recovery_revision: None,
            root_recovery_latch: RevisionRecoveryLatch::default(),
            root_monitor_failure_revision: None,
            root_monitor: None,
        }
    }
}

struct IndexRefreshStart {
    identity: IndexRefreshIdentity,
    config: QuickFoxConfig,
}

struct IndexRefreshHandoff {
    tail_deltas: Vec<CommittedIndexDelta>,
    manifest: Vec<crate::core::targeted_index_scanner::DirectoryFingerprint>,
    authoritative_generation: u64,
}

#[derive(Debug, Default)]
struct IndexRefreshAccumulator {
    entries_by_path: std::collections::BTreeMap<String, IndexedEntry>,
    summary: IndexReport,
}

#[derive(Debug)]
struct IndexRefreshPayload {
    entries: Vec<IndexedEntry>,
    summary: IndexReport,
}

impl From<IndexReport> for IndexRefreshPayload {
    fn from(mut report: IndexReport) -> Self {
        let entries = std::mem::take(&mut report.entries);
        Self {
            entries,
            summary: report,
        }
    }
}

impl IndexRefreshAccumulator {
    fn merge(&mut self, stage_report: IndexReport) {
        self.summary.failures.extend(stage_report.failures);
        self.summary.scan_events.extend(stage_report.scan_events);
        self.summary.scan_stats = IndexScanStats {
            scanned: self
                .summary
                .scan_stats
                .scanned
                .saturating_add(stage_report.scan_stats.scanned),
            accepted: self
                .summary
                .scan_stats
                .accepted
                .saturating_add(stage_report.scan_stats.accepted),
            skipped: self
                .summary
                .scan_stats
                .skipped
                .saturating_add(stage_report.scan_stats.skipped),
            failures: self
                .summary
                .scan_stats
                .failures
                .saturating_add(stage_report.scan_stats.failures),
        };

        for entry in stage_report.entries {
            self.entries_by_path.insert(entry.path.clone(), entry);
        }
    }

    fn progress_payload(&self) -> IndexRefreshPayload {
        IndexRefreshPayload {
            entries: self.entries(),
            summary: self.summary.clone(),
        }
    }

    fn final_payload(self) -> IndexRefreshPayload {
        IndexRefreshPayload {
            entries: self.entries_by_path.into_values().collect(),
            summary: self.summary,
        }
    }

    fn entries(&self) -> Vec<IndexedEntry> {
        self.entries_by_path.values().cloned().collect()
    }

    #[cfg(test)]
    fn entry_count(&self) -> usize {
        self.entries_by_path.len()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrayWindowTarget {
    Launcher,
    Settings,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct AppPaths {
    config_file_path: String,
    index_snapshot_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct GlobalHotkeyStatus {
    enabled: bool,
    message: String,
    permission_settings_url: Option<String>,
}

#[tauri::command]
fn search(state: tauri::State<QuickFoxAppState>, query: String) -> Vec<SearchResult> {
    let runtime = state
        .runtime
        .lock()
        .expect("quickfox runtime lock poisoned");
    perform_search_with_index_status(
        &runtime.config,
        &runtime.index,
        &runtime.index_status(),
        &query,
    )
}

#[tauri::command]
fn health_check() -> &'static str {
    "ok"
}

#[tauri::command]
fn index_status(state: tauri::State<QuickFoxAppState>) -> IndexStatus {
    state
        .runtime
        .lock()
        .expect("quickfox runtime lock poisoned")
        .index_status()
}

#[tauri::command]
fn app_paths() -> Result<AppPaths, String> {
    let config_path =
        config_file_path().ok_or_else(|| "config file path is unavailable".to_owned())?;
    let index_path =
        storage_file_path().ok_or_else(|| "index snapshot path is unavailable".to_owned())?;

    Ok(build_app_paths(config_path, index_path))
}

#[tauri::command]
fn global_hotkey_status(state: tauri::State<QuickFoxAppState>) -> GlobalHotkeyStatus {
    state
        .global_hotkey_status
        .lock()
        .expect("quickfox global hotkey status lock poisoned")
        .clone()
}

#[tauri::command]
fn toggle_launcher_window(
    app: tauri::AppHandle,
    state: tauri::State<QuickFoxAppState>,
) -> Result<&'static str, String> {
    toggle_launcher_window_for_app(&app, &state)?;
    Ok("completed")
}

#[tauri::command]
fn open_settings_window(app: tauri::AppHandle) -> Result<&'static str, String> {
    show_settings_window(&app);
    Ok("completed")
}

#[tauri::command]
fn execute_action(app: tauri::AppHandle, action: Action) -> Result<&'static str, String> {
    match action {
        Action::OpenPath { path } => app
            .opener()
            .open_path(expand_user_path(&path), None::<&str>)
            .map_err(|error| error.to_string())?,
        Action::OpenContainingFolder { path } => app
            .opener()
            .reveal_item_in_dir(expand_user_path(&path))
            .map_err(|error| error.to_string())?,
        Action::OpenWithApplication { path, application } => {
            execute_open_with_application(&path, &application)?;
        }
        Action::OpenUrl { url } => app
            .opener()
            .open_url(url, None::<&str>)
            .map_err(|error| error.to_string())?,
        Action::CopyText { text } => copy_text_to_clipboard(&text)?,
        Action::ExecuteCommand {
            command,
            requires_confirmation,
        } => {
            validate_command_action(&command, requires_confirmation)?;
            execute_command_in_terminal(&command)?;
        }
    }

    Ok("completed")
}

#[tauri::command]
fn refresh_index(
    app: tauri::AppHandle,
    state: tauri::State<QuickFoxAppState>,
) -> Result<IndexStatus, String> {
    start_background_index_refresh(app, &state)
}

#[tauri::command]
fn load_config(state: tauri::State<QuickFoxAppState>) -> QuickFoxConfig {
    state
        .runtime
        .lock()
        .expect("quickfox runtime lock poisoned")
        .config
        .clone()
}

#[tauri::command]
fn save_config(
    app: tauri::AppHandle,
    state: tauri::State<QuickFoxAppState>,
    config: QuickFoxConfig,
) -> Result<&'static str, String> {
    let errors = config.validate();
    if let Some(error) = errors.first() {
        return Err(format!("{error:?}"));
    }

    let storage =
        storage_store().ok_or_else(|| "index journal storage is unavailable".to_owned())?;
    let revision = state
        .runtime
        .lock()
        .expect("quickfox runtime lock poisoned")
        .index_refresh
        .config_revision
        .saturating_add(1);
    let candidate = match prepare_config_revision_candidate(config, &storage, revision) {
        Ok(candidate) => candidate,
        Err(error) => {
            record_config_transition_failure(&state);
            return Err(error);
        }
    };
    if candidate.session.phase()
        != crate::core::index_refresh_orchestrator::CalibrationPhase::Calibrated
    {
        record_config_transition_failure(&state);
        return Err("config revision candidate did not finish calibration".to_owned());
    }
    let store = config_store();
    let restore_store = store.clone();
    let config_to_restore = state
        .runtime
        .lock()
        .expect("quickfox runtime lock poisoned")
        .config
        .clone();
    let config_to_persist = candidate.config.clone();
    let publish_app = app.clone();
    let next_shortcut = transition_runtime_config_revision_with_persist(
        &state,
        candidate,
        &storage,
        move || {
            store
                .as_ref()
                .map(|store| {
                    store
                        .save(&config_to_persist)
                        .map_err(|error| format!("{error:?}"))
                })
                .unwrap_or(Ok(()))
        },
        move || {
            restore_store
                .as_ref()
                .map(|store| {
                    store
                        .save(&config_to_restore)
                        .map_err(|error| format!("{error:?}"))
                })
                .unwrap_or(Ok(()))
        },
        move |service, event| publish_runtime_indexing_event(publish_app.clone(), service, event),
    )?;
    refresh_enabled_global_hotkey_status(&app, &next_shortcut);

    Ok("saved")
}

struct ConfigRevisionCandidate {
    config: QuickFoxConfig,
    roots: Vec<PathBuf>,
    rules: IndexPathRules,
    capture_watcher: RuntimeIndexWatcher,
    entries: Vec<IndexedEntry>,
    report: IndexReport,
    session: RuntimeCalibrationSession,
    buffered_capture_events: Vec<crate::core::index_watcher::IndexWatchEvent>,
}

struct ConfigRevisionTransitionHooks<'a> {
    before_old_fence: &'a dyn Fn() -> Result<(), String>,
    before_activation: &'a dyn Fn() -> Result<(), String>,
    before_successor_start: &'a dyn Fn() -> Result<(), String>,
    before_successor_fence: &'a dyn Fn() -> Result<(), String>,
    after_successor_fence: &'a dyn Fn() -> Result<(), String>,
}

#[derive(Clone, Copy)]
struct ActivatedConfigRollback<'a> {
    storage: &'a SqliteStorage,
    baseline_id: i64,
    generation: u64,
    manifest: &'a [DirectoryFingerprint],
}

impl ConfigRevisionTransitionHooks<'static> {
    fn production() -> Self {
        Self {
            before_old_fence: &|| Ok(()),
            before_activation: &|| Ok(()),
            before_successor_start: &|| Ok(()),
            before_successor_fence: &|| Ok(()),
            after_successor_fence: &|| Ok(()),
        }
    }
}

fn prepare_config_revision_candidate(
    config: QuickFoxConfig,
    storage: &SqliteStorage,
    revision: u64,
) -> Result<ConfigRevisionCandidate, String> {
    let roots = refresh_capture_roots(&config, Vec::new())?;
    prepare_config_revision_candidate_for_roots(config, storage, revision, roots)
}

fn prepare_config_revision_candidate_for_roots(
    config: QuickFoxConfig,
    storage: &SqliteStorage,
    revision: u64,
    roots: Vec<PathBuf>,
) -> Result<ConfigRevisionCandidate, String> {
    prepare_config_revision_candidate_with_capture_tail(config, storage, revision, roots, Vec::new)
}

fn prepare_config_revision_candidate_with_capture_tail(
    config: QuickFoxConfig,
    storage: &SqliteStorage,
    revision: u64,
    roots: Vec<PathBuf>,
    capture_tail: impl FnOnce() -> Vec<crate::core::index_watcher::IndexWatchEvent>,
) -> Result<ConfigRevisionCandidate, String> {
    let starting_generation = storage
        .highest_committed_generation()
        .map_err(|error| error.to_string())?;
    let capture_watcher =
        RuntimeIndexWatcher::watch_roots(roots.clone()).map_err(|failure| failure.message)?;
    let mut session = RuntimeCalibrationSession::new(revision, starting_generation);
    session
        .mark_capture_registered()
        .map_err(|error| format!("config capture transition failed: {error:?}"))?;
    let mut accumulator = IndexRefreshAccumulator::default();
    let scan_options = build_scan_options(&config);
    accumulator.merge(
        IndexScanner
            .scan_plan(IndexScanPlan {
                include_roots: roots.clone(),
                exclude_dirs: scan_options.exclude_dirs,
                exclude_patterns: scan_options.exclude_patterns,
                respect_project_ignores: scan_options.respect_project_ignores,
                stage: None,
            })
            .map_err(|error| error.to_string())?,
    );
    let payload = accumulator.final_payload();
    if !payload.summary.failures.is_empty() {
        return Err("config revision calibration reported filesystem failures".to_owned());
    }
    session
        .mark_calibration_complete(starting_generation)
        .map_err(|error| format!("config calibration transition failed: {error:?}"))?;
    let buffered_capture_events = capture_tail();
    let options = build_scan_options(&config);
    let rules = IndexPathRules::from_plan(&IndexScanPlan {
        include_roots: roots.clone(),
        exclude_dirs: options.exclude_dirs,
        exclude_patterns: options.exclude_patterns,
        respect_project_ignores: options.respect_project_ignores,
        stage: None,
    })
    .map_err(|error| error.to_string())?;
    Ok(ConfigRevisionCandidate {
        config,
        roots,
        rules,
        capture_watcher,
        entries: payload.entries,
        report: payload.summary,
        session,
        buffered_capture_events,
    })
}

#[cfg(test)]
fn transition_runtime_config_revision(
    state: &QuickFoxAppState,
    candidate: ConfigRevisionCandidate,
    storage: &SqliteStorage,
) -> Result<WakeShortcut, String> {
    transition_runtime_config_revision_with_persist(
        state,
        candidate,
        storage,
        || Ok(()),
        || Ok(()),
        |_, _| {},
    )
}

fn transition_runtime_config_revision_with_persist(
    state: &QuickFoxAppState,
    candidate: ConfigRevisionCandidate,
    storage: &SqliteStorage,
    persist: impl FnOnce() -> Result<(), String>,
    restore_config: impl FnOnce() -> Result<(), String>,
    publish: impl Fn(RuntimeServiceIdentity, RuntimeIndexingEvent) + Send + 'static,
) -> Result<WakeShortcut, String> {
    transition_runtime_config_revision_with_hooks(
        state,
        candidate,
        storage,
        persist,
        restore_config,
        publish,
        ConfigRevisionTransitionHooks::production(),
    )
}

fn transition_runtime_config_revision_with_hooks(
    state: &QuickFoxAppState,
    mut candidate: ConfigRevisionCandidate,
    storage: &SqliteStorage,
    persist: impl FnOnce() -> Result<(), String>,
    restore_config: impl FnOnce() -> Result<(), String>,
    publish: impl Fn(RuntimeServiceIdentity, RuntimeIndexingEvent) + Send + 'static,
    hooks: ConfigRevisionTransitionHooks<'_>,
) -> Result<WakeShortcut, String> {
    let _refresh_fence = state
        .index_refresh_fence
        .lock()
        .expect("index refresh fence poisoned");
    let preflight_generation = match storage.highest_committed_generation() {
        Ok(generation) => generation,
        Err(error) => {
            record_config_transition_failure(state);
            return Err(error.to_string());
        }
    };
    let (
        mut previous,
        previous_service,
        mut rollback_entries,
        rollback_view_generation,
        next_service_epoch,
    ) = {
        let mut runtime = state
            .runtime
            .lock()
            .expect("quickfox runtime lock poisoned");
        let previous_service = runtime.index_refresh.active_service.take();
        let rollback_entries = runtime.index.materialized_entries();
        let rollback_view_generation = runtime.index.generation();
        runtime.index_refresh.next_service_epoch =
            runtime.index_refresh.next_service_epoch.saturating_add(1);
        (
            runtime.runtime_indexing.take(),
            previous_service,
            rollback_entries,
            rollback_view_generation,
            runtime.index_refresh.next_service_epoch,
        )
    };
    if let Err(error) = (hooks.before_old_fence)() {
        return Err(restore_fenced_config_service(
            state,
            previous,
            previous_service,
            error,
        ));
    }
    let mut fence_generation = match previous.as_mut() {
        Some(handle) => match handle.fence() {
            Ok(generation) => generation.max(preflight_generation),
            Err(error) => {
                return Err(restore_fenced_config_service(
                    state,
                    previous,
                    previous_service,
                    error,
                ));
            }
        },
        None => preflight_generation,
    };
    fence_generation = match storage.highest_committed_generation() {
        Ok(generation) => fence_generation.max(generation),
        Err(error) => {
            return Err(restore_fenced_config_service(
                state,
                previous,
                previous_service,
                error.to_string(),
            ));
        }
    };
    let rollback_tail = match storage.committed_index_deltas_after(rollback_view_generation) {
        Ok(tail) => tail,
        Err(error) => {
            return Err(restore_fenced_config_service(
                state,
                previous,
                previous_service,
                error.to_string(),
            ));
        }
    };
    rollback_entries = entries_after_committed_deltas(rollback_entries, &rollback_tail);
    let successor_watcher = match RuntimeIndexWatcher::watch_roots(candidate.roots.clone()) {
        Ok(watcher) => watcher,
        Err(failure) => {
            return Err(restore_fenced_config_service(
                state,
                previous,
                previous_service,
                failure.message,
            ));
        }
    };
    let inbox = match candidate.capture_watcher.take_inbox() {
        Some(inbox) => inbox,
        None => {
            return Err(restore_fenced_config_service(
                state,
                previous,
                previous_service,
                "config candidate watcher inbox is unavailable".to_owned(),
            ));
        }
    };
    drop(candidate.capture_watcher);
    if let Some(failure) = inbox.take_failure() {
        return Err(restore_fenced_config_service(
            state,
            previous,
            previous_service,
            failure.message,
        ));
    }
    if inbox.take_degradation_code().is_some() || !inbox.take_dirty_roots().is_empty() {
        return Err(restore_fenced_config_service(
            state,
            previous,
            previous_service,
            "config candidate capture requires a fresh calibration".to_owned(),
        ));
    }
    let mut coordinator = crate::core::index_update_coordinator::CoordinatorState::new(
        crate::core::index_update_coordinator::CoordinatorPolicy::production(),
    );
    for event in candidate.buffered_capture_events {
        if coordinator.push_event(event, Instant::now())
            == crate::core::index_update_coordinator::CoordinatorPushOutcome::CapacityReached
        {
            return Err(restore_fenced_config_service(
                state,
                previous,
                previous_service,
                "config candidate buffered capture overflowed".to_owned(),
            ));
        }
    }
    while let Ok(event) = inbox.try_recv() {
        if coordinator.push_event(event, Instant::now())
            == crate::core::index_update_coordinator::CoordinatorPushOutcome::CapacityReached
        {
            return Err(restore_fenced_config_service(
                state,
                previous,
                previous_service,
                "config candidate capture overflowed".to_owned(),
            ));
        }
    }
    if !coordinator.is_empty() {
        let calibration = match TargetedIndexScanner::new(candidate.rules.clone())
            .scan_batch(coordinator.drain())
        {
            Ok(calibration) => calibration,
            Err(error) => {
                return Err(restore_fenced_config_service(
                    state,
                    previous,
                    previous_service,
                    error.to_string(),
                ));
            }
        };
        if !calibration.failures.is_empty() {
            return Err(restore_fenced_config_service(
                state,
                previous,
                previous_service,
                "config candidate targeted calibration failed".to_owned(),
            ));
        }
        candidate.entries = entries_after_committed_deltas(
            candidate.entries,
            &[CommittedIndexDelta {
                generation: fence_generation,
                upserts: calibration.upserts,
                removals: calibration.removals,
            }],
        );
    }
    if let Err(error) = candidate.session.mark_fenced(fence_generation) {
        return Err(restore_fenced_config_service(
            state,
            previous,
            previous_service,
            format!("config candidate fence transition failed: {error:?}"),
        ));
    }
    let manifest = baseline_manifest_from_entries(&candidate.entries, &candidate.roots);
    let rollback_manifest = load_complete_directory_manifest(storage).map_err(|error| {
        restore_fenced_config_service(state, previous.take(), previous_service, error)
    })?;
    let rollback_baseline_id = storage
        .save_completed_index_batch(current_time_ms(), &rollback_entries)
        .map_err(|error| {
            restore_fenced_config_service(
                state,
                previous.take(),
                previous_service,
                error.to_string(),
            )
        })?;
    let candidate_baseline_id = storage
        .save_completed_index_batch(current_time_ms(), &candidate.entries)
        .map_err(|error| {
            restore_fenced_config_service(
                state,
                previous.take(),
                previous_service,
                error.to_string(),
            )
        })?;
    if let Err(error) = persist() {
        let restore_error = restore_config().err();
        return Err(restore_fenced_config_service(
            state,
            previous,
            previous_service,
            restore_error
                .map(|restore| format!("{error}; failed to restore config: {restore}"))
                .unwrap_or(error),
        ));
    }
    if let Err(error) = (hooks.before_activation)() {
        let restore_error = restore_config().err();
        return Err(restore_fenced_config_service(
            state,
            previous,
            previous_service,
            restore_error
                .map(|restore| format!("{error}; failed to restore config: {restore}"))
                .unwrap_or(error),
        ));
    }
    if let Err(error) = storage.activate_baseline_with_manifest_and_clear_incremental_state(
        candidate_baseline_id,
        fence_generation,
        &manifest,
    ) {
        let restore_error = restore_config().err();
        return Err(restore_fenced_config_service(
            state,
            previous,
            previous_service,
            restore_error
                .map(|restore| format!("{error}; failed to restore config: {restore}"))
                .unwrap_or_else(|| error.to_string()),
        ));
    }
    let rollback_target = ActivatedConfigRollback {
        storage,
        baseline_id: rollback_baseline_id,
        generation: fence_generation,
        manifest: &rollback_manifest,
    };
    let future_service = RuntimeServiceIdentity {
        epoch: next_service_epoch,
        config_revision: candidate.session.revision(),
    };
    let publish_service = future_service;
    if let Err(error) = (hooks.before_successor_start)() {
        return Err(rollback_activated_config_revision(
            state,
            previous,
            previous_service,
            rollback_target,
            restore_config,
            error,
        ));
    }
    let successor_storage = match storage.reopen() {
        Ok(storage) => storage,
        Err(error) => {
            return Err(rollback_activated_config_revision(
                state,
                previous,
                previous_service,
                rollback_target,
                restore_config,
                error.to_string(),
            ));
        }
    };
    let mut successor = match start_runtime_indexing(
        successor_watcher,
        TargetedIndexScanner::new(candidate.rules.clone()),
        Box::new(successor_storage),
        RuntimeIndexingOptions {
            roots: candidate.roots.clone(),
            policy: crate::core::index_update_coordinator::CoordinatorPolicy::production(),
            initial_generation: fence_generation,
        },
        move |event| publish(publish_service, event),
    ) {
        Ok(handle) => handle,
        Err(failure) => {
            return Err(rollback_activated_config_revision(
                state,
                previous,
                previous_service,
                rollback_target,
                restore_config,
                failure.message,
            ));
        }
    };
    if let Err(error) = (hooks.before_successor_fence)() {
        successor.stop();
        return Err(rollback_activated_config_revision(
            state,
            previous,
            previous_service,
            rollback_target,
            restore_config,
            error,
        ));
    }
    let watcher_enabled = candidate.config.index.watcher_enabled;
    let (mut successor_generation, mut successor) = if watcher_enabled {
        let generation = match successor.fence() {
            Ok(generation) => generation,
            Err(error) => {
                successor.stop();
                return Err(rollback_activated_config_revision(
                    state,
                    previous,
                    previous_service,
                    rollback_target,
                    restore_config,
                    error,
                ));
            }
        };
        (generation, Some(successor))
    } else {
        let handoff = successor.handoff_with_generation();
        if handoff.outcome == RuntimeIndexingHandoffOutcome::RecoveryRequired {
            return Err(rollback_activated_config_revision(
                state,
                previous,
                previous_service,
                rollback_target,
                restore_config,
                "disabled config successor handoff requires recovery".to_owned(),
            ));
        }
        (handoff.last_committed_generation, None)
    };
    if let Err(error) = (hooks.after_successor_fence)() {
        if let Some(successor) = successor {
            successor.stop();
        }
        return Err(rollback_activated_config_revision(
            state,
            previous,
            previous_service,
            rollback_target,
            restore_config,
            error,
        ));
    }
    let successor_tail = match storage.committed_index_deltas_after(fence_generation) {
        Ok(tail) => tail,
        Err(error) => {
            if let Some(successor) = successor {
                successor.stop();
            }
            return Err(rollback_activated_config_revision(
                state,
                previous,
                previous_service,
                rollback_target,
                restore_config,
                error.to_string(),
            ));
        }
    };
    if let Some(last) = successor_tail.last() {
        successor_generation = successor_generation.max(last.generation);
    }
    if let Err(error) = candidate.session.mark_watching() {
        if let Some(successor) = successor {
            successor.stop();
        }
        return Err(rollback_activated_config_revision(
            state,
            previous,
            previous_service,
            rollback_target,
            restore_config,
            format!("config candidate watching transition failed: {error:?}"),
        ));
    }
    let final_candidate_entries =
        entries_after_committed_deltas(candidate.entries, &successor_tail);
    let candidate_search_index =
        build_search_index_with_content_for_config(&candidate.config, final_candidate_entries);
    let final_manifest = baseline_manifest_after_committed_deltas(
        candidate_search_index.entries(),
        &successor_tail,
        &candidate.roots,
    );
    let final_baseline_id = match storage
        .save_completed_index_batch(current_time_ms(), candidate_search_index.entries())
    {
        Ok(baseline_id) => baseline_id,
        Err(error) => {
            if let Some(successor) = successor {
                successor.stop();
            }
            return Err(rollback_activated_config_revision(
                state,
                previous,
                previous_service,
                rollback_target,
                restore_config,
                error.to_string(),
            ));
        }
    };
    if let Err(error) = storage.activate_baseline_with_manifest_and_clear_incremental_state(
        final_baseline_id,
        successor_generation,
        &final_manifest,
    ) {
        if let Some(successor) = successor {
            successor.stop();
        }
        return Err(rollback_activated_config_revision(
            state,
            previous,
            previous_service,
            rollback_target,
            restore_config,
            error.to_string(),
        ));
    }
    let monitor = {
        let mut runtime = state
            .runtime
            .lock()
            .expect("quickfox runtime lock poisoned");
        runtime.config = candidate.config;
        runtime.index_refresh.config_revision = candidate.session.revision();
        runtime.index_refresh.config_fingerprint =
            index_semantic_config_fingerprint(&runtime.config);
        runtime.index_refresh.active = None;
        runtime.index_refresh.pending = false;
        runtime.index_refresh.active_service = watcher_enabled.then_some(future_service);
        runtime.index_refresh.standby_watcher = None;
        runtime.index_refresh.revision_capture_fence = None;
        runtime.index_refresh.restart_recovery_revision = None;
        runtime.index_refresh.root_recovery_latch = RevisionRecoveryLatch::default();
        runtime.index_refresh.root_monitor_failure_revision = None;
        runtime
            .index
            .replace_baseline_search_index(candidate_search_index, successor_generation);
        runtime.index_lifecycle =
            IndexLifecycle::from_ready(runtime.index.entry_count(), current_time_ms());
        runtime.last_report = candidate.report;
        runtime.manifest_ready = true;
        runtime.incremental_status.enabled = runtime.config.index.watcher_enabled;
        runtime.incremental_status.state = if watcher_enabled {
            IncrementalState::Watching
        } else {
            IncrementalState::Disabled
        };
        runtime.incremental_status.degradation_code = None;
        runtime.runtime_indexing = successor.take();
        runtime.index_refresh.root_monitor.take()
    };
    if let Some(handle) = previous {
        handle.stop();
    }
    if let Some(mut monitor) = monitor {
        monitor.cancel_and_join();
    }
    if watcher_enabled {
        state
            .runtime
            .lock()
            .expect("quickfox runtime lock poisoned")
            .runtime_indexing
            .as_mut()
            .ok_or_else(|| "config successor service disappeared".to_owned())?
            .resume()?;
    }
    let next_shortcut = current_wake_shortcut(&state.runtime.lock().unwrap().config);
    debug_assert_eq!(
        successor_generation,
        storage.highest_committed_generation().unwrap_or(0)
    );
    Ok(next_shortcut)
}

fn load_complete_directory_manifest(
    storage: &SqliteStorage,
) -> Result<Vec<DirectoryFingerprint>, String> {
    let mut manifest = Vec::new();
    for root in storage
        .directory_manifest_roots()
        .map_err(|error| error.to_string())?
    {
        manifest.extend(
            storage
                .directory_manifest_for_root(&root)
                .map_err(|error| error.to_string())?,
        );
    }
    Ok(manifest)
}

fn restore_fenced_config_service(
    state: &QuickFoxAppState,
    mut previous: Option<RuntimeIndexingHandle>,
    previous_service: Option<RuntimeServiceIdentity>,
    error: String,
) -> String {
    let resume_error = previous.as_mut().and_then(|handle| handle.resume().err());
    let mut runtime = state
        .runtime
        .lock()
        .expect("quickfox runtime lock poisoned");
    runtime.runtime_indexing = previous;
    runtime.index_refresh.active_service = previous_service;
    runtime.incremental_status.enabled = runtime.config.index.watcher_enabled;
    runtime.incremental_status.state = IncrementalState::Degraded;
    runtime.incremental_status.degradation_code = Some(IndexDegradationCode::FullRefreshFallback);
    resume_error
        .map(|resume| format!("{error}; failed to resume old runtime indexing: {resume}"))
        .unwrap_or(error)
}

fn rollback_activated_config_revision(
    state: &QuickFoxAppState,
    previous: Option<RuntimeIndexingHandle>,
    previous_service: Option<RuntimeServiceIdentity>,
    rollback: ActivatedConfigRollback<'_>,
    restore_config: impl FnOnce() -> Result<(), String>,
    original_error: String,
) -> String {
    let storage_error = rollback
        .storage
        .restore_baseline_after_failed_revision(
            rollback.baseline_id,
            rollback.generation,
            rollback.manifest,
        )
        .err()
        .map(|error| format!("storage rollback failed: {error}"));
    let config_error = restore_config()
        .err()
        .map(|error| format!("config rollback failed: {error}"));
    if storage_error.is_none() && config_error.is_none() {
        return restore_fenced_config_service(state, previous, previous_service, original_error);
    }
    let combined = [Some(original_error), storage_error, config_error]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("; ");
    let error = restore_fenced_config_service(state, previous, previous_service, combined);
    state
        .runtime
        .lock()
        .expect("quickfox runtime lock poisoned")
        .index_refresh
        .pending = true;
    error
}

fn record_config_transition_failure(state: &QuickFoxAppState) {
    let mut runtime = state
        .runtime
        .lock()
        .expect("quickfox runtime lock poisoned");
    runtime.incremental_status.enabled = runtime.config.index.watcher_enabled;
    runtime.incremental_status.state = IncrementalState::Degraded;
    runtime.incremental_status.degradation_code = Some(IndexDegradationCode::FullRefreshFallback);
}

fn apply_runtime_failure_state(
    runtime: &mut QuickFoxRuntime,
    kind: RuntimeFailureKind,
    degradation_code: IndexDegradationCode,
    detach_service: bool,
) -> Option<RuntimeIndexingHandle> {
    let application =
        CoreRuntimeFailureApplication::degraded(runtime.index_refresh.config_revision, kind);
    if application.preserve_live_view {
        runtime.incremental_status.enabled = runtime.config.index.watcher_enabled;
        runtime.incremental_status.state = IncrementalState::Degraded;
        runtime.incremental_status.degradation_code = Some(degradation_code);
    }
    if application.schedule_recovery
        && runtime.index_refresh.restart_recovery_revision != Some(application.revision)
    {
        runtime.index_refresh.pending = true;
        runtime.index_refresh.restart_recovery_revision = Some(application.revision);
    }
    if detach_service {
        runtime.index_refresh.active_service = None;
        runtime.runtime_indexing.take()
    } else {
        None
    }
}

fn index_semantic_config_fingerprint(config: &QuickFoxConfig) -> String {
    serde_json::to_string(&config.index).unwrap_or_default()
}

#[cfg(test)]
fn replace_runtime_config_for_full_refresh(runtime: &mut QuickFoxRuntime, config: QuickFoxConfig) {
    runtime.index_refresh.standby_watcher.take();
    runtime.index_refresh.revision_capture_fence = None;
    runtime.index_refresh.restart_recovery_revision = None;
    runtime.index_refresh.root_recovery_latch = RevisionRecoveryLatch::default();
    runtime.index_refresh.root_monitor_failure_revision = None;
    runtime.config = config;
    runtime.index_refresh.config_revision = runtime.index_refresh.config_revision.saturating_add(1);
    runtime.index_refresh.config_fingerprint = index_semantic_config_fingerprint(&runtime.config);
    runtime.index_refresh.pending = runtime.index_refresh.active.is_some();
    runtime.manifest_ready = false;
    runtime.incremental_status.enabled = runtime.config.index.watcher_enabled;
    runtime.incremental_status.state = if runtime.config.index.watcher_enabled {
        IncrementalState::Preparing
    } else {
        IncrementalState::Disabled
    };
}

fn begin_runtime_index_refresh(runtime: &mut QuickFoxRuntime) -> Option<IndexRefreshStart> {
    if runtime.index_refresh.active.is_some() {
        runtime.index_refresh.pending = true;
        return None;
    }
    let has_existing_index = runtime.index.entry_count() > 0;
    let lifecycle_generation = runtime.index_lifecycle.start_refresh(has_existing_index);
    let identity = IndexRefreshIdentity {
        lifecycle_generation,
        config_revision: runtime.index_refresh.config_revision,
        config_fingerprint: runtime.index_refresh.config_fingerprint.clone(),
    };
    runtime.index_refresh.active = Some(identity.clone());
    runtime.index_refresh.pending = false;
    Some(IndexRefreshStart {
        identity,
        config: runtime.config.clone(),
    })
}

#[tauri::command]
fn clear_command_history() -> &'static str {
    "cleared"
}

#[tauri::command]
fn record_input_history(
    state: tauri::State<QuickFoxAppState>,
    input: String,
) -> Result<&'static str, String> {
    let config = state
        .runtime
        .lock()
        .expect("quickfox runtime lock poisoned")
        .config
        .clone();
    if let Some(storage) = storage_store() {
        storage
            .record_input(
                &input,
                current_time_ms(),
                config.history.input_history_enabled,
                config.history.input_max_entries,
            )
            .map_err(|error| format!("{error:?}"))?;
    }
    Ok("recorded")
}

#[tauri::command]
fn recent_input_history() -> Result<Vec<String>, String> {
    if let Some(storage) = storage_store() {
        return storage
            .recent_inputs()
            .map_err(|error| format!("{error:?}"));
    }
    Ok(Vec::new())
}

#[tauri::command]
fn clear_input_history() -> Result<&'static str, String> {
    if let Some(storage) = storage_store() {
        storage
            .clear_input_history()
            .map_err(|error| format!("{error:?}"))?;
    }
    Ok("cleared")
}

fn default_index_dirs() -> Vec<String> {
    #[cfg(target_os = "windows")]
    {
        let roots = windows_existing_drive_roots();
        if !roots.is_empty() {
            return roots;
        }
    }

    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .into_iter()
        .collect()
}

#[cfg(target_os = "windows")]
fn windows_existing_drive_roots() -> Vec<String> {
    windows_drive_roots_from_letters('C'..='Z', |root| PathBuf::from(root).is_dir())
}

#[cfg(any(target_os = "windows", test))]
fn windows_drive_roots_from_letters<I, F>(letters: I, exists: F) -> Vec<String>
where
    I: IntoIterator<Item = char>,
    F: Fn(&str) -> bool,
{
    letters
        .into_iter()
        .map(|letter| format!("{letter}:\\"))
        .filter(|root| exists(root))
        .collect()
}

fn copy_text_to_clipboard(text: &str) -> Result<(), String> {
    let mut clipboard = arboard::Clipboard::new().map_err(|error| error.to_string())?;
    clipboard
        .set_text(text.to_owned())
        .map_err(|error| error.to_string())
}

fn config_store() -> Option<ConfigStore> {
    Some(ConfigStore::new(config_file_path()?))
}

fn config_file_path() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        home_dir().map(|home| {
            home.join("Library")
                .join("Application Support")
                .join("QuickFox")
                .join("config.toml")
        })
    }

    #[cfg(target_os = "windows")]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            Some(PathBuf::from(appdata).join("QuickFox").join("config.toml"))
        } else {
            home_dir().map(|home| {
                home.join("AppData")
                    .join("Roaming")
                    .join("QuickFox")
                    .join("config.toml")
            })
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        home_dir().map(|home| home.join(".config").join("quickfox").join("config.toml"))
    }
}

fn storage_store() -> Option<SqliteStorage> {
    SqliteStorage::open(storage_file_path()?).ok()
}

fn storage_file_path() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        home_dir().map(|home| {
            home.join("Library")
                .join("Application Support")
                .join("QuickFox")
                .join("quickfox.sqlite")
        })
    }

    #[cfg(target_os = "windows")]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            Some(
                PathBuf::from(appdata)
                    .join("QuickFox")
                    .join("quickfox.sqlite"),
            )
        } else {
            home_dir().map(|home| {
                home.join("AppData")
                    .join("Roaming")
                    .join("QuickFox")
                    .join("quickfox.sqlite")
            })
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        home_dir().map(|home| {
            home.join(".local")
                .join("share")
                .join("quickfox")
                .join("quickfox.sqlite")
        })
    }
}

fn current_time_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

fn load_startup_config() -> QuickFoxConfig {
    config_store()
        .and_then(|store| store.load_or_create_default(default_index_dirs()).ok())
        .unwrap_or_else(|| QuickFoxConfig::default_with_index_dirs(default_index_dirs()))
}

fn build_scan_options(config: &QuickFoxConfig) -> IndexScanOptions {
    let mut exclude_dirs: Vec<_> = config
        .index
        .exclude_dirs
        .iter()
        .map(|path| PathBuf::from(expand_user_path(path)))
        .collect();
    exclude_dirs.extend(implicit_exclude_dirs(config));

    let mut exclude_patterns = config.index.exclude_patterns.clone();
    for pattern in implicit_exclude_patterns() {
        if !exclude_patterns.contains(&pattern) {
            exclude_patterns.push(pattern);
        }
    }

    IndexScanOptions {
        include_dirs: config
            .index
            .include_dirs
            .iter()
            .map(|path| PathBuf::from(expand_user_path(path)))
            .collect(),
        exclude_dirs,
        exclude_patterns,
        respect_project_ignores: config.index.respect_project_ignores,
    }
}

fn build_scan_plans(config: &QuickFoxConfig) -> Vec<IndexScanPlan> {
    let options = build_scan_options(config);
    let mut plans = Vec::new();
    let mode = config.index.performance_mode;

    let applications = existing_paths(application_index_roots());
    if !applications.is_empty() {
        plans.push(scan_plan_for_stage(
            "applications",
            10,
            applications,
            &options,
        ));
    }

    let configured_roots = unique_pathbufs(options.include_dirs.clone());
    let hot_paths = existing_paths(user_hot_path_roots())
        .into_iter()
        .filter(|path| !configured_roots.iter().any(|root| root == path))
        .collect::<Vec<_>>();
    if !hot_paths.is_empty() {
        plans.push(scan_plan_for_stage(
            "user-hot-paths",
            20,
            hot_paths,
            &options,
        ));
    }

    if mode != IndexPerformanceMode::Fast && !configured_roots.is_empty() {
        plans.push(scan_plan_for_stage(
            "configured-roots",
            30,
            configured_roots.clone(),
            &options,
        ));
    }

    let remaining_drives = if mode == IndexPerformanceMode::Complete {
        remaining_drive_roots(&configured_roots)
    } else {
        Vec::new()
    };
    if !remaining_drives.is_empty() {
        plans.push(scan_plan_for_stage(
            "remaining-drives",
            40,
            remaining_drives,
            &options,
        ));
    }

    if plans.is_empty() && mode != IndexPerformanceMode::Fast {
        plans.push(scan_plan_for_stage(
            "configured-roots",
            30,
            options.include_dirs.clone(),
            &options,
        ));
    }

    plans
}

fn startup_calibration_roots_from_plans(
    configured_roots: Vec<PathBuf>,
    recovered_roots: Vec<PathBuf>,
    plans: Vec<IndexScanPlan>,
) -> Vec<PathBuf> {
    let mut roots: std::collections::BTreeSet<_> = configured_roots.into_iter().collect();
    roots.extend(recovered_roots);
    roots.extend(plans.into_iter().flat_map(|plan| plan.include_roots));
    roots.into_iter().collect()
}

fn schedule_startup_indexing_with(
    spawner: &dyn RefreshWorkerSpawner,
    gate: Arc<StartupIndexingGate>,
    job: impl FnOnce() + Send + 'static,
) -> Result<(), String> {
    let worker_gate = Arc::clone(&gate);
    spawner.spawn(Box::new(move || {
        worker_gate.wait_for_setup_return();
        job();
    }))?;
    gate.worker_scheduled.store(true, Ordering::Release);
    Ok(())
}

fn schedule_startup_indexing_in_setup<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    gate: Arc<StartupIndexingGate>,
    spawner: &dyn RefreshWorkerSpawner,
) -> Result<(), String> {
    let worker_app = app;
    schedule_startup_indexing_with(spawner, gate, move || {
        let state = worker_app.state::<QuickFoxAppState>();
        let enabled = state
            .runtime
            .lock()
            .expect("quickfox runtime lock poisoned")
            .config
            .index
            .watcher_enabled;
        if enabled {
            let _ = start_background_index_refresh(worker_app.clone(), &state);
        }
    })
}

fn record_startup_scheduling_failure<R: tauri::Runtime>(app: &tauri::AppHandle<R>, error: &str) {
    eprintln!("QuickFox startup indexing scheduling failed: {error}");
    let state = app.state::<QuickFoxAppState>();
    let status = {
        let mut runtime = state
            .runtime
            .lock()
            .expect("quickfox runtime lock poisoned");
        apply_runtime_failure_state(
            &mut runtime,
            RuntimeFailureKind::WorkerSpawn,
            IndexDegradationCode::FullRefreshFallback,
            false,
        );
        runtime.index_status()
    };
    let _ = app.emit("quickfox://index-status", status);
}

fn scan_plan_for_stage(
    stage: &str,
    root_priority: u32,
    include_roots: Vec<PathBuf>,
    options: &IndexScanOptions,
) -> IndexScanPlan {
    IndexScanPlan {
        include_roots,
        exclude_dirs: options.exclude_dirs.clone(),
        exclude_patterns: options.exclude_patterns.clone(),
        respect_project_ignores: options.respect_project_ignores,
        stage: Some(IndexScanStage::new(stage, root_priority)),
    }
}

fn application_index_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();

    #[cfg(target_os = "macos")]
    {
        roots.push(PathBuf::from("/Applications"));
        if let Some(home) = home_dir() {
            roots.push(home.join("Applications"));
        }
    }

    #[cfg(target_os = "windows")]
    {
        if let Ok(program_data) = std::env::var("PROGRAMDATA") {
            roots.push(
                PathBuf::from(program_data)
                    .join("Microsoft")
                    .join("Windows")
                    .join("Start Menu")
                    .join("Programs"),
            );
        }
        if let Ok(program_files) = std::env::var("ProgramFiles") {
            roots.push(PathBuf::from(program_files));
        }
        if let Ok(program_files_x86) = std::env::var("ProgramFiles(x86)") {
            roots.push(PathBuf::from(program_files_x86));
        }
    }

    #[cfg(target_os = "linux")]
    {
        roots.push(PathBuf::from("/usr/share/applications"));
        if let Some(home) = home_dir() {
            roots.push(home.join(".local").join("share").join("applications"));
        }
    }

    roots
}

fn user_hot_path_roots() -> Vec<PathBuf> {
    let Some(home) = home_dir() else {
        return Vec::new();
    };

    [
        "Desktop",
        "Documents",
        "Downloads",
        "Projects",
        "workspace",
        "Workspace",
    ]
    .into_iter()
    .map(|name| home.join(name))
    .collect()
}

fn remaining_drive_roots(configured_roots: &[PathBuf]) -> Vec<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        return windows_existing_drive_roots()
            .into_iter()
            .map(PathBuf::from)
            .filter(|path| !configured_roots.iter().any(|root| root == path))
            .collect();
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = configured_roots;
        Vec::new()
    }
}

fn existing_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths.into_iter().filter(|path| path.is_dir()).collect()
}

fn unique_pathbufs(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut unique = Vec::new();
    for path in paths {
        if !unique.iter().any(|existing| existing == &path) {
            unique.push(path);
        }
    }
    unique
}

fn implicit_exclude_patterns() -> Vec<String> {
    [
        ".*",
        "$Recycle.Bin",
        "System Volume Information",
        "Windows",
        "Recovery",
        "AppData",
        "node_modules",
        "target",
        ".git",
        ".cache",
        "__pycache__",
        ".venv",
        "venv",
        "dist",
        "build",
        ".next",
        ".turbo",
        "*.tmp",
        "*.log",
        "pagefile.sys",
        "hiberfil.sys",
        "swapfile.sys",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

fn implicit_exclude_dirs(_config: &QuickFoxConfig) -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = home_dir() {
            let home_text = home.to_string_lossy().to_string();
            if _config
                .index
                .include_dirs
                .iter()
                .any(|dir| expand_user_path(dir) == home_text)
            {
                return vec![home.join("Library")];
            }
        }
    }

    Vec::new()
}

fn build_query_parser_config(config: &QuickFoxConfig) -> QueryParserConfig {
    QueryParserConfig {
        regex_prefix: config.query.regex_prefix.clone(),
        web_search_prefixes: config.valid_web_search_engines().into_keys().collect(),
        command_prefix: config.command.prefix.clone(),
    }
}

fn build_provider_registry<'a>(
    config: &QuickFoxConfig,
    index: &'a dyn FileSearchIndex,
    index_status: &IndexStatus,
) -> ProviderRegistry<'a> {
    let mut registry = ProviderRegistry::default();
    if file_index_is_available(index, index_status) {
        registry.register(FileProvider::with_candidate_limit(
            index,
            config.results.limit.max(1).saturating_mul(4),
        ));
    } else {
        registry.register(FileProvider::unavailable(index_unavailable_message(
            index_status,
        )));
    }
    registry.register(CalculatorProvider);
    registry.register(WebSearchProvider::new(
        config
            .valid_web_search_engines()
            .into_iter()
            .map(|(prefix, engine)| WebSearchEngine {
                prefix,
                name: engine.name,
                url_template: engine.url,
            })
            .collect(),
    ));
    registry.register(CommandProvider::new(CommandProviderConfig {
        enabled: config.command.enabled,
        prefix: config.command.prefix.clone(),
    }));
    registry
}

#[cfg(test)]
fn perform_search(config: &QuickFoxConfig, index: &SearchIndex, query: &str) -> Vec<SearchResult> {
    let status = if index.entries().is_empty() {
        IndexLifecycle::default().status().clone()
    } else {
        IndexLifecycle::from_ready(index.entries().len(), current_time_ms())
            .status()
            .clone()
    };
    perform_search_with_index_status(config, index, &status, query)
}

fn perform_search_with_index_status(
    config: &QuickFoxConfig,
    index: &dyn FileSearchIndex,
    index_status: &IndexStatus,
    query: &str,
) -> Vec<SearchResult> {
    let parser = QueryParser::new(build_query_parser_config(config));
    let request = parser.parse(query);
    if request.text.is_empty() {
        return Vec::new();
    }

    let registry = build_provider_registry(config, index, index_status);
    let results = registry.search(&request);
    let mut ranked = Ranker::default().rank(&request.text, results, &HistoryScores::default());
    ranked.truncate(config.results.limit.max(1));
    ranked
}

fn file_index_is_available(index: &dyn FileSearchIndex, status: &IndexStatus) -> bool {
    index.indexed_entry_count() > 0
        || matches!(
            status.kind,
            crate::core::index::IndexStatusKind::Ready
                | crate::core::index::IndexStatusKind::Refreshing
        )
}

fn index_unavailable_message(status: &IndexStatus) -> String {
    match status.kind {
        crate::core::index::IndexStatusKind::Building => "文件索引正在建立".to_owned(),
        crate::core::index::IndexStatusKind::Failed => status
            .message
            .clone()
            .unwrap_or_else(|| "文件索引构建失败".to_owned()),
        _ => "文件索引尚未建立".to_owned(),
    }
}

fn start_background_index_refresh<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: &QuickFoxAppState,
) -> Result<IndexStatus, String> {
    start_background_index_refresh_with_spawner(app, state, &SystemRefreshWorkerSpawner)
}

fn start_background_index_refresh_with_spawner<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: &QuickFoxAppState,
    spawner: &dyn RefreshWorkerSpawner,
) -> Result<IndexStatus, String> {
    let start = {
        let mut runtime = state
            .runtime
            .lock()
            .expect("quickfox runtime lock poisoned");
        let Some(start) = begin_runtime_index_refresh(&mut runtime) else {
            return Ok(runtime.index_status());
        };
        start
    };
    let IndexRefreshStart { identity, config } = start;
    if let Err(error) = prepare_refresh_standby_capture(state, &identity, &config) {
        let message = error.clone();
        let unavailable_roots = message == "one or more configured index roots are unavailable";
        record_index_refresh_runtime_failure(state, &identity, RuntimeRestartFailureKind::Watcher);
        let _ = apply_failed_index_refresh_for_identity(state, &identity, error);
        let retry_now = finish_current_index_refresh(state, &identity);
        if unavailable_roots {
            schedule_configured_root_recovery(app.clone(), state, &identity);
        } else if retry_now {
            let _ = start_background_index_refresh(app.clone(), state);
        }
        return Err(message);
    }
    let baseline_storage = storage_store().ok_or_else(|| {
        let message = "index journal storage is unavailable".to_owned();
        record_index_refresh_runtime_failure(state, &identity, RuntimeRestartFailureKind::Storage);
        let _ = apply_failed_index_refresh_for_identity(state, &identity, message.clone());
        if finish_current_index_refresh(state, &identity) {
            let _ = start_background_index_refresh(app.clone(), state);
        }
        message
    })?;
    let baseline_generation = match baseline_storage.highest_committed_generation() {
        Ok(generation) => generation,
        Err(error) => {
            let message = error.to_string();
            record_index_refresh_runtime_failure(
                state,
                &identity,
                RuntimeRestartFailureKind::Storage,
            );
            let _ = apply_failed_index_refresh_for_identity(state, &identity, message.clone());
            if finish_current_index_refresh(state, &identity) {
                let _ = start_background_index_refresh(app.clone(), state);
            }
            return Err(message);
        }
    };
    let status = {
        state
            .runtime
            .lock()
            .expect("quickfox runtime lock poisoned")
            .index_status()
    };

    let spawn_identity = identity.clone();
    let spawn_failure_app = app.clone();
    let spawn_result = spawn_refresh_worker_for_identity(
        state,
        &spawn_identity,
        spawner,
        Box::new(move || {
            let scanner = IndexScanner;
            let mut accumulator = IndexRefreshAccumulator::default();
            let mut update_result = Ok(());
            let mut scan_failed = false;

            for plan in build_scan_plans(&config) {
                let stage_name = plan
                    .stage
                    .as_ref()
                    .map(|stage| stage.name.clone())
                    .unwrap_or_else(|| "configured-roots".to_owned());
                let scan_result = scanner.scan_plan(plan);
                let completed_at_ms = current_time_ms();
                let app_for_update = app.clone();
                let refresh_state = app.state::<QuickFoxAppState>();
                if !index_refresh_identity_is_current_in_state(&refresh_state, &identity) {
                    schedule_pending_refresh_after_superseded(app.clone(), identity.clone());
                    return;
                }
                match scan_result {
                    Ok(stage_report) => {
                        accumulator.merge(stage_report);
                        if should_persist_index_checkpoint(&stage_name, false) {
                            let checkpoint_entries = accumulator.entries();
                            if !persist_checkpoint_for_identity(
                                &refresh_state,
                                &identity,
                                completed_at_ms,
                                &checkpoint_entries,
                            ) {
                                schedule_pending_refresh_after_superseded(
                                    app.clone(),
                                    identity.clone(),
                                );
                                return;
                            }
                        }
                        let progress_payload = accumulator.progress_payload();
                        let current_root =
                            last_finished_root_for_stage(&progress_payload.summary, &stage_name);
                        let app_for_state = app_for_update.clone();
                        let progress_identity = identity.clone();
                        update_result = app_for_update.run_on_main_thread(move || {
                            let state = app_for_state.state::<QuickFoxAppState>();
                            if let Some(status) = apply_index_refresh_progress_for_identity(
                                &state,
                                &progress_identity,
                                stage_name,
                                current_root,
                                progress_payload,
                            ) {
                                let _ = app_for_state.emit("quickfox://index-status", status);
                            }
                        });
                    }
                    Err(error) => {
                        scan_failed = true;
                        let message = error.to_string();
                        let app_for_state = app_for_update.clone();
                        let failure_identity = identity.clone();
                        update_result = app_for_update.run_on_main_thread(move || {
                            let state = app_for_state.state::<QuickFoxAppState>();
                            if let Some(status) = apply_failed_index_refresh_for_identity(
                                &state,
                                &failure_identity,
                                message,
                            ) {
                                if finish_current_index_refresh(&state, &failure_identity) {
                                    let _ = start_background_index_refresh(
                                        app_for_state.clone(),
                                        &state,
                                    );
                                    return;
                                }
                                let index_remains_available = !matches!(
                                    status.availability,
                                    crate::core::index::IndexAvailability::Unavailable
                                );
                                let _ = app_for_state.emit("quickfox://index-status", status);
                                if index_remains_available {
                                    let _ = restart_runtime_incremental_indexing(
                                        app_for_state.clone(),
                                        &state,
                                    );
                                }
                            } else if finish_superseded_index_refresh(&state, &failure_identity) {
                                let _ =
                                    start_background_index_refresh(app_for_state.clone(), &state);
                            }
                        });
                        break;
                    }
                }

                if update_result.is_err() {
                    break;
                }
            }

            if update_result.is_ok() && !scan_failed {
                let completed_at_ms = current_time_ms();
                let app_for_update = app.clone();
                let mut final_payload = accumulator.final_payload();
                let handoff_state = app.state::<QuickFoxAppState>();
                let (handoff, handoff_error) = match prepare_index_refresh_handoff(
                    &handoff_state,
                    &identity,
                    &config,
                    &final_payload.entries,
                    baseline_generation,
                ) {
                    Ok(Some(handoff)) => (Some(handoff), None),
                    Ok(None) => {
                        schedule_pending_refresh_after_superseded(app.clone(), identity.clone());
                        return;
                    }
                    Err(error) => {
                        if let Some(previous) =
                            detach_runtime_indexing_for_refresh(&handoff_state, &identity)
                        {
                            previous.stop();
                        }
                        (None, Some(error))
                    }
                };
                let tail_deltas = handoff
                    .as_ref()
                    .map(|handoff| handoff.tail_deltas.clone())
                    .unwrap_or_default();
                let authoritative_generation = handoff
                    .as_ref()
                    .map(|handoff| handoff.authoritative_generation)
                    .unwrap_or(baseline_generation);
                let content_entries =
                    entries_after_committed_deltas(final_payload.entries.clone(), &tail_deltas);
                final_payload.entries = content_entries.clone();
                let should_build_content_index =
                    should_build_content_index_for_config(&config, &content_entries);
                let persistence_state = app.state::<QuickFoxAppState>();
                let mut persistence =
                    if let Some(handoff) = &handoff {
                        let manifest = handoff.manifest.clone();
                        persist_index_refresh_for_identity(
                            &persistence_state,
                            &identity,
                            completed_at_ms,
                            final_payload,
                            authoritative_generation,
                            move |completed_at_ms, entries, baseline_generation, _| {
                                persist_and_activate_baseline_with_manifest(
                                    completed_at_ms,
                                    entries,
                                    baseline_generation,
                                    &manifest,
                                )
                            },
                        )
                    } else {
                        BaselinePersistenceOutcome::Failed(handoff_error.unwrap_or_else(|| {
                            "runtime index handoff preparation failed".to_owned()
                        }))
                    };
                let baseline_persistence_completed =
                    matches!(persistence, BaselinePersistenceOutcome::Completed(_));
                let app_for_state = app_for_update.clone();
                let persistence_identity = identity.clone();
                let persistence_tail_deltas =
                    if baseline_persistence_completed && !should_build_content_index {
                        match finalize_durable_refresh_successor(
                            &persistence_state,
                            &identity,
                            authoritative_generation,
                        ) {
                            Ok(tail) => tail,
                            Err(error) => {
                                persistence = BaselinePersistenceOutcome::Failed(error);
                                Vec::new()
                            }
                        }
                    } else {
                        Vec::new()
                    };
                update_result = app_for_update.run_on_main_thread(move || {
                    let state = app_for_state.state::<QuickFoxAppState>();
                    match apply_baseline_persistence_outcome_for_identity(
                        &state,
                        &persistence_identity,
                        authoritative_generation,
                        persistence,
                        completed_at_ms,
                        !should_build_content_index,
                        &persistence_tail_deltas,
                    ) {
                        BaselinePersistenceApplicationOutcome::Applied(Some(application)) => {
                            if (!application.completed || !should_build_content_index)
                                && finish_current_index_refresh(&state, &persistence_identity)
                            {
                                let _ =
                                    start_background_index_refresh(app_for_state.clone(), &state);
                                return;
                            }
                            let should_restart = should_restart_after_baseline_persistence(
                                &state,
                                &application,
                                should_build_content_index,
                            );
                            let _ =
                                app_for_state.emit("quickfox://index-status", application.status);
                            if should_restart {
                                let _ = restart_runtime_incremental_indexing(
                                    app_for_state.clone(),
                                    &state,
                                );
                            }
                        }
                        BaselinePersistenceApplicationOutcome::Superseded => {
                            if finish_superseded_index_refresh(&state, &persistence_identity) {
                                let _ =
                                    start_background_index_refresh(app_for_state.clone(), &state);
                            }
                        }
                        BaselinePersistenceApplicationOutcome::Applied(None) => {}
                    }
                });

                if update_result.is_ok()
                    && baseline_persistence_completed
                    && should_build_content_index
                {
                    let content_state = app.state::<QuickFoxAppState>();
                    if !index_refresh_identity_is_current_in_state(&content_state, &identity) {
                        schedule_pending_refresh_after_superseded(app.clone(), identity.clone());
                        return;
                    }
                    let content_progress_report = IndexReport {
                        failures: Vec::new(),
                        scan_stats: IndexScanStats {
                            scanned: content_entries.len(),
                            accepted: content_entries.len(),
                            skipped: 0,
                            failures: 0,
                        },
                        scan_events: Vec::new(),
                        ..Default::default()
                    };
                    let content_progress_payload = IndexRefreshPayload {
                        entries: content_entries.clone(),
                        summary: content_progress_report,
                    };
                    let app_for_update = app.clone();
                    let app_for_state = app_for_update.clone();
                    let content_progress_identity = identity.clone();
                    update_result = app_for_update.run_on_main_thread(move || {
                        let state = app_for_state.state::<QuickFoxAppState>();
                        if let Some(status) = apply_index_refresh_progress_for_identity(
                            &state,
                            &content_progress_identity,
                            "content-index".to_owned(),
                            None,
                            content_progress_payload,
                        ) {
                            let _ = app_for_state.emit("quickfox://index-status", status);
                        }
                    });

                    if update_result.is_ok() {
                        let content_completed_at_ms = current_time_ms();
                        let content_index =
                            build_search_index_with_content_for_config(&config, content_entries);
                        let content_entries = content_index.entries().to_vec();
                        let content_report = IndexReport {
                            failures: Vec::new(),
                            ..Default::default()
                        };
                        let content_payload = IndexRefreshPayload {
                            entries: content_entries,
                            summary: content_report,
                        };
                        let persistence_state = app.state::<QuickFoxAppState>();
                        let manifest = handoff
                            .as_ref()
                            .map(|handoff| handoff.manifest.clone())
                            .unwrap_or_default();
                        let mut persistence = persist_index_refresh_for_identity(
                            &persistence_state,
                            &identity,
                            content_completed_at_ms,
                            content_payload,
                            authoritative_generation,
                            move |completed_at_ms, entries, baseline_generation, _| {
                                persist_and_activate_baseline_with_manifest(
                                    completed_at_ms,
                                    entries,
                                    baseline_generation,
                                    &manifest,
                                )
                            },
                        );
                        let app_for_update = app.clone();
                        let app_for_state = app_for_update.clone();
                        let content_identity = identity.clone();
                        let content_tail_deltas =
                            if matches!(persistence, BaselinePersistenceOutcome::Completed(_)) {
                                match finalize_durable_refresh_successor(
                                    &persistence_state,
                                    &identity,
                                    authoritative_generation,
                                ) {
                                    Ok(tail) => tail,
                                    Err(error) => {
                                        persistence = BaselinePersistenceOutcome::Failed(error);
                                        Vec::new()
                                    }
                                }
                            } else {
                                Vec::new()
                            };
                        update_result = app_for_update.run_on_main_thread(move || {
                            let state = app_for_state.state::<QuickFoxAppState>();
                            match persistence {
                                BaselinePersistenceOutcome::Completed(payload) => {
                                    if let Some(status) =
                                        apply_completed_content_index_refresh_for_identity(
                                            &state,
                                            &content_identity,
                                            authoritative_generation,
                                            content_index,
                                            payload,
                                            content_completed_at_ms,
                                            &content_tail_deltas,
                                        )
                                    {
                                        if finish_current_index_refresh(&state, &content_identity) {
                                            let _ = start_background_index_refresh(
                                                app_for_state.clone(),
                                                &state,
                                            );
                                            return;
                                        }
                                        let _ =
                                            app_for_state.emit("quickfox://index-status", status);
                                        let _ = restart_runtime_incremental_indexing(
                                            app_for_state.clone(),
                                            &state,
                                        );
                                    } else if finish_superseded_index_refresh(
                                        &state,
                                        &content_identity,
                                    ) {
                                        let _ = start_background_index_refresh(
                                            app_for_state.clone(),
                                            &state,
                                        );
                                    }
                                }
                                BaselinePersistenceOutcome::Failed(error) => {
                                    if let Some(status) = apply_failed_index_refresh_for_identity(
                                        &state,
                                        &content_identity,
                                        error,
                                    ) {
                                        if finish_current_index_refresh(&state, &content_identity) {
                                            let _ = start_background_index_refresh(
                                                app_for_state.clone(),
                                                &state,
                                            );
                                            return;
                                        }
                                        let index_remains_available = !matches!(
                                            status.availability,
                                            crate::core::index::IndexAvailability::Unavailable
                                        );
                                        let _ =
                                            app_for_state.emit("quickfox://index-status", status);
                                        if index_remains_available {
                                            let _ = restart_runtime_incremental_indexing(
                                                app_for_state.clone(),
                                                &state,
                                            );
                                        }
                                    } else if finish_superseded_index_refresh(
                                        &state,
                                        &content_identity,
                                    ) {
                                        let _ = start_background_index_refresh(
                                            app_for_state.clone(),
                                            &state,
                                        );
                                    }
                                }
                                BaselinePersistenceOutcome::Superseded => {
                                    if finish_superseded_index_refresh(&state, &content_identity) {
                                        let _ = start_background_index_refresh(
                                            app_for_state.clone(),
                                            &state,
                                        );
                                    }
                                }
                            }
                        });
                    }
                }
            }

            if let Err(error) = update_result {
                eprintln!("QuickFox index refresh dispatch failed: {error}");
                let state = app.state::<QuickFoxAppState>();
                if apply_failed_index_refresh_for_identity(
                    &state,
                    &identity,
                    format!("index refresh dispatch failed: {error}"),
                )
                .is_some()
                {
                    if finish_current_index_refresh(&state, &identity) {
                        let _ = start_background_index_refresh(app.clone(), &state);
                    }
                } else if finish_superseded_index_refresh(&state, &identity) {
                    let _ = start_background_index_refresh(app.clone(), &state);
                }
            }
        }),
    );
    if let Err(failure) = spawn_result {
        if failure.retry {
            let _ = start_background_index_refresh(spawn_failure_app, state);
        }
        return Err(failure.message);
    }

    Ok(status)
}

fn schedule_configured_root_recovery<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: &QuickFoxAppState,
    identity: &IndexRefreshIdentity,
) {
    let _ = schedule_configured_root_recovery_with_spawner(
        app,
        state,
        identity,
        &SystemRootMonitorSpawner,
        Arc::new(SystemRefreshWorkerSpawner),
        std::time::Duration::from_secs(1),
        |app| {
            let dispatch = app.clone();
            app.run_on_main_thread(move || {
                let state = dispatch.state::<QuickFoxAppState>();
                let _ = start_background_index_refresh(dispatch.clone(), &state);
            })
            .map_err(|error| error.to_string())
        },
    );
}

#[cfg(test)]
fn schedule_configured_root_recovery_with<R, F>(
    app: tauri::AppHandle<R>,
    state: &QuickFoxAppState,
    identity: &IndexRefreshIdentity,
    on_ready: F,
) where
    R: tauri::Runtime,
    F: FnOnce(tauri::AppHandle<R>) + Send + 'static,
{
    let _ = schedule_configured_root_recovery_with_spawner(
        app,
        state,
        identity,
        &SystemRootMonitorSpawner,
        Arc::new(SystemRefreshWorkerSpawner),
        std::time::Duration::from_secs(1),
        move |app| {
            on_ready(app);
            Ok(())
        },
    );
}

fn schedule_configured_root_recovery_with_spawner<R, F>(
    app: tauri::AppHandle<R>,
    state: &QuickFoxAppState,
    identity: &IndexRefreshIdentity,
    spawner: &dyn RootMonitorSpawner,
    recovery_spawner: Arc<dyn RefreshWorkerSpawner>,
    interval: std::time::Duration,
    on_ready: F,
) -> Result<(), String>
where
    R: tauri::Runtime,
    F: FnOnce(tauri::AppHandle<R>) -> Result<(), String> + Send + 'static,
{
    let previous_monitor = {
        let mut runtime = state
            .runtime
            .lock()
            .expect("quickfox runtime lock poisoned");
        if runtime.index_refresh.config_revision != identity.config_revision
            || runtime.index_refresh.config_fingerprint != identity.config_fingerprint
            || runtime.index_refresh.root_recovery_latch.claimed_revision()
                == Some(identity.config_revision)
        {
            return Ok(());
        }
        runtime
            .index_refresh
            .root_recovery_latch
            .claim(identity.config_revision);
        runtime.incremental_status.dirty_roots = build_scan_options(&runtime.config)
            .include_dirs
            .iter()
            .filter(|root| !root.is_dir())
            .count();
        runtime.index_refresh.root_monitor.take()
    };
    if let Some(mut previous_monitor) = previous_monitor {
        previous_monitor.cancel_and_join();
    }
    let revision = identity.config_revision;
    let fingerprint = identity.config_fingerprint.clone();
    let probe_app = app.clone();
    let ready_app = app.clone();
    let failure_app = app.clone();
    let ready_fingerprint = fingerprint.clone();
    let completion_app = app.clone();
    let handle = spawn_root_availability_monitor_with_completion_gate(
        spawner,
        interval,
        move || {
            let state = probe_app.state::<QuickFoxAppState>();
            let runtime = state
                .runtime
                .lock()
                .expect("quickfox runtime lock poisoned");
            if runtime.index_refresh.config_revision != revision
                || runtime.index_refresh.config_fingerprint != fingerprint
            {
                return Ok(false);
            }
            Ok(build_scan_options(&runtime.config)
                .include_dirs
                .iter()
                .all(|root| root.is_dir()))
        },
        move || {
            let state = ready_app.state::<QuickFoxAppState>();
            {
                let mut runtime = state
                    .runtime
                    .lock()
                    .expect("quickfox runtime lock poisoned");
                if runtime.index_refresh.config_revision != revision
                    || runtime.index_refresh.config_fingerprint != ready_fingerprint
                {
                    return Ok(());
                }
                runtime.index_refresh.root_recovery_latch.clear(revision);
                runtime.index_refresh.root_monitor_failure_revision = None;
                runtime.incremental_status.dirty_roots = 0;
            }
            on_ready(ready_app.clone())
        },
        move |outcome, completion| {
            if matches!(
                outcome,
                MonitorExit::ProbeFailed(_)
                    | MonitorExit::DispatchFailed(_)
                    | MonitorExit::ThreadPanicked
            ) && apply_root_monitor_failure_for_revision(&completion_app, revision)
            {
                let retry_app = completion_app.clone();
                let spawn_result = recovery_spawner.spawn(Box::new(move || {
                    completion.wait();
                    let state = retry_app.state::<QuickFoxAppState>();
                    let _ = start_background_index_refresh(retry_app.clone(), &state);
                }));
                if spawn_result.is_err() {
                    clear_failed_recovery_spawn_claim(&completion_app, revision);
                }
            }
        },
    );
    let handle = match handle {
        Ok(handle) => handle,
        Err(error) => {
            if apply_root_monitor_failure_for_revision(&failure_app, revision) {
                let state = failure_app.state::<QuickFoxAppState>();
                let _ = start_background_index_refresh(failure_app.clone(), &state);
            }
            return Err(error);
        }
    };
    let mut handle = Some(handle);
    {
        let mut runtime = state
            .runtime
            .lock()
            .expect("quickfox runtime lock poisoned");
        if runtime.index_refresh.config_revision == revision
            && runtime.index_refresh.root_recovery_latch.claimed_revision() == Some(revision)
        {
            runtime.index_refresh.root_monitor = handle.take();
        }
    }
    if let Some(mut handle) = handle {
        handle.cancel_and_join();
    }
    Ok(())
}

fn clear_failed_recovery_spawn_claim<R: tauri::Runtime>(app: &tauri::AppHandle<R>, revision: u64) {
    let state = app.state::<QuickFoxAppState>();
    let mut runtime = state
        .runtime
        .lock()
        .expect("quickfox runtime lock poisoned");
    if runtime.index_refresh.config_revision == revision {
        runtime.index_refresh.root_monitor_failure_revision = None;
        runtime.index_refresh.root_recovery_latch.clear(revision);
        runtime.index_refresh.pending = true;
        runtime.incremental_status.state = IncrementalState::Degraded;
    }
}

fn apply_root_monitor_failure_for_revision<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    revision: u64,
) -> bool {
    let state = app.state::<QuickFoxAppState>();
    let mut runtime = state
        .runtime
        .lock()
        .expect("quickfox runtime lock poisoned");
    if runtime.index_refresh.config_revision != revision {
        return false;
    }
    let request_recovery = runtime.index_refresh.root_monitor_failure_revision != Some(revision);
    if request_recovery {
        runtime.index_refresh.root_monitor_failure_revision = Some(revision);
    }
    runtime.index_refresh.root_recovery_latch.clear(revision);
    apply_runtime_failure_state(
        &mut runtime,
        RuntimeFailureKind::Monitor,
        IndexDegradationCode::CalibrationFailed,
        false,
    );
    request_recovery
}

struct RefreshWorkerSpawnFailure {
    message: String,
    retry: bool,
}

fn spawn_refresh_worker_for_identity(
    state: &QuickFoxAppState,
    identity: &IndexRefreshIdentity,
    spawner: &dyn RefreshWorkerSpawner,
    task: Box<dyn FnOnce() + Send>,
) -> Result<(), RefreshWorkerSpawnFailure> {
    spawner
        .spawn(task)
        .map_err(|message| RefreshWorkerSpawnFailure {
            retry: apply_refresh_worker_spawn_failure(state, identity, message.clone()),
            message,
        })
}

fn apply_refresh_worker_spawn_failure(
    state: &QuickFoxAppState,
    identity: &IndexRefreshIdentity,
    error: String,
) -> bool {
    record_index_refresh_runtime_failure(state, identity, RuntimeRestartFailureKind::WorkerSpawn);
    let _ = apply_failed_index_refresh_for_identity(state, identity, error);
    finish_current_index_refresh(state, identity)
}

fn schedule_pending_refresh_after_superseded<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    identity: IndexRefreshIdentity,
) {
    let dispatch = app.clone();
    let fallback_app = app.clone();
    let fallback_identity = identity.clone();
    if app
        .run_on_main_thread(move || {
            let state = dispatch.state::<QuickFoxAppState>();
            if finish_superseded_index_refresh(&state, &identity) {
                let _ = start_background_index_refresh(dispatch.clone(), &state);
            }
        })
        .is_err()
    {
        let state = fallback_app.state::<QuickFoxAppState>();
        if finish_superseded_index_refresh(&state, &fallback_identity) {
            let _ = start_background_index_refresh(fallback_app.clone(), &state);
        }
    }
}

fn persist_and_activate_baseline_with_manifest(
    completed_at_ms: i64,
    entries: &[IndexedEntry],
    baseline_generation: u64,
    manifest: &[DirectoryFingerprint],
) -> Result<(), String> {
    let storage = storage_store().ok_or_else(|| "index storage is unavailable".to_owned())?;
    let baseline_id = storage
        .save_completed_index_batch(completed_at_ms, entries)
        .map_err(|error| error.to_string())?;
    storage
        .activate_baseline_with_manifest_and_clear_incremental_state(
            baseline_id,
            baseline_generation,
            manifest,
        )
        .map_err(|error| error.to_string())
}

fn prepare_index_refresh_handoff(
    state: &QuickFoxAppState,
    identity: &IndexRefreshIdentity,
    config: &QuickFoxConfig,
    entries: &[IndexedEntry],
    scan_start_generation: u64,
) -> Result<Option<IndexRefreshHandoff>, String> {
    let handoff_parts = {
        let _fence = state
            .index_refresh_fence
            .lock()
            .expect("index refresh fence poisoned");
        let mut runtime = state
            .runtime
            .lock()
            .expect("quickfox runtime lock poisoned");
        if !index_refresh_identity_is_current(&runtime, identity) {
            Ok(None)
        } else if let Some(capture_watcher) = runtime.index_refresh.standby_watcher.take() {
            let roots = capture_watcher.watched_roots().to_vec();
            if roots.is_empty() {
                Err("index refresh has no watchable roots".to_owned())
            } else {
                runtime.index_refresh.active_service = None;
                Ok(Some((
                    runtime.runtime_indexing.take(),
                    capture_watcher,
                    roots,
                    runtime.index_refresh.revision_capture_fence.take(),
                )))
            }
        } else {
            Err("index refresh standby watcher is unavailable".to_owned())
        }
    };
    let (previous, capture_watcher, roots, revision_capture_fence) = match handoff_parts {
        Ok(Some(parts)) => parts,
        Ok(None) => return Ok(None),
        Err(error) => {
            record_index_refresh_runtime_failure(
                state,
                identity,
                RuntimeRestartFailureKind::Handoff,
            );
            return Err(error);
        }
    };
    if let Some(previous) = previous {
        if previous.handoff() == RuntimeIndexingHandoffOutcome::RecoveryRequired {
            record_index_refresh_runtime_failure(
                state,
                identity,
                RuntimeRestartFailureKind::Handoff,
            );
            return Err("runtime index handoff requires a recovery scan".to_owned());
        }
    }

    let successor_watcher = RuntimeIndexWatcher::watch_roots(roots.clone()).map_err(|failure| {
        record_index_refresh_runtime_failure(state, identity, RuntimeRestartFailureKind::Watcher);
        failure.message
    })?;
    let capture_storage = storage_store().ok_or_else(|| {
        record_index_refresh_runtime_failure(state, identity, RuntimeRestartFailureKind::Storage);
        "index journal storage is unavailable".to_owned()
    })?;
    let capture_generation = capture_storage
        .highest_committed_generation()
        .map_err(|error| {
            record_index_refresh_runtime_failure(
                state,
                identity,
                RuntimeRestartFailureKind::Storage,
            );
            error.to_string()
        })?;
    let options = build_scan_options(config);
    let rules = IndexPathRules::from_plan(&IndexScanPlan {
        include_roots: roots.clone(),
        exclude_dirs: options.exclude_dirs,
        exclude_patterns: options.exclude_patterns,
        respect_project_ignores: options.respect_project_ignores,
        stage: None,
    })
    .map_err(|error| {
        record_index_refresh_runtime_failure(state, identity, RuntimeRestartFailureKind::Rules);
        error.to_string()
    })?;
    let capture_handle = start_runtime_indexing(
        capture_watcher,
        TargetedIndexScanner::new(rules.clone()),
        Box::new(capture_storage),
        RuntimeIndexingOptions {
            roots: roots.clone(),
            policy: crate::core::index_update_coordinator::CoordinatorPolicy::production(),
            initial_generation: capture_generation,
        },
        |_| {},
    )
    .map_err(|failure| {
        record_index_refresh_runtime_failure(
            state,
            identity,
            RuntimeRestartFailureKind::WorkerSpawn,
        );
        failure.message
    })?;
    if capture_handle.handoff() == RuntimeIndexingHandoffOutcome::RecoveryRequired {
        record_index_refresh_runtime_failure(state, identity, RuntimeRestartFailureKind::Handoff);
        return Err("standby capture handoff requires a recovery scan".to_owned());
    }

    let successor_storage = storage_store().ok_or_else(|| {
        record_index_refresh_runtime_failure(state, identity, RuntimeRestartFailureKind::Storage);
        "index journal storage is unavailable".to_owned()
    })?;
    let successor_generation = successor_storage
        .highest_committed_generation()
        .map_err(|error| error.to_string())?;
    install_durable_refresh_successor(
        state,
        identity,
        successor_watcher,
        rules,
        roots.clone(),
        successor_storage,
        successor_generation,
    )?;

    let storage = storage_store().ok_or_else(|| {
        record_index_refresh_runtime_failure(state, identity, RuntimeRestartFailureKind::Storage);
        "index journal storage is unavailable".to_owned()
    })?;
    let (tail_deltas, manifest) = load_revision_compatible_handoff_snapshot(
        &storage,
        scan_start_generation,
        revision_capture_fence,
        &roots,
        entries,
    )
    .inspect_err(|_| {
        record_index_refresh_runtime_failure(state, identity, RuntimeRestartFailureKind::Storage);
    })?;

    Ok(Some(IndexRefreshHandoff {
        authoritative_generation: authoritative_install_generation(
            scan_start_generation,
            &tail_deltas
                .iter()
                .map(|delta| delta.generation)
                .collect::<Vec<_>>(),
        ),
        tail_deltas,
        manifest,
    }))
}

fn install_durable_refresh_successor(
    state: &QuickFoxAppState,
    identity: &IndexRefreshIdentity,
    watcher: RuntimeIndexWatcher,
    rules: IndexPathRules,
    roots: Vec<PathBuf>,
    storage: SqliteStorage,
    initial_generation: u64,
) -> Result<(), String> {
    let handle = start_runtime_indexing(
        watcher,
        TargetedIndexScanner::new(rules),
        Box::new(storage),
        RuntimeIndexingOptions {
            roots,
            policy: crate::core::index_update_coordinator::CoordinatorPolicy::production(),
            initial_generation,
        },
        |_| {},
    )
    .map_err(|failure| {
        record_index_refresh_runtime_failure(
            state,
            identity,
            RuntimeRestartFailureKind::WorkerSpawn,
        );
        failure.message
    })?;
    let _fence = state
        .index_refresh_fence
        .lock()
        .expect("index refresh fence poisoned");
    let mut runtime = state
        .runtime
        .lock()
        .expect("quickfox runtime lock poisoned");
    if !index_refresh_identity_is_current(&runtime, identity) {
        drop(runtime);
        handle.stop();
        return Err("index refresh was superseded before successor activation".to_owned());
    }
    runtime.runtime_indexing = Some(handle);
    runtime.incremental_status.state = IncrementalState::Preparing;
    Ok(())
}

fn record_index_refresh_runtime_failure(
    state: &QuickFoxAppState,
    identity: &IndexRefreshIdentity,
    failure: RuntimeRestartFailureKind,
) {
    let handle_to_stop = {
        let mut runtime = state
            .runtime
            .lock()
            .expect("quickfox runtime lock poisoned");
        if !index_refresh_identity_is_current(&runtime, identity) {
            return;
        }
        let application = record_runtime_restart_failure(&mut runtime, failure);
        if application.request_recovery {
            runtime.index_refresh.pending = true;
        }
        application.handle_to_stop
    };
    if let Some(handle) = handle_to_stop {
        handle.stop();
    }
}

fn detach_runtime_indexing_for_refresh(
    state: &QuickFoxAppState,
    identity: &IndexRefreshIdentity,
) -> Option<RuntimeIndexingHandle> {
    let _fence = state
        .index_refresh_fence
        .lock()
        .expect("index refresh fence poisoned");
    let mut runtime = state
        .runtime
        .lock()
        .expect("quickfox runtime lock poisoned");
    if !index_refresh_identity_is_current(&runtime, identity) {
        return None;
    }
    runtime.index_refresh.active_service = None;
    runtime.runtime_indexing.take()
}

fn finalize_durable_refresh_successor(
    state: &QuickFoxAppState,
    identity: &IndexRefreshIdentity,
    baseline_generation: u64,
) -> Result<Vec<CommittedIndexDelta>, String> {
    let storage =
        storage_store().ok_or_else(|| "index journal storage is unavailable".to_owned())?;
    finalize_durable_refresh_successor_with_storage(state, identity, baseline_generation, &storage)
}

fn finalize_durable_refresh_successor_with_storage(
    state: &QuickFoxAppState,
    identity: &IndexRefreshIdentity,
    baseline_generation: u64,
    storage: &SqliteStorage,
) -> Result<Vec<CommittedIndexDelta>, String> {
    let (config, recovered_roots) = {
        let runtime = state
            .runtime
            .lock()
            .expect("quickfox runtime lock poisoned");
        (runtime.config.clone(), runtime.index.watched_roots())
    };
    let roots = refresh_capture_roots(&config, recovered_roots)?;
    let standby = RuntimeIndexWatcher::watch_roots(roots).map_err(|failure| failure.message)?;
    let handle = {
        let _fence = state
            .index_refresh_fence
            .lock()
            .expect("index refresh fence poisoned");
        let mut runtime = state
            .runtime
            .lock()
            .expect("quickfox runtime lock poisoned");
        if !index_refresh_identity_is_current(&runtime, identity) {
            return Err("index refresh was superseded before successor handoff".to_owned());
        }
        runtime.index_refresh.active_service = None;
        runtime.index_refresh.standby_watcher = Some(standby);
        runtime
            .runtime_indexing
            .take()
            .ok_or_else(|| "durable refresh successor is unavailable".to_owned())?
    };
    if handle.handoff() == RuntimeIndexingHandoffOutcome::RecoveryRequired {
        record_index_refresh_runtime_failure(state, identity, RuntimeRestartFailureKind::Handoff);
        return Err("durable refresh successor requires recovery".to_owned());
    }
    storage
        .committed_index_deltas_after(baseline_generation)
        .map_err(|error| error.to_string())
}

fn load_full_refresh_handoff_snapshot(
    storage: &SqliteStorage,
    scan_start_generation: u64,
    roots: &[PathBuf],
    scanned_entries: &[IndexedEntry],
) -> Result<(Vec<CommittedIndexDelta>, Vec<DirectoryFingerprint>), String> {
    let tail_deltas = storage
        .committed_index_deltas_after(scan_start_generation)
        .map_err(|error| error.to_string())?;
    let final_entries = entries_after_committed_deltas(scanned_entries.to_vec(), &tail_deltas);
    let mut manifest_by_path: std::collections::BTreeMap<String, DirectoryFingerprint> =
        baseline_manifest_from_entries(&final_entries, roots)
            .into_iter()
            .map(|row| (normalize_path_text_key(&row.path), row))
            .collect();
    let touched_paths: Vec<PathBuf> = tail_deltas
        .iter()
        .flat_map(|delta| {
            delta
                .upserts
                .iter()
                .map(|entry| PathBuf::from(&entry.path))
                .chain(delta.removals.iter().cloned())
        })
        .collect();
    refresh_tail_touched_manifest_fingerprints(&mut manifest_by_path, &touched_paths, roots);
    Ok((tail_deltas, manifest_by_path.into_values().collect()))
}

fn load_revision_compatible_handoff_snapshot(
    storage: &SqliteStorage,
    scan_start_generation: u64,
    revision_capture_fence: Option<RevisionCaptureFence>,
    roots: &[PathBuf],
    scanned_entries: &[IndexedEntry],
) -> Result<(Vec<CommittedIndexDelta>, Vec<DirectoryFingerprint>), String> {
    let compatible_tail_start =
        compatible_tail_start_generation(scan_start_generation, revision_capture_fence);
    load_full_refresh_handoff_snapshot(storage, compatible_tail_start, roots, scanned_entries)
}

fn refresh_tail_touched_manifest_fingerprints(
    manifest_by_path: &mut std::collections::BTreeMap<String, DirectoryFingerprint>,
    touched_paths: &[PathBuf],
    roots: &[PathBuf],
) {
    let comparison_mode = PathComparisonMode::native();
    for touched in touched_paths {
        let Some(root) = roots
            .iter()
            .filter(|root| path_is_same_or_descendant_for_mode(root, touched, comparison_mode))
            .max_by_key(|root| root.components().count())
        else {
            continue;
        };
        let mut candidate = if touched.is_dir() {
            Some(touched.as_path())
        } else {
            touched.parent()
        };
        while let Some(directory) = candidate {
            if !path_is_same_or_descendant_for_mode(root, directory, comparison_mode) {
                break;
            }
            if let Some(row) = manifest_by_path.get_mut(&normalize_path_key(directory)) {
                row.modified_ms = std::fs::metadata(directory)
                    .ok()
                    .and_then(|metadata| metadata.modified().ok())
                    .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|duration| duration.as_millis() as i64);
            }
            if directory == root {
                break;
            }
            candidate = directory.parent();
        }
    }
}

fn baseline_manifest_after_committed_deltas(
    entries: &[IndexedEntry],
    tail_deltas: &[CommittedIndexDelta],
    roots: &[PathBuf],
) -> Vec<DirectoryFingerprint> {
    let mut manifest_by_path: std::collections::BTreeMap<String, DirectoryFingerprint> =
        baseline_manifest_from_entries(entries, roots)
            .into_iter()
            .map(|row| (normalize_path_text_key(&row.path), row))
            .collect();
    let touched_paths: Vec<PathBuf> = tail_deltas
        .iter()
        .flat_map(|delta| {
            delta
                .upserts
                .iter()
                .map(|entry| PathBuf::from(&entry.path))
                .chain(delta.removals.iter().cloned())
        })
        .collect();
    refresh_tail_touched_manifest_fingerprints(&mut manifest_by_path, &touched_paths, roots);
    manifest_by_path.into_values().collect()
}

fn prepare_refresh_standby_capture(
    state: &QuickFoxAppState,
    identity: &IndexRefreshIdentity,
    config: &QuickFoxConfig,
) -> Result<(), String> {
    let recovered_roots = state
        .runtime
        .lock()
        .expect("quickfox runtime lock poisoned")
        .index
        .watched_roots();
    let roots = refresh_capture_roots(config, recovered_roots)?;
    let _fence = state
        .index_refresh_fence
        .lock()
        .expect("index refresh fence poisoned");
    let mut runtime = state
        .runtime
        .lock()
        .expect("quickfox runtime lock poisoned");
    if !index_refresh_identity_is_current(&runtime, identity) {
        return Err("index refresh was superseded before standby capture".to_owned());
    }
    if runtime
        .index_refresh
        .standby_watcher
        .as_ref()
        .is_some_and(|watcher| watcher.watched_roots() == roots)
    {
        return Ok(());
    }
    let watcher = RuntimeIndexWatcher::watch_roots(roots).map_err(|failure| failure.message)?;
    runtime.index_refresh.standby_watcher = Some(watcher);
    Ok(())
}

fn refresh_capture_roots(
    config: &QuickFoxConfig,
    recovered_roots: Vec<PathBuf>,
) -> Result<Vec<PathBuf>, String> {
    let configured_roots = build_scan_options(config).include_dirs;
    if configured_roots.iter().any(|root| !root.is_dir()) {
        return Err("one or more configured index roots are unavailable".to_owned());
    }
    let roots = startup_calibration_roots_from_plans(
        configured_roots,
        recovered_roots,
        build_scan_plans(config),
    )
    .into_iter()
    .filter(|root| root.is_dir())
    .collect::<Vec<_>>();
    if roots.is_empty() {
        return Err("index refresh has no watchable roots".to_owned());
    }
    Ok(roots)
}

#[derive(Debug)]
enum BaselinePersistenceOutcome {
    Completed(IndexRefreshPayload),
    Failed(String),
    Superseded,
}

struct BaselinePersistenceApplication {
    status: IndexStatus,
    completed: bool,
}

enum BaselinePersistenceApplicationOutcome {
    Applied(Option<BaselinePersistenceApplication>),
    Superseded,
}

fn persist_index_refresh_with(
    completed_at_ms: i64,
    payload: IndexRefreshPayload,
    baseline_generation: u64,
    config: &QuickFoxConfig,
    persist: impl FnOnce(i64, &[IndexedEntry], u64, &QuickFoxConfig) -> Result<(), String>,
) -> BaselinePersistenceOutcome {
    match persist(
        completed_at_ms,
        &payload.entries,
        baseline_generation,
        config,
    ) {
        Ok(()) => BaselinePersistenceOutcome::Completed(payload),
        Err(error) => BaselinePersistenceOutcome::Failed(error),
    }
}

fn persist_index_refresh_for_identity(
    state: &QuickFoxAppState,
    identity: &IndexRefreshIdentity,
    completed_at_ms: i64,
    payload: IndexRefreshPayload,
    baseline_generation: u64,
    persist: impl FnOnce(i64, &[IndexedEntry], u64, &QuickFoxConfig) -> Result<(), String>,
) -> BaselinePersistenceOutcome {
    let _fence = state
        .index_refresh_fence
        .lock()
        .expect("index refresh fence poisoned");
    let config = {
        let runtime = state
            .runtime
            .lock()
            .expect("quickfox runtime lock poisoned");
        if !index_refresh_identity_is_current(&runtime, identity) {
            return BaselinePersistenceOutcome::Superseded;
        }
        runtime.config.clone()
    };
    persist_index_refresh_with(
        completed_at_ms,
        payload,
        baseline_generation,
        &config,
        persist,
    )
}

fn index_refresh_identity_is_current(
    runtime: &QuickFoxRuntime,
    identity: &IndexRefreshIdentity,
) -> bool {
    runtime.index_refresh.active.as_ref() == Some(identity)
        && runtime.index_refresh.config_revision == identity.config_revision
        && runtime.index_refresh.config_fingerprint == identity.config_fingerprint
}

fn index_refresh_identity_is_current_in_state(
    state: &QuickFoxAppState,
    identity: &IndexRefreshIdentity,
) -> bool {
    index_refresh_identity_is_current(
        &state
            .runtime
            .lock()
            .expect("quickfox runtime lock poisoned"),
        identity,
    )
}

fn finish_superseded_index_refresh(
    state: &QuickFoxAppState,
    identity: &IndexRefreshIdentity,
) -> bool {
    let mut runtime = state
        .runtime
        .lock()
        .expect("quickfox runtime lock poisoned");
    if runtime.index_refresh.active.as_ref() == Some(identity) {
        runtime.index_refresh.active = None;
    }
    runtime.index_refresh.pending
        && (runtime.index_refresh.config_revision != identity.config_revision
            || runtime.index_refresh.config_fingerprint != identity.config_fingerprint)
}

fn finish_current_index_refresh(state: &QuickFoxAppState, identity: &IndexRefreshIdentity) -> bool {
    let _fence = state
        .index_refresh_fence
        .lock()
        .expect("index refresh fence poisoned");
    let mut runtime = state
        .runtime
        .lock()
        .expect("quickfox runtime lock poisoned");
    if index_refresh_identity_is_current(&runtime, identity) {
        let pending = runtime.index_refresh.pending;
        runtime.index_refresh.active = None;
        runtime.index_refresh.pending = false;
        return pending;
    }
    if runtime.index_refresh.active.as_ref() == Some(identity) {
        runtime.index_refresh.active = None;
    }
    runtime.index_refresh.pending
        && (runtime.index_refresh.config_revision != identity.config_revision
            || runtime.index_refresh.config_fingerprint != identity.config_fingerprint)
}

fn persist_checkpoint_for_identity(
    state: &QuickFoxAppState,
    identity: &IndexRefreshIdentity,
    completed_at_ms: i64,
    entries: &[IndexedEntry],
) -> bool {
    let _fence = state
        .index_refresh_fence
        .lock()
        .expect("index refresh fence poisoned");
    if !index_refresh_identity_is_current(
        &state
            .runtime
            .lock()
            .expect("quickfox runtime lock poisoned"),
        identity,
    ) {
        return false;
    }
    if let Some(storage) = storage_store() {
        let _ = storage.save_completed_index_batch(completed_at_ms, entries);
    }
    true
}

fn apply_index_refresh_progress_for_identity(
    state: &QuickFoxAppState,
    identity: &IndexRefreshIdentity,
    stage: String,
    current_root: Option<String>,
    payload: IndexRefreshPayload,
) -> Option<IndexStatus> {
    let _fence = state
        .index_refresh_fence
        .lock()
        .expect("index refresh fence poisoned");
    if !index_refresh_identity_is_current(
        &state
            .runtime
            .lock()
            .expect("quickfox runtime lock poisoned"),
        identity,
    ) {
        return None;
    }
    apply_index_refresh_progress(
        state,
        identity.lifecycle_generation,
        stage,
        current_root,
        payload,
    )
}

fn apply_failed_index_refresh_for_identity(
    state: &QuickFoxAppState,
    identity: &IndexRefreshIdentity,
    message: String,
) -> Option<IndexStatus> {
    let _fence = state
        .index_refresh_fence
        .lock()
        .expect("index refresh fence poisoned");
    if !index_refresh_identity_is_current(
        &state
            .runtime
            .lock()
            .expect("quickfox runtime lock poisoned"),
        identity,
    ) {
        return None;
    }
    apply_failed_index_refresh(state, identity.lifecycle_generation, message)
}

fn apply_baseline_persistence_outcome_for_identity(
    state: &QuickFoxAppState,
    identity: &IndexRefreshIdentity,
    baseline_generation: u64,
    outcome: BaselinePersistenceOutcome,
    completed_at_ms: i64,
    complete_lifecycle: bool,
    tail_deltas: &[CommittedIndexDelta],
) -> BaselinePersistenceApplicationOutcome {
    let _fence = state
        .index_refresh_fence
        .lock()
        .expect("index refresh fence poisoned");
    if !index_refresh_identity_is_current(
        &state
            .runtime
            .lock()
            .expect("quickfox runtime lock poisoned"),
        identity,
    ) {
        return BaselinePersistenceApplicationOutcome::Superseded;
    }
    let persistence_failed = matches!(&outcome, BaselinePersistenceOutcome::Failed(_));
    let mut application = match outcome {
        BaselinePersistenceOutcome::Completed(payload) => {
            let mut runtime = state
                .runtime
                .lock()
                .expect("quickfox runtime lock poisoned");
            let entry_count = payload.entries.len();
            let baseline = build_search_index_for_config(&runtime.config, payload.entries);
            let installed = runtime.index.replace_baseline_with_authoritative_tail(
                baseline,
                baseline_generation,
                tail_deltas,
            );
            let lifecycle_completed = !complete_lifecycle
                || runtime.index_lifecycle.complete_refresh(
                    identity.lifecycle_generation,
                    entry_count,
                    completed_at_ms,
                );
            if installed {
                runtime.manifest_ready = true;
                runtime.last_report = payload.summary;
            }
            Some(BaselinePersistenceApplication {
                status: runtime.index_status(),
                completed: installed && lifecycle_completed,
            })
        }
        outcome => apply_baseline_persistence_outcome(
            state,
            identity.lifecycle_generation,
            baseline_generation,
            outcome,
            completed_at_ms,
        ),
    };
    if persistence_failed {
        let handle_to_stop = {
            let mut runtime = state
                .runtime
                .lock()
                .expect("quickfox runtime lock poisoned");
            let handle = apply_runtime_failure_state(
                &mut runtime,
                RuntimeFailureKind::BaselinePersistence,
                IndexDegradationCode::FullRefreshFallback,
                true,
            );
            if let Some(application) = application.as_mut() {
                application.status = runtime.index_status();
            }
            handle
        };
        if let Some(handle) = handle_to_stop {
            handle.stop();
        }
    }
    BaselinePersistenceApplicationOutcome::Applied(application)
}

fn apply_completed_content_index_refresh_for_identity(
    state: &QuickFoxAppState,
    identity: &IndexRefreshIdentity,
    baseline_generation: u64,
    content_index: SearchIndex,
    payload: IndexRefreshPayload,
    completed_at_ms: i64,
    tail_deltas: &[CommittedIndexDelta],
) -> Option<IndexStatus> {
    let _fence = state
        .index_refresh_fence
        .lock()
        .expect("index refresh fence poisoned");
    if !index_refresh_identity_is_current(
        &state
            .runtime
            .lock()
            .expect("quickfox runtime lock poisoned"),
        identity,
    ) {
        return None;
    }
    if tail_deltas.is_empty() {
        return apply_completed_content_index_refresh(
            state,
            identity.lifecycle_generation,
            baseline_generation,
            content_index,
            payload,
            completed_at_ms,
        );
    }
    let mut runtime = state
        .runtime
        .lock()
        .expect("quickfox runtime lock poisoned");
    let entry_count = payload.entries.len();
    if !runtime.index.replace_baseline_with_authoritative_tail(
        content_index,
        baseline_generation,
        tail_deltas,
    ) || !runtime.index_lifecycle.complete_refresh(
        identity.lifecycle_generation,
        entry_count,
        completed_at_ms,
    ) {
        return None;
    }
    runtime.manifest_ready = true;
    runtime.last_report = payload.summary;
    Some(runtime.index_status())
}

fn entries_after_committed_deltas(
    entries: Vec<IndexedEntry>,
    deltas: &[CommittedIndexDelta],
) -> Vec<IndexedEntry> {
    let mut by_path: std::collections::BTreeMap<String, IndexedEntry> = entries
        .into_iter()
        .map(|entry| (normalize_path_text_key(&entry.path), entry))
        .collect();
    let comparison_mode = PathComparisonMode::native();
    for delta in deltas {
        for removal in &delta.removals {
            let removal_key = normalize_path_key(removal);
            let removes_directory = by_path
                .get(&removal_key)
                .is_some_and(|entry| entry.kind == IndexedEntryKind::Directory);
            if removes_directory {
                by_path.retain(|_, entry| {
                    !path_is_same_or_descendant_for_mode(
                        removal,
                        std::path::Path::new(&entry.path),
                        comparison_mode,
                    )
                });
            } else {
                by_path.remove(&removal_key);
            }
        }
        for entry in &delta.upserts {
            let entry_key = normalize_path_text_key(&entry.path);
            let replaces_directory = entry.kind != IndexedEntryKind::Directory
                && by_path
                    .get(&entry_key)
                    .is_some_and(|existing| existing.kind == IndexedEntryKind::Directory);
            if replaces_directory {
                by_path.retain(|_, existing| {
                    normalize_path_text_key(&existing.path) == entry_key
                        || !path_is_same_or_descendant_for_mode(
                            std::path::Path::new(&entry.path),
                            std::path::Path::new(&existing.path),
                            comparison_mode,
                        )
                });
            }
            by_path.insert(entry_key, entry.clone());
        }
    }
    by_path.into_values().collect()
}

fn apply_baseline_persistence_outcome(
    state: &QuickFoxAppState,
    generation: u64,
    baseline_generation: u64,
    outcome: BaselinePersistenceOutcome,
    completed_at_ms: i64,
) -> Option<BaselinePersistenceApplication> {
    match outcome {
        BaselinePersistenceOutcome::Completed(payload) => apply_completed_index_refresh(
            state,
            generation,
            baseline_generation,
            payload,
            completed_at_ms,
        )
        .map(|status| BaselinePersistenceApplication {
            status,
            completed: true,
        }),
        BaselinePersistenceOutcome::Failed(error) => {
            apply_failed_index_refresh(state, generation, error).map(|status| {
                BaselinePersistenceApplication {
                    status,
                    completed: false,
                }
            })
        }
        BaselinePersistenceOutcome::Superseded => None,
    }
}

fn should_restart_after_baseline_persistence(
    state: &QuickFoxAppState,
    application: &BaselinePersistenceApplication,
    should_build_content_index: bool,
) -> bool {
    if application.completed {
        return !should_build_content_index;
    }
    if matches!(
        application.status.availability,
        crate::core::index::IndexAvailability::Unavailable
    ) {
        return false;
    }
    let runtime = state
        .runtime
        .lock()
        .expect("quickfox runtime lock poisoned");
    let roots = runtime_watch_roots(&runtime.config, &runtime.index);
    runtime_incremental_start_allowed(&runtime, &roots)
}

fn apply_completed_index_refresh(
    state: &QuickFoxAppState,
    generation: u64,
    baseline_generation: u64,
    payload: impl Into<IndexRefreshPayload>,
    completed_at_ms: i64,
) -> Option<IndexStatus> {
    let payload = payload.into();
    let mut runtime = state
        .runtime
        .lock()
        .expect("quickfox runtime lock poisoned");
    let entry_count = payload.entries.len();
    let baseline = build_search_index_for_config(&runtime.config, payload.entries);
    if !runtime
        .index
        .replace_baseline_with_authoritative_tail(baseline, baseline_generation, &[])
        || !runtime
            .index_lifecycle
            .complete_refresh(generation, entry_count, completed_at_ms)
    {
        return None;
    }
    runtime.manifest_ready = true;
    runtime.last_report = payload.summary;
    Some(runtime.index_status())
}

fn apply_completed_content_index_refresh(
    state: &QuickFoxAppState,
    generation: u64,
    baseline_generation: u64,
    content_index: SearchIndex,
    payload: impl Into<IndexRefreshPayload>,
    completed_at_ms: i64,
) -> Option<IndexStatus> {
    let payload = payload.into();
    let mut runtime = state
        .runtime
        .lock()
        .expect("quickfox runtime lock poisoned");
    let entry_count = payload.entries.len();
    if !runtime.index.replace_baseline_with_authoritative_tail(
        content_index,
        baseline_generation,
        &[],
    ) || !runtime
        .index_lifecycle
        .complete_refresh(generation, entry_count, completed_at_ms)
    {
        return None;
    }
    runtime.manifest_ready = true;
    runtime.last_report = payload.summary;
    Some(runtime.index_status())
}

fn record_runtime_restart_failure(
    runtime: &mut QuickFoxRuntime,
    failure: RuntimeRestartFailureKind,
) -> RuntimeRestartFailureApplication {
    let request_recovery = runtime.index_refresh.restart_recovery_revision
        != Some(runtime.index_refresh.config_revision);
    runtime.index_refresh.standby_watcher.take();
    runtime.manifest_ready = false;
    let (kind, degradation_code) = match failure {
        RuntimeRestartFailureKind::Watcher => (
            RuntimeFailureKind::Watcher,
            IndexDegradationCode::WatcherInitializationFailed,
        ),
        RuntimeRestartFailureKind::Rules
        | RuntimeRestartFailureKind::Storage
        | RuntimeRestartFailureKind::WorkerSpawn
        | RuntimeRestartFailureKind::Handoff
        | RuntimeRestartFailureKind::Dispatch => (
            match failure {
                RuntimeRestartFailureKind::Storage => RuntimeFailureKind::Storage,
                RuntimeRestartFailureKind::WorkerSpawn => RuntimeFailureKind::WorkerSpawn,
                RuntimeRestartFailureKind::Dispatch => RuntimeFailureKind::Dispatch,
                RuntimeRestartFailureKind::Rules | RuntimeRestartFailureKind::Handoff => {
                    RuntimeFailureKind::Calibration
                }
                RuntimeRestartFailureKind::Watcher => unreachable!(),
            },
            IndexDegradationCode::FullRefreshFallback,
        ),
    };
    let handle_to_stop = apply_runtime_failure_state(runtime, kind, degradation_code, true);
    RuntimeRestartFailureApplication {
        status: runtime.index_status(),
        request_recovery,
        handle_to_stop,
    }
}

fn restart_runtime_incremental_indexing<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: &QuickFoxAppState,
) -> Result<(), String> {
    let (
        previous,
        config,
        roots,
        initial_generation,
        config_revision,
        config_fingerprint,
        service_identity,
        standby_watcher,
        should_start,
    ) = {
        let mut runtime = state
            .runtime
            .lock()
            .expect("quickfox runtime lock poisoned");
        let previous = runtime.runtime_indexing.take();
        runtime.index_refresh.active_service = None;
        runtime.index_refresh.next_service_epoch =
            runtime.index_refresh.next_service_epoch.saturating_add(1);
        let service_identity = RuntimeServiceIdentity {
            epoch: runtime.index_refresh.next_service_epoch,
            config_revision: runtime.index_refresh.config_revision,
        };
        runtime.incremental_status.enabled = runtime.config.index.watcher_enabled;
        let roots = runtime_watch_roots(&runtime.config, &runtime.index);
        let standby_watcher = runtime
            .index_refresh
            .standby_watcher
            .take()
            .filter(|watcher| watcher.watched_roots() == roots);
        let should_start = runtime_incremental_start_allowed(&runtime, &roots);
        if !runtime.config.index.watcher_enabled {
            runtime.incremental_status.state = IncrementalState::Disabled;
        } else if should_start {
            runtime.incremental_status.state = IncrementalState::Preparing;
        }
        (
            previous,
            runtime.config.clone(),
            roots,
            runtime.index.generation(),
            runtime.index_refresh.config_revision,
            runtime.index_refresh.config_fingerprint.clone(),
            service_identity,
            standby_watcher,
            should_start,
        )
    };
    if let Some(previous) = previous {
        previous.stop();
    }
    if !should_start {
        return Ok(());
    }

    let watcher = match standby_watcher {
        Some(watcher) => watcher,
        None => match RuntimeIndexWatcher::watch_roots(roots.clone()) {
            Ok(watcher) => watcher,
            Err(failure) => {
                let message = failure.message.clone();
                publish_runtime_restart_failure_for_snapshot(
                    app.clone(),
                    state,
                    config_revision,
                    &config_fingerprint,
                    &roots,
                    RuntimeRestartFailureKind::Watcher,
                );
                return Err(message);
            }
        },
    };
    let options = build_scan_options(&config);
    let rules = match IndexPathRules::from_plan(&IndexScanPlan {
        include_roots: roots.clone(),
        exclude_dirs: options.exclude_dirs,
        exclude_patterns: options.exclude_patterns,
        respect_project_ignores: options.respect_project_ignores,
        stage: None,
    }) {
        Ok(rules) => rules,
        Err(error) => {
            publish_runtime_restart_failure_for_snapshot(
                app.clone(),
                state,
                config_revision,
                &config_fingerprint,
                &roots,
                RuntimeRestartFailureKind::Rules,
            );
            return Err(error.to_string());
        }
    };
    let scanner = TargetedIndexScanner::new(rules);
    let journal = match storage_store() {
        Some(journal) => journal,
        None => {
            publish_runtime_restart_failure_for_snapshot(
                app.clone(),
                state,
                config_revision,
                &config_fingerprint,
                &roots,
                RuntimeRestartFailureKind::Storage,
            );
            return Err("index journal storage is unavailable".to_owned());
        }
    };
    let publish_app = app.clone();
    let publish_service = service_identity;
    let validation_roots = roots.clone();
    let refresh_fence = state
        .index_refresh_fence
        .lock()
        .expect("index refresh fence poisoned");
    {
        let runtime = state
            .runtime
            .lock()
            .expect("quickfox runtime lock poisoned");
        if !runtime_incremental_restart_snapshot_is_current(
            &runtime,
            config_revision,
            &config_fingerprint,
            &validation_roots,
        ) {
            return Ok(());
        }
    }
    let handle = match start_runtime_indexing(
        watcher,
        scanner,
        Box::new(journal),
        RuntimeIndexingOptions {
            roots,
            policy: crate::core::index_update_coordinator::CoordinatorPolicy::production(),
            initial_generation,
        },
        move |event| publish_runtime_indexing_event(publish_app.clone(), publish_service, event),
    ) {
        Ok(handle) => handle,
        Err(error) => {
            let message = error.message;
            drop(refresh_fence);
            publish_runtime_restart_failure_for_snapshot(
                app.clone(),
                state,
                config_revision,
                &config_fingerprint,
                &validation_roots,
                RuntimeRestartFailureKind::WorkerSpawn,
            );
            return Err(message);
        }
    };
    let handle_to_stop = {
        let mut runtime = state
            .runtime
            .lock()
            .expect("quickfox runtime lock poisoned");
        if runtime_incremental_restart_snapshot_is_current(
            &runtime,
            config_revision,
            &config_fingerprint,
            &validation_roots,
        ) {
            runtime.index_refresh.active_service = Some(service_identity);
            runtime.index_refresh.restart_recovery_revision = None;
            runtime.runtime_indexing.replace(handle)
        } else {
            Some(handle)
        }
    };
    drop(refresh_fence);
    if let Some(handle) = handle_to_stop {
        handle.stop();
    }
    Ok(())
}

fn runtime_incremental_restart_snapshot_is_current(
    runtime: &QuickFoxRuntime,
    config_revision: u64,
    config_fingerprint: &str,
    roots: &[PathBuf],
) -> bool {
    runtime.index_refresh.config_revision == config_revision
        && runtime.index_refresh.config_fingerprint == config_fingerprint
        && runtime_watch_roots(&runtime.config, &runtime.index) == roots
        && runtime_incremental_start_allowed(runtime, roots)
}

fn record_runtime_restart_failure_for_snapshot(
    runtime: &mut QuickFoxRuntime,
    config_revision: u64,
    config_fingerprint: &str,
    roots: &[PathBuf],
    failure: RuntimeRestartFailureKind,
) -> Option<RuntimeRestartFailureApplication> {
    if !runtime_incremental_restart_snapshot_is_current(
        runtime,
        config_revision,
        config_fingerprint,
        roots,
    ) {
        return None;
    }
    Some(record_runtime_restart_failure(runtime, failure))
}

fn publish_runtime_restart_failure_for_snapshot<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: &QuickFoxAppState,
    config_revision: u64,
    config_fingerprint: &str,
    roots: &[PathBuf],
    failure: RuntimeRestartFailureKind,
) {
    let application = {
        let _fence = state
            .index_refresh_fence
            .lock()
            .expect("index refresh fence poisoned");
        let mut runtime = state
            .runtime
            .lock()
            .expect("quickfox runtime lock poisoned");
        record_runtime_restart_failure_for_snapshot(
            &mut runtime,
            config_revision,
            config_fingerprint,
            roots,
            failure,
        )
    };
    let Some(mut application) = application else {
        return;
    };
    if let Some(handle) = application.handle_to_stop.take() {
        handle.stop();
    }
    let _ = app.emit("quickfox://index-status", application.status);
    if application.request_recovery {
        let _ = start_background_index_refresh(app, state);
    }
}

fn runtime_incremental_start_allowed(runtime: &QuickFoxRuntime, roots: &[PathBuf]) -> bool {
    runtime.config.index.watcher_enabled && runtime.manifest_ready && !roots.is_empty()
}

fn runtime_watch_roots(config: &QuickFoxConfig, index: &LayeredSearchIndex) -> Vec<PathBuf> {
    let mut roots: std::collections::BTreeSet<_> = index.watched_roots().into_iter().collect();
    roots.extend(
        build_scan_options(config)
            .include_dirs
            .into_iter()
            .filter(|root| root.is_dir()),
    );
    roots.into_iter().collect()
}

fn publish_runtime_indexing_event<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    service: RuntimeServiceIdentity,
    event: RuntimeIndexingEvent,
) {
    let dispatch = app.clone();
    let failure_app = app.clone();
    let dispatch_failed = app
        .run_on_main_thread(move || {
            let state = dispatch.state::<QuickFoxAppState>();
            let application = {
                let mut runtime = state
                    .runtime
                    .lock()
                    .expect("quickfox runtime lock poisoned");
                apply_runtime_indexing_event(&mut runtime, &service, event)
            };
            let Some(application) = application else {
                return;
            };
            let _ = dispatch.emit("quickfox://index-status", application.status);
            if application.request_refresh {
                let _ = start_background_index_refresh(dispatch.clone(), &state);
            }
        })
        .is_err();
    if dispatch_failed
        && schedule_runtime_dispatch_recovery(
            failure_app.clone(),
            service,
            &SystemRefreshWorkerSpawner,
        )
        .is_err()
    {
        mark_runtime_dispatch_recovery_spawn_failure(&failure_app, service);
    }
}

fn schedule_runtime_dispatch_recovery<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    service: RuntimeServiceIdentity,
    spawner: &dyn RefreshWorkerSpawner,
) -> Result<(), String> {
    spawner.spawn(Box::new(move || {
        let state = app.state::<QuickFoxAppState>();
        let mut application = {
            let mut runtime = state
                .runtime
                .lock()
                .expect("quickfox runtime lock poisoned");
            (runtime.index_refresh.active_service == Some(service)).then(|| {
                record_runtime_restart_failure(&mut runtime, RuntimeRestartFailureKind::Dispatch)
            })
        };
        let Some(mut application) = application.take() else {
            return;
        };
        if let Some(handle) = application.handle_to_stop.take() {
            handle.stop();
        }
        let _ = app.emit("quickfox://index-status", application.status);
        if application.request_recovery {
            let _ = start_background_index_refresh(app.clone(), &state);
        }
    }))
}

fn mark_runtime_dispatch_recovery_spawn_failure<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    service: RuntimeServiceIdentity,
) {
    let state = app.state::<QuickFoxAppState>();
    let mut runtime = state
        .runtime
        .lock()
        .expect("quickfox runtime lock poisoned");
    if runtime.index_refresh.active_service == Some(service) {
        runtime.index_refresh.restart_recovery_revision = None;
        runtime.index_refresh.pending = true;
        runtime.incremental_status.state = IncrementalState::Degraded;
        runtime.incremental_status.degradation_code =
            Some(IndexDegradationCode::WatcherRuntimeFailed);
    }
}

struct RuntimeIndexingEventApplication {
    status: IndexStatus,
    request_refresh: bool,
}

fn apply_runtime_indexing_event(
    runtime: &mut QuickFoxRuntime,
    service: &RuntimeServiceIdentity,
    event: RuntimeIndexingEvent,
) -> Option<RuntimeIndexingEventApplication> {
    match event {
        RuntimeIndexingEvent::DeltaCommitted(delta) => {
            if service.config_revision != runtime.index_refresh.config_revision
                || delta.generation <= runtime.index.generation()
            {
                return None;
            }
            runtime.index.apply_delta(delta);
            let delta_safety_reached = baseline_refresh_event_for_delta_state(
                runtime.index.delta_entry_count(),
                runtime.index.estimated_delta_bytes(),
            )
            .is_some();
            let request_refresh = delta_safety_reached
                && refresh_request_decision(
                    runtime.index_refresh.active.is_some(),
                    RefreshRequestReason::DeltaSafetyLimit,
                ) == RefreshRequestDecision::Start;
            Some(RuntimeIndexingEventApplication {
                status: runtime.index_status(),
                request_refresh,
            })
        }
        RuntimeIndexingEvent::Status(incremental_status) => {
            if runtime.index_refresh.active_service.as_ref() != Some(service)
                || service.config_revision != runtime.index_refresh.config_revision
            {
                return None;
            }
            runtime.incremental_status = incremental_status;
            Some(RuntimeIndexingEventApplication {
                status: runtime.index_status(),
                request_refresh: false,
            })
        }
        RuntimeIndexingEvent::BaselineRefreshRequired { reason } => {
            if runtime.index_refresh.active_service.as_ref() != Some(service)
                || service.config_revision != runtime.index_refresh.config_revision
            {
                return None;
            }
            if baseline_refresh_requires_manifest_rebuild(reason) {
                runtime.manifest_ready = false;
            }
            let request_reason = if reason == BaselineRefreshReason::DeltaSafetyLimit {
                RefreshRequestReason::DeltaSafetyLimit
            } else {
                RefreshRequestReason::DirtyRoots
            };
            let decision =
                refresh_request_decision(runtime.index_refresh.active.is_some(), request_reason);
            if decision == RefreshRequestDecision::QueueRerun {
                runtime.index_refresh.pending = true;
            }
            Some(RuntimeIndexingEventApplication {
                status: runtime.index_status(),
                request_refresh: decision != RefreshRequestDecision::AbsorbedByActiveRefresh,
            })
        }
    }
}

fn baseline_refresh_requires_manifest_rebuild(reason: BaselineRefreshReason) -> bool {
    matches!(
        reason,
        BaselineRefreshReason::CalibrationFailed
            | BaselineRefreshReason::DirtyRoots
            | BaselineRefreshReason::WatcherFailure
    )
}

fn stop_runtime_incremental_indexing(state: &QuickFoxAppState) {
    let (handle, monitor) = {
        let mut runtime = state
            .runtime
            .lock()
            .expect("quickfox runtime lock poisoned");
        runtime.index_refresh.active_service = None;
        (
            runtime.runtime_indexing.take(),
            runtime.index_refresh.root_monitor.take(),
        )
    };
    if let Some(handle) = handle {
        handle.stop();
    }
    if let Some(mut monitor) = monitor {
        monitor.cancel_and_join();
    }
}

fn apply_index_refresh_progress(
    state: &QuickFoxAppState,
    generation: u64,
    stage: String,
    current_root: Option<String>,
    payload: impl Into<IndexRefreshPayload>,
) -> Option<IndexStatus> {
    let payload = payload.into();
    let mut runtime = state
        .runtime
        .lock()
        .expect("quickfox runtime lock poisoned");
    let entry_count = payload.entries.len();
    if !runtime.index_lifecycle.update_progress(
        generation,
        stage,
        current_root,
        payload.summary.scan_stats.clone(),
        entry_count,
    ) {
        return None;
    }
    if runtime.index.entry_count() == 0 {
        let index_generation = runtime.index.generation();
        runtime
            .index
            .replace_baseline(payload.entries, index_generation);
    }
    runtime.last_report = payload.summary;
    Some(runtime.index_status())
}

fn apply_failed_index_refresh(
    state: &QuickFoxAppState,
    generation: u64,
    message: String,
) -> Option<IndexStatus> {
    let mut runtime = state
        .runtime
        .lock()
        .expect("quickfox runtime lock poisoned");
    if runtime.index_lifecycle.fail_refresh(generation, message) {
        runtime.manifest_ready = false;
        runtime.incremental_status.state = IncrementalState::Degraded;
        runtime.incremental_status.degradation_code =
            Some(IndexDegradationCode::FullRefreshFallback);
        Some(runtime.index_status())
    } else {
        None
    }
}

fn last_finished_root_for_stage(report: &IndexReport, stage: &str) -> Option<String> {
    report
        .scan_events
        .iter()
        .rev()
        .find_map(|event| match event {
            ScanEvent::RootFinished {
                root,
                stage: event_stage,
                ..
            } if event_stage.as_deref() == Some(stage) => Some(root.clone()),
            ScanEvent::RootStarted {
                root,
                stage: event_stage,
            } if event_stage.as_deref() == Some(stage) => Some(root.clone()),
            _ => None,
        })
}

fn should_persist_index_checkpoint(stage: &str, is_final: bool) -> bool {
    is_final || stage == "user-hot-paths"
}

fn build_runtime() -> QuickFoxRuntime {
    let config = load_startup_config();
    if let Some(storage) = storage_store() {
        return build_runtime_with_startup_calibration(config, &storage);
    }
    build_runtime_from_snapshot(config, load_latest_index_snapshot())
}

fn build_runtime_with_startup_calibration(
    config: QuickFoxConfig,
    storage: &SqliteStorage,
) -> QuickFoxRuntime {
    let recovery = recover_layered_index(storage);
    build_runtime_from_recovery(config, recovery)
}

fn build_runtime_from_recovery(
    config: QuickFoxConfig,
    mut recovery: crate::core::index_journal::IndexRecovery,
) -> QuickFoxRuntime {
    let lifecycle = if recovery.baseline_available() {
        IndexLifecycle::from_ready(recovery.baseline_entry_count(), current_time_ms())
    } else {
        IndexLifecycle::default()
    };
    let mut incremental_status = RuntimeIncrementalStatus::default();
    if let Some(code) = recovery.degradation_code() {
        incremental_status.state = IncrementalState::Degraded;
        incremental_status.degradation_code = Some(code);
    }
    let manifest_ready = !config.index.watcher_enabled && recovery.manifest_ready();
    if incremental_status.degradation_code.is_none() {
        incremental_status.state = if config.index.watcher_enabled {
            IncrementalState::Preparing
        } else {
            IncrementalState::Disabled
        };
    }
    let index_refresh = IndexRefreshControl::for_config(&config);
    if should_build_content_index_for_config(&config, &recovery.index.materialized_entries()) {
        let generation = recovery.index.generation();
        let entries = recovery.index.materialized_entries();
        recovery.index.replace_baseline_search_index(
            build_search_index_with_content_for_config(&config, entries),
            generation,
        );
    }
    QuickFoxRuntime {
        config,
        index: recovery.index,
        last_report: IndexReport::default(),
        index_lifecycle: lifecycle,
        runtime_indexing: None,
        incremental_status,
        manifest_ready,
        index_refresh,
    }
}

fn load_latest_index_snapshot() -> Option<crate::core::storage::IndexSnapshot> {
    storage_store().and_then(|storage| storage.latest_index_snapshot().ok().flatten())
}

fn build_runtime_from_snapshot(
    config: QuickFoxConfig,
    snapshot: Option<crate::core::storage::IndexSnapshot>,
) -> QuickFoxRuntime {
    let (index, index_lifecycle, report) = if let Some(snapshot) = snapshot {
        let entry_count = snapshot.entries.len();
        let completed_at_ms = snapshot.completed_at_ms;
        (
            LayeredSearchIndex::from_search_index(build_search_index_for_config(
                &config,
                snapshot.entries,
            )),
            IndexLifecycle::from_ready(entry_count, completed_at_ms),
            IndexReport::default(),
        )
    } else {
        (
            LayeredSearchIndex::from_search_index(build_search_index_for_config(
                &config,
                Vec::new(),
            )),
            IndexLifecycle::default(),
            IndexReport::default(),
        )
    };
    let index_refresh = IndexRefreshControl::for_config(&config);
    QuickFoxRuntime {
        config,
        index,
        index_lifecycle,
        last_report: report,
        runtime_indexing: None,
        incremental_status: RuntimeIncrementalStatus::default(),
        manifest_ready: false,
        index_refresh,
    }
}

fn build_search_index_for_config(
    config: &QuickFoxConfig,
    entries: Vec<IndexedEntry>,
) -> SearchIndex {
    let _ = config;
    SearchIndex::from_entries(entries)
}

fn build_search_index_with_content_for_config(
    config: &QuickFoxConfig,
    mut entries: Vec<IndexedEntry>,
) -> SearchIndex {
    let content_roots = content_index_roots(config);
    if content_roots.is_empty() || entries.is_empty() {
        return SearchIndex::from_entries(entries);
    }

    let mut content_entries: Vec<_> = entries
        .iter()
        .filter(|entry| entry_is_under_content_root(entry, &content_roots))
        .cloned()
        .collect();
    if content_entries.is_empty() {
        return SearchIndex::from_entries(entries);
    }

    match ContentIndex::build(&mut content_entries, content_index_options(config)) {
        Ok(content_index) => {
            let states_by_path: std::collections::HashMap<_, _> = content_entries
                .into_iter()
                .map(|entry| (entry.path, entry.content_index_state))
                .collect();
            for entry in &mut entries {
                entry.content_index_state = states_by_path
                    .get(&entry.path)
                    .cloned()
                    .unwrap_or(ContentIndexState::NotIndexed);
            }
            SearchIndex::from_entries_with_content_index(entries, content_index)
        }
        Err(error) => {
            eprintln!("QuickFox content index build failed: {error}");
            SearchIndex::from_entries(entries)
        }
    }
}

fn should_build_content_index_for_config(
    config: &QuickFoxConfig,
    entries: &[IndexedEntry],
) -> bool {
    let content_roots = content_index_roots(config);
    !content_roots.is_empty()
        && entries
            .iter()
            .any(|entry| entry_is_under_content_root(entry, &content_roots))
}

fn content_index_roots(config: &QuickFoxConfig) -> Vec<PathBuf> {
    config
        .index
        .content_include_dirs
        .iter()
        .map(|root| PathBuf::from(expand_user_path(root)))
        .collect()
}

fn entry_is_under_content_root(entry: &IndexedEntry, roots: &[PathBuf]) -> bool {
    let path = PathBuf::from(&entry.path);
    roots.iter().any(|root| path.starts_with(root))
}

fn content_index_options(config: &QuickFoxConfig) -> ContentIndexOptions {
    ContentIndexOptions {
        index_dir: content_index_base_dir(),
        max_file_bytes: config.index.content_max_file_bytes,
    }
}

#[cfg(not(test))]
fn content_index_base_dir() -> PathBuf {
    storage_file_path()
        .and_then(|path| path.parent().map(|parent| parent.join("content-index")))
        .unwrap_or_else(|| std::env::temp_dir().join("quickfox").join("content-index"))
}

#[cfg(test)]
fn content_index_base_dir() -> PathBuf {
    static CONTENT_INDEX_TEST_COUNTER: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(0);
    let nonce = CONTENT_INDEX_TEST_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir()
        .join("quickfox")
        .join(format!("content-index-test-{}-{nonce}", std::process::id()))
}

impl QuickFoxRuntime {
    fn index_status(&self) -> IndexStatus {
        let mut status = self.index_lifecycle.status().clone();
        status.entry_count = self.index.entry_count();
        status.incremental = self.incremental_status.clone();
        if status.message.is_none() {
            status.message = status
                .incremental
                .degradation_code
                .map(incremental_degradation_summary)
                .map(str::to_owned);
        }
        status
    }
}

fn incremental_degradation_summary(code: IndexDegradationCode) -> &'static str {
    match code {
        IndexDegradationCode::WatcherInitializationFailed => {
            "自动增量监听初始化失败，文件搜索仍使用最近可用索引"
        }
        IndexDegradationCode::WatcherRuntimeFailed => {
            "自动增量监听运行失败，文件搜索仍使用最近可用索引"
        }
        IndexDegradationCode::WatcherOverflow | IndexDegradationCode::ChannelOverflow => {
            "文件变化过多，正在安排索引恢复"
        }
        IndexDegradationCode::JournalWriteFailed => "增量索引保存失败，未应用本次文件变化",
        IndexDegradationCode::JournalReplayFailed => "增量索引恢复失败，文件搜索仍使用最近可用基线",
        IndexDegradationCode::CalibrationFailed => "增量索引校准失败，文件搜索仍使用最近可用索引",
        IndexDegradationCode::FullRefreshFallback => "增量索引不可用，正在安排完整刷新",
    }
}

fn execute_command_in_terminal(command: &str) -> Result<(), String> {
    let process = build_terminal_command(command)?;
    Command::new(&process.program)
        .args(&process.args)
        .spawn()
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn execute_open_with_application(path: &str, application: &OpenApplication) -> Result<(), String> {
    let process = build_open_with_application_command(path, application)?;
    Command::new(&process.program)
        .args(&process.args)
        .spawn()
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn build_open_with_application_command(
    path: &str,
    application: &OpenApplication,
) -> Result<ProcessCommand, String> {
    match application {
        OpenApplication::DevelopmentTool => {
            let candidates = [
                "code",
                "cursor",
                "code.cmd",
                "cursor.cmd",
                "code.exe",
                "cursor.exe",
                "open",
                "xdg-open",
            ];
            let available = detect_available_programs(&candidates);
            let available_refs: Vec<_> = available.iter().map(String::as_str).collect();
            DevelopmentToolAdapter::new(Vec::new())
                .build_command(&expand_user_path(path), &available_refs)
                .map_err(|error| format!("{error:?}"))
        }
        OpenApplication::SystemChooser => build_system_open_with_command(&expand_user_path(path)),
    }
}

fn build_system_open_with_command(path: &str) -> Result<ProcessCommand, String> {
    #[cfg(target_os = "windows")]
    {
        return Ok(ProcessCommand {
            program: "rundll32.exe".to_owned(),
            args: vec!["shell32.dll,OpenAs_RunDLL".to_owned(), path.to_owned()],
        });
    }

    #[cfg(target_os = "macos")]
    {
        let escaped_path = path.replace('\\', "\\\\").replace('"', "\\\"");
        let script = format!(
            "set chosenApp to choose application with prompt \"选择打开方式\" as alias\n\
             set chosenAppPath to POSIX path of chosenApp\n\
             do shell script \"open -a \" & quoted form of chosenAppPath & \" \" & quoted form of \"{escaped_path}\""
        );
        return Ok(ProcessCommand {
            program: "osascript".to_owned(),
            args: vec!["-e".to_owned(), script],
        });
    }

    #[cfg(target_os = "linux")]
    {
        return Ok(ProcessCommand {
            program: "xdg-open".to_owned(),
            args: vec![path.to_owned()],
        });
    }

    #[allow(unreachable_code)]
    Err("system open with is unavailable".to_owned())
}

fn build_terminal_command(command: &str) -> Result<ProcessCommand, String> {
    #[cfg(target_os = "windows")]
    {
        return WindowsTerminalAdapter
            .build_command(command)
            .map_err(|error| format!("{error:?}"));
    }

    #[cfg(target_os = "macos")]
    {
        return MacosTerminalAdapter
            .build_command(command)
            .map_err(|error| format!("{error:?}"));
    }

    #[cfg(target_os = "linux")]
    {
        let terminals = detect_available_terminals(&[
            "x-terminal-emulator",
            "gnome-terminal",
            "konsole",
            "xfce4-terminal",
            "xterm",
        ]);
        let terminal_refs: Vec<_> = terminals.iter().map(String::as_str).collect();
        return LinuxTerminalAdapter::new(Vec::new())
            .build_command(command, &terminal_refs)
            .map_err(|error| format!("{error:?}"));
    }

    #[allow(unreachable_code)]
    Err("unsupported platform".to_owned())
}

#[cfg(target_os = "linux")]
fn detect_available_terminals(candidates: &[&str]) -> Vec<String> {
    detect_available_programs(candidates)
}

fn detect_available_programs(candidates: &[&str]) -> Vec<String> {
    let Some(path) = std::env::var_os("PATH") else {
        return Vec::new();
    };

    let paths: Vec<_> = std::env::split_paths(&path).collect();
    candidates
        .iter()
        .filter(|candidate| {
            paths.iter().any(|path| {
                let candidate_path = path.join(candidate);
                candidate_path.is_file()
            })
        })
        .map(|candidate| (*candidate).to_owned())
        .collect()
}

fn expand_user_path(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = home_dir() {
            return home.join(rest).to_string_lossy().to_string();
        }
    }

    path.to_owned()
}

fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .map(PathBuf::from)
}

fn build_app_paths(config_file_path: PathBuf, index_snapshot_path: PathBuf) -> AppPaths {
    AppPaths {
        config_file_path: config_file_path.to_string_lossy().to_string(),
        index_snapshot_path: index_snapshot_path.to_string_lossy().to_string(),
    }
}

fn pending_global_hotkey_status() -> GlobalHotkeyStatus {
    GlobalHotkeyStatus {
        enabled: false,
        message: "全局唤醒监听启动中".to_owned(),
        permission_settings_url: None,
    }
}

fn enabled_global_hotkey_status(shortcut: &WakeShortcut) -> GlobalHotkeyStatus {
    GlobalHotkeyStatus {
        enabled: true,
        message: format!("{} 全局唤醒可用", shortcut.display_label()),
        permission_settings_url: None,
    }
}

fn failed_global_hotkey_status(
    error: &keytap::Error,
    shortcut: &WakeShortcut,
) -> GlobalHotkeyStatus {
    let (message, permission_settings_url) = match error {
        keytap::Error::PermissionDenied => (
            global_hotkey_permission_denied_message(shortcut),
            global_hotkey_permission_settings_url(),
        ),
        keytap::Error::NoDevices => (
            format!(
                "未找到可监听的键盘设备，{} 全局唤醒不可用",
                shortcut.display_label()
            ),
            None,
        ),
        _ => (
            format!("{} 全局唤醒监听启动失败: {error}", shortcut.display_label()),
            None,
        ),
    };

    GlobalHotkeyStatus {
        enabled: false,
        message,
        permission_settings_url,
    }
}

fn global_hotkey_permission_denied_message(shortcut: &WakeShortcut) -> String {
    let shortcut_label = shortcut.display_label();
    #[cfg(target_os = "macos")]
    {
        format!("需要授予输入监控权限后才能使用 {shortcut_label} 全局唤醒")
    }

    #[cfg(target_os = "linux")]
    {
        format!("需要授予键盘设备读取权限后才能使用 {shortcut_label} 全局唤醒")
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        format!("需要授予系统键盘监听权限后才能使用 {shortcut_label} 全局唤醒")
    }
}

fn global_hotkey_permission_settings_url() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        Some(
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent"
                .to_owned(),
        )
    }

    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

fn set_global_hotkey_status<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    status: GlobalHotkeyStatus,
) {
    let app_state = app.state::<QuickFoxAppState>();
    {
        let mut current_status = app_state
            .global_hotkey_status
            .lock()
            .expect("quickfox global hotkey status lock poisoned");
        *current_status = status.clone();
    }
    let _ = app.emit_to("main", "quickfox://global-hotkey-status", status);
}

fn refresh_enabled_global_hotkey_status<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    shortcut: &WakeShortcut,
) {
    let app_state = app.state::<QuickFoxAppState>();
    let should_refresh = app_state
        .global_hotkey_status
        .lock()
        .expect("quickfox global hotkey status lock poisoned")
        .enabled;
    if should_refresh {
        set_global_hotkey_status(app, enabled_global_hotkey_status(shortcut));
    }
}

fn toggle_main_window<R: tauri::Runtime>(app: &tauri::AppHandle<R>, state: &QuickFoxAppState) {
    let effect = if let Some(window) = app.get_webview_window("main") {
        let visible = window.is_visible().unwrap_or(false);
        let focused = window.is_focused().unwrap_or(false);
        let mut window_state = state
            .window_state
            .lock()
            .expect("quickfox window state lock poisoned");
        next_launcher_window_effect(visible, focused, &mut window_state)
    } else {
        let mut window_state = state
            .window_state
            .lock()
            .expect("quickfox window state lock poisoned");
        sync_launcher_window_state_for_tray_toggle(&mut window_state)
    };
    apply_launcher_window_effect(app, effect);
}

fn show_settings_window<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    if let Some(window) = app.get_webview_window("settings") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        return;
    }

    match WebviewWindowBuilder::new(app, "settings", WebviewUrl::App("?view=settings".into()))
        .title("QuickFox 设置")
        .inner_size(940.0, 680.0)
        .min_inner_size(420.0, 520.0)
        .resizable(true)
        .decorations(true)
        .transparent(false)
        .visible(true)
        .build()
    {
        Ok(window) => {
            let _ = window.set_focus();
        }
        Err(error) => {
            eprintln!("QuickFox settings window open failed: {error}");
        }
    }
}

fn hide_launcher_after_focus_loss<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    state: &QuickFoxAppState,
) {
    {
        let mut window_state = state
            .window_state
            .lock()
            .expect("quickfox window state lock poisoned");
        window_state.hide();
    }
    apply_launcher_window_effect(app, LauncherWindowEffect::Hide);
}

fn tray_window_target(menu_id: &str) -> Option<TrayWindowTarget> {
    match menu_id {
        "show" => Some(TrayWindowTarget::Launcher),
        "settings" => Some(TrayWindowTarget::Settings),
        _ => None,
    }
}

fn apply_launcher_window_effect<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    effect: LauncherWindowEffect,
) {
    if let Some(window) = app.get_webview_window("main") {
        match effect {
            LauncherWindowEffect::ShowAndFocus => {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
            LauncherWindowEffect::Hide => {
                let _ = window.hide();
            }
        }
    }
}

fn toggle_launcher_window_for_app<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    state: &QuickFoxAppState,
) -> Result<(), String> {
    let Some(window) = app.get_webview_window("main") else {
        return Err("main window not found".to_owned());
    };
    let visible = window.is_visible().unwrap_or(false);
    let focused = window.is_focused().unwrap_or(false);
    let mut window_state = state
        .window_state
        .lock()
        .expect("quickfox window state lock poisoned");
    let effect = next_launcher_window_effect(visible, focused, &mut window_state);
    apply_launcher_window_effect(app, effect);
    Ok(())
}

fn key_press_from_global_event(event: &keytap::Event) -> Option<KeyPress> {
    match event.kind {
        EventKind::KeyDown(key) => Some(KeyPress::KeyDown(hotkey_key_from_keytap(key))),
        EventKind::KeyUp(key) => Some(KeyPress::KeyUp(hotkey_key_from_keytap(key))),
        EventKind::KeyRepeat(_) => None,
    }
}

fn hotkey_key_from_keytap(key: Key) -> HotkeyKey {
    match key {
        Key::ShiftLeft | Key::ShiftRight => HotkeyKey::Shift,
        Key::ControlLeft | Key::ControlRight => HotkeyKey::Control,
        Key::AltLeft | Key::AltRight => HotkeyKey::Alt,
        Key::MetaLeft | Key::MetaRight => HotkeyKey::Command,
        Key::Space => HotkeyKey::Space,
        Key::Enter | Key::NumpadEnter => HotkeyKey::Enter,
        Key::Escape => HotkeyKey::Escape,
        Key::Tab => HotkeyKey::Tab,
        Key::Backspace => HotkeyKey::Backspace,
        Key::Delete => HotkeyKey::Delete,
        Key::ArrowUp => HotkeyKey::ArrowUp,
        Key::ArrowDown => HotkeyKey::ArrowDown,
        Key::ArrowLeft => HotkeyKey::ArrowLeft,
        Key::ArrowRight => HotkeyKey::ArrowRight,
        Key::A => HotkeyKey::Character('A'),
        Key::B => HotkeyKey::Character('B'),
        Key::C => HotkeyKey::Character('C'),
        Key::D => HotkeyKey::Character('D'),
        Key::E => HotkeyKey::Character('E'),
        Key::F => HotkeyKey::Character('F'),
        Key::G => HotkeyKey::Character('G'),
        Key::H => HotkeyKey::Character('H'),
        Key::I => HotkeyKey::Character('I'),
        Key::J => HotkeyKey::Character('J'),
        Key::K => HotkeyKey::Character('K'),
        Key::L => HotkeyKey::Character('L'),
        Key::M => HotkeyKey::Character('M'),
        Key::N => HotkeyKey::Character('N'),
        Key::O => HotkeyKey::Character('O'),
        Key::P => HotkeyKey::Character('P'),
        Key::Q => HotkeyKey::Character('Q'),
        Key::R => HotkeyKey::Character('R'),
        Key::S => HotkeyKey::Character('S'),
        Key::T => HotkeyKey::Character('T'),
        Key::U => HotkeyKey::Character('U'),
        Key::V => HotkeyKey::Character('V'),
        Key::W => HotkeyKey::Character('W'),
        Key::X => HotkeyKey::Character('X'),
        Key::Y => HotkeyKey::Character('Y'),
        Key::Z => HotkeyKey::Character('Z'),
        Key::Digit0 | Key::Numpad0 => HotkeyKey::Character('0'),
        Key::Digit1 | Key::Numpad1 => HotkeyKey::Character('1'),
        Key::Digit2 | Key::Numpad2 => HotkeyKey::Character('2'),
        Key::Digit3 | Key::Numpad3 => HotkeyKey::Character('3'),
        Key::Digit4 | Key::Numpad4 => HotkeyKey::Character('4'),
        Key::Digit5 | Key::Numpad5 => HotkeyKey::Character('5'),
        Key::Digit6 | Key::Numpad6 => HotkeyKey::Character('6'),
        Key::Digit7 | Key::Numpad7 => HotkeyKey::Character('7'),
        Key::Digit8 | Key::Numpad8 => HotkeyKey::Character('8'),
        Key::Digit9 | Key::Numpad9 => HotkeyKey::Character('9'),
        Key::F1 => HotkeyKey::Function(1),
        Key::F2 => HotkeyKey::Function(2),
        Key::F3 => HotkeyKey::Function(3),
        Key::F4 => HotkeyKey::Function(4),
        Key::F5 => HotkeyKey::Function(5),
        Key::F6 => HotkeyKey::Function(6),
        Key::F7 => HotkeyKey::Function(7),
        Key::F8 => HotkeyKey::Function(8),
        Key::F9 => HotkeyKey::Function(9),
        Key::F10 => HotkeyKey::Function(10),
        Key::F11 => HotkeyKey::Function(11),
        Key::F12 => HotkeyKey::Function(12),
        Key::F13 => HotkeyKey::Function(13),
        Key::F14 => HotkeyKey::Function(14),
        Key::F15 => HotkeyKey::Function(15),
        Key::F16 => HotkeyKey::Function(16),
        Key::F17 => HotkeyKey::Function(17),
        Key::F18 => HotkeyKey::Function(18),
        Key::F19 => HotkeyKey::Function(19),
        Key::F20 => HotkeyKey::Function(20),
        Key::F21 => HotkeyKey::Function(21),
        Key::F22 => HotkeyKey::Function(22),
        Key::F23 => HotkeyKey::Function(23),
        Key::F24 => HotkeyKey::Function(24),
        _ => HotkeyKey::Other,
    }
}

fn current_wake_shortcut(config: &QuickFoxConfig) -> WakeShortcut {
    WakeShortcut::parse(&config.hotkey.wake_shortcut).unwrap_or_default()
}

fn start_global_double_shift_listener(app: tauri::AppHandle) {
    thread::Builder::new()
        .name("quickfox-global-hotkey".to_owned())
        .spawn(move || {
            let initial_shortcut = {
                let state = app.state::<QuickFoxAppState>();
                let runtime = state
                    .runtime
                    .lock()
                    .expect("quickfox runtime lock poisoned");
                current_wake_shortcut(&runtime.config)
            };
            let tap = match Tap::builder().macos_no_repeat_detection().build() {
                Ok(tap) => tap,
                Err(error) => {
                    let status = failed_global_hotkey_status(&error, &initial_shortcut);
                    set_global_hotkey_status(&app, status);
                    eprintln!("QuickFox global hotkey listener disabled: {error}");
                    return;
                }
            };
            set_global_hotkey_status(&app, enabled_global_hotkey_status(&initial_shortcut));
            let mut hotkey_state = HotkeyState::with_shortcut(initial_shortcut);

            for event in tap.iter() {
                let shortcut = {
                    let state = app.state::<QuickFoxAppState>();
                    let runtime = state
                        .runtime
                        .lock()
                        .expect("quickfox runtime lock poisoned");
                    current_wake_shortcut(&runtime.config)
                };
                hotkey_state.set_shortcut(shortcut);
                let Some(key_press) = key_press_from_global_event(&event) else {
                    continue;
                };

                if hotkey_state.register_key_press(key_press, current_time_ms() as u64) {
                    let dispatch_app = app.clone();
                    let app_for_main_thread = dispatch_app.clone();
                    if let Err(error) = dispatch_app.run_on_main_thread(move || {
                        let state = app_for_main_thread.state::<QuickFoxAppState>();
                        let _ = toggle_launcher_window_for_app(&app_for_main_thread, &state);
                    }) {
                        eprintln!("QuickFox global hotkey dispatch failed: {error}");
                    }
                }
            }
        })
        .expect("failed to spawn QuickFox global hotkey listener");
}

fn next_launcher_window_effect(
    visible: bool,
    focused: bool,
    state: &mut LauncherWindowState,
) -> LauncherWindowEffect {
    if visible && focused {
        state.show();
    } else if visible {
        state.mark_backgrounded();
    } else {
        state.hide();
    }
    state.toggle_for_global_hotkey()
}

fn sync_launcher_window_state_for_tray_toggle(
    state: &mut LauncherWindowState,
) -> LauncherWindowEffect {
    state.toggle_for_global_hotkey()
}

fn validate_command_action(command: &str, requires_confirmation: bool) -> Result<(), String> {
    if !requires_confirmation {
        return Err("command requires confirmation".to_owned());
    }

    match CommandSafetyChecker.check(command) {
        CommandSafetyDecision::AllowWithConfirmation
        | CommandSafetyDecision::RequireStrongConfirmation { .. } => Ok(()),
        CommandSafetyDecision::Blocked { reason } => Err(reason),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    const TRAY_ICON: Image<'_> = tauri::include_image!("./icons/icon.png");
    let startup_gate = Arc::new(StartupIndexingGate::default());
    let setup_startup_gate = Arc::clone(&startup_gate);

    tauri::Builder::default()
        .manage(QuickFoxAppState {
            runtime: Mutex::new(build_runtime()),
            index_refresh_fence: Mutex::new(()),
            window_state: Mutex::new(LauncherWindowState::default()),
            global_hotkey_status: Mutex::new(pending_global_hotkey_status()),
        })
        .plugin(tauri_plugin_opener::init())
        .setup(move |app| {
            use tauri::menu::{Menu, MenuItem};
            use tauri::tray::TrayIconBuilder;

            let settings = MenuItem::with_id(app, "settings", "设置", true, None::<&str>)?;
            let show = MenuItem::with_id(app, "show", "显示/隐藏 QuickFox", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &settings, &quit])?;
            TrayIconBuilder::with_id("quickfox")
                .tooltip("QuickFox")
                .icon(TRAY_ICON.clone())
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "show" => match tray_window_target("show") {
                        Some(TrayWindowTarget::Launcher) => {
                            let state = app.state::<QuickFoxAppState>();
                            toggle_main_window(app, &state);
                        }
                        Some(TrayWindowTarget::Settings) => show_settings_window(app),
                        None => {}
                    },
                    "settings" => match tray_window_target("settings") {
                        Some(TrayWindowTarget::Settings) => show_settings_window(app),
                        Some(TrayWindowTarget::Launcher) | None => {}
                    },
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;

            if let Some(main_window) = app.get_webview_window("main") {
                let handle = app.handle().clone();
                main_window.on_window_event(move |event| {
                    if matches!(event, WindowEvent::Focused(false)) {
                        let state = handle.state::<QuickFoxAppState>();
                        hide_launcher_after_focus_loss(&handle, &state);
                    }
                });
            }

            start_global_double_shift_listener(app.handle().clone());
            if let Err(error) = schedule_startup_indexing_in_setup(
                app.handle().clone(),
                Arc::clone(&setup_startup_gate),
                &SystemRefreshWorkerSpawner,
            ) {
                record_startup_scheduling_failure(app.handle(), &error);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            health_check,
            index_status,
            toggle_launcher_window,
            open_settings_window,
            search,
            execute_action,
            refresh_index,
            app_paths,
            global_hotkey_status,
            load_config,
            save_config,
            clear_command_history,
            record_input_history,
            recent_input_history,
            clear_input_history
        ])
        .build(tauri::generate_context!())
        .expect("error while building QuickFox")
        .run(move |app, event| match event {
            RunEvent::Ready => {
                startup_gate.release_after_setup();
                if startup_gate.claim_retry() {
                    if let Err(error) = schedule_startup_indexing_in_setup(
                        app.clone(),
                        Arc::clone(&startup_gate),
                        &SystemRefreshWorkerSpawner,
                    ) {
                        record_startup_scheduling_failure(app, &error);
                    }
                }
            }
            RunEvent::Exit | RunEvent::ExitRequested { .. } => {
                stop_runtime_incremental_indexing(&app.state::<QuickFoxAppState>());
            }
            _ => {}
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::index_watcher::IndexWatchEvent;
    use crate::core::layered_index::CommittedIndexDelta;
    use std::cell::Cell;
    use std::fs;
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn execute_action_refuses_unconfirmed_commands() {
        assert_eq!(
            validate_command_action("git status", false),
            Err("command requires confirmation".to_owned())
        );
    }

    #[test]
    fn execute_action_blocks_dangerous_commands() {
        assert_eq!(
            validate_command_action("rm -rf /", true),
            Err("命令会递归删除根目录".to_owned())
        );
    }

    #[test]
    fn load_config_returns_default_config_for_tauri_command_contract() {
        let config = QuickFoxConfig::default_with_index_dirs(default_index_dirs());

        assert_eq!(config.query.regex_prefix, "re:");
        assert!(!config.command.enabled);
    }

    #[test]
    fn app_paths_returns_config_and_index_snapshot_paths_for_settings() {
        let paths = build_app_paths(
            PathBuf::from("/Users/frank/Library/Application Support/QuickFox/config.toml"),
            PathBuf::from("/Users/frank/Library/Application Support/QuickFox/quickfox.sqlite"),
        );

        assert_eq!(
            paths.config_file_path,
            "/Users/frank/Library/Application Support/QuickFox/config.toml"
        );
        assert_eq!(
            paths.index_snapshot_path,
            "/Users/frank/Library/Application Support/QuickFox/quickfox.sqlite"
        );
    }

    #[test]
    fn tauri_config_separates_launcher_and_settings_window_shapes() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).expect("valid tauri config");
        let windows = config["app"]["windows"]
            .as_array()
            .expect("tauri windows array");
        let launcher_window = windows
            .iter()
            .find(|window| window["label"] == "main")
            .expect("launcher window config");
        let settings_window = windows
            .iter()
            .find(|window| window["label"] == "settings")
            .expect("settings window config");

        assert_eq!(launcher_window["decorations"], false);
        assert_eq!(launcher_window["resizable"], false);
        assert_eq!(launcher_window["transparent"], true);
        assert_eq!(settings_window["decorations"], true);
        assert_eq!(settings_window["resizable"], true);
        assert_eq!(settings_window["transparent"], false);
    }

    #[test]
    fn default_capability_allows_launcher_window_hide() {
        let capability: serde_json::Value =
            serde_json::from_str(include_str!("../capabilities/default.json"))
                .expect("valid default capability");
        let permissions = capability["permissions"]
            .as_array()
            .expect("default capability permissions array");

        assert!(
            permissions
                .iter()
                .any(|permission| permission == "core:window:allow-hide"),
            "launcher Escape handling needs permission to hide the current Tauri window"
        );
    }

    #[test]
    fn tray_menu_routes_show_and_settings_to_separate_windows() {
        assert_eq!(tray_window_target("show"), Some(TrayWindowTarget::Launcher));
        assert_eq!(
            tray_window_target("settings"),
            Some(TrayWindowTarget::Settings)
        );
        assert_eq!(tray_window_target("quit"), None);
    }

    #[test]
    fn runtime_reports_current_index_status() {
        let runtime = QuickFoxRuntime {
            config: QuickFoxConfig::default_with_index_dirs(vec!["/tmp".to_owned()]),
            index: LayeredSearchIndex::default(),
            last_report: IndexReport::default(),
            index_lifecycle: IndexLifecycle::default(),
            runtime_indexing: None,
            incremental_status: RuntimeIncrementalStatus::default(),
            manifest_ready: true,
            index_refresh: IndexRefreshControl::default(),
        };

        assert_eq!(
            runtime.index_status().kind,
            crate::core::index::IndexStatusKind::Unbuilt
        );
    }

    #[test]
    fn runtime_status_uses_layered_visible_entry_count_after_delta() {
        let mut index =
            LayeredSearchIndex::from_baseline(vec![crate::core::index::IndexedEntry::legacy(
                "/tmp/base.md",
                "base.md",
                crate::core::index::IndexedEntryKind::File,
            )]);
        index.apply_delta(crate::core::layered_index::CommittedIndexDelta {
            generation: 1,
            upserts: vec![crate::core::index::IndexedEntry::legacy(
                "/tmp/new.md",
                "new.md",
                crate::core::index::IndexedEntryKind::File,
            )],
            removals: Vec::new(),
        });
        let runtime = QuickFoxRuntime {
            config: QuickFoxConfig::default_with_index_dirs(vec!["/tmp".to_owned()]),
            index,
            last_report: IndexReport::default(),
            index_lifecycle: IndexLifecycle::from_ready(1, 123),
            runtime_indexing: None,
            incremental_status: RuntimeIncrementalStatus::default(),
            manifest_ready: true,
            index_refresh: IndexRefreshControl::default(),
        };

        assert_eq!(runtime.index_status().entry_count, 2);
    }

    #[test]
    fn runtime_builds_from_persisted_snapshot_when_available() {
        let runtime = build_runtime_from_snapshot(
            QuickFoxConfig::default_with_index_dirs(vec!["/tmp".to_owned()]),
            Some(crate::core::storage::IndexSnapshot {
                completed_at_ms: 123,
                entries: vec![crate::core::index::IndexedEntry {
                    path: "/tmp/notes.md".to_owned(),
                    name: "notes.md".to_owned(),
                    kind: crate::core::index::IndexedEntryKind::File,
                    ..crate::core::index::IndexedEntry::legacy(
                        "",
                        "",
                        crate::core::index::IndexedEntryKind::File,
                    )
                }],
                needs_full_refresh: false,
            }),
        );

        assert_eq!(
            runtime.index_status().kind,
            crate::core::index::IndexStatusKind::Ready
        );
        assert_eq!(runtime.index_status().entry_count, 1);
        assert_eq!(runtime.index.entries()[0].name, "notes.md");
        assert!(
            runtime.last_report.entries.is_empty(),
            "snapshot startup must not keep a duplicate full entry report resident"
        );
    }

    #[test]
    fn legacy_baseline_with_empty_manifest_stays_searchable_while_runtime_prepares_rebuild() {
        let temp = tempfile::tempdir().unwrap();
        let storage = SqliteStorage::open(temp.path().join("legacy.sqlite")).unwrap();
        let entry = IndexedEntry::from_path_metadata(
            "/root/legacy.md",
            "/root",
            crate::core::index::IndexedEntryKind::File,
        );
        let baseline_id = storage
            .save_completed_index_batch(10, std::slice::from_ref(&entry))
            .unwrap();
        storage.activate_baseline(baseline_id, 0).unwrap();

        let recovery = recover_layered_index(&storage);
        let runtime = build_runtime_from_recovery(
            QuickFoxConfig::default_with_index_dirs(vec!["/root".to_owned()]),
            recovery,
        );

        assert_eq!(runtime.index.entries(), &[entry]);
        assert_eq!(
            runtime.index_status().kind,
            crate::core::index::IndexStatusKind::Ready
        );
        assert_eq!(
            runtime.incremental_status.state,
            IncrementalState::Preparing
        );
        assert_eq!(runtime.incremental_status.degradation_code, None);
        assert!(!runtime.manifest_ready);
        assert!(runtime.runtime_indexing.is_none());
    }

    #[test]
    fn recovered_manifest_must_cover_newly_configured_roots_before_watcher_start() {
        let temp = tempfile::tempdir().unwrap();
        let old_root = temp.path().join("old-root");
        let new_root = temp.path().join("new-root");
        fs::create_dir_all(&old_root).unwrap();
        fs::create_dir_all(&new_root).unwrap();
        let old_file = old_root.join("old.md");
        fs::write(&old_file, "old").unwrap();
        let storage = SqliteStorage::open(temp.path().join("configured-roots.sqlite")).unwrap();
        let baseline_id = storage
            .save_completed_index_batch(
                10,
                &[IndexedEntry::from_path_metadata(
                    &old_file,
                    &old_root,
                    crate::core::index::IndexedEntryKind::File,
                )],
            )
            .unwrap();
        storage.activate_baseline(baseline_id, 0).unwrap();
        storage
            .replace_directory_manifest(
                &old_root,
                &[crate::core::targeted_index_scanner::DirectoryFingerprint {
                    path: old_root.to_string_lossy().into_owned(),
                    parent: None,
                    root: old_root.to_string_lossy().into_owned(),
                    modified_ms: None,
                }],
            )
            .unwrap();

        let recovery = recover_layered_index(&storage);
        assert!(recovery.manifest_ready());
        let runtime = build_runtime_from_recovery(
            QuickFoxConfig::default_with_index_dirs(vec![new_root.to_string_lossy().into_owned()]),
            recovery,
        );

        assert!(!runtime.manifest_ready);
        assert_eq!(
            runtime.incremental_status.state,
            IncrementalState::Preparing
        );
    }

    #[test]
    fn startup_runtime_recovers_journal_without_running_calibration_inline() {
        let temp = tempfile::Builder::new()
            .prefix("quickfox-startup-")
            .tempdir_in(std::env::temp_dir())
            .unwrap();
        let root_alias = temp.path().join("root");
        fs::create_dir_all(&root_alias).unwrap();
        let root = root_alias.canonicalize().unwrap();
        let storage = SqliteStorage::open(temp.path().join("startup-calibration.sqlite")).unwrap();
        let baseline_id = storage.save_completed_index_batch(1, &[]).unwrap();
        storage
            .activate_baseline_with_manifest_and_clear_incremental_state(
                baseline_id,
                0,
                &[DirectoryFingerprint {
                    path: root.to_string_lossy().into_owned(),
                    parent: None,
                    root: root.to_string_lossy().into_owned(),
                    modified_ms: Some(-1),
                }],
            )
            .unwrap();
        let created_while_off = root.join("while-off.md");
        fs::write(&created_while_off, "new").unwrap();
        let config = QuickFoxConfig::default_with_index_dirs(vec![root.to_string_lossy().into()]);

        let runtime = build_runtime_with_startup_calibration(config, &storage);

        assert_eq!(runtime.index.entry_count(), 0);
        assert_eq!(runtime.index.generation(), 0);
        assert!(!runtime.manifest_ready);
        assert_eq!(
            runtime.incremental_status.state,
            IncrementalState::Preparing
        );
        assert!(storage.committed_index_deltas_after(0).unwrap().is_empty());
    }

    #[test]
    fn startup_with_missing_configured_root_defers_probe_to_background_worker() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("removed-root");
        fs::create_dir_all(&root).unwrap();
        let storage = SqliteStorage::open(temp.path().join("missing-root.sqlite")).unwrap();
        let baseline_id = storage.save_completed_index_batch(1, &[]).unwrap();
        storage
            .activate_baseline_with_manifest_and_clear_incremental_state(
                baseline_id,
                0,
                &baseline_manifest_from_entries(&[], std::slice::from_ref(&root)),
            )
            .unwrap();
        fs::remove_dir_all(&root).unwrap();
        let config = QuickFoxConfig::default_with_index_dirs(vec![root.to_string_lossy().into()]);

        let runtime = build_runtime_with_startup_calibration(config, &storage);

        assert!(!runtime.manifest_ready);
        assert_eq!(
            runtime.incremental_status.state,
            IncrementalState::Preparing
        );
        assert_eq!(runtime.incremental_status.degradation_code, None);
    }

    #[test]
    fn startup_calibration_roots_cover_scan_plans_configured_and_recovered_roots() {
        let configured = PathBuf::from("/configured");
        let recovered = PathBuf::from("/recovered");
        let application = PathBuf::from("/applications");
        let hot = PathBuf::from("/hot");
        let remaining = PathBuf::from("/remaining-drive");
        let plans = vec![
            IndexScanPlan {
                include_roots: vec![application.clone(), hot.clone()],
                exclude_dirs: Vec::new(),
                exclude_patterns: Vec::new(),
                respect_project_ignores: true,
                stage: None,
            },
            IndexScanPlan {
                include_roots: vec![remaining.clone(), application.clone()],
                exclude_dirs: Vec::new(),
                exclude_patterns: Vec::new(),
                respect_project_ignores: true,
                stage: None,
            },
        ];

        let roots = startup_calibration_roots_from_plans(
            vec![configured.clone()],
            vec![recovered.clone()],
            plans,
        );

        assert_eq!(
            roots,
            vec![application, configured, hot, recovered, remaining]
        );
    }

    #[test]
    fn startup_indexing_job_is_queued_without_running_inline() {
        type PendingStartupJob = Arc<Mutex<Option<Box<dyn FnOnce() + Send>>>>;

        struct CapturingSpawner {
            pending: PendingStartupJob,
        }

        impl RefreshWorkerSpawner for CapturingSpawner {
            fn spawn(&self, task: Box<dyn FnOnce() + Send>) -> Result<(), String> {
                *self.pending.lock().unwrap() = Some(task);
                Ok(())
            }
        }

        let ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ran_from_job = Arc::clone(&ran);
        let pending = Arc::new(Mutex::new(None::<Box<dyn FnOnce() + Send>>));
        let spawner = CapturingSpawner {
            pending: Arc::clone(&pending),
        };
        let gate = Arc::new(StartupIndexingGate::default());

        schedule_startup_indexing_with(&spawner, Arc::clone(&gate), move || {
            ran_from_job.store(true, std::sync::atomic::Ordering::Release)
        })
        .unwrap();

        assert!(!ran.load(std::sync::atomic::Ordering::Acquire));
        let worker = pending.lock().unwrap().take().unwrap();
        let join = thread::spawn(worker);
        std::thread::yield_now();
        assert!(!ran.load(std::sync::atomic::Ordering::Acquire));
        gate.release_after_setup();
        join.join().unwrap();
        assert!(ran.load(std::sync::atomic::Ordering::Acquire));
    }

    #[test]
    fn startup_spawn_failure_allows_exactly_one_ready_retry() {
        struct FailingSpawner;
        impl RefreshWorkerSpawner for FailingSpawner {
            fn spawn(&self, _task: Box<dyn FnOnce() + Send>) -> Result<(), String> {
                Err("injected startup spawn failure".to_owned())
            }
        }

        let gate = Arc::new(StartupIndexingGate::default());
        assert_eq!(
            schedule_startup_indexing_with(&FailingSpawner, Arc::clone(&gate), || {}),
            Err("injected startup spawn failure".to_owned())
        );
        assert!(!gate.worker_scheduled.load(Ordering::Acquire));
        gate.release_after_setup();
        assert!(gate.claim_retry());
        assert!(!gate.claim_retry());
    }

    #[test]
    fn missing_root_recovery_monitor_dispatches_when_root_returns() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("temporarily-unavailable");
        let config =
            QuickFoxConfig::default_with_index_dirs(vec![missing.to_string_lossy().into()]);
        let mut runtime = build_runtime_from_snapshot(config, None);
        let identity = begin_runtime_index_refresh(&mut runtime).unwrap().identity;
        runtime.index_refresh.active = None;
        let app = tauri::test::mock_app();
        app.manage(QuickFoxAppState {
            runtime: Mutex::new(runtime),
            index_refresh_fence: Mutex::new(()),
            window_state: Mutex::new(LauncherWindowState::default()),
            global_hotkey_status: Mutex::new(pending_global_hotkey_status()),
        });
        let state = app.state::<QuickFoxAppState>();
        let dispatched = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let dispatched_from_monitor = Arc::clone(&dispatched);

        schedule_configured_root_recovery_with(
            app.handle().clone(),
            &state,
            &identity,
            move |_| {
                dispatched_from_monitor.store(true, std::sync::atomic::Ordering::Release);
            },
        );
        assert_eq!(
            state.runtime.lock().unwrap().incremental_status.dirty_roots,
            1
        );
        fs::create_dir_all(&missing).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !dispatched.load(std::sync::atomic::Ordering::Acquire)
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(50));
        }

        assert!(dispatched.load(std::sync::atomic::Ordering::Acquire));
        let runtime = state.runtime.lock().unwrap();
        assert_eq!(
            runtime.index_refresh.root_recovery_latch.claimed_revision(),
            None
        );
        assert_eq!(runtime.incremental_status.dirty_roots, 0);
    }

    #[test]
    fn root_monitor_spawn_failure_clears_latch_and_degrades_current_revision() {
        struct FailingSpawner;

        impl crate::core::root_availability_monitor::RootMonitorSpawner for FailingSpawner {
            fn spawn(
                &self,
                _task: Box<
                    dyn FnOnce() -> crate::core::root_availability_monitor::MonitorExit + Send,
                >,
            ) -> Result<
                std::thread::JoinHandle<crate::core::root_availability_monitor::MonitorExit>,
                String,
            > {
                Err("injected root monitor spawn failure".to_owned())
            }
        }

        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("spawn-failure-root");
        let config =
            QuickFoxConfig::default_with_index_dirs(vec![missing.to_string_lossy().into_owned()]);
        let mut runtime = build_runtime_from_snapshot(config, None);
        let identity = begin_runtime_index_refresh(&mut runtime).unwrap().identity;
        runtime.index_refresh.active = None;
        let app = tauri::test::mock_app();
        app.manage(QuickFoxAppState {
            runtime: Mutex::new(runtime),
            index_refresh_fence: Mutex::new(()),
            window_state: Mutex::new(LauncherWindowState::default()),
            global_hotkey_status: Mutex::new(pending_global_hotkey_status()),
        });
        let state = app.state::<QuickFoxAppState>();

        let result = schedule_configured_root_recovery_with_spawner(
            app.handle().clone(),
            &state,
            &identity,
            &FailingSpawner,
            Arc::new(SystemRefreshWorkerSpawner),
            Duration::from_secs(60),
            |_| Ok(()),
        );

        assert_eq!(
            result,
            Err("injected root monitor spawn failure".to_owned())
        );
        {
            let runtime = state.runtime.lock().unwrap();
            assert_eq!(
                runtime.index_refresh.root_recovery_latch.claimed_revision(),
                Some(identity.config_revision)
            );
            assert_eq!(runtime.incremental_status.state, IncrementalState::Degraded);
            assert!(runtime.index_refresh.root_monitor.is_some());
        }
        stop_runtime_incremental_indexing(&state);
    }

    #[test]
    fn root_monitor_retry_spawn_failure_clears_claim_for_next_trigger() {
        struct FailingRecoverySpawner;
        impl RefreshWorkerSpawner for FailingRecoverySpawner {
            fn spawn(&self, _task: Box<dyn FnOnce() + Send>) -> Result<(), String> {
                Err("injected monitor retry spawn failure".to_owned())
            }
        }

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("available");
        fs::create_dir_all(&root).unwrap();
        let config =
            QuickFoxConfig::default_with_index_dirs(vec![root.to_string_lossy().into_owned()]);
        let mut runtime = build_runtime_from_snapshot(config, None);
        let identity = begin_runtime_index_refresh(&mut runtime).unwrap().identity;
        runtime.index_refresh.active = None;
        let app = tauri::test::mock_app();
        app.manage(QuickFoxAppState {
            runtime: Mutex::new(runtime),
            index_refresh_fence: Mutex::new(()),
            window_state: Mutex::new(LauncherWindowState::default()),
            global_hotkey_status: Mutex::new(pending_global_hotkey_status()),
        });
        let state = app.state::<QuickFoxAppState>();

        schedule_configured_root_recovery_with_spawner(
            app.handle().clone(),
            &state,
            &identity,
            &SystemRootMonitorSpawner,
            Arc::new(FailingRecoverySpawner),
            Duration::from_millis(1),
            |_| Err("injected monitor dispatch failure".to_owned()),
        )
        .unwrap();

        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let runtime = state.runtime.lock().unwrap();
            if runtime.index_refresh.pending
                && runtime
                    .index_refresh
                    .root_monitor_failure_revision
                    .is_none()
            {
                assert_eq!(
                    runtime.index_refresh.root_recovery_latch.claimed_revision(),
                    None
                );
                assert_eq!(runtime.incremental_status.state, IncrementalState::Degraded);
                break;
            }
            drop(runtime);
            assert!(
                Instant::now() < deadline,
                "monitor retry claim was not released"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        stop_runtime_incremental_indexing(&state);
    }

    #[test]
    fn runtime_shutdown_cancels_and_joins_owned_root_monitor() {
        let config = QuickFoxConfig::default_with_index_dirs(vec!["/missing".to_owned()]);
        let mut runtime = build_runtime_from_snapshot(config, None);
        runtime.index_refresh.root_monitor = Some(
            crate::core::root_availability_monitor::spawn_root_availability_monitor(
                Duration::from_secs(60),
                || Ok(false),
                || Ok(()),
            )
            .unwrap(),
        );
        let state = QuickFoxAppState {
            runtime: Mutex::new(runtime),
            index_refresh_fence: Mutex::new(()),
            window_state: Mutex::new(LauncherWindowState::default()),
            global_hotkey_status: Mutex::new(pending_global_hotkey_status()),
        };

        stop_runtime_incremental_indexing(&state);

        assert!(state
            .runtime
            .lock()
            .unwrap()
            .index_refresh
            .root_monitor
            .is_none());
    }

    #[test]
    fn baseline_persistence_failure_stops_noop_successor_and_requests_one_recovery() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("persistence-root");
        fs::create_dir_all(&root).unwrap();
        let database_path = temp.path().join("persistence-failure.sqlite");
        let storage = SqliteStorage::open(database_path.clone()).unwrap();
        let baseline_id = storage.save_completed_index_batch(1, &[]).unwrap();
        storage
            .activate_baseline_with_manifest_and_clear_incremental_state(
                baseline_id,
                0,
                &baseline_manifest_from_entries(&[], std::slice::from_ref(&root)),
            )
            .unwrap();
        let config =
            QuickFoxConfig::default_with_index_dirs(vec![root.to_string_lossy().into_owned()]);
        let rules = IndexPathRules::from_plan(&IndexScanPlan {
            include_roots: vec![root.clone()],
            ..IndexScanPlan::default()
        })
        .unwrap();
        let noop_successor = start_runtime_indexing(
            RuntimeIndexWatcher::watch_roots(vec![root.clone()]).unwrap(),
            TargetedIndexScanner::new(rules),
            Box::new(SqliteStorage::open(database_path).unwrap()),
            RuntimeIndexingOptions {
                roots: vec![root],
                policy: crate::core::index_update_coordinator::CoordinatorPolicy::production(),
                initial_generation: 0,
            },
            |_| {},
        )
        .unwrap();
        let mut runtime = build_runtime_from_recovery(config, recover_layered_index(&storage));
        runtime.runtime_indexing = Some(noop_successor);
        let identity = begin_runtime_index_refresh(&mut runtime).unwrap().identity;
        let state = QuickFoxAppState {
            runtime: Mutex::new(runtime),
            index_refresh_fence: Mutex::new(()),
            window_state: Mutex::new(LauncherWindowState::default()),
            global_hotkey_status: Mutex::new(pending_global_hotkey_status()),
        };

        let application = apply_baseline_persistence_outcome_for_identity(
            &state,
            &identity,
            0,
            BaselinePersistenceOutcome::Failed("injected baseline write failure".to_owned()),
            2,
            true,
            &[],
        );

        assert!(matches!(
            application,
            BaselinePersistenceApplicationOutcome::Applied(Some(BaselinePersistenceApplication {
                completed: false,
                ..
            }))
        ));
        {
            let runtime = state.runtime.lock().unwrap();
            assert!(runtime.runtime_indexing.is_none());
            assert_eq!(runtime.index.entry_count(), 0);
            assert_eq!(runtime.incremental_status.state, IncrementalState::Degraded);
            assert!(runtime.index_refresh.pending);
        }
        assert!(finish_current_index_refresh(&state, &identity));
        assert!(!finish_current_index_refresh(&state, &identity));
    }

    #[test]
    fn legacy_manifest_rebuild_still_waits_for_post_registration_startup_calibration() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("watched-root");
        fs::create_dir_all(&root).unwrap();
        let file = root.join("legacy.md");
        fs::write(&file, "legacy").unwrap();
        let database_path = temp.path().join("legacy-rebuild.sqlite");
        let storage = SqliteStorage::open(database_path.clone()).unwrap();
        let entries = vec![IndexedEntry::from_path_metadata(
            &file,
            &root,
            crate::core::index::IndexedEntryKind::File,
        )];
        let legacy_id = storage.save_completed_index_batch(1, &entries).unwrap();
        storage.activate_baseline(legacy_id, 0).unwrap();
        let config =
            QuickFoxConfig::default_with_index_dirs(vec![root.to_string_lossy().into_owned()]);

        let before = build_runtime_from_recovery(config.clone(), recover_layered_index(&storage));
        assert!(!before.manifest_ready);
        assert!(before.runtime_indexing.is_none());

        let rebuilt_id = storage.save_completed_index_batch(2, &entries).unwrap();
        let manifest = baseline_manifest_from_entries(&entries, std::slice::from_ref(&root));
        storage
            .activate_baseline_with_manifest_and_clear_incremental_state(rebuilt_id, 0, &manifest)
            .unwrap();
        assert!(recover_layered_index(&storage).manifest_ready());
        let after = build_runtime_from_recovery(config.clone(), recover_layered_index(&storage));
        assert!(!after.manifest_ready);
        assert_eq!(after.incremental_status.state, IncrementalState::Preparing);
        assert!(after.runtime_indexing.is_none());
    }

    #[test]
    fn dirty_or_failed_runtime_refresh_requires_manifest_rebuild_before_restart() {
        assert!(baseline_refresh_requires_manifest_rebuild(
            BaselineRefreshReason::CalibrationFailed
        ));
        assert!(baseline_refresh_requires_manifest_rebuild(
            BaselineRefreshReason::DirtyRoots
        ));
        assert!(baseline_refresh_requires_manifest_rebuild(
            BaselineRefreshReason::WatcherFailure
        ));
        assert!(!baseline_refresh_requires_manifest_rebuild(
            BaselineRefreshReason::DeltaSafetyLimit
        ));
    }

    #[test]
    fn live_root_change_keeps_old_view_but_blocks_watcher_until_atomic_refresh_succeeds() {
        let temp = tempfile::tempdir().unwrap();
        let old_root = temp.path().join("old-root");
        let new_root = temp.path().join("new-root");
        fs::create_dir_all(&old_root).unwrap();
        fs::create_dir_all(&new_root).unwrap();
        let old_file = old_root.join("old.md");
        let new_file = new_root.join("new.md");
        fs::write(&old_file, "old").unwrap();
        fs::write(&new_file, "new").unwrap();
        let old_entry = IndexedEntry::from_path_metadata(
            &old_file,
            &old_root,
            crate::core::index::IndexedEntryKind::File,
        );
        let refreshed_entries = vec![
            old_entry.clone(),
            IndexedEntry::from_path_metadata(
                &new_file,
                &new_root,
                crate::core::index::IndexedEntryKind::File,
            ),
        ];
        let state = QuickFoxAppState {
            runtime: Mutex::new(QuickFoxRuntime {
                config: QuickFoxConfig::default_with_index_dirs(vec![old_root
                    .to_string_lossy()
                    .into_owned()]),
                index: LayeredSearchIndex::from_baseline(vec![old_entry.clone()]),
                last_report: IndexReport::default(),
                index_lifecycle: IndexLifecycle::from_ready(1, 1),
                runtime_indexing: None,
                incremental_status: RuntimeIncrementalStatus {
                    enabled: true,
                    state: IncrementalState::Watching,
                    ..RuntimeIncrementalStatus::default()
                },
                manifest_ready: true,
                index_refresh: IndexRefreshControl::default(),
            }),
            index_refresh_fence: Mutex::new(()),
            window_state: Mutex::new(LauncherWindowState::default()),
            global_hotkey_status: Mutex::new(pending_global_hotkey_status()),
        };
        let mut new_config = QuickFoxConfig::default_with_index_dirs(vec![
            old_root.to_string_lossy().into_owned(),
            new_root.to_string_lossy().into_owned(),
        ]);
        new_config.index.watcher_enabled = true;

        let failed_generation = {
            let mut runtime = state.runtime.lock().unwrap();
            replace_runtime_config_for_full_refresh(&mut runtime, new_config.clone());
            assert!(!runtime.manifest_ready);
            assert_eq!(
                runtime.incremental_status.state,
                IncrementalState::Preparing
            );
            assert_eq!(runtime.index.entries(), std::slice::from_ref(&old_entry));
            runtime.index_lifecycle.start_refresh(true)
        };
        let failed_outcome = persist_index_refresh_with(
            2,
            IndexRefreshPayload {
                entries: refreshed_entries.clone(),
                summary: IndexReport::default(),
            },
            0,
            &new_config,
            |_, _, _, _| Err("injected baseline persistence failure".to_owned()),
        );
        let failed_application =
            apply_baseline_persistence_outcome(&state, failed_generation, 0, failed_outcome, 2)
                .expect("current persistence failure applies");
        assert!(!failed_application.completed);
        assert!(!should_restart_after_baseline_persistence(
            &state,
            &failed_application,
            false,
        ));
        {
            let runtime = state.runtime.lock().unwrap();
            let roots = runtime_watch_roots(&runtime.config, &runtime.index);
            assert!(!runtime_incremental_start_allowed(&runtime, &roots));
            assert_ne!(runtime.incremental_status.state, IncrementalState::Watching);
            assert_eq!(runtime.index.entries(), std::slice::from_ref(&old_entry));
        }

        let storage = SqliteStorage::open(temp.path().join("live-config.sqlite")).unwrap();
        let successful_generation = state
            .runtime
            .lock()
            .unwrap()
            .index_lifecycle
            .start_refresh(true);
        let successful_outcome = persist_index_refresh_with(
            2,
            IndexRefreshPayload {
                entries: refreshed_entries,
                summary: IndexReport::default(),
            },
            0,
            &new_config,
            |completed_at_ms, entries, baseline_generation, _| {
                let baseline_id = storage
                    .save_completed_index_batch(completed_at_ms, entries)
                    .map_err(|error| error.to_string())?;
                let manifest =
                    baseline_manifest_from_entries(entries, &[old_root.clone(), new_root.clone()]);
                storage
                    .activate_baseline_with_manifest_and_clear_incremental_state(
                        baseline_id,
                        baseline_generation,
                        &manifest,
                    )
                    .map_err(|error| error.to_string())
            },
        );
        let successful_application = apply_baseline_persistence_outcome(
            &state,
            successful_generation,
            0,
            successful_outcome,
            2,
        )
        .expect("current persistence success applies");
        assert!(successful_application.completed);

        let runtime = state.runtime.lock().unwrap();
        let roots = runtime_watch_roots(&runtime.config, &runtime.index);
        assert!(runtime.manifest_ready);
        assert!(runtime_incremental_start_allowed(&runtime, &roots));
        assert_eq!(runtime.index.entries().len(), 2);
        assert!(recover_layered_index(&storage).manifest_covers_roots(&roots));
    }

    #[test]
    fn in_flight_old_revision_cannot_persist_after_live_root_change_and_schedules_current_refresh()
    {
        let temp = tempfile::tempdir().unwrap();
        let old_root = temp.path().join("old-root");
        let new_root = temp.path().join("new-root");
        fs::create_dir_all(&old_root).unwrap();
        fs::create_dir_all(&new_root).unwrap();
        let old_file = old_root.join("old.md");
        let new_file = new_root.join("new.md");
        fs::write(&old_file, "old").unwrap();
        fs::write(&new_file, "new").unwrap();
        let old_entry = IndexedEntry::from_path_metadata(
            &old_file,
            &old_root,
            crate::core::index::IndexedEntryKind::File,
        );
        let old_config =
            QuickFoxConfig::default_with_index_dirs(vec![old_root.to_string_lossy().into_owned()]);
        let state = QuickFoxAppState {
            runtime: Mutex::new(QuickFoxRuntime {
                config: old_config.clone(),
                index: LayeredSearchIndex::from_baseline(vec![old_entry.clone()]),
                last_report: IndexReport::default(),
                index_lifecycle: IndexLifecycle::default(),
                runtime_indexing: None,
                incremental_status: RuntimeIncrementalStatus::default(),
                manifest_ready: true,
                index_refresh: IndexRefreshControl::for_config(&old_config),
            }),
            index_refresh_fence: Mutex::new(()),
            window_state: Mutex::new(LauncherWindowState::default()),
            global_hotkey_status: Mutex::new(pending_global_hotkey_status()),
        };
        let (old_refresh, old_restart_roots) = {
            let mut runtime = state.runtime.lock().unwrap();
            let identity = begin_runtime_index_refresh(&mut runtime)
                .expect("old refresh starts")
                .identity;
            let roots = runtime_watch_roots(&runtime.config, &runtime.index);
            assert!(runtime_incremental_restart_snapshot_is_current(
                &runtime,
                identity.config_revision,
                &identity.config_fingerprint,
                &roots,
            ));
            (identity, roots)
        };
        let new_config = QuickFoxConfig::default_with_index_dirs(vec![
            old_root.to_string_lossy().into_owned(),
            new_root.to_string_lossy().into_owned(),
        ]);
        {
            let _fence = state.index_refresh_fence.lock().unwrap();
            let mut runtime = state.runtime.lock().unwrap();
            replace_runtime_config_for_full_refresh(&mut runtime, new_config);
            assert!(runtime.index_refresh.pending);
            assert!(runtime.index_refresh.config_revision > old_refresh.config_revision);
            assert!(!runtime_incremental_restart_snapshot_is_current(
                &runtime,
                old_refresh.config_revision,
                &old_refresh.config_fingerprint,
                &old_restart_roots,
            ));
            assert!(record_runtime_restart_failure_for_snapshot(
                &mut runtime,
                old_refresh.config_revision,
                &old_refresh.config_fingerprint,
                &old_restart_roots,
                RuntimeRestartFailureKind::Watcher,
            )
            .is_none());
            assert_eq!(
                runtime.incremental_status.state,
                IncrementalState::Preparing
            );
            assert!(runtime.incremental_status.degradation_code.is_none());
        }

        let persistence_called = Cell::new(false);
        let outcome = persist_index_refresh_for_identity(
            &state,
            &old_refresh,
            2,
            IndexRefreshPayload {
                entries: Vec::new(),
                summary: IndexReport::default(),
            },
            0,
            |_, _, _, _| {
                persistence_called.set(true);
                Ok(())
            },
        );

        assert!(matches!(outcome, BaselinePersistenceOutcome::Superseded));
        assert!(!persistence_called.get());
        assert_eq!(
            state.runtime.lock().unwrap().index.entries(),
            std::slice::from_ref(&old_entry)
        );
        assert!(finish_superseded_index_refresh(&state, &old_refresh));
        let current_refresh = {
            let mut runtime = state.runtime.lock().unwrap();
            begin_runtime_index_refresh(&mut runtime)
                .expect("pending current refresh starts")
                .identity
        };
        assert!(current_refresh.config_revision > old_refresh.config_revision);
        assert_ne!(
            current_refresh.config_fingerprint,
            old_refresh.config_fingerprint
        );
        assert!(!state.runtime.lock().unwrap().index_refresh.pending);

        let refreshed_entries = vec![
            old_entry,
            IndexedEntry::from_path_metadata(
                &new_file,
                &new_root,
                crate::core::index::IndexedEntryKind::File,
            ),
        ];
        let storage = SqliteStorage::open(temp.path().join("revision-race.sqlite")).unwrap();
        let current_outcome = persist_index_refresh_for_identity(
            &state,
            &current_refresh,
            3,
            IndexRefreshPayload {
                entries: refreshed_entries,
                summary: IndexReport::default(),
            },
            0,
            |completed_at_ms, entries, baseline_generation, _| {
                let baseline_id = storage
                    .save_completed_index_batch(completed_at_ms, entries)
                    .map_err(|error| error.to_string())?;
                let manifest =
                    baseline_manifest_from_entries(entries, &[old_root.clone(), new_root.clone()]);
                storage
                    .activate_baseline_with_manifest_and_clear_incremental_state(
                        baseline_id,
                        baseline_generation,
                        &manifest,
                    )
                    .map_err(|error| error.to_string())
            },
        );
        let BaselinePersistenceApplicationOutcome::Applied(Some(current_application)) =
            apply_baseline_persistence_outcome_for_identity(
                &state,
                &current_refresh,
                0,
                current_outcome,
                3,
                true,
                &[],
            )
        else {
            panic!("current revision must apply");
        };
        assert!(current_application.completed);
        assert!(!finish_current_index_refresh(&state, &current_refresh));
        let runtime = state.runtime.lock().unwrap();
        let roots = runtime_watch_roots(&runtime.config, &runtime.index);
        assert!(runtime.manifest_ready);
        assert!(runtime_incremental_start_allowed(&runtime, &roots));
        assert_eq!(runtime.index.entries().len(), 2);
        assert!(recover_layered_index(&storage).manifest_covers_roots(&roots));
    }

    #[test]
    fn completed_index_refresh_returns_status_for_frontend_event() {
        let state = QuickFoxAppState {
            runtime: Mutex::new(QuickFoxRuntime {
                config: QuickFoxConfig::default_with_index_dirs(vec!["/tmp".to_owned()]),
                index: LayeredSearchIndex::default(),
                last_report: IndexReport::default(),
                index_lifecycle: IndexLifecycle::default(),
                runtime_indexing: None,
                incremental_status: RuntimeIncrementalStatus::default(),
                manifest_ready: true,
                index_refresh: IndexRefreshControl::default(),
            }),
            index_refresh_fence: Mutex::new(()),
            window_state: Mutex::new(LauncherWindowState::default()),
            global_hotkey_status: Mutex::new(pending_global_hotkey_status()),
        };
        let generation = {
            let mut runtime = state.runtime.lock().unwrap();
            runtime.index_lifecycle.start_refresh(false)
        };

        let status = apply_completed_index_refresh(
            &state,
            generation,
            0,
            IndexReport {
                entries: vec![crate::core::index::IndexedEntry {
                    path: "/tmp/notes.md".to_owned(),
                    name: "notes.md".to_owned(),
                    kind: crate::core::index::IndexedEntryKind::File,
                    ..crate::core::index::IndexedEntry::legacy(
                        "",
                        "",
                        crate::core::index::IndexedEntryKind::File,
                    )
                }],
                failures: Vec::new(),
                ..Default::default()
            },
            123,
        )
        .expect("fresh completion emits status");

        assert_eq!(status.kind, crate::core::index::IndexStatusKind::Ready);
        assert_eq!(status.entry_count, 1);
        assert_eq!(status.completed_at_ms, Some(123));
    }

    #[test]
    fn stale_runtime_service_status_and_fallback_cannot_mutate_active_service() {
        let config = QuickFoxConfig::default_with_index_dirs(vec!["/tmp".to_owned()]);
        let current = RuntimeServiceIdentity {
            epoch: 2,
            config_revision: 0,
        };
        let stale = RuntimeServiceIdentity {
            epoch: 1,
            config_revision: 0,
        };
        let mut runtime = build_runtime_from_snapshot(config, None);
        runtime.index_refresh.active_service = Some(current);
        runtime.incremental_status.state = IncrementalState::Preparing;
        runtime.manifest_ready = true;

        assert!(apply_runtime_indexing_event(
            &mut runtime,
            &stale,
            RuntimeIndexingEvent::Status(RuntimeIncrementalStatus {
                state: IncrementalState::Watching,
                ..RuntimeIncrementalStatus::default()
            }),
        )
        .is_none());
        assert!(apply_runtime_indexing_event(
            &mut runtime,
            &stale,
            RuntimeIndexingEvent::BaselineRefreshRequired {
                reason: BaselineRefreshReason::WatcherFailure,
            },
        )
        .is_none());
        assert_eq!(
            runtime.incremental_status.state,
            IncrementalState::Preparing
        );
        assert!(runtime.manifest_ready);
    }

    #[test]
    fn committed_delta_from_previous_epoch_is_applied_by_generation_for_same_config() {
        let config = QuickFoxConfig::default_with_index_dirs(vec!["/tmp".to_owned()]);
        let current = RuntimeServiceIdentity {
            epoch: 2,
            config_revision: 0,
        };
        let stale = RuntimeServiceIdentity {
            epoch: 1,
            config_revision: 0,
        };
        let mut runtime = build_runtime_from_snapshot(config, None);
        runtime.index_refresh.active_service = Some(current);
        let application = apply_runtime_indexing_event(
            &mut runtime,
            &stale,
            RuntimeIndexingEvent::DeltaCommitted(CommittedIndexDelta {
                generation: 1,
                upserts: vec![IndexedEntry::from_path_metadata(
                    "/tmp/from-old-service.md",
                    "/tmp",
                    crate::core::index::IndexedEntryKind::File,
                )],
                removals: Vec::new(),
            }),
        )
        .expect("committed delta remains recoverable across epoch handoff");

        assert!(!application.request_refresh);
        assert_eq!(runtime.index.generation(), 1);
        assert_eq!(runtime.index.entry_count(), 1);
    }

    #[test]
    fn active_old_service_status_is_rejected_after_config_revision_changes() {
        let config = QuickFoxConfig::default_with_index_dirs(vec!["/tmp".to_owned()]);
        let old_service = RuntimeServiceIdentity {
            epoch: 1,
            config_revision: 0,
        };
        let mut runtime = build_runtime_from_snapshot(config, None);
        runtime.index_refresh.active_service = Some(old_service);
        runtime.index_refresh.config_revision = 1;
        runtime.incremental_status.state = IncrementalState::Preparing;

        assert!(apply_runtime_indexing_event(
            &mut runtime,
            &old_service,
            RuntimeIndexingEvent::Status(RuntimeIncrementalStatus {
                state: IncrementalState::Watching,
                ..RuntimeIncrementalStatus::default()
            }),
        )
        .is_none());
        assert_eq!(
            runtime.incremental_status.state,
            IncrementalState::Preparing
        );
    }

    #[test]
    fn active_refresh_latches_same_revision_runtime_fallback_for_rerun() {
        let config = QuickFoxConfig::default_with_index_dirs(vec!["/tmp".to_owned()]);
        let service = RuntimeServiceIdentity {
            epoch: 1,
            config_revision: 0,
        };
        let mut runtime = build_runtime_from_snapshot(config, None);
        let refresh = begin_runtime_index_refresh(&mut runtime).expect("refresh starts");
        runtime.index_refresh.active_service = Some(service);

        let application = apply_runtime_indexing_event(
            &mut runtime,
            &service,
            RuntimeIndexingEvent::BaselineRefreshRequired {
                reason: BaselineRefreshReason::DirtyRoots,
            },
        )
        .expect("active service fallback is accepted");

        assert!(application.request_refresh);
        assert!(runtime.index_refresh.pending);
        assert_eq!(runtime.index_refresh.active, Some(refresh.identity));
    }

    #[test]
    fn active_refresh_absorbs_delta_safety_without_queuing_second_full_scan() {
        let config = QuickFoxConfig::default_with_index_dirs(vec!["/tmp".to_owned()]);
        let service = RuntimeServiceIdentity {
            epoch: 1,
            config_revision: 0,
        };
        let mut runtime = build_runtime_from_snapshot(config, None);
        let refresh = begin_runtime_index_refresh(&mut runtime).expect("refresh starts");
        runtime.index_refresh.active_service = Some(service);

        let application = apply_runtime_indexing_event(
            &mut runtime,
            &service,
            RuntimeIndexingEvent::BaselineRefreshRequired {
                reason: BaselineRefreshReason::DeltaSafetyLimit,
            },
        )
        .expect("active service delta safety is acknowledged");

        assert!(!application.request_refresh);
        assert!(!runtime.index_refresh.pending);
        assert_eq!(runtime.index_refresh.active, Some(refresh.identity));
    }

    #[test]
    fn active_refresh_absorbs_delta_commit_that_crosses_safety_limit() {
        let config = QuickFoxConfig::default_with_index_dirs(vec!["/tmp".to_owned()]);
        let service = RuntimeServiceIdentity {
            epoch: 1,
            config_revision: 0,
        };
        let mut runtime = build_runtime_from_snapshot(config, None);
        begin_runtime_index_refresh(&mut runtime).expect("refresh starts");
        runtime.index_refresh.active_service = Some(service);
        let upserts = (0..crate::core::runtime_indexing::MAX_DELTA_ENTRIES)
            .map(|ordinal| {
                IndexedEntry::legacy(
                    format!("/tmp/{ordinal}.md"),
                    format!("{ordinal}.md"),
                    IndexedEntryKind::File,
                )
            })
            .collect();

        let application = apply_runtime_indexing_event(
            &mut runtime,
            &service,
            RuntimeIndexingEvent::DeltaCommitted(CommittedIndexDelta {
                generation: 1,
                upserts,
                removals: vec![],
            }),
        )
        .expect("committed safety delta is applied");

        assert!(!application.request_refresh);
        assert!(!runtime.index_refresh.pending);
    }

    #[test]
    fn full_refresh_handoff_replays_scan_tail_and_standby_event_after_restart() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        fs::create_dir_all(&root).unwrap();
        let stale_directory = root.join("stale");
        fs::create_dir_all(&stale_directory).unwrap();
        let storage = SqliteStorage::open(temp.path().join("handoff.sqlite")).unwrap();
        let initial_id = storage.save_completed_index_batch(1, &[]).unwrap();
        let stale_entry = IndexedEntry::from_path_metadata(
            &stale_directory,
            &root,
            crate::core::index::IndexedEntryKind::Directory,
        );
        let initial_manifest = baseline_manifest_from_entries(
            std::slice::from_ref(&stale_entry),
            std::slice::from_ref(&root),
        );
        storage
            .activate_baseline_with_manifest_and_clear_incremental_state(
                initial_id,
                0,
                &initial_manifest,
            )
            .unwrap();
        fs::remove_dir_all(&stale_directory).unwrap();
        let scanned_directory = root.join("scanned");
        fs::create_dir_all(&scanned_directory).unwrap();
        let scanned_entry = IndexedEntry::from_path_metadata(
            &scanned_directory,
            &root,
            crate::core::index::IndexedEntryKind::Directory,
        );
        let during_scan = IndexedEntry::from_path_metadata(
            root.join("during-scan.md"),
            &root,
            crate::core::index::IndexedEntryKind::File,
        );
        storage
            .commit_incremental_batch(
                &CommittedIndexDelta {
                    generation: 1,
                    upserts: vec![during_scan],
                    removals: Vec::new(),
                },
                &[],
                &[],
            )
            .unwrap();

        let (tail, manifest) = load_full_refresh_handoff_snapshot(
            &storage,
            0,
            std::slice::from_ref(&root),
            std::slice::from_ref(&scanned_entry),
        )
        .unwrap();
        assert_eq!(tail.len(), 1);
        assert!(manifest
            .iter()
            .any(|row| row.path == scanned_directory.to_string_lossy()));
        assert!(!manifest
            .iter()
            .any(|row| row.path == stale_directory.to_string_lossy()));
        let scanned_id = storage.save_completed_index_batch(2, &[]).unwrap();
        storage
            .activate_baseline_with_manifest_and_clear_incremental_state(scanned_id, 0, &manifest)
            .unwrap();
        assert_eq!(recover_layered_index(&storage).index.entry_count(), 1);

        let standby_event = IndexedEntry::from_path_metadata(
            root.join("after-standby.md"),
            &root,
            crate::core::index::IndexedEntryKind::File,
        );
        storage
            .commit_incremental_batch(
                &CommittedIndexDelta {
                    generation: 2,
                    upserts: vec![standby_event],
                    removals: Vec::new(),
                },
                &[],
                &[],
            )
            .unwrap();
        let recovered = recover_layered_index(&storage);
        assert_eq!(recovered.index.entry_count(), 2);
        assert_eq!(recovered.index.generation(), 2);
        assert!(recovered.manifest_covers_roots(&[root]));
    }

    #[test]
    fn full_refresh_tail_directory_rename_cannot_resurrect_old_descendants() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let old_dir = root.join("old");
        let old_nested = old_dir.join("nested");
        let old_file = old_nested.join("ghost.md");
        let new_dir = root.join("new");
        let new_file = new_dir.join("visible.md");
        fs::create_dir_all(&old_nested).unwrap();
        fs::write(&old_file, "ghost").unwrap();
        let scanned_entries = vec![
            IndexedEntry::from_path_metadata(
                &old_dir,
                &root,
                crate::core::index::IndexedEntryKind::Directory,
            ),
            IndexedEntry::from_path_metadata(
                &old_nested,
                &root,
                crate::core::index::IndexedEntryKind::Directory,
            ),
            IndexedEntry::from_path_metadata(
                &old_file,
                &root,
                crate::core::index::IndexedEntryKind::File,
            ),
        ];
        fs::create_dir_all(&new_dir).unwrap();
        fs::write(&new_file, "visible").unwrap();
        let renamed_entries = vec![
            IndexedEntry::from_path_metadata(
                &new_dir,
                &root,
                crate::core::index::IndexedEntryKind::Directory,
            ),
            IndexedEntry::from_path_metadata(
                &new_file,
                &root,
                crate::core::index::IndexedEntryKind::File,
            ),
        ];
        let tail = CommittedIndexDelta {
            generation: 1,
            upserts: renamed_entries.clone(),
            removals: vec![old_dir.clone()],
        };

        let content_baseline =
            entries_after_committed_deltas(scanned_entries.clone(), std::slice::from_ref(&tail));
        assert_eq!(
            content_baseline
                .iter()
                .map(|entry| entry.path.clone())
                .collect::<std::collections::BTreeSet<_>>(),
            std::collections::BTreeSet::from([
                new_dir.to_string_lossy().into_owned(),
                new_file.to_string_lossy().into_owned(),
            ])
        );

        let storage = SqliteStorage::open(temp.path().join("directory-tail.sqlite")).unwrap();
        let scanned_id = storage
            .save_completed_index_batch(1, &scanned_entries)
            .unwrap();
        let scanned_manifest =
            baseline_manifest_from_entries(&scanned_entries, std::slice::from_ref(&root));
        storage
            .activate_baseline_with_manifest_and_clear_incremental_state(
                scanned_id,
                0,
                &scanned_manifest,
            )
            .unwrap();
        let new_manifest =
            baseline_manifest_from_entries(&renamed_entries, std::slice::from_ref(&root));
        storage
            .commit_incremental_batch(&tail, &new_manifest, std::slice::from_ref(&old_dir))
            .unwrap();
        let content_id = storage
            .save_completed_index_batch(2, &content_baseline)
            .unwrap();
        let content_manifest =
            baseline_manifest_from_entries(&content_baseline, std::slice::from_ref(&root));
        storage
            .activate_baseline_with_manifest_and_clear_incremental_state(
                content_id,
                0,
                &content_manifest,
            )
            .unwrap();

        let recovered = recover_layered_index(&storage);
        assert_eq!(recovered.index.entry_count(), 2);
        assert!(recovered
            .index
            .search_files(
                &crate::core::search::QueryRequest::new(
                    "ghost",
                    crate::core::search::SearchMode::Normal,
                ),
                20,
            )
            .is_empty());
        assert_eq!(
            recovered
                .index
                .search_files(
                    &crate::core::search::QueryRequest::new(
                        "visible",
                        crate::core::search::SearchMode::Normal,
                    ),
                    20,
                )
                .len(),
            1
        );
    }

    #[test]
    fn full_refresh_tail_restats_touched_nested_directory_without_manifest_upsert() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let nested = root.join("nested");
        fs::create_dir_all(&nested).unwrap();
        let mut scanned_nested = IndexedEntry::from_path_metadata(
            &nested,
            &root,
            crate::core::index::IndexedEntryKind::Directory,
        );
        scanned_nested.modified_ms = Some(10);
        let storage = SqliteStorage::open(temp.path().join("fingerprint-tail.sqlite")).unwrap();
        let baseline_id = storage
            .save_completed_index_batch(1, std::slice::from_ref(&scanned_nested))
            .unwrap();
        let baseline_manifest = baseline_manifest_from_entries(
            std::slice::from_ref(&scanned_nested),
            std::slice::from_ref(&root),
        );
        storage
            .activate_baseline_with_manifest_and_clear_incremental_state(
                baseline_id,
                0,
                &baseline_manifest,
            )
            .unwrap();
        let changed_path = nested.join("changed.md");
        fs::write(&changed_path, "changed").unwrap();
        let changed_file = IndexedEntry::from_path_metadata(
            &changed_path,
            &root,
            crate::core::index::IndexedEntryKind::File,
        );
        let expected_modified_ms = IndexedEntry::from_path_metadata(
            &nested,
            &root,
            crate::core::index::IndexedEntryKind::Directory,
        )
        .modified_ms;
        storage
            .commit_incremental_batch(
                &CommittedIndexDelta {
                    generation: 1,
                    upserts: vec![changed_file],
                    removals: Vec::new(),
                },
                &[],
                &[],
            )
            .unwrap();

        let (_, manifest) = load_full_refresh_handoff_snapshot(
            &storage,
            0,
            std::slice::from_ref(&root),
            std::slice::from_ref(&scanned_nested),
        )
        .unwrap();
        let nested_row = manifest
            .iter()
            .find(|row| row.path == nested.to_string_lossy())
            .expect("nested manifest row");
        assert_eq!(nested_row.modified_ms, expected_modified_ms);
        assert_ne!(nested_row.modified_ms, Some(10));
    }

    #[test]
    fn missing_configured_root_prevents_manifest_ready_handoff() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("not-created");
        let config =
            QuickFoxConfig::default_with_index_dirs(vec![missing.to_string_lossy().into_owned()]);

        assert!(refresh_capture_roots(&config, Vec::new()).is_err());
    }

    #[test]
    fn revision_handoff_fence_excludes_old_service_journal_tail() {
        let temp = tempfile::tempdir().unwrap();
        let new_root = temp.path().join("new-root");
        fs::create_dir_all(&new_root).unwrap();
        let storage = SqliteStorage::open(temp.path().join("revision-fence.sqlite")).unwrap();
        let baseline_id = storage.save_completed_index_batch(1, &[]).unwrap();
        storage
            .activate_baseline_with_manifest_and_clear_incremental_state(
                baseline_id,
                0,
                &baseline_manifest_from_entries(&[], std::slice::from_ref(&new_root)),
            )
            .unwrap();
        storage
            .commit_incremental_batch(
                &CommittedIndexDelta {
                    generation: 1,
                    upserts: vec![IndexedEntry::from_path_metadata(
                        "/old-root/contamination.md",
                        "/old-root",
                        IndexedEntryKind::File,
                    )],
                    removals: vec![],
                },
                &[],
                &[],
            )
            .unwrap();
        let new_entry = IndexedEntry::from_path_metadata(
            new_root.join("captured.md"),
            &new_root,
            IndexedEntryKind::File,
        );
        storage
            .commit_incremental_batch(
                &CommittedIndexDelta {
                    generation: 2,
                    upserts: vec![new_entry.clone()],
                    removals: vec![],
                },
                &[],
                &[],
            )
            .unwrap();

        let (tail, _) = load_revision_compatible_handoff_snapshot(
            &storage,
            0,
            Some(RevisionCaptureFence::after_old_service_join(1)),
            std::slice::from_ref(&new_root),
            &[],
        )
        .unwrap();

        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].generation, 2);
        assert_eq!(tail[0].upserts, vec![new_entry]);
    }

    #[test]
    fn production_config_transition_joins_old_service_before_generation_fence() {
        let temp = tempfile::Builder::new()
            .prefix("quickfox-revision-transition-")
            .tempdir_in(std::env::temp_dir())
            .unwrap();
        let old_alias = temp.path().join("old-root");
        let new_alias = temp.path().join("new-root");
        fs::create_dir_all(&old_alias).unwrap();
        fs::create_dir_all(&new_alias).unwrap();
        let old_root = old_alias.canonicalize().unwrap();
        let new_root = new_alias.canonicalize().unwrap();
        let database_path = temp.path().join("transition.sqlite");
        let storage = SqliteStorage::open(database_path.clone()).unwrap();
        let baseline_id = storage.save_completed_index_batch(1, &[]).unwrap();
        storage
            .activate_baseline_with_manifest_and_clear_incremental_state(
                baseline_id,
                0,
                &baseline_manifest_from_entries(&[], std::slice::from_ref(&old_root)),
            )
            .unwrap();
        storage
            .commit_incremental_batch(
                &CommittedIndexDelta {
                    generation: 1,
                    upserts: vec![IndexedEntry::from_path_metadata(
                        old_root.join("old-event.md"),
                        &old_root,
                        IndexedEntryKind::File,
                    )],
                    removals: Vec::new(),
                },
                &[],
                &[],
            )
            .unwrap();
        let old_config =
            QuickFoxConfig::default_with_index_dirs(vec![old_root.to_string_lossy().into()]);
        let old_rules = IndexPathRules::from_plan(&IndexScanPlan {
            include_roots: vec![old_root.clone()],
            exclude_dirs: vec![],
            exclude_patterns: vec![],
            respect_project_ignores: false,
            stage: None,
        })
        .unwrap();
        let old_handle = start_runtime_indexing(
            RuntimeIndexWatcher::watch_roots(vec![old_root.clone()]).unwrap(),
            TargetedIndexScanner::new(old_rules),
            Box::new(SqliteStorage::open(database_path.clone()).unwrap()),
            RuntimeIndexingOptions {
                roots: vec![old_root.clone()],
                policy: crate::core::index_update_coordinator::CoordinatorPolicy::production(),
                initial_generation: 1,
            },
            |_| {},
        )
        .unwrap();
        let mut runtime = build_runtime_from_recovery(
            old_config,
            recover_layered_index(&SqliteStorage::open(database_path.clone()).unwrap()),
        );
        runtime.runtime_indexing = Some(old_handle);
        runtime.index_refresh.active_service = Some(RuntimeServiceIdentity {
            epoch: 1,
            config_revision: 0,
        });
        let state = QuickFoxAppState {
            runtime: Mutex::new(runtime),
            index_refresh_fence: Mutex::new(()),
            window_state: Mutex::new(LauncherWindowState::default()),
            global_hotkey_status: Mutex::new(pending_global_hotkey_status()),
        };
        let new_config =
            QuickFoxConfig::default_with_index_dirs(vec![new_root.to_string_lossy().into()]);

        let candidate = prepare_config_revision_candidate_for_roots(
            new_config,
            &storage,
            1,
            vec![new_root.clone()],
        )
        .unwrap();
        transition_runtime_config_revision(&state, candidate, &storage).unwrap();

        let fence_generation = storage.highest_committed_generation().unwrap();
        assert_eq!(fence_generation, 1);
        assert_eq!(state.runtime.lock().unwrap().index.generation(), 1);
        fs::write(old_root.join("after-fence.md"), "must not commit").unwrap();
        assert_eq!(storage.highest_committed_generation().unwrap(), 1);
        let runtime = state.runtime.lock().unwrap();
        assert_eq!(
            runtime.config.index.include_dirs,
            vec![new_root.to_string_lossy()]
        );
        assert!(runtime.runtime_indexing.is_some());
        assert_eq!(runtime.incremental_status.state, IncrementalState::Watching);
    }

    #[test]
    fn revision_storage_fence_failure_preserves_old_config_view_and_service() {
        let temp = tempfile::tempdir().unwrap();
        let old_root = temp.path().join("old");
        let new_root = temp.path().join("new");
        fs::create_dir_all(&old_root).unwrap();
        fs::create_dir_all(&new_root).unwrap();
        let database_path = temp.path().join("fence-failure.sqlite");
        let storage = SqliteStorage::open(database_path.clone()).unwrap();
        let baseline_id = storage.save_completed_index_batch(1, &[]).unwrap();
        storage
            .activate_baseline_with_manifest_and_clear_incremental_state(
                baseline_id,
                0,
                &baseline_manifest_from_entries(&[], std::slice::from_ref(&old_root)),
            )
            .unwrap();
        let old_config =
            QuickFoxConfig::default_with_index_dirs(vec![old_root.to_string_lossy().into()]);
        let old_rules = IndexPathRules::from_plan(&IndexScanPlan {
            include_roots: vec![old_root.clone()],
            ..IndexScanPlan::default()
        })
        .unwrap();
        let old_handle = start_runtime_indexing(
            RuntimeIndexWatcher::watch_roots(vec![old_root.clone()]).unwrap(),
            TargetedIndexScanner::new(old_rules),
            Box::new(SqliteStorage::open(database_path.clone()).unwrap()),
            RuntimeIndexingOptions {
                roots: vec![old_root.clone()],
                policy: crate::core::index_update_coordinator::CoordinatorPolicy::production(),
                initial_generation: 0,
            },
            |_| {},
        )
        .unwrap();
        let mut runtime = build_runtime_from_snapshot(old_config.clone(), None);
        runtime.runtime_indexing = Some(old_handle);
        let state = QuickFoxAppState {
            runtime: Mutex::new(runtime),
            index_refresh_fence: Mutex::new(()),
            window_state: Mutex::new(LauncherWindowState::default()),
            global_hotkey_status: Mutex::new(pending_global_hotkey_status()),
        };
        rusqlite::Connection::open(database_path)
            .unwrap()
            .execute("DROP TABLE index_delta_batches", [])
            .unwrap();

        let candidate_error = prepare_config_revision_candidate_for_roots(
            QuickFoxConfig::default_with_index_dirs(vec![new_root.to_string_lossy().into()]),
            &storage,
            1,
            vec![new_root],
        );
        let result: Result<WakeShortcut, String> = match candidate_error {
            Ok(_) => panic!("storage fence preparation unexpectedly succeeded"),
            Err(error) => {
                record_config_transition_failure(&state);
                Err(error)
            }
        };

        assert!(result.is_err());
        let runtime = state.runtime.lock().unwrap();
        assert_eq!(runtime.config, old_config);
        assert!(runtime.runtime_indexing.is_some());
        assert_eq!(runtime.incremental_status.state, IncrementalState::Degraded);
        assert_eq!(runtime.index_refresh.restart_recovery_revision, None);
    }

    #[test]
    fn revision_config_persistence_failure_resumes_fenced_old_service() {
        let temp = tempfile::tempdir().unwrap();
        let old_root = temp.path().join("persist-old");
        let new_root = temp.path().join("persist-new");
        fs::create_dir_all(&old_root).unwrap();
        fs::create_dir_all(&new_root).unwrap();
        let database_path = temp.path().join("persist-failure.sqlite");
        let storage = SqliteStorage::open(database_path.clone()).unwrap();
        let baseline_id = storage.save_completed_index_batch(1, &[]).unwrap();
        storage
            .activate_baseline_with_manifest_and_clear_incremental_state(
                baseline_id,
                0,
                &baseline_manifest_from_entries(&[], std::slice::from_ref(&old_root)),
            )
            .unwrap();
        let old_config =
            QuickFoxConfig::default_with_index_dirs(vec![old_root.to_string_lossy().into()]);
        let rules = IndexPathRules::from_plan(&IndexScanPlan {
            include_roots: vec![old_root.clone()],
            ..IndexScanPlan::default()
        })
        .unwrap();
        let old_handle = start_runtime_indexing(
            RuntimeIndexWatcher::watch_roots(vec![old_root.clone()]).unwrap(),
            TargetedIndexScanner::new(rules),
            Box::new(SqliteStorage::open(database_path).unwrap()),
            RuntimeIndexingOptions {
                roots: vec![old_root],
                policy: crate::core::index_update_coordinator::CoordinatorPolicy::production(),
                initial_generation: 0,
            },
            |_| {},
        )
        .unwrap();
        let mut runtime = build_runtime_from_snapshot(old_config.clone(), None);
        runtime.runtime_indexing = Some(old_handle);
        let state = QuickFoxAppState {
            runtime: Mutex::new(runtime),
            index_refresh_fence: Mutex::new(()),
            window_state: Mutex::new(LauncherWindowState::default()),
            global_hotkey_status: Mutex::new(pending_global_hotkey_status()),
        };

        let candidate = prepare_config_revision_candidate_for_roots(
            QuickFoxConfig::default_with_index_dirs(vec![new_root.to_string_lossy().into()]),
            &storage,
            1,
            vec![new_root],
        )
        .unwrap();
        let result = transition_runtime_config_revision_with_persist(
            &state,
            candidate,
            &storage,
            || Err("injected config persistence failure".to_owned()),
            || Ok(()),
            |_, _| {},
        );

        assert_eq!(
            result,
            Err("injected config persistence failure".to_owned())
        );
        let runtime = state.runtime.lock().unwrap();
        assert_eq!(runtime.config, old_config);
        assert!(runtime.runtime_indexing.is_some());
        assert_eq!(runtime.incremental_status.state, IncrementalState::Degraded);
    }

    #[test]
    fn revision_post_persist_failures_roll_back_storage_config_and_old_service() {
        for failure_point in ["activation", "start", "fence", "config-restore"] {
            let temp = tempfile::Builder::new()
                .prefix("quickfox-revision-rollback-")
                .tempdir_in(std::env::temp_dir())
                .unwrap();
            let old_root = temp.path().join("old");
            let new_root = temp.path().join("new");
            fs::create_dir_all(&old_root).unwrap();
            fs::create_dir_all(&new_root).unwrap();
            let old_file = old_root.join("old.txt");
            let new_file = new_root.join("new.txt");
            let queued_old_file = old_root.join("queued.txt");
            fs::write(&old_file, "old").unwrap();
            fs::write(&new_file, "new").unwrap();
            let database_path = temp.path().join(format!("{failure_point}.sqlite"));
            let storage = SqliteStorage::open(database_path.clone()).unwrap();
            let old_entry =
                IndexedEntry::from_path_metadata(&old_file, &old_root, IndexedEntryKind::File);
            let baseline_id = storage
                .save_completed_index_batch(1, std::slice::from_ref(&old_entry))
                .unwrap();
            storage
                .activate_baseline_with_manifest_and_clear_incremental_state(
                    baseline_id,
                    0,
                    &baseline_manifest_from_entries(
                        std::slice::from_ref(&old_entry),
                        std::slice::from_ref(&old_root),
                    ),
                )
                .unwrap();
            let old_config = QuickFoxConfig::default_with_index_dirs(vec![old_root
                .to_string_lossy()
                .into_owned()]);
            let old_rules = IndexPathRules::from_plan(&IndexScanPlan {
                include_roots: vec![old_root.clone()],
                ..IndexScanPlan::default()
            })
            .unwrap();
            let old_handle = start_runtime_indexing(
                RuntimeIndexWatcher::watch_roots(vec![old_root.clone()]).unwrap(),
                TargetedIndexScanner::new(old_rules),
                Box::new(SqliteStorage::open(database_path.clone()).unwrap()),
                RuntimeIndexingOptions {
                    roots: vec![old_root.clone()],
                    policy: crate::core::index_update_coordinator::CoordinatorPolicy::production(),
                    initial_generation: 0,
                },
                |_| {},
            )
            .unwrap();
            let mut runtime =
                build_runtime_from_recovery(old_config.clone(), recover_layered_index(&storage));
            runtime.runtime_indexing = Some(old_handle);
            runtime.index_refresh.active_service = Some(RuntimeServiceIdentity {
                epoch: 1,
                config_revision: 0,
            });
            let state = QuickFoxAppState {
                runtime: Mutex::new(runtime),
                index_refresh_fence: Mutex::new(()),
                window_state: Mutex::new(LauncherWindowState::default()),
                global_hotkey_status: Mutex::new(pending_global_hotkey_status()),
            };
            let candidate = prepare_config_revision_candidate_for_roots(
                QuickFoxConfig::default_with_index_dirs(vec![new_root
                    .to_string_lossy()
                    .into_owned()]),
                &storage,
                1,
                vec![new_root],
            )
            .unwrap();
            let fail_activation = || {
                (failure_point != "activation")
                    .then_some(())
                    .ok_or_else(|| "injected activation failure".to_owned())
            };
            let fail_start = || {
                (failure_point != "start" && failure_point != "config-restore")
                    .then_some(())
                    .ok_or_else(|| "injected successor start failure".to_owned())
            };
            let fail_fence = || {
                (failure_point != "fence")
                    .then_some(())
                    .ok_or_else(|| "injected successor fence failure".to_owned())
            };
            let before_old_fence = || {
                if failure_point != "start" {
                    return Ok(());
                }
                fs::write(&queued_old_file, "queued old service event")
                    .map_err(|error| error.to_string())?;
                let queued_entry = IndexedEntry::from_path_metadata(
                    &queued_old_file,
                    &old_root,
                    IndexedEntryKind::File,
                );
                let root_modified_ms = fs::metadata(&old_root)
                    .ok()
                    .and_then(|metadata| metadata.modified().ok())
                    .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|duration| duration.as_millis() as i64);
                storage
                    .commit_incremental_batch(
                        &CommittedIndexDelta {
                            generation: 1,
                            upserts: vec![queued_entry],
                            removals: Vec::new(),
                        },
                        &[DirectoryFingerprint {
                            path: old_root.to_string_lossy().into_owned(),
                            parent: None,
                            root: old_root.to_string_lossy().into_owned(),
                            modified_ms: root_modified_ms,
                        }],
                        &[],
                    )
                    .map_err(|error| error.to_string())
            };

            let result = transition_runtime_config_revision_with_hooks(
                &state,
                candidate,
                &storage,
                || Ok(()),
                || {
                    (failure_point != "config-restore")
                        .then_some(())
                        .ok_or_else(|| "injected config rollback failure".to_owned())
                },
                |_, _| {},
                ConfigRevisionTransitionHooks {
                    before_old_fence: &before_old_fence,
                    before_activation: &fail_activation,
                    before_successor_start: &fail_start,
                    before_successor_fence: &fail_fence,
                    after_successor_fence: &|| Ok(()),
                },
            );

            assert!(result.is_err(), "failure point {failure_point} succeeded");
            {
                let runtime = state.runtime.lock().unwrap();
                assert_eq!(runtime.config, old_config);
                assert!(runtime.runtime_indexing.is_some());
                assert_eq!(
                    runtime
                        .index_refresh
                        .active_service
                        .unwrap()
                        .config_revision,
                    0
                );
                assert_eq!(runtime.index_refresh.restart_recovery_revision, None);
                assert!(runtime
                    .index
                    .materialized_entries()
                    .iter()
                    .any(|entry| entry.path == old_file.to_string_lossy()));
            }
            let recovery = recover_layered_index(&storage);
            assert!(recovery
                .index
                .materialized_entries()
                .iter()
                .any(|entry| entry.path == old_file.to_string_lossy()));
            if failure_point == "start" {
                assert!(recovery
                    .index
                    .materialized_entries()
                    .iter()
                    .any(|entry| entry.path == queued_old_file.to_string_lossy()));
            }
            assert_eq!(
                storage.highest_committed_generation().unwrap(),
                u64::from(failure_point == "start")
            );
            stop_runtime_incremental_indexing(&state);
        }
    }

    #[test]
    fn revision_transition_to_disabled_stops_watcher_and_persists_content_baseline() {
        let temp = tempfile::Builder::new()
            .prefix("quickfox-revision-disabled-")
            .tempdir_in(std::env::temp_dir())
            .unwrap();
        let root = temp.path().join("root");
        fs::create_dir_all(&root).unwrap();
        let file = root.join("content.txt");
        fs::write(&file, "unique disabled transition content").unwrap();
        let database_path = temp.path().join("disabled.sqlite");
        let storage = SqliteStorage::open(database_path.clone()).unwrap();
        let baseline_id = storage.save_completed_index_batch(1, &[]).unwrap();
        storage
            .activate_baseline_with_manifest_and_clear_incremental_state(
                baseline_id,
                0,
                &baseline_manifest_from_entries(&[], std::slice::from_ref(&root)),
            )
            .unwrap();
        let old_config =
            QuickFoxConfig::default_with_index_dirs(vec![root.to_string_lossy().into_owned()]);
        let old_rules = IndexPathRules::from_plan(&IndexScanPlan {
            include_roots: vec![root.clone()],
            ..IndexScanPlan::default()
        })
        .unwrap();
        let old_handle = start_runtime_indexing(
            RuntimeIndexWatcher::watch_roots(vec![root.clone()]).unwrap(),
            TargetedIndexScanner::new(old_rules),
            Box::new(SqliteStorage::open(database_path.clone()).unwrap()),
            RuntimeIndexingOptions {
                roots: vec![root.clone()],
                policy: crate::core::index_update_coordinator::CoordinatorPolicy::production(),
                initial_generation: 0,
            },
            |_| {},
        )
        .unwrap();
        let mut runtime = build_runtime_from_recovery(old_config, recover_layered_index(&storage));
        runtime.runtime_indexing = Some(old_handle);
        runtime.index_refresh.active_service = Some(RuntimeServiceIdentity {
            epoch: 1,
            config_revision: 0,
        });
        let state = QuickFoxAppState {
            runtime: Mutex::new(runtime),
            index_refresh_fence: Mutex::new(()),
            window_state: Mutex::new(LauncherWindowState::default()),
            global_hotkey_status: Mutex::new(pending_global_hotkey_status()),
        };
        let mut disabled_config =
            QuickFoxConfig::default_with_index_dirs(vec![root.to_string_lossy().into_owned()]);
        disabled_config.index.watcher_enabled = false;
        disabled_config.index.content_include_dirs = vec![root.to_string_lossy().into_owned()];
        let candidate = prepare_config_revision_candidate_for_roots(
            disabled_config,
            &storage,
            1,
            vec![root.clone()],
        )
        .unwrap();

        transition_runtime_config_revision(&state, candidate, &storage).unwrap();

        {
            let runtime = state.runtime.lock().unwrap();
            assert_eq!(runtime.incremental_status.state, IncrementalState::Disabled);
            assert!(runtime.runtime_indexing.is_none());
            assert!(runtime.index_refresh.active_service.is_none());
            assert!(runtime.index.materialized_entries().iter().any(|entry| {
                entry.path == file.to_string_lossy()
                    && entry.content_index_state == ContentIndexState::Indexed
            }));
        }
        let reopened = SqliteStorage::open(database_path).unwrap();
        let recovered_runtime = build_runtime_from_recovery(
            state.runtime.lock().unwrap().config.clone(),
            recover_layered_index(&reopened),
        );
        assert!(recovered_runtime
            .index
            .materialized_entries()
            .iter()
            .any(|entry| {
                entry.path == file.to_string_lossy()
                    && entry.content_index_state == ContentIndexState::Indexed
            }));
        let content_results = recovered_runtime.index.search(
            &crate::core::search::QueryRequest::new(
                "content:disabled",
                crate::core::search::SearchMode::Normal,
            ),
            20,
        );
        assert!(content_results
            .iter()
            .any(|result| result.detail.as_deref() == Some(file.to_string_lossy().as_ref())));
    }

    #[test]
    fn revision_candidate_calibrates_authoritative_state_after_watcher_registration() {
        let temp = tempfile::Builder::new()
            .prefix("quickfox-candidate-")
            .tempdir_in(std::env::temp_dir())
            .unwrap();
        let root = temp.path().join("candidate-root");
        fs::create_dir_all(&root).unwrap();
        let storage = SqliteStorage::open(temp.path().join("candidate.sqlite")).unwrap();
        let baseline_id = storage.save_completed_index_batch(1, &[]).unwrap();
        storage
            .activate_baseline_with_manifest_and_clear_incremental_state(
                baseline_id,
                0,
                &baseline_manifest_from_entries(&[], std::slice::from_ref(&root)),
            )
            .unwrap();
        let config =
            QuickFoxConfig::default_with_index_dirs(vec![root.to_string_lossy().into_owned()]);
        let captured = root.join("captured");
        let candidate = prepare_config_revision_candidate_with_capture_tail(
            config.clone(),
            &storage,
            1,
            vec![root.clone()],
            || {
                fs::write(&captured, "captured").unwrap();
                vec![crate::core::index_watcher::IndexWatchEvent::Create(
                    captured.clone(),
                )]
            },
        )
        .unwrap();

        assert!(!candidate
            .entries
            .iter()
            .any(|entry| entry.name == "captured"));
        assert_eq!(
            candidate.session.phase(),
            crate::core::index_refresh_orchestrator::CalibrationPhase::Calibrated
        );
        assert_eq!(storage.highest_committed_generation().unwrap(), 0);

        let runtime = build_runtime_from_recovery(config, recover_layered_index(&storage));
        let state = QuickFoxAppState {
            runtime: Mutex::new(runtime),
            index_refresh_fence: Mutex::new(()),
            window_state: Mutex::new(LauncherWindowState::default()),
            global_hotkey_status: Mutex::new(pending_global_hotkey_status()),
        };
        transition_runtime_config_revision(&state, candidate, &storage).unwrap();

        assert!(state
            .runtime
            .lock()
            .unwrap()
            .index
            .materialized_entries()
            .iter()
            .any(|entry| entry.path == captured.to_string_lossy()));
        let reopened = SqliteStorage::open(temp.path().join("candidate.sqlite")).unwrap();
        assert!(recover_layered_index(&reopened)
            .index
            .materialized_entries()
            .iter()
            .any(|entry| entry.path == captured.to_string_lossy()));
        stop_runtime_incremental_indexing(&state);
    }

    #[test]
    fn revision_successor_file_tail_refreshes_parent_fingerprint_before_restart() {
        let temp = tempfile::Builder::new()
            .prefix("quickfox-revision-successor-tail-")
            .tempdir_in(std::env::temp_dir())
            .unwrap();
        let root = temp.path().join("root");
        let nested = root.join("nested");
        fs::create_dir_all(&nested).unwrap();
        let database_path = temp.path().join("successor-tail.sqlite");
        let storage = SqliteStorage::open(database_path.clone()).unwrap();
        let nested_entry =
            IndexedEntry::from_path_metadata(&nested, &root, IndexedEntryKind::Directory);
        let baseline_id = storage
            .save_completed_index_batch(1, std::slice::from_ref(&nested_entry))
            .unwrap();
        storage
            .activate_baseline_with_manifest_and_clear_incremental_state(
                baseline_id,
                0,
                &baseline_manifest_from_entries(
                    std::slice::from_ref(&nested_entry),
                    std::slice::from_ref(&root),
                ),
            )
            .unwrap();
        let config =
            QuickFoxConfig::default_with_index_dirs(vec![root.to_string_lossy().into_owned()]);
        let candidate = prepare_config_revision_candidate_for_roots(
            config.clone(),
            &storage,
            1,
            vec![root.clone()],
        )
        .unwrap();
        let runtime = build_runtime_from_recovery(config, recover_layered_index(&storage));
        let state = QuickFoxAppState {
            runtime: Mutex::new(runtime),
            index_refresh_fence: Mutex::new(()),
            window_state: Mutex::new(LauncherWindowState::default()),
            global_hotkey_status: Mutex::new(pending_global_hotkey_status()),
        };
        let created = nested.join("created.txt");
        let after_successor_fence = || {
            std::thread::sleep(std::time::Duration::from_millis(5));
            fs::write(&created, "successor tail").map_err(|error| error.to_string())?;
            let created_entry =
                IndexedEntry::from_path_metadata(&created, &root, IndexedEntryKind::File);
            let nested_modified_ms = fs::metadata(&nested)
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_millis() as i64);
            storage
                .commit_incremental_batch(
                    &CommittedIndexDelta {
                        generation: 1,
                        upserts: vec![created_entry],
                        removals: Vec::new(),
                    },
                    &[DirectoryFingerprint {
                        path: nested.to_string_lossy().into_owned(),
                        parent: Some(root.to_string_lossy().into_owned()),
                        root: root.to_string_lossy().into_owned(),
                        modified_ms: nested_modified_ms,
                    }],
                    &[],
                )
                .map_err(|error| error.to_string())?;
            Ok(())
        };

        transition_runtime_config_revision_with_hooks(
            &state,
            candidate,
            &storage,
            || Ok(()),
            || Ok(()),
            |_, _| {},
            ConfigRevisionTransitionHooks {
                before_old_fence: &|| Ok(()),
                before_activation: &|| Ok(()),
                before_successor_start: &|| Ok(()),
                before_successor_fence: &|| Ok(()),
                after_successor_fence: &after_successor_fence,
            },
        )
        .unwrap();

        let actual_modified_ms = fs::metadata(&nested)
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        let reopened = SqliteStorage::open(database_path).unwrap();
        let nested_fingerprint = reopened
            .directory_manifest_for_root(&root)
            .unwrap()
            .into_iter()
            .find(|row| row.path == nested.to_string_lossy())
            .expect("nested directory fingerprint");
        assert_eq!(nested_fingerprint.modified_ms, Some(actual_modified_ms));
        assert!(recover_layered_index(&reopened)
            .index
            .materialized_entries()
            .iter()
            .any(|entry| entry.path == created.to_string_lossy()));
        stop_runtime_incremental_indexing(&state);
    }

    #[test]
    fn activation_crash_restart_recovers_directory_create_and_delete_while_preparing() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("activation-root");
        let deleted = root.join("deleted");
        let created = root.join("created");
        fs::create_dir_all(&deleted).unwrap();
        fs::create_dir_all(&created).unwrap();
        let database_path = temp.path().join("activation-crash.sqlite");
        let storage = SqliteStorage::open(database_path.clone()).unwrap();
        let deleted_entry =
            IndexedEntry::from_path_metadata(&deleted, &root, IndexedEntryKind::Directory);
        let initial_id = storage
            .save_completed_index_batch(1, std::slice::from_ref(&deleted_entry))
            .unwrap();
        let stale_manifest = baseline_manifest_from_entries(
            std::slice::from_ref(&deleted_entry),
            std::slice::from_ref(&root),
        );
        storage
            .activate_baseline_with_manifest_and_clear_incremental_state(
                initial_id,
                0,
                &stale_manifest,
            )
            .unwrap();
        storage
            .commit_incremental_batch(
                &CommittedIndexDelta {
                    generation: 1,
                    upserts: Vec::new(),
                    removals: vec![deleted.clone()],
                },
                &[],
                std::slice::from_ref(&deleted),
            )
            .unwrap();
        let created_entry =
            IndexedEntry::from_path_metadata(&created, &root, IndexedEntryKind::Directory);
        storage
            .commit_incremental_batch(
                &CommittedIndexDelta {
                    generation: 2,
                    upserts: vec![created_entry.clone()],
                    removals: Vec::new(),
                },
                &[DirectoryFingerprint {
                    path: created.to_string_lossy().into_owned(),
                    parent: Some(root.to_string_lossy().into_owned()),
                    root: root.to_string_lossy().into_owned(),
                    modified_ms: created_entry.modified_ms,
                }],
                &[],
            )
            .unwrap();
        let stale_id = storage
            .save_completed_index_batch(2, std::slice::from_ref(&deleted_entry))
            .unwrap();
        storage
            .activate_baseline_with_manifest_and_clear_incremental_state(
                stale_id,
                0,
                &stale_manifest,
            )
            .unwrap();
        fs::remove_dir_all(&deleted).unwrap();
        drop(storage);

        let reopened = SqliteStorage::open(database_path).unwrap();
        let config =
            QuickFoxConfig::default_with_index_dirs(vec![root.to_string_lossy().into_owned()]);
        let runtime = build_runtime_with_startup_calibration(config, &reopened);

        let created_results = runtime.index.search(
            &crate::core::search::QueryRequest::new(
                "created",
                crate::core::search::SearchMode::Normal,
            ),
            20,
        );
        let deleted_results = runtime.index.search(
            &crate::core::search::QueryRequest::new(
                "deleted",
                crate::core::search::SearchMode::Normal,
            ),
            20,
        );
        assert!(created_results
            .iter()
            .any(|result| result.detail.as_deref() == Some(created.to_string_lossy().as_ref())));
        assert!(deleted_results.is_empty());
        assert_eq!(
            runtime.incremental_status.state,
            IncrementalState::Preparing
        );
        assert!(!runtime.manifest_ready);
    }

    #[test]
    fn standby_capture_is_live_before_full_scan_handoff() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("new-configured-root");
        fs::create_dir_all(&root).unwrap();
        let config =
            QuickFoxConfig::default_with_index_dirs(vec![root.to_string_lossy().into_owned()]);
        let mut runtime = build_runtime_from_snapshot(config.clone(), None);
        let refresh = begin_runtime_index_refresh(&mut runtime).expect("refresh starts");
        let state = QuickFoxAppState {
            runtime: Mutex::new(runtime),
            index_refresh_fence: Mutex::new(()),
            window_state: Mutex::new(LauncherWindowState::default()),
            global_hotkey_status: Mutex::new(pending_global_hotkey_status()),
        };

        prepare_refresh_standby_capture(&state, &refresh.identity, &config).unwrap();
        let inbox = {
            let mut runtime = state.runtime.lock().unwrap();
            runtime
                .index_refresh
                .standby_watcher
                .as_mut()
                .unwrap()
                .take_inbox()
                .expect("standby inbox")
        };
        // FSEvents registers asynchronously even though `watch` returned successfully.
        std::thread::sleep(Duration::from_millis(500));
        let created = root.join("created-after-scan-start.md");
        fs::write(&created, "captured").unwrap();
        let canonical_created = created.canonicalize().unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut observed = false;
        while std::time::Instant::now() < deadline {
            let Ok(event) = inbox.recv_timeout(Duration::from_millis(100)) else {
                continue;
            };
            observed = match event {
                IndexWatchEvent::Create(path)
                | IndexWatchEvent::Write(path)
                | IndexWatchEvent::Remove(path) => path == created || path == canonical_created,
                IndexWatchEvent::Rename { from, to } => {
                    from == created
                        || to == created
                        || from == canonical_created
                        || to == canonical_created
                }
            };
            if observed {
                break;
            }
        }
        assert!(
            observed,
            "pre-scan standby watcher must capture root changes"
        );
    }

    #[test]
    fn real_capture_successor_drain_tail_content_and_restart_preserve_events() {
        let temp = tempfile::tempdir().unwrap();
        let root_alias = temp.path().join("root");
        fs::create_dir_all(&root_alias).unwrap();
        let root = root_alias.canonicalize().unwrap();
        let database_path = temp.path().join("real-handoff.sqlite");
        let storage = SqliteStorage::open(database_path.clone()).unwrap();
        let baseline_id = storage.save_completed_index_batch(1, &[]).unwrap();
        let initial_manifest = baseline_manifest_from_entries(&[], std::slice::from_ref(&root));
        storage
            .activate_baseline_with_manifest_and_clear_incremental_state(
                baseline_id,
                0,
                &initial_manifest,
            )
            .unwrap();
        let rules = IndexPathRules::from_plan(&IndexScanPlan {
            include_roots: vec![root.clone()],
            exclude_dirs: Vec::new(),
            exclude_patterns: Vec::new(),
            respect_project_ignores: false,
            stage: None,
        })
        .unwrap();
        let capture_events = Arc::new(Mutex::new(Vec::new()));
        let published_capture_events = Arc::clone(&capture_events);
        let capture = RuntimeIndexWatcher::watch_roots(vec![root.clone()]).unwrap();
        let capture_handle = start_runtime_indexing(
            capture,
            TargetedIndexScanner::new(rules.clone()),
            Box::new(SqliteStorage::open(database_path.clone()).unwrap()),
            RuntimeIndexingOptions {
                roots: vec![root.clone()],
                policy: crate::core::index_update_coordinator::CoordinatorPolicy::production(),
                initial_generation: 0,
            },
            move |event| published_capture_events.lock().unwrap().push(event),
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(500));
        let during_scan = root.join("during-scan.md");
        fs::write(&during_scan, "during").unwrap();
        std::thread::sleep(Duration::from_millis(1500));

        let successor = RuntimeIndexWatcher::watch_roots(vec![root.clone()]).unwrap();
        assert_eq!(
            capture_handle.handoff(),
            RuntimeIndexingHandoffOutcome::Clean
        );

        let storage = SqliteStorage::open(database_path.clone()).unwrap();
        let (tail, manifest) =
            load_full_refresh_handoff_snapshot(&storage, 0, std::slice::from_ref(&root), &[])
                .unwrap();
        let content_baseline = entries_after_committed_deltas(Vec::new(), &tail);
        assert!(
            content_baseline
                .iter()
                .any(|entry| entry.path == during_scan.to_string_lossy()),
            "capture tail: {tail:?}; events: {:?}",
            capture_events.lock().unwrap()
        );
        let content_id = storage
            .save_completed_index_batch(2, &content_baseline)
            .unwrap();
        storage
            .activate_baseline_with_manifest_and_clear_incremental_state(content_id, 0, &manifest)
            .unwrap();

        std::thread::sleep(Duration::from_millis(500));
        let after_barrier = root.join("after-barrier.md");
        fs::write(&after_barrier, "after").unwrap();
        std::thread::sleep(Duration::from_millis(750));
        let successor_generation = storage.highest_committed_generation().unwrap();
        let successor_handle = start_runtime_indexing(
            successor,
            TargetedIndexScanner::new(rules),
            Box::new(SqliteStorage::open(database_path.clone()).unwrap()),
            RuntimeIndexingOptions {
                roots: vec![root.clone()],
                policy: crate::core::index_update_coordinator::CoordinatorPolicy::production(),
                initial_generation: successor_generation,
            },
            |_| {},
        )
        .unwrap();
        assert_eq!(
            successor_handle.handoff(),
            RuntimeIndexingHandoffOutcome::Clean
        );

        let recovered = recover_layered_index(&SqliteStorage::open(database_path).unwrap());
        assert_eq!(recovered.index.entry_count(), 2);
        for query in ["during-scan", "after-barrier"] {
            assert_eq!(
                recovered
                    .index
                    .search_files(
                        &crate::core::search::QueryRequest::new(
                            query,
                            crate::core::search::SearchMode::Normal,
                        ),
                        20,
                    )
                    .len(),
                1
            );
        }
    }

    #[test]
    fn production_successor_persists_event_before_activation_and_crash_recovery() {
        let temp = tempfile::Builder::new()
            .prefix("quickfox-successor-")
            .tempdir_in(std::env::temp_dir())
            .unwrap();
        let root_alias = temp.path().join("root");
        fs::create_dir_all(&root_alias).unwrap();
        let root = root_alias.canonicalize().unwrap();
        let database_path = temp.path().join("successor.sqlite");
        let storage = SqliteStorage::open(database_path.clone()).unwrap();
        let baseline_id = storage.save_completed_index_batch(1, &[]).unwrap();
        storage
            .activate_baseline_with_manifest_and_clear_incremental_state(
                baseline_id,
                0,
                &baseline_manifest_from_entries(&[], std::slice::from_ref(&root)),
            )
            .unwrap();
        let config = QuickFoxConfig::default_with_index_dirs(vec![root.to_string_lossy().into()]);
        let mut runtime =
            build_runtime_from_recovery(config.clone(), recover_layered_index(&storage));
        let refresh = begin_runtime_index_refresh(&mut runtime).unwrap();
        let state = QuickFoxAppState {
            runtime: Mutex::new(runtime),
            index_refresh_fence: Mutex::new(()),
            window_state: Mutex::new(LauncherWindowState::default()),
            global_hotkey_status: Mutex::new(pending_global_hotkey_status()),
        };
        let options = build_scan_options(&config);
        let rules = IndexPathRules::from_plan(&IndexScanPlan {
            include_roots: vec![root.clone()],
            exclude_dirs: options.exclude_dirs,
            exclude_patterns: options.exclude_patterns,
            respect_project_ignores: options.respect_project_ignores,
            stage: None,
        })
        .unwrap();
        install_durable_refresh_successor(
            &state,
            &refresh.identity,
            RuntimeIndexWatcher::watch_roots(vec![root.clone()]).unwrap(),
            rules,
            vec![root.clone()],
            SqliteStorage::open(database_path.clone()).unwrap(),
            0,
        )
        .unwrap();
        assert_eq!(
            state.runtime.lock().unwrap().incremental_status.state,
            IncrementalState::Preparing
        );
        std::thread::sleep(Duration::from_millis(500));
        let arrived_before_activation = root.join("before-activation.md");
        fs::write(&arrived_before_activation, "durable").unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(6);
        while std::time::Instant::now() < deadline
            && storage.highest_committed_generation().unwrap() == 0
        {
            std::thread::sleep(Duration::from_millis(100));
        }
        assert_eq!(storage.highest_committed_generation().unwrap(), 1);

        let tail =
            finalize_durable_refresh_successor_with_storage(&state, &refresh.identity, 0, &storage)
                .unwrap();
        assert_eq!(tail.len(), 1);
        let standby_inbox = state
            .runtime
            .lock()
            .unwrap()
            .index_refresh
            .standby_watcher
            .as_mut()
            .unwrap()
            .take_inbox()
            .unwrap();
        let after_successor_handoff = root.join("after-successor-handoff.md");
        fs::write(&after_successor_handoff, "captured by standby").unwrap();
        let observed = standby_inbox
            .recv_timeout(Duration::from_secs(3))
            .map(|event| match event {
                IndexWatchEvent::Create(path)
                | IndexWatchEvent::Write(path)
                | IndexWatchEvent::Remove(path) => path == after_successor_handoff,
                IndexWatchEvent::Rename { from, to } => {
                    from == after_successor_handoff || to == after_successor_handoff
                }
            })
            .unwrap_or(false);
        assert!(
            observed,
            "standby watcher must cover final apply/restart gap"
        );
        let recovered = recover_layered_index(&SqliteStorage::open(database_path).unwrap());
        assert_eq!(recovered.index.entry_count(), 1);
        assert_eq!(
            recovered
                .index
                .search(
                    &crate::core::search::QueryRequest::new(
                        "before-activation",
                        crate::core::search::SearchMode::Normal,
                    ),
                    10,
                )
                .len(),
            1
        );
    }

    #[test]
    fn runtime_restart_failures_share_degraded_once_per_revision_recovery_funnel() {
        for failure in [
            RuntimeRestartFailureKind::Watcher,
            RuntimeRestartFailureKind::Rules,
            RuntimeRestartFailureKind::Storage,
            RuntimeRestartFailureKind::WorkerSpawn,
            RuntimeRestartFailureKind::Handoff,
            RuntimeRestartFailureKind::Dispatch,
        ] {
            let config = QuickFoxConfig::default_with_index_dirs(vec!["/tmp".to_owned()]);
            let mut runtime = build_runtime_from_snapshot(config, None);
            runtime.index_refresh.active_service = Some(RuntimeServiceIdentity {
                epoch: 1,
                config_revision: 0,
            });
            runtime.manifest_ready = true;

            let first = record_runtime_restart_failure(&mut runtime, failure);
            assert!(first.request_recovery);
            assert_eq!(first.status.incremental.state, IncrementalState::Degraded);
            assert!(first.status.incremental.degradation_code.is_some());
            assert!(runtime.index_refresh.active_service.is_none());
            assert!(!runtime.manifest_ready);

            let repeated = record_runtime_restart_failure(&mut runtime, failure);
            assert!(!repeated.request_recovery);
        }
    }

    #[test]
    fn active_refresh_failure_schedules_exactly_one_same_revision_recovery() {
        let config = QuickFoxConfig::default_with_index_dirs(vec!["/tmp".to_owned()]);
        let mut runtime = build_runtime_from_snapshot(config, None);
        let first = begin_runtime_index_refresh(&mut runtime).expect("first refresh");
        let state = QuickFoxAppState {
            runtime: Mutex::new(runtime),
            index_refresh_fence: Mutex::new(()),
            window_state: Mutex::new(LauncherWindowState::default()),
            global_hotkey_status: Mutex::new(pending_global_hotkey_status()),
        };

        record_index_refresh_runtime_failure(
            &state,
            &first.identity,
            RuntimeRestartFailureKind::Storage,
        );
        assert!(state.runtime.lock().unwrap().index_refresh.pending);
        assert!(finish_current_index_refresh(&state, &first.identity));

        let second = {
            let mut runtime = state.runtime.lock().unwrap();
            begin_runtime_index_refresh(&mut runtime).expect("one recovery refresh")
        };
        record_index_refresh_runtime_failure(
            &state,
            &second.identity,
            RuntimeRestartFailureKind::Storage,
        );
        assert!(!state.runtime.lock().unwrap().index_refresh.pending);
        assert!(!finish_current_index_refresh(&state, &second.identity));
    }

    #[test]
    fn injected_refresh_spawn_failure_clears_standby_and_queues_one_retry() {
        struct FailingSpawner;
        impl RefreshWorkerSpawner for FailingSpawner {
            fn spawn(&self, _task: Box<dyn FnOnce() + Send>) -> Result<(), String> {
                Err("injected spawn failure".to_owned())
            }
        }
        let config = QuickFoxConfig::default_with_index_dirs(vec!["/tmp".to_owned()]);
        let mut runtime = build_runtime_from_snapshot(config, None);
        let refresh = begin_runtime_index_refresh(&mut runtime).unwrap();
        runtime.index_refresh.standby_watcher =
            Some(RuntimeIndexWatcher::watch_roots(vec![PathBuf::from("/tmp")]).unwrap());
        let state = QuickFoxAppState {
            runtime: Mutex::new(runtime),
            index_refresh_fence: Mutex::new(()),
            window_state: Mutex::new(LauncherWindowState::default()),
            global_hotkey_status: Mutex::new(pending_global_hotkey_status()),
        };

        let failure = match spawn_refresh_worker_for_identity(
            &state,
            &refresh.identity,
            &FailingSpawner,
            Box::new(|| {}),
        ) {
            Ok(()) => panic!("injected spawn must fail"),
            Err(failure) => failure,
        };

        assert!(failure.retry);
        assert_eq!(failure.message, "injected spawn failure");
        let runtime = state.runtime.lock().unwrap();
        assert!(runtime.index_refresh.standby_watcher.is_none());
        assert_eq!(runtime.incremental_status.state, IncrementalState::Degraded);
        assert!(!runtime.index_refresh.pending);
    }

    #[test]
    fn runtime_dispatch_recovery_spawn_failure_preserves_service_and_releases_retry_claim() {
        struct FailingSpawner;
        impl RefreshWorkerSpawner for FailingSpawner {
            fn spawn(&self, _task: Box<dyn FnOnce() + Send>) -> Result<(), String> {
                Err("injected dispatch recovery spawn failure".to_owned())
            }
        }

        let app = tauri::test::mock_app();
        let mut runtime =
            build_runtime_from_snapshot(QuickFoxConfig::default_with_index_dirs(Vec::new()), None);
        let service = RuntimeServiceIdentity {
            epoch: 3,
            config_revision: 0,
        };
        runtime.index_refresh.active_service = Some(service);
        runtime.index_refresh.restart_recovery_revision = Some(0);
        app.manage(QuickFoxAppState {
            runtime: Mutex::new(runtime),
            index_refresh_fence: Mutex::new(()),
            window_state: Mutex::new(LauncherWindowState::default()),
            global_hotkey_status: Mutex::new(pending_global_hotkey_status()),
        });

        assert_eq!(
            schedule_runtime_dispatch_recovery(app.handle().clone(), service, &FailingSpawner),
            Err("injected dispatch recovery spawn failure".to_owned())
        );
        mark_runtime_dispatch_recovery_spawn_failure(app.handle(), service);

        let state = app.state::<QuickFoxAppState>();
        let runtime = state.runtime.lock().unwrap();
        assert_eq!(runtime.index_refresh.active_service, Some(service));
        assert_eq!(runtime.index_refresh.restart_recovery_revision, None);
        assert!(runtime.index_refresh.pending);
        assert_eq!(runtime.incremental_status.state, IncrementalState::Degraded);
    }

    #[test]
    fn refresh_baseline_generation_absorbs_a_committed_but_queued_runtime_publish() {
        let queued_delta = CommittedIndexDelta {
            generation: 1,
            upserts: vec![IndexedEntry::legacy(
                "/tmp/new.md",
                "new.md",
                crate::core::index::IndexedEntryKind::File,
            )],
            removals: vec![PathBuf::from("/tmp/old.md")],
        };
        let state = QuickFoxAppState {
            runtime: Mutex::new(QuickFoxRuntime {
                config: QuickFoxConfig::default_with_index_dirs(vec!["/tmp".to_owned()]),
                index: LayeredSearchIndex::from_baseline(vec![IndexedEntry::legacy(
                    "/tmp/old.md",
                    "old.md",
                    crate::core::index::IndexedEntryKind::File,
                )]),
                last_report: IndexReport::default(),
                index_lifecycle: IndexLifecycle::default(),
                runtime_indexing: None,
                incremental_status: RuntimeIncrementalStatus::default(),
                manifest_ready: false,
                index_refresh: IndexRefreshControl::default(),
            }),
            index_refresh_fence: Mutex::new(()),
            window_state: Mutex::new(LauncherWindowState::default()),
            global_hotkey_status: Mutex::new(pending_global_hotkey_status()),
        };
        let refresh_generation = state
            .runtime
            .lock()
            .unwrap()
            .index_lifecycle
            .start_refresh(true);

        apply_completed_index_refresh(
            &state,
            refresh_generation,
            1,
            IndexReport {
                entries: queued_delta.upserts.clone(),
                ..IndexReport::default()
            },
            123,
        )
        .unwrap();
        state
            .runtime
            .lock()
            .unwrap()
            .index
            .apply_delta(queued_delta);

        let runtime = state.runtime.lock().unwrap();
        assert_eq!(runtime.index.generation(), 1);
        assert_eq!(runtime.index.entries().len(), 1);
        assert_eq!(runtime.index.entries()[0].name, "new.md");
    }

    #[test]
    fn production_baseline_apply_replaces_older_scan_with_authoritative_tail() {
        let config = QuickFoxConfig::default_with_index_dirs(vec!["/tmp".to_owned()]);
        let mut runtime = build_runtime_from_snapshot(config, None);
        runtime.index.replace_baseline(
            vec![IndexedEntry::legacy(
                "/tmp/old.md",
                "old.md",
                IndexedEntryKind::File,
            )],
            3,
        );
        runtime.index.apply_delta(CommittedIndexDelta {
            generation: 4,
            upserts: vec![IndexedEntry::legacy(
                "/tmp/stale-overlay.md",
                "stale-overlay.md",
                IndexedEntryKind::File,
            )],
            removals: vec![],
        });
        runtime.index.apply_delta(CommittedIndexDelta {
            generation: 5,
            upserts: vec![],
            removals: vec![],
        });
        let refresh = begin_runtime_index_refresh(&mut runtime).unwrap();
        let state = QuickFoxAppState {
            runtime: Mutex::new(runtime),
            index_refresh_fence: Mutex::new(()),
            window_state: Mutex::new(LauncherWindowState::default()),
            global_hotkey_status: Mutex::new(pending_global_hotkey_status()),
        };
        let tail = vec![
            CommittedIndexDelta {
                generation: 4,
                upserts: vec![IndexedEntry::legacy(
                    "/tmp/tail.md",
                    "tail.md",
                    IndexedEntryKind::File,
                )],
                removals: vec![],
            },
            CommittedIndexDelta {
                generation: 5,
                upserts: vec![],
                removals: vec![],
            },
        ];

        let outcome = apply_baseline_persistence_outcome_for_identity(
            &state,
            &refresh.identity,
            3,
            BaselinePersistenceOutcome::Completed(IndexRefreshPayload {
                entries: vec![IndexedEntry::legacy(
                    "/tmp/new.md",
                    "new.md",
                    IndexedEntryKind::File,
                )],
                summary: IndexReport::default(),
            }),
            10,
            true,
            &tail,
        );

        assert!(matches!(
            outcome,
            BaselinePersistenceApplicationOutcome::Applied(Some(BaselinePersistenceApplication {
                completed: true,
                ..
            }))
        ));
        let runtime = state.runtime.lock().unwrap();
        assert_eq!(runtime.index.generation(), 5);
        assert_eq!(
            runtime
                .index
                .search(
                    &crate::core::search::QueryRequest::new(
                        "new",
                        crate::core::search::SearchMode::Normal
                    ),
                    10
                )
                .len(),
            1
        );
        assert_eq!(
            runtime
                .index
                .search(
                    &crate::core::search::QueryRequest::new(
                        "tail",
                        crate::core::search::SearchMode::Normal
                    ),
                    10
                )
                .len(),
            1
        );
        assert_eq!(
            runtime
                .index
                .search(
                    &crate::core::search::QueryRequest::new(
                        "stale-overlay",
                        crate::core::search::SearchMode::Normal
                    ),
                    10
                )
                .len(),
            0
        );
    }

    #[test]
    fn completed_index_refresh_delays_configured_content_index() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("AGENTS.md");
        fs::write(
            &file,
            "intro\nAgent type: md\ncontent mentions openspec workflow\nnext line\n",
        )
        .unwrap();
        let root_text = root.path().to_string_lossy().to_string();
        let mut config = QuickFoxConfig::default_with_index_dirs(vec![root_text.clone()]);
        config.index.content_include_dirs = vec![root_text.clone()];
        config.index.watcher_enabled = false;
        let state = QuickFoxAppState {
            runtime: Mutex::new(QuickFoxRuntime {
                config: config.clone(),
                index: LayeredSearchIndex::default(),
                last_report: IndexReport::default(),
                index_lifecycle: IndexLifecycle::default(),
                runtime_indexing: None,
                incremental_status: RuntimeIncrementalStatus::default(),
                manifest_ready: true,
                index_refresh: IndexRefreshControl::default(),
            }),
            index_refresh_fence: Mutex::new(()),
            window_state: Mutex::new(LauncherWindowState::default()),
            global_hotkey_status: Mutex::new(pending_global_hotkey_status()),
        };
        let generation = {
            let mut runtime = state.runtime.lock().unwrap();
            runtime.index_lifecycle.start_refresh(false)
        };

        apply_completed_index_refresh(
            &state,
            generation,
            0,
            IndexReport {
                entries: vec![crate::core::index::IndexedEntry::from_path_metadata(
                    &file,
                    root.path(),
                    crate::core::index::IndexedEntryKind::File,
                )],
                failures: Vec::new(),
                ..Default::default()
            },
            123,
        )
        .expect("fresh completion emits status");

        let runtime = state.runtime.lock().unwrap();
        let name_results = perform_search_with_index_status(
            &runtime.config,
            &runtime.index,
            &runtime.index_status(),
            "AGENTS",
        );
        let content_results = perform_search_with_index_status(
            &runtime.config,
            &runtime.index,
            &runtime.index_status(),
            "content:openspec",
        );

        assert_eq!(name_results.len(), 1);
        assert_eq!(name_results[0].title, "AGENTS.md");
        assert!(content_results
            .iter()
            .any(|result| result.title.contains("内容索引")));
        assert_eq!(
            runtime.index.entries()[0].content_index_state,
            ContentIndexState::NotIndexed
        );
    }

    #[test]
    fn production_baseline_apply_stays_refreshing_until_content_install() {
        let config = QuickFoxConfig::default_with_index_dirs(vec!["/tmp".to_owned()]);
        let mut runtime = build_runtime_from_snapshot(config, None);
        let refresh = begin_runtime_index_refresh(&mut runtime).unwrap();
        let state = QuickFoxAppState {
            runtime: Mutex::new(runtime),
            index_refresh_fence: Mutex::new(()),
            window_state: Mutex::new(LauncherWindowState::default()),
            global_hotkey_status: Mutex::new(pending_global_hotkey_status()),
        };

        let outcome = apply_baseline_persistence_outcome_for_identity(
            &state,
            &refresh.identity,
            0,
            BaselinePersistenceOutcome::Completed(IndexRefreshPayload {
                entries: vec![IndexedEntry::legacy(
                    "/tmp/name.md",
                    "name.md",
                    IndexedEntryKind::File,
                )],
                summary: IndexReport::default(),
            }),
            10,
            false,
            &[],
        );

        let BaselinePersistenceApplicationOutcome::Applied(Some(application)) = outcome else {
            panic!("baseline applies");
        };
        assert!(application.completed);
        assert_eq!(
            application.status.kind,
            crate::core::index::IndexStatusKind::Building
        );
        assert!(state.runtime.lock().unwrap().index_refresh.active.is_some());
    }

    #[test]
    fn index_refresh_progress_updates_runtime_and_stage_payload() {
        let state = QuickFoxAppState {
            runtime: Mutex::new(QuickFoxRuntime {
                config: QuickFoxConfig::default_with_index_dirs(vec!["/tmp".to_owned()]),
                index: LayeredSearchIndex::default(),
                last_report: IndexReport::default(),
                index_lifecycle: IndexLifecycle::default(),
                runtime_indexing: None,
                incremental_status: RuntimeIncrementalStatus::default(),
                manifest_ready: true,
                index_refresh: IndexRefreshControl::default(),
            }),
            index_refresh_fence: Mutex::new(()),
            window_state: Mutex::new(LauncherWindowState::default()),
            global_hotkey_status: Mutex::new(pending_global_hotkey_status()),
        };
        let generation = {
            let mut runtime = state.runtime.lock().unwrap();
            runtime.index_lifecycle.start_refresh(false)
        };

        let status = apply_index_refresh_progress(
            &state,
            generation,
            "configured-roots".to_owned(),
            Some("/tmp".to_owned()),
            IndexReport {
                entries: vec![crate::core::index::IndexedEntry::legacy(
                    "/tmp/notes.md",
                    "notes.md",
                    crate::core::index::IndexedEntryKind::File,
                )],
                failures: Vec::new(),
                scan_stats: IndexScanStats {
                    scanned: 4,
                    accepted: 1,
                    skipped: 2,
                    failures: 1,
                },
                scan_events: Vec::new(),
            },
        )
        .expect("fresh progress emits status");

        assert_eq!(status.kind, crate::core::index::IndexStatusKind::Building);
        assert_eq!(status.entry_count, 1);
        assert_eq!(status.stage, "configured-roots");
        assert_eq!(status.current_root.as_deref(), Some("/tmp"));
        assert_eq!(status.scanned, 4);
        assert_eq!(status.accepted, 1);
        assert_eq!(status.skipped, 2);
        assert_eq!(status.failures, 1);
        assert_eq!(
            state.runtime.lock().unwrap().index.entries()[0].name,
            "notes.md"
        );
    }

    #[test]
    fn runtime_reports_do_not_retain_duplicate_entries_after_index_updates() {
        let state = QuickFoxAppState {
            runtime: Mutex::new(QuickFoxRuntime {
                config: QuickFoxConfig::default_with_index_dirs(vec!["/tmp".to_owned()]),
                index: LayeredSearchIndex::default(),
                last_report: IndexReport::default(),
                index_lifecycle: IndexLifecycle::default(),
                runtime_indexing: None,
                incremental_status: RuntimeIncrementalStatus::default(),
                manifest_ready: true,
                index_refresh: IndexRefreshControl::default(),
            }),
            index_refresh_fence: Mutex::new(()),
            window_state: Mutex::new(LauncherWindowState::default()),
            global_hotkey_status: Mutex::new(pending_global_hotkey_status()),
        };
        let generation = {
            let mut runtime = state.runtime.lock().unwrap();
            runtime.index_lifecycle.start_refresh(false)
        };

        apply_index_refresh_progress(
            &state,
            generation,
            "user-hot-paths".to_owned(),
            Some("/tmp".to_owned()),
            IndexReport {
                entries: vec![crate::core::index::IndexedEntry::legacy(
                    "/tmp/notes.md",
                    "notes.md",
                    crate::core::index::IndexedEntryKind::File,
                )],
                failures: Vec::new(),
                scan_stats: IndexScanStats {
                    scanned: 1,
                    accepted: 1,
                    skipped: 0,
                    failures: 0,
                },
                scan_events: Vec::new(),
            },
        )
        .expect("fresh progress emits status");

        {
            let runtime = state.runtime.lock().unwrap();
            assert_eq!(runtime.index.entries().len(), 1);
            assert!(runtime.last_report.entries.is_empty());
            assert_eq!(runtime.last_report.scan_stats.accepted, 1);
        }

        apply_completed_index_refresh(
            &state,
            generation,
            0,
            IndexReport {
                entries: vec![crate::core::index::IndexedEntry::legacy(
                    "/tmp/done.md",
                    "done.md",
                    crate::core::index::IndexedEntryKind::File,
                )],
                failures: Vec::new(),
                scan_stats: IndexScanStats {
                    scanned: 2,
                    accepted: 1,
                    skipped: 1,
                    failures: 0,
                },
                scan_events: Vec::new(),
            },
            456,
        )
        .expect("fresh completion emits status");

        let runtime = state.runtime.lock().unwrap();
        assert_eq!(runtime.index.entries().len(), 1);
        assert_eq!(runtime.index.entries()[0].name, "done.md");
        assert!(runtime.last_report.entries.is_empty());
        assert_eq!(runtime.last_report.scan_stats.scanned, 2);
    }

    #[test]
    fn index_refresh_accumulator_builds_progress_payload_without_entry_report_clone() {
        let mut accumulator = IndexRefreshAccumulator::default();
        accumulator.merge(IndexReport {
            entries: vec![crate::core::index::IndexedEntry::legacy(
                "/tmp/notes.md",
                "notes.md",
                crate::core::index::IndexedEntryKind::File,
            )],
            failures: Vec::new(),
            scan_stats: IndexScanStats {
                scanned: 3,
                accepted: 1,
                skipped: 2,
                failures: 0,
            },
            scan_events: vec![ScanEvent::RootFinished {
                root: "/tmp".to_owned(),
                stage: Some("user-hot-paths".to_owned()),
                stats: IndexScanStats {
                    scanned: 3,
                    accepted: 1,
                    skipped: 2,
                    failures: 0,
                },
            }],
        });

        let progress = accumulator.progress_payload();

        assert_eq!(progress.entries.len(), 1);
        assert_eq!(progress.summary.entries.len(), 0);
        assert_eq!(progress.summary.scan_stats.accepted, 1);
        assert_eq!(
            last_finished_root_for_stage(&progress.summary, "user-hot-paths").as_deref(),
            Some("/tmp")
        );
        assert_eq!(accumulator.entry_count(), 1);
    }

    #[test]
    fn quick_index_progress_is_searchable_before_background_completion() {
        let state = QuickFoxAppState {
            runtime: Mutex::new(QuickFoxRuntime {
                config: QuickFoxConfig::default_with_index_dirs(vec!["/tmp".to_owned()]),
                index: LayeredSearchIndex::default(),
                last_report: IndexReport::default(),
                index_lifecycle: IndexLifecycle::default(),
                runtime_indexing: None,
                incremental_status: RuntimeIncrementalStatus::default(),
                manifest_ready: true,
                index_refresh: IndexRefreshControl::default(),
            }),
            index_refresh_fence: Mutex::new(()),
            window_state: Mutex::new(LauncherWindowState::default()),
            global_hotkey_status: Mutex::new(pending_global_hotkey_status()),
        };
        let generation = {
            let mut runtime = state.runtime.lock().unwrap();
            runtime.index_lifecycle.start_refresh(false)
        };

        let status = apply_index_refresh_progress(
            &state,
            generation,
            "user-hot-paths".to_owned(),
            Some("/tmp".to_owned()),
            IndexReport {
                entries: vec![crate::core::index::IndexedEntry::legacy(
                    "/tmp/notes.md",
                    "notes.md",
                    crate::core::index::IndexedEntryKind::File,
                )],
                failures: Vec::new(),
                ..Default::default()
            },
        )
        .expect("fresh progress emits status");

        let runtime = state.runtime.lock().unwrap();
        let results =
            perform_search_with_index_status(&runtime.config, &runtime.index, &status, "notes");

        assert_eq!(
            status.availability,
            crate::core::index::IndexAvailability::QuickAvailable
        );
        assert!(results.iter().any(|result| result.title == "notes.md"));
    }

    #[test]
    fn failed_background_completion_keeps_quick_index_entries() {
        let state = QuickFoxAppState {
            runtime: Mutex::new(QuickFoxRuntime {
                config: QuickFoxConfig::default_with_index_dirs(vec!["/tmp".to_owned()]),
                index: LayeredSearchIndex::from_baseline(vec![
                    crate::core::index::IndexedEntry::legacy(
                        "/tmp/notes.md",
                        "notes.md",
                        crate::core::index::IndexedEntryKind::File,
                    ),
                ]),
                last_report: IndexReport::default(),
                index_lifecycle: IndexLifecycle::default(),
                runtime_indexing: None,
                incremental_status: RuntimeIncrementalStatus::default(),
                manifest_ready: true,
                index_refresh: IndexRefreshControl::default(),
            }),
            index_refresh_fence: Mutex::new(()),
            window_state: Mutex::new(LauncherWindowState::default()),
            global_hotkey_status: Mutex::new(pending_global_hotkey_status()),
        };
        let generation = {
            let mut runtime = state.runtime.lock().unwrap();
            let generation = runtime.index_lifecycle.start_refresh(false);
            runtime.index_lifecycle.update_progress(
                generation,
                "user-hot-paths",
                None,
                IndexScanStats {
                    scanned: 1,
                    accepted: 1,
                    skipped: 0,
                    failures: 0,
                },
                1,
            );
            generation
        };

        let status = apply_failed_index_refresh(&state, generation, "权限不足".to_owned())
            .expect("fresh failure emits status");

        let runtime = state.runtime.lock().unwrap();
        assert_eq!(runtime.index.entries().len(), 1);
        assert_eq!(runtime.index.entries()[0].name, "notes.md");
        assert_eq!(
            status.availability,
            crate::core::index::IndexAvailability::QuickAvailable
        );
    }

    #[test]
    fn snapshot_checkpoint_policy_skips_background_completion_stages() {
        assert!(should_persist_index_checkpoint("user-hot-paths", false));
        assert!(should_persist_index_checkpoint("configured-roots", true));
        assert!(!should_persist_index_checkpoint("configured-roots", false));
        assert!(!should_persist_index_checkpoint("remaining-drives", false));
    }

    #[test]
    fn failed_index_refresh_returns_status_for_frontend_event() {
        let state = QuickFoxAppState {
            runtime: Mutex::new(QuickFoxRuntime {
                config: QuickFoxConfig::default_with_index_dirs(vec!["/tmp".to_owned()]),
                index: LayeredSearchIndex::default(),
                last_report: IndexReport::default(),
                index_lifecycle: IndexLifecycle::default(),
                runtime_indexing: None,
                incremental_status: RuntimeIncrementalStatus::default(),
                manifest_ready: true,
                index_refresh: IndexRefreshControl::default(),
            }),
            index_refresh_fence: Mutex::new(()),
            window_state: Mutex::new(LauncherWindowState::default()),
            global_hotkey_status: Mutex::new(pending_global_hotkey_status()),
        };
        let generation = {
            let mut runtime = state.runtime.lock().unwrap();
            runtime.index_lifecycle.start_refresh(false)
        };

        let status = apply_failed_index_refresh(&state, generation, "权限不足".to_owned())
            .expect("fresh failure emits status");

        assert_eq!(status.kind, crate::core::index::IndexStatusKind::Failed);
        assert_eq!(status.message.as_deref(), Some("权限不足"));
        assert_eq!(status.incremental.state, IncrementalState::Degraded);
        assert_eq!(
            status.incremental.degradation_code,
            Some(IndexDegradationCode::FullRefreshFallback)
        );
        assert!(!state.runtime.lock().unwrap().manifest_ready);
    }

    #[test]
    fn watcher_failure_summary_is_exposed_without_disabling_ready_index() {
        let state = QuickFoxAppState {
            runtime: Mutex::new(QuickFoxRuntime {
                config: QuickFoxConfig::default_with_index_dirs(vec!["/tmp".to_owned()]),
                index: LayeredSearchIndex::from_baseline(vec![
                    crate::core::index::IndexedEntry::legacy(
                        "/tmp/notes.md",
                        "notes.md",
                        crate::core::index::IndexedEntryKind::File,
                    ),
                ]),
                last_report: IndexReport::default(),
                index_lifecycle: IndexLifecycle::from_ready(1, 123),
                runtime_indexing: None,
                incremental_status: RuntimeIncrementalStatus::default(),
                manifest_ready: true,
                index_refresh: IndexRefreshControl::default(),
            }),
            index_refresh_fence: Mutex::new(()),
            window_state: Mutex::new(LauncherWindowState::default()),
            global_hotkey_status: Mutex::new(pending_global_hotkey_status()),
        };

        let status = {
            let mut runtime = state.runtime.lock().unwrap();
            record_runtime_restart_failure(&mut runtime, RuntimeRestartFailureKind::Watcher).status
        };

        assert_eq!(status.kind, crate::core::index::IndexStatusKind::Ready);
        assert_eq!(status.entry_count, 1);
        assert_eq!(
            status.incremental.degradation_code,
            Some(IndexDegradationCode::WatcherInitializationFailed)
        );
        assert_eq!(
            status.message.as_deref(),
            Some("自动增量监听初始化失败，文件搜索仍使用最近可用索引")
        );
        assert_eq!(state.runtime.lock().unwrap().index.entries().len(), 1);
    }

    #[test]
    fn runtime_watcher_uses_index_entry_roots() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("notes.md");
        fs::write(&file, "notes").unwrap();
        let runtime = QuickFoxRuntime {
            config: QuickFoxConfig::default_with_index_dirs(vec![root
                .path()
                .to_string_lossy()
                .to_string()]),
            index: LayeredSearchIndex::from_baseline(vec![
                crate::core::index::IndexedEntry::from_path_metadata(
                    &file,
                    root.path(),
                    crate::core::index::IndexedEntryKind::File,
                ),
            ]),
            last_report: IndexReport::default(),
            index_lifecycle: IndexLifecycle::from_ready(1, 123),
            runtime_indexing: None,
            incremental_status: RuntimeIncrementalStatus::default(),
            manifest_ready: true,
            index_refresh: IndexRefreshControl::default(),
        };

        assert_eq!(
            runtime.index.watched_roots(),
            vec![root.path().to_path_buf()]
        );
    }

    #[test]
    fn completed_empty_baseline_still_watches_configured_root_for_first_file() {
        let root = tempfile::tempdir().unwrap();
        let config = QuickFoxConfig::default_with_index_dirs(vec![root
            .path()
            .to_string_lossy()
            .to_string()]);
        let index = LayeredSearchIndex::default();

        assert_eq!(
            runtime_watch_roots(&config, &index),
            vec![root.path().to_path_buf()]
        );
    }

    #[test]
    fn runtime_starts_unbuilt_when_no_persisted_snapshot_exists() {
        let runtime = build_runtime_from_snapshot(
            QuickFoxConfig::default_with_index_dirs(vec!["/tmp".to_owned()]),
            None,
        );

        assert_eq!(
            runtime.index_status().kind,
            crate::core::index::IndexStatusKind::Unbuilt
        );
        assert!(runtime.index.entries().is_empty());
    }

    #[test]
    fn perform_search_returns_empty_results_for_empty_query() {
        let config = QuickFoxConfig::default_with_index_dirs(vec!["/tmp".to_owned()]);
        let index = SearchIndex::from_entries(vec![]);

        let results = perform_search(&config, &index, "");

        assert!(results.is_empty());
    }

    #[test]
    fn perform_search_uses_registered_providers_with_runtime_index() {
        let mut config = QuickFoxConfig::default_with_index_dirs(vec!["/tmp".to_owned()]);
        config.command.enabled = true;
        let index = SearchIndex::from_entries(vec![crate::core::index::IndexedEntry {
            path: "/tmp/Downloads".to_owned(),
            name: "Downloads".to_owned(),
            kind: crate::core::index::IndexedEntryKind::Directory,
            ..crate::core::index::IndexedEntry::legacy(
                "",
                "",
                crate::core::index::IndexedEntryKind::Directory,
            )
        }]);

        let file_results = perform_search(&config, &index, "down");
        let command_results = perform_search(&config, &index, "> git status");

        assert!(file_results
            .iter()
            .any(|result| result.title == "Downloads"));
        assert!(command_results
            .iter()
            .any(|result| result.title == "git status"));
    }

    #[test]
    fn perform_search_keeps_non_file_providers_available_without_file_index() {
        let mut config = QuickFoxConfig::default_with_index_dirs(vec!["/tmp".to_owned()]);
        config.command.enabled = true;
        let index = SearchIndex::default();
        let unavailable_status = crate::core::index::IndexStatus {
            kind: crate::core::index::IndexStatusKind::Building,
            availability: crate::core::index::IndexAvailability::Unavailable,
            entry_count: 0,
            message: None,
            generation: 1,
            completed_at_ms: None,
            stage: String::new(),
            current_root: None,
            scanned: 0,
            accepted: 0,
            skipped: 0,
            failures: 0,
            incremental: crate::core::index_entry::RuntimeIncrementalStatus::default(),
        };

        let calculator_results =
            perform_search_with_index_status(&config, &index, &unavailable_status, "2^10");
        let web_results =
            perform_search_with_index_status(&config, &index, &unavailable_status, "ddg rust");
        let command_results =
            perform_search_with_index_status(&config, &index, &unavailable_status, "> git status");

        assert!(calculator_results
            .iter()
            .any(|result| result.kind == crate::core::search::SearchResultKind::Calculator));
        assert!(web_results
            .iter()
            .any(|result| result.kind == crate::core::search::SearchResultKind::WebSearch));
        assert!(command_results
            .iter()
            .any(|result| result.kind == crate::core::search::SearchResultKind::Command));
    }

    #[test]
    fn perform_search_limits_result_count() {
        let mut config = QuickFoxConfig::default_with_index_dirs(vec!["/tmp".to_owned()]);
        config.results.limit = 1;
        let index = SearchIndex::from_entries(vec![
            crate::core::index::IndexedEntry {
                path: "/tmp/Documents".to_owned(),
                name: "Documents".to_owned(),
                kind: crate::core::index::IndexedEntryKind::Directory,
                ..crate::core::index::IndexedEntry::legacy(
                    "",
                    "",
                    crate::core::index::IndexedEntryKind::Directory,
                )
            },
            crate::core::index::IndexedEntry {
                path: "/tmp/Documents-2".to_owned(),
                name: "Documents-2".to_owned(),
                kind: crate::core::index::IndexedEntryKind::Directory,
                ..crate::core::index::IndexedEntry::legacy(
                    "",
                    "",
                    crate::core::index::IndexedEntryKind::Directory,
                )
            },
        ]);

        let results = perform_search(&config, &index, "doc");

        assert_eq!(results.len(), 1);
    }

    #[test]
    fn perform_search_does_not_clone_search_index() {
        let config = QuickFoxConfig::default_with_index_dirs(vec!["/tmp".to_owned()]);
        let entries: Vec<_> = (0..100)
            .map(|index| {
                crate::core::index::IndexedEntry::legacy(
                    format!("/tmp/project-{index}.md"),
                    format!("project-{index}.md"),
                    crate::core::index::IndexedEntryKind::File,
                )
            })
            .collect();
        let index = SearchIndex::from_entries(entries);
        SearchIndex::reset_clone_count();

        let results = perform_search(&config, &index, "project");

        assert!(!results.is_empty());
        assert_eq!(SearchIndex::clone_count(), 0);
    }

    #[test]
    fn build_scan_options_adds_hidden_exclude_pattern() {
        let config = QuickFoxConfig::default_with_index_dirs(vec!["/tmp".to_owned()]);

        let options = build_scan_options(&config);

        assert!(options.exclude_patterns.contains(&".*".to_owned()));
        assert!(options.exclude_patterns.contains(&"Windows".to_owned()));
        assert!(options.exclude_patterns.contains(&"AppData".to_owned()));
        assert!(options
            .exclude_patterns
            .contains(&"System Volume Information".to_owned()));
    }

    #[test]
    fn build_scan_plan_uses_project_ignore_config() {
        let mut config = QuickFoxConfig::default_with_index_dirs(vec!["/tmp".to_owned()]);
        config.index.respect_project_ignores = false;

        let plans = build_scan_plans(&config);

        assert!(plans.iter().any(|plan| !plan.respect_project_ignores));
    }

    #[test]
    fn fast_scan_plan_skips_configured_roots() {
        let configured_root = tempfile::tempdir().unwrap();
        let mut config = QuickFoxConfig::default_with_index_dirs(vec![configured_root
            .path()
            .to_string_lossy()
            .to_string()]);
        config.index.performance_mode = crate::core::config::IndexPerformanceMode::Fast;

        let plans = build_scan_plans(&config);

        assert!(plans.iter().all(|plan| {
            plan.stage.as_ref().map(|stage| stage.name.as_str()) != Some("configured-roots")
        }));
        assert!(plans.iter().all(|plan| !plan
            .include_roots
            .contains(&configured_root.path().to_path_buf())));
    }

    #[test]
    fn balanced_scan_plan_defers_configured_roots_after_quick_stages() {
        let configured_root = tempfile::tempdir().unwrap();
        let mut config = QuickFoxConfig::default_with_index_dirs(vec![configured_root
            .path()
            .to_string_lossy()
            .to_string()]);
        config.index.performance_mode = crate::core::config::IndexPerformanceMode::Balanced;

        let plans = build_scan_plans(&config);
        let configured_plan_position = plans
            .iter()
            .position(|plan| {
                plan.stage.as_ref().map(|stage| stage.name.as_str()) == Some("configured-roots")
            })
            .expect("balanced mode should include configured roots");

        assert!(plans[configured_plan_position]
            .include_roots
            .contains(&configured_root.path().to_path_buf()));
        assert!(plans[..configured_plan_position].iter().all(|plan| plan
            .stage
            .as_ref()
            .map(|stage| stage.root_priority)
            .unwrap_or(0)
            < 30));
    }

    #[test]
    fn complete_scan_plan_keeps_configured_roots_and_exclusions() {
        let configured_root = tempfile::tempdir().unwrap();
        let excluded_root = configured_root.path().join("target");
        let mut config = QuickFoxConfig::default_with_index_dirs(vec![configured_root
            .path()
            .to_string_lossy()
            .to_string()]);
        config.index.performance_mode = crate::core::config::IndexPerformanceMode::Complete;
        config.index.exclude_dirs = vec![excluded_root.to_string_lossy().to_string()];
        config.index.exclude_patterns = vec!["*.tmp".to_owned()];

        let plans = build_scan_plans(&config);
        let configured_plan = plans
            .iter()
            .find(|plan| {
                plan.stage.as_ref().map(|stage| stage.name.as_str()) == Some("configured-roots")
            })
            .expect("complete mode should include configured roots");

        assert!(configured_plan
            .include_roots
            .contains(&configured_root.path().to_path_buf()));
        assert!(configured_plan.exclude_dirs.contains(&excluded_root));
        assert!(configured_plan
            .exclude_patterns
            .contains(&"*.tmp".to_owned()));
        assert!(configured_plan
            .exclude_patterns
            .contains(&".git".to_owned()));
    }

    #[test]
    fn disabled_incremental_status_keeps_runtime_index_available() {
        let mut runtime = QuickFoxRuntime {
            config: QuickFoxConfig::default_with_index_dirs(vec!["/tmp".to_owned()]),
            index: LayeredSearchIndex::from_baseline(vec![crate::core::index::IndexedEntry {
                path: "/tmp/report.md".to_owned(),
                name: "report.md".to_owned(),
                kind: crate::core::index::IndexedEntryKind::File,
                root: "/tmp".to_owned(),
                ..crate::core::index::IndexedEntry::legacy(
                    "",
                    "",
                    crate::core::index::IndexedEntryKind::File,
                )
            }]),
            last_report: IndexReport::default(),
            index_lifecycle: IndexLifecycle::from_ready(1, 123),
            runtime_indexing: None,
            incremental_status: RuntimeIncrementalStatus::default(),
            manifest_ready: true,
            index_refresh: IndexRefreshControl::default(),
        };
        runtime.config.index.watcher_enabled = false;
        runtime.incremental_status.enabled = false;
        runtime.incremental_status.state = IncrementalState::Disabled;

        assert_eq!(runtime.index_status().entry_count, 1);
        assert_eq!(runtime.index.entry_count(), 1);
        assert!(runtime.runtime_indexing.is_none());
    }

    #[test]
    fn windows_drive_root_discovery_uses_available_fixed_drives() {
        let roots = windows_drive_roots_from_letters(['C', 'D', 'E'], |root| {
            root == "C:\\" || root == "E:\\"
        });

        assert_eq!(roots, vec!["C:\\".to_owned(), "E:\\".to_owned()]);
    }

    #[test]
    fn build_terminal_command_returns_platform_specific_terminal_process() {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        let process = build_terminal_command("git status").unwrap();

        #[cfg(target_os = "macos")]
        assert_eq!(process.program, "osascript");

        #[cfg(target_os = "windows")]
        assert_eq!(process.program, "wt.exe");
    }

    #[test]
    fn build_open_with_application_uses_development_tool_adapter() {
        let process =
            build_open_with_application_command("/tmp/project", &OpenApplication::DevelopmentTool);

        assert!(process.is_ok() || process == Err("NoTerminalAvailable".to_owned()));
    }

    #[test]
    fn build_open_with_application_uses_system_open_with_command() {
        let process =
            build_open_with_application_command("/tmp/readme.md", &OpenApplication::SystemChooser)
                .unwrap();

        #[cfg(target_os = "macos")]
        {
            assert_eq!(process.program, "osascript");
            assert!(process.args[1].contains("choose application"));
            assert!(process.args[1].contains("/tmp/readme.md"));
        }

        #[cfg(target_os = "windows")]
        {
            assert_eq!(process.program, "rundll32.exe");
            assert_eq!(
                process.args,
                vec![
                    "shell32.dll,OpenAs_RunDLL".to_owned(),
                    "/tmp/readme.md".to_owned(),
                ]
            );
        }

        #[cfg(target_os = "linux")]
        {
            assert_eq!(process.program, "xdg-open");
            assert_eq!(process.args, vec!["/tmp/readme.md".to_owned()]);
        }
    }

    #[test]
    fn launcher_window_effect_shows_background_window_and_hides_focused_window() {
        let mut state = LauncherWindowState::default();

        assert_eq!(
            next_launcher_window_effect(false, false, &mut state),
            LauncherWindowEffect::ShowAndFocus
        );
        assert_eq!(
            next_launcher_window_effect(true, false, &mut state),
            LauncherWindowEffect::ShowAndFocus
        );
        assert_eq!(
            next_launcher_window_effect(true, true, &mut state),
            LauncherWindowEffect::Hide
        );
    }

    #[test]
    fn tray_toggle_hides_focused_window_and_shows_hidden_window() {
        let mut state = LauncherWindowState::default();

        state.show();
        assert_eq!(
            sync_launcher_window_state_for_tray_toggle(&mut state),
            LauncherWindowEffect::Hide
        );
        assert!(!state.is_visible());

        assert_eq!(
            sync_launcher_window_state_for_tray_toggle(&mut state),
            LauncherWindowEffect::ShowAndFocus
        );
        assert!(state.is_visible());
        assert!(state.is_focused());
    }

    #[test]
    fn global_hotkey_permission_failure_returns_actionable_status() {
        let status =
            failed_global_hotkey_status(&keytap::Error::PermissionDenied, &WakeShortcut::default());

        assert!(!status.enabled);
        assert!(status.message.contains("Shift+Shift"));
        #[cfg(target_os = "macos")]
        {
            assert!(status.message.contains("输入监控权限"));
            assert_eq!(
                status.permission_settings_url.as_deref(),
                Some("x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent")
            );
        }
        #[cfg(not(target_os = "macos"))]
        assert_eq!(status.permission_settings_url, None);
    }

    #[test]
    fn global_hotkey_enabled_status_describes_shift_shift() {
        let status = enabled_global_hotkey_status(&WakeShortcut::default());

        assert!(status.enabled);
        assert_eq!(status.message, "Shift+Shift 全局唤醒可用");
        assert_eq!(status.permission_settings_url, None);
    }

    #[test]
    fn global_hotkey_enabled_status_describes_custom_shortcut() {
        let status = enabled_global_hotkey_status(
            &WakeShortcut::parse("Control+Space").expect("valid shortcut"),
        );

        assert!(status.enabled);
        assert_eq!(status.message, "Control+Space 全局唤醒可用");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_system_open_with_uses_finder_to_open_with_chosen_application() {
        let command = build_system_open_with_command("/tmp/report.md").unwrap();

        assert_eq!(command.program, "osascript");
        assert!(command.args[1].contains("choose application"));
        assert!(command.args[1].contains("as alias"));
        assert!(command.args[1].contains("POSIX path of chosenApp"));
        assert!(command.args[1].contains("open -a"));
        assert!(command.args[1].contains("quoted form of \"/tmp/report.md\""));
    }

    #[test]
    fn global_hotkey_mapper_only_tracks_key_down_events() {
        assert_eq!(
            key_press_from_global_event(&keytap::Event {
                time: std::time::Instant::now(),
                kind: EventKind::KeyDown(Key::ShiftLeft),
            }),
            Some(KeyPress::KeyDown(HotkeyKey::Shift))
        );
        assert_eq!(
            key_press_from_global_event(&keytap::Event {
                time: std::time::Instant::now(),
                kind: EventKind::KeyDown(Key::A),
            }),
            Some(KeyPress::KeyDown(HotkeyKey::Character('A')))
        );
        assert_eq!(
            key_press_from_global_event(&keytap::Event {
                time: std::time::Instant::now(),
                kind: EventKind::KeyUp(Key::ControlLeft),
            }),
            Some(KeyPress::KeyUp(HotkeyKey::Control))
        );
        assert_eq!(
            key_press_from_global_event(&keytap::Event {
                time: std::time::Instant::now(),
                kind: EventKind::KeyRepeat(Key::ShiftLeft),
            }),
            None
        );
    }
}
