pub mod core;

use crate::core::actions::{Action, OpenApplication};
use crate::core::config::{ConfigStore, IndexPerformanceMode, QuickFoxConfig};
use crate::core::content_index::{ContentIndex, ContentIndexOptions};
use crate::core::index::{
    FileSearchIndex, IndexLifecycle, IndexReport, IndexScanOptions, IndexScanner, IndexStatus,
    SearchIndex,
};
use crate::core::index_entry::{
    ContentIndexState, IncrementalState, IndexDegradationCode, IndexScanStats, IndexedEntry,
    RuntimeIncrementalStatus, ScanEvent,
};
use crate::core::index_journal::recover_layered_index;
use crate::core::index_scanner::{IndexPathRules, IndexScanPlan, IndexScanStage};
use crate::core::index_watcher::{RuntimeIndexWatcher, WatcherFailure};
use crate::core::layered_index::LayeredSearchIndex;
use crate::core::platform::{
    CommandSafetyChecker, CommandSafetyDecision, DevelopmentToolAdapter, HotkeyKey, HotkeyState,
    KeyPress, LauncherWindowEffect, LauncherWindowState, ProcessCommand, WakeShortcut,
};
use crate::core::providers::{
    CalculatorProvider, CommandProvider, CommandProviderConfig, FileProvider, ProviderRegistry,
    WebSearchEngine, WebSearchProvider,
};
use crate::core::runtime_indexing::{
    baseline_refresh_event_for_delta_state, start_runtime_indexing, BaselineRefreshReason,
    RuntimeIndexingEvent, RuntimeIndexingHandle, RuntimeIndexingOptions,
};
use crate::core::search::{HistoryScores, QueryParser, QueryParserConfig, Ranker, SearchResult};
use crate::core::storage::SqliteStorage;
use crate::core::targeted_index_scanner::{baseline_manifest_from_entries, TargetedIndexScanner};
use keytap::{EventKind, Key, Tap};
use serde::Serialize;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;
use std::thread;
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct IndexRefreshIdentity {
    lifecycle_generation: u64,
    config_revision: u64,
    config_fingerprint: String,
}

#[derive(Debug, Clone, Default)]
struct IndexRefreshControl {
    config_revision: u64,
    config_fingerprint: String,
    active: Option<IndexRefreshIdentity>,
    pending: bool,
}

impl IndexRefreshControl {
    fn for_config(config: &QuickFoxConfig) -> Self {
        Self {
            config_revision: 0,
            config_fingerprint: index_semantic_config_fingerprint(config),
            active: None,
            pending: false,
        }
    }
}

struct IndexRefreshStart {
    identity: IndexRefreshIdentity,
    config: QuickFoxConfig,
    previous_runtime_indexing: Option<RuntimeIndexingHandle>,
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

    let refresh_fence = state
        .index_refresh_fence
        .lock()
        .expect("index refresh fence poisoned");
    let mut runtime = state
        .runtime
        .lock()
        .expect("quickfox runtime lock poisoned");
    if let Some(store) = config_store() {
        store.save(&config).map_err(|error| format!("{error:?}"))?;
    }
    replace_runtime_config_for_full_refresh(&mut runtime, config);
    let next_shortcut = current_wake_shortcut(&runtime.config);
    drop(runtime);
    drop(refresh_fence);
    refresh_enabled_global_hotkey_status(&app, &next_shortcut);
    let _ = start_background_index_refresh(app, &state)?;

    Ok("saved")
}

