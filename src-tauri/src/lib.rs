pub mod core;

use crate::core::actions::{Action, OpenApplication};
use crate::core::config::{ConfigStore, IndexPerformanceMode, QuickFoxConfig};
use crate::core::content_index::{ContentIndex, ContentIndexOptions};
use crate::core::index::{
    IndexLifecycle, IndexReport, IndexScanOptions, IndexScanner, IndexStatus, SearchIndex,
};
use crate::core::index_entry::{ContentIndexState, IndexScanStats, IndexedEntry, ScanEvent};
use crate::core::index_scanner::{IndexScanPlan, IndexScanStage};
use crate::core::index_watcher::{roots_from_entries, RuntimeIndexWatcher, WatcherFailure};
use crate::core::platform::{
    CommandSafetyChecker, CommandSafetyDecision, DevelopmentToolAdapter, HotkeyKey, HotkeyState,
    KeyPress, LauncherWindowEffect, LauncherWindowState, ProcessCommand, WakeShortcut,
};
use crate::core::providers::{
    CalculatorProvider, CommandProvider, CommandProviderConfig, FileProvider, ProviderRegistry,
    WebSearchEngine, WebSearchProvider,
};
use crate::core::search::{HistoryScores, QueryParser, QueryParserConfig, Ranker, SearchResult};
use crate::core::storage::SqliteStorage;
use keytap::{EventKind, Key, Tap};
use serde::Serialize;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;
use std::thread;
use tauri::image::Image;
use tauri::{Emitter, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_opener::OpenerExt;

#[cfg(target_os = "linux")]
use crate::core::platform::LinuxTerminalAdapter;
#[cfg(target_os = "macos")]
use crate::core::platform::MacosTerminalAdapter;
#[cfg(target_os = "windows")]
use crate::core::platform::WindowsTerminalAdapter;

struct QuickFoxRuntime {
    config: QuickFoxConfig,
    index: SearchIndex,
    last_report: IndexReport,
    index_lifecycle: IndexLifecycle,
    index_watcher: Option<RuntimeIndexWatcher>,
    watcher_failure_summary: Option<String>,
}

struct QuickFoxAppState {
    runtime: Mutex<QuickFoxRuntime>,
    window_state: Mutex<LauncherWindowState>,
    global_hotkey_status: Mutex<GlobalHotkeyStatus>,
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

    let mut runtime = state
        .runtime
        .lock()
        .expect("quickfox runtime lock poisoned");
    if let Some(store) = config_store() {
        store.save(&config).map_err(|error| format!("{error:?}"))?;
    }
    runtime.config = config;
    let next_shortcut = current_wake_shortcut(&runtime.config);
    drop(runtime);
    refresh_enabled_global_hotkey_status(&app, &next_shortcut);
    let _ = start_background_index_refresh(app, &state)?;

    Ok("saved")
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
    index: &'a SearchIndex,
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
    index: &SearchIndex,
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

fn file_index_is_available(index: &SearchIndex, status: &IndexStatus) -> bool {
    !index.entries().is_empty()
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
    let (config, generation) = {
        let mut runtime = state
            .runtime
            .lock()
            .expect("quickfox runtime lock poisoned");
        let has_existing_index = !runtime.index.entries().is_empty();
        let generation = runtime.index_lifecycle.start_refresh(has_existing_index);
        (runtime.config.clone(), generation)
    };
    let status = {
        state
            .runtime
            .lock()
            .expect("quickfox runtime lock poisoned")
            .index_status()
    };

    thread::Builder::new()
        .name("quickfox-index-refresh".to_owned())
        .spawn(move || {
            let scanner = IndexScanner;
            let mut accumulator = IndexRefreshAccumulator::default();
            let mut update_result = Ok(());

            for plan in build_scan_plans(&config) {
                let stage_name = plan
                    .stage
                    .as_ref()
                    .map(|stage| stage.name.clone())
                    .unwrap_or_else(|| "configured-roots".to_owned());
                let scan_result = scanner.scan_plan(plan);
                let completed_at_ms = current_time_ms();
                let app_for_update = app.clone();
                match scan_result {
                    Ok(stage_report) => {
                        accumulator.merge(stage_report);
                        if should_persist_index_checkpoint(&stage_name, false) {
                            if let Some(storage) = storage_store() {
                                let checkpoint_entries = accumulator.entries();
                                let _ = storage.save_completed_index_batch(
                                    completed_at_ms,
                                    &checkpoint_entries,
                                );
                            }
                        }
                        let progress_payload = accumulator.progress_payload();
                        let current_root =
                            last_finished_root_for_stage(&progress_payload.summary, &stage_name);
                        let app_for_state = app_for_update.clone();
                        update_result = app_for_update.run_on_main_thread(move || {
                            let state = app_for_state.state::<QuickFoxAppState>();
                            if let Some(status) = apply_index_refresh_progress(
                                &state,
                                generation,
                                stage_name,
                                current_root,
                                progress_payload,
                            ) {
                                let _ = app_for_state.emit("quickfox://index-status", status);
                            }
                        });
                    }
                    Err(error) => {
                        let message = error.to_string();
                        let app_for_state = app_for_update.clone();
                        update_result = app_for_update.run_on_main_thread(move || {
                            let state = app_for_state.state::<QuickFoxAppState>();
                            if let Some(status) =
                                apply_failed_index_refresh(&state, generation, message)
                            {
                                let _ = app_for_state.emit("quickfox://index-status", status);
                            }
                        });
                        break;
                    }
                }

                if update_result.is_err() {
                    break;
                }
            }

            if update_result.is_ok() {
                let completed_at_ms = current_time_ms();
                let app_for_update = app.clone();
                let final_payload = accumulator.final_payload();
                let content_entries = final_payload.entries.clone();
                let should_build_content_index =
                    should_build_content_index_for_config(&config, &content_entries);
                update_result = {
                    if let Some(storage) = storage_store() {
                        let _ = storage
                            .save_completed_index_batch(completed_at_ms, &final_payload.entries);
                    }
                    let app_for_state = app_for_update.clone();
                    app_for_update.run_on_main_thread(move || {
                        let state = app_for_state.state::<QuickFoxAppState>();
                        if let Some(status) = apply_completed_index_refresh(
                            &state,
                            generation,
                            final_payload,
                            completed_at_ms,
                        ) {
                            let _ = app_for_state.emit("quickfox://index-status", status);
                        }
                    })
                };

                if update_result.is_ok() && should_build_content_index {
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
                    update_result = app_for_update.run_on_main_thread(move || {
                        let state = app_for_state.state::<QuickFoxAppState>();
                        if let Some(status) = apply_index_refresh_progress(
                            &state,
                            generation,
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
                        if let Some(storage) = storage_store() {
                            let _ = storage.save_completed_index_batch(
                                content_completed_at_ms,
                                &content_entries,
                            );
                        }
                        let content_payload = IndexRefreshPayload {
                            entries: content_entries,
                            summary: content_report,
                        };
                        let app_for_update = app.clone();
                        let app_for_state = app_for_update.clone();
                        update_result = app_for_update.run_on_main_thread(move || {
                            let state = app_for_state.state::<QuickFoxAppState>();
                            if let Some(status) = apply_completed_content_index_refresh(
                                &state,
                                generation,
                                content_index,
                                content_payload,
                                content_completed_at_ms,
                            ) {
                                let _ = app_for_state.emit("quickfox://index-status", status);
                            }
                        });
                    }
                }
            }

            if let Err(error) = update_result {
                eprintln!("QuickFox index refresh dispatch failed: {error}");
            }
        })
        .map_err(|error| error.to_string())?;

    Ok(status)
}

fn apply_completed_index_refresh(
    state: &QuickFoxAppState,
    generation: u64,
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
    runtime.index = build_search_index_for_config(&runtime.config, payload.entries);
    runtime.last_report = payload.summary;
    match start_runtime_index_watcher(&mut runtime) {
        Ok(()) => {}
        Err(failure) => {
            let _ = record_watcher_failure(&mut runtime, failure);
        }
    }
    Some(runtime.index_status())
}

fn apply_completed_content_index_refresh(
    state: &QuickFoxAppState,
    generation: u64,
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
    runtime.index = content_index;
    runtime.last_report = payload.summary;
    Some(runtime.index_status())
}

fn start_runtime_index_watcher(runtime: &mut QuickFoxRuntime) -> Result<(), WatcherFailure> {
    if !runtime.config.index.watcher_enabled {
        runtime.index_watcher = None;
        runtime.watcher_failure_summary = None;
        return Ok(());
    }

    let roots = roots_from_entries(runtime.index.entries());
    if roots.is_empty() {
        runtime.index_watcher = None;
        runtime.watcher_failure_summary = None;
        return Ok(());
    }

    let (sender, _receiver) = std::sync::mpsc::channel();
    let watcher = RuntimeIndexWatcher::watch_roots(roots, sender)?;
    runtime.index_watcher = Some(watcher);
    runtime.watcher_failure_summary = None;
    Ok(())
}

fn record_watcher_failure(runtime: &mut QuickFoxRuntime, failure: WatcherFailure) -> IndexStatus {
    runtime.watcher_failure_summary = Some(failure.message);
    runtime.index_status()
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
    runtime.index = SearchIndex::from_entries(payload.entries);
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
    build_runtime_from_snapshot(config, load_latest_index_snapshot())
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
            build_search_index_for_config(&config, snapshot.entries),
            IndexLifecycle::from_ready(entry_count, completed_at_ms),
            IndexReport::default(),
        )
    } else {
        (
            build_search_index_for_config(&config, Vec::new()),
            IndexLifecycle::default(),
            IndexReport::default(),
        )
    };
    QuickFoxRuntime {
        config,
        index,
        index_lifecycle,
        last_report: report,
        index_watcher: None,
        watcher_failure_summary: None,
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
        if let Some(message) = &self.watcher_failure_summary {
            status.message = Some(message.clone());
        }
        status
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
            let _ = start_background_index_refresh(app.handle().clone(), &app.state());

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
        .run(tauri::generate_context!())
        .expect("error while running QuickFox");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

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
            index: SearchIndex::default(),
            last_report: IndexReport::default(),
            index_lifecycle: IndexLifecycle::default(),
            index_watcher: None,
            watcher_failure_summary: None,
        };

        assert_eq!(
            runtime.index_status().kind,
            crate::core::index::IndexStatusKind::Unbuilt
        );
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
    fn completed_index_refresh_returns_status_for_frontend_event() {
        let state = QuickFoxAppState {
            runtime: Mutex::new(QuickFoxRuntime {
                config: QuickFoxConfig::default_with_index_dirs(vec!["/tmp".to_owned()]),
                index: SearchIndex::default(),
                last_report: IndexReport::default(),
                index_lifecycle: IndexLifecycle::default(),
                index_watcher: None,
                watcher_failure_summary: None,
            }),
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
                index: SearchIndex::default(),
                last_report: IndexReport::default(),
                index_lifecycle: IndexLifecycle::default(),
                index_watcher: None,
                watcher_failure_summary: None,
            }),
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
                index: SearchIndex::default(),
                last_report: IndexReport::default(),
                index_lifecycle: IndexLifecycle::default(),
                index_watcher: None,
                watcher_failure_summary: None,
            }),
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
                index: SearchIndex::default(),
                last_report: IndexReport::default(),
                index_lifecycle: IndexLifecycle::default(),
                index_watcher: None,
                watcher_failure_summary: None,
            }),
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
                index: SearchIndex::default(),
                last_report: IndexReport::default(),
                index_lifecycle: IndexLifecycle::default(),
                index_watcher: None,
                watcher_failure_summary: None,
            }),
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
                index: SearchIndex::from_entries(vec![crate::core::index::IndexedEntry::legacy(
                    "/tmp/notes.md",
                    "notes.md",
                    crate::core::index::IndexedEntryKind::File,
                )]),
                last_report: IndexReport::default(),
                index_lifecycle: IndexLifecycle::default(),
                index_watcher: None,
                watcher_failure_summary: None,
            }),
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
                index: SearchIndex::default(),
                last_report: IndexReport::default(),
                index_lifecycle: IndexLifecycle::default(),
                index_watcher: None,
                watcher_failure_summary: None,
            }),
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
                index: SearchIndex::from_entries(vec![crate::core::index::IndexedEntry::legacy(
                    "/tmp/notes.md",
                    "notes.md",
                    crate::core::index::IndexedEntryKind::File,
                )]),
                last_report: IndexReport::default(),
                index_lifecycle: IndexLifecycle::from_ready(1, 123),
                index_watcher: None,
                watcher_failure_summary: None,
            }),
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
        assert!(status.message.unwrap().contains("background refresh"));
        assert_eq!(state.runtime.lock().unwrap().index.entries().len(), 1);
    }

    #[test]
    fn runtime_watcher_uses_index_entry_roots() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("notes.md");
        fs::write(&file, "notes").unwrap();
        let mut runtime = QuickFoxRuntime {
            config: QuickFoxConfig::default_with_index_dirs(vec![root
                .path()
                .to_string_lossy()
                .to_string()]),
            index: SearchIndex::from_entries(vec![
                crate::core::index::IndexedEntry::from_path_metadata(
                    &file,
                    root.path(),
                    crate::core::index::IndexedEntryKind::File,
                ),
            ]),
            last_report: IndexReport::default(),
            index_lifecycle: IndexLifecycle::from_ready(1, 123),
            index_watcher: None,
            watcher_failure_summary: None,
        };

        start_runtime_index_watcher(&mut runtime).unwrap();

        let roots = runtime.index_watcher.as_ref().unwrap().watched_roots();
        assert_eq!(roots, &[root.path().to_path_buf()]);
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
    fn disabled_watcher_skips_runtime_watcher_start() {
        let mut runtime = QuickFoxRuntime {
            config: QuickFoxConfig::default_with_index_dirs(vec!["/tmp".to_owned()]),
            index: SearchIndex::from_entries(vec![crate::core::index::IndexedEntry {
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
            index_lifecycle: IndexLifecycle::default(),
            index_watcher: None,
            watcher_failure_summary: None,
        };
        runtime.config.index.watcher_enabled = false;

        start_runtime_index_watcher(&mut runtime).unwrap();

        assert!(runtime.index_watcher.is_none());
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