fn replace_runtime_config_for_full_refresh(runtime: &mut QuickFoxRuntime, config: QuickFoxConfig) {
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

fn index_semantic_config_fingerprint(config: &QuickFoxConfig) -> String {
    serde_json::to_string(&config.index).unwrap_or_default()
}

fn begin_runtime_index_refresh(runtime: &mut QuickFoxRuntime) -> Option<IndexRefreshStart> {
    if runtime.index_refresh.active.is_some() {
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
        previous_runtime_indexing: runtime.runtime_indexing.take(),
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
    let IndexRefreshStart {
        identity,
        config,
        previous_runtime_indexing,
    } = start;
    if let Some(previous_runtime_indexing) = previous_runtime_indexing {
        previous_runtime_indexing.stop();
    }
    let baseline_generation = if let Some(storage) = storage_store() {
        match storage.highest_committed_generation() {
            Ok(generation) => generation,
            Err(error) => {
                let message = error.to_string();
                let _ = apply_failed_index_refresh_for_identity(state, &identity, message.clone());
                if finish_current_index_refresh(state, &identity) {
                    let _ = start_background_index_refresh(app.clone(), state);
                }
                return Err(message);
            }
        }
    } else {
        state
            .runtime
            .lock()
            .expect("quickfox runtime lock poisoned")
            .index
            .generation()
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
    let spawn_result = thread::Builder::new()
        .name("quickfox-index-refresh".to_owned())
        .spawn(move || {
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
                let final_payload = accumulator.final_payload();
                let content_entries = final_payload.entries.clone();
                let should_build_content_index =
                    should_build_content_index_for_config(&config, &content_entries);
                let persistence_state = app.state::<QuickFoxAppState>();
                let persistence = persist_index_refresh_for_identity(
                    &persistence_state,
                    &identity,
                    completed_at_ms,
                    final_payload,
                    baseline_generation,
                    persist_and_activate_baseline,
                );
                let baseline_persistence_completed =
                    matches!(persistence, BaselinePersistenceOutcome::Completed(_));
                let app_for_state = app_for_update.clone();
                let persistence_identity = identity.clone();
                update_result = app_for_update.run_on_main_thread(move || {
                    let state = app_for_state.state::<QuickFoxAppState>();
                    match apply_baseline_persistence_outcome_for_identity(
                        &state,
                        &persistence_identity,
                        baseline_generation,
                        persistence,
                        completed_at_ms,
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
                        let persistence = persist_index_refresh_for_identity(
                            &persistence_state,
                            &identity,
                            content_completed_at_ms,
                            content_payload,
                            baseline_generation,
                            persist_and_activate_baseline,
                        );
                        let app_for_update = app.clone();
                        let app_for_state = app_for_update.clone();
                        let content_identity = identity.clone();
                        update_result = app_for_update.run_on_main_thread(move || {
                            let state = app_for_state.state::<QuickFoxAppState>();
                            match persistence {
                                BaselinePersistenceOutcome::Completed(payload) => {
                                    if let Some(status) =
                                        apply_completed_content_index_refresh_for_identity(
                                            &state,
                                            &content_identity,
                                            baseline_generation,
                                            content_index,
                                            payload,
                                            content_completed_at_ms,
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
        });
    if let Err(error) = spawn_result {
        let message = error.to_string();
        let _ = apply_failed_index_refresh_for_identity(state, &spawn_identity, message.clone());
        if finish_current_index_refresh(state, &spawn_identity) {
            let _ = start_background_index_refresh(spawn_failure_app, state);
        }
        return Err(message);
    }

    Ok(status)
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

fn persist_and_activate_baseline(
    completed_at_ms: i64,
    entries: &[IndexedEntry],
    baseline_generation: u64,
    config: &QuickFoxConfig,
) -> Result<(), String> {
    let Some(storage) = storage_store() else {
        return Ok(());
    };
    let baseline_id = storage
        .save_completed_index_batch(completed_at_ms, entries)
        .map_err(|error| error.to_string())?;
    let mut roots: std::collections::BTreeSet<PathBuf> = entries
        .iter()
        .filter(|entry| !entry.root.is_empty())
        .map(|entry| PathBuf::from(&entry.root))
        .collect();
    roots.extend(
        build_scan_options(config)
            .include_dirs
            .into_iter()
            .filter(|root| root.is_dir()),
    );
    let roots: Vec<_> = roots.into_iter().collect();
    let manifest = baseline_manifest_from_entries(entries, &roots);
    storage
        .activate_baseline_with_manifest_and_clear_incremental_state(
            baseline_id,
            baseline_generation,
            &manifest,
        )
        .map_err(|error| error.to_string())
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
        runtime.index_refresh.active = None;
        runtime.index_refresh.pending = false;
        return false;
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
    BaselinePersistenceApplicationOutcome::Applied(apply_baseline_persistence_outcome(
        state,
        identity.lifecycle_generation,
        baseline_generation,
        outcome,
        completed_at_ms,
    ))
}

fn apply_completed_content_index_refresh_for_identity(
    state: &QuickFoxAppState,
    identity: &IndexRefreshIdentity,
    baseline_generation: u64,
    content_index: SearchIndex,
    payload: IndexRefreshPayload,
    completed_at_ms: i64,
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
    apply_completed_content_index_refresh(
        state,
        identity.lifecycle_generation,
        baseline_generation,
        content_index,
        payload,
        completed_at_ms,
    )
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
    if !runtime
        .index_lifecycle
        .complete_refresh(generation, entry_count, completed_at_ms)
    {
        return None;
    }
    let baseline = build_search_index_for_config(&runtime.config, payload.entries);
    runtime
        .index
        .replace_baseline_search_index(baseline, baseline_generation);
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
    if !runtime
        .index_lifecycle
        .complete_refresh(generation, entry_count, completed_at_ms)
    {
        return None;
    }
    runtime
        .index
        .replace_baseline_search_index(content_index, baseline_generation);
    runtime.manifest_ready = true;
    runtime.last_report = payload.summary;
    Some(runtime.index_status())
}

fn record_watcher_failure(runtime: &mut QuickFoxRuntime, failure: WatcherFailure) -> IndexStatus {
    let _ = failure;
    runtime.incremental_status.enabled = runtime.config.index.watcher_enabled;
    runtime.incremental_status.state = IncrementalState::Degraded;
    runtime.incremental_status.degradation_code =
        Some(IndexDegradationCode::WatcherInitializationFailed);
    runtime.index_status()
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
        should_start,
    ) = {
        let mut runtime = state
            .runtime
            .lock()
            .expect("quickfox runtime lock poisoned");
        let previous = runtime.runtime_indexing.take();
        runtime.incremental_status.enabled = runtime.config.index.watcher_enabled;
        let roots = runtime_watch_roots(&runtime.config, &runtime.index);
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
            should_start,
        )
    };
    if let Some(previous) = previous {
        previous.stop();
    }
    if !should_start {
        return Ok(());
    }

    let watcher = match RuntimeIndexWatcher::watch_roots(roots.clone()) {
        Ok(watcher) => watcher,
        Err(failure) => {
            let refresh_fence = state
                .index_refresh_fence
                .lock()
                .expect("index refresh fence poisoned");
            let status = {
                let mut runtime = state
                    .runtime
                    .lock()
                    .expect("quickfox runtime lock poisoned");
                record_watcher_failure_for_restart_snapshot(
                    &mut runtime,
                    config_revision,
                    &config_fingerprint,
                    &roots,
                    failure,
                )
            };
            let Some(status) = status else {
                return Ok(());
            };
            let _ = app.emit("quickfox://index-status", status);
            drop(refresh_fence);
            return Err("runtime index watcher initialization failed".to_owned());
        }
    };
    let options = build_scan_options(&config);
    let rules = IndexPathRules::from_plan(&IndexScanPlan {
        include_roots: roots.clone(),
        exclude_dirs: options.exclude_dirs,
        exclude_patterns: options.exclude_patterns,
        respect_project_ignores: options.respect_project_ignores,
        stage: None,
    })
    .map_err(|error| error.to_string())?;
    let scanner = TargetedIndexScanner::new(rules);
    let journal =
        storage_store().ok_or_else(|| "index journal storage is unavailable".to_owned())?;
    let publish_app = app.clone();
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
    let handle = start_runtime_indexing(
        watcher,
        scanner,
        Box::new(journal),
        RuntimeIndexingOptions {
            roots,
            policy: crate::core::index_update_coordinator::CoordinatorPolicy::production(),
            initial_generation,
        },
        move |event| publish_runtime_indexing_event(publish_app.clone(), event),
    )
    .map_err(|error| error.message)?;
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

fn record_watcher_failure_for_restart_snapshot(
    runtime: &mut QuickFoxRuntime,
    config_revision: u64,
    config_fingerprint: &str,
    roots: &[PathBuf],
    failure: WatcherFailure,
) -> Option<IndexStatus> {
    if !runtime_incremental_restart_snapshot_is_current(
        runtime,
        config_revision,
        config_fingerprint,
        roots,
    ) {
        return None;
    }
    Some(record_watcher_failure(runtime, failure))
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
    event: RuntimeIndexingEvent,
) {
    let dispatch = app.clone();
    let _ = app.run_on_main_thread(move || {
        let state = dispatch.state::<QuickFoxAppState>();
        let mut request_refresh = false;
        let status = match event {
            RuntimeIndexingEvent::DeltaCommitted(delta) => {
                let mut runtime = state
                    .runtime
                    .lock()
                    .expect("quickfox runtime lock poisoned");
                runtime.index.apply_delta(delta);
                request_refresh = baseline_refresh_event_for_delta_state(
                    runtime.index.delta_entry_count(),
                    runtime.index.estimated_delta_bytes(),
                )
                .is_some();
                runtime.index_status()
            }
            RuntimeIndexingEvent::Status(incremental_status) => {
                let mut runtime = state
                    .runtime
                    .lock()
                    .expect("quickfox runtime lock poisoned");
                runtime.incremental_status = incremental_status;
                runtime.index_status()
            }
            RuntimeIndexingEvent::BaselineRefreshRequired { reason } => {
                request_refresh = true;
                let mut runtime = state
                    .runtime
                    .lock()
                    .expect("quickfox runtime lock poisoned");
                if baseline_refresh_requires_manifest_rebuild(reason) {
                    runtime.manifest_ready = false;
                }
                runtime.index_status()
            }
        };
        let _ = dispatch.emit("quickfox://index-status", status);
        if request_refresh {
            let _ = start_background_index_refresh(dispatch.clone(), &state);
        }
    });
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
    let handle = state
        .runtime
        .lock()
        .expect("quickfox runtime lock poisoned")
        .runtime_indexing
        .take();
    if let Some(handle) = handle {
        handle.stop();
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
        let recovery = recover_layered_index(&storage);
        return build_runtime_from_recovery(config, recovery);
    }
    build_runtime_from_snapshot(config, load_latest_index_snapshot())
}

fn build_runtime_from_recovery(
    config: QuickFoxConfig,
    recovery: crate::core::index_journal::IndexRecovery,
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
    let configured_roots: Vec<PathBuf> = build_scan_options(&config)
        .include_dirs
        .into_iter()
        .filter(|root| root.is_dir())
        .collect();
    let manifest_ready = recovery.manifest_covers_roots(&configured_roots);
    let index_refresh = IndexRefreshControl::for_config(&config);
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

    tauri::Builder::default()
        .manage(QuickFoxAppState {
            runtime: Mutex::new(build_runtime()),
            index_refresh_fence: Mutex::new(()),
            window_state: Mutex::new(LauncherWindowState::default()),
            global_hotkey_status: Mutex::new(pending_global_hotkey_status()),
        })
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
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
            let state = app.state::<QuickFoxAppState>();
            let recovered_incremental_ready = {
                let runtime = state
                    .runtime
                    .lock()
                    .expect("quickfox runtime lock poisoned");
                matches!(
                    runtime.index_status().kind,
                    crate::core::index::IndexStatusKind::Ready
                ) && runtime.incremental_status.degradation_code.is_none()
                    && runtime.manifest_ready
            };
            if recovered_incremental_ready {
                let _ = restart_runtime_incremental_indexing(app.handle().clone(), &state);
            } else {
                let _ = start_background_index_refresh(app.handle().clone(), &state);
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
        .run(|app, event| {
            if matches!(event, RunEvent::Exit | RunEvent::ExitRequested { .. }) {
                stop_runtime_incremental_indexing(&app.state::<QuickFoxAppState>());
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn legacy_manifest_rebuild_atomically_enables_runtime_watcher() {
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
        let after = build_runtime_from_recovery(config.clone(), recover_layered_index(&storage));
        assert!(after.manifest_ready);

        let roots = runtime_watch_roots(&after.config, &after.index);
        let watcher = RuntimeIndexWatcher::watch_roots(roots.clone()).unwrap();
        let options = build_scan_options(&config);
        let rules = IndexPathRules::from_plan(&IndexScanPlan {
            include_roots: roots.clone(),
            exclude_dirs: options.exclude_dirs,
            exclude_patterns: options.exclude_patterns,
            respect_project_ignores: options.respect_project_ignores,
            stage: None,
        })
        .unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let published = Arc::clone(&events);
        let handle = start_runtime_indexing(
            watcher,
            TargetedIndexScanner::new(rules),
            Box::new(SqliteStorage::open(database_path).unwrap()),
            RuntimeIndexingOptions {
                roots,
                policy: crate::core::index_update_coordinator::CoordinatorPolicy::new(
                    Duration::from_millis(10),
                    Duration::from_millis(25),
                ),
                initial_generation: after.index.generation(),
            },
            move |event| published.lock().unwrap().push(event),
        )
        .unwrap();
        thread::sleep(Duration::from_millis(75));
        handle.stop();

        assert!(events.lock().unwrap().iter().any(|event| matches!(
            event,
            RuntimeIndexingEvent::Status(RuntimeIncrementalStatus {
                state: IncrementalState::Watching,
                ..
            })
        )));
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
            assert!(record_watcher_failure_for_restart_snapshot(
                &mut runtime,
                old_refresh.config_revision,
                &old_refresh.config_fingerprint,
                &old_restart_roots,
                WatcherFailure::new(old_root.clone(), "injected stale watcher failure"),
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
            record_watcher_failure(
                &mut runtime,
                crate::core::index_watcher::WatcherFailure::new(
                    PathBuf::from("/tmp"),
                    "too many open files".to_owned(),
                ),
            )
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
