pub mod core;

use crate::core::actions::{Action, OpenApplication};
use crate::core::config::{ConfigStore, QuickFoxConfig};
use crate::core::index::{
    IndexLifecycle, IndexReport, IndexScanOptions, IndexScanner, IndexStatus, SearchIndex,
};
use crate::core::platform::{
    CommandSafetyChecker, CommandSafetyDecision, DevelopmentToolAdapter, LauncherWindowEffect,
    LauncherWindowState, ProcessCommand,
};
use crate::core::providers::{
    CalculatorProvider, CommandProvider, CommandProviderConfig, FileProvider, ProviderRegistry,
    WebSearchEngine, WebSearchProvider,
};
use crate::core::search::{HistoryScores, QueryParser, QueryParserConfig, Ranker, SearchResult};
use crate::core::storage::SqliteStorage;
use keytap::{EventKind, Key, Tap};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;
use std::thread;
use tauri::image::Image;
use tauri::{Emitter, Manager};
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
}

struct QuickFoxAppState {
    runtime: Mutex<QuickFoxRuntime>,
    window_state: Mutex<LauncherWindowState>,
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
fn toggle_launcher_window(
    app: tauri::AppHandle,
    state: tauri::State<QuickFoxAppState>,
) -> Result<&'static str, String> {
    toggle_launcher_window_for_app(&app, &state)?;
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
    drop(runtime);
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
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .into_iter()
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
    }
}

fn implicit_exclude_patterns() -> Vec<String> {
    vec![".*".to_owned()]
}

fn implicit_exclude_dirs(config: &QuickFoxConfig) -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = home_dir() {
            let home_text = home.to_string_lossy().to_string();
            if config
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

fn build_provider_registry(
    config: &QuickFoxConfig,
    index: SearchIndex,
    index_status: &IndexStatus,
) -> ProviderRegistry {
    let mut registry = ProviderRegistry::default();
    if file_index_is_available(&index, index_status) {
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

    let registry = build_provider_registry(config, index.clone(), index_status);
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
            let completed_at_ms = current_time_ms();
            let scan_result = IndexScanner.scan(build_scan_options(&config));
            let app_for_update = app.clone();
            let update_result = match scan_result {
                Ok(report) => {
                    if let Some(storage) = storage_store() {
                        let _ =
                            storage.save_completed_index_batch(completed_at_ms, &report.entries);
                    }
                    let app_for_state = app_for_update.clone();
                    app_for_update.run_on_main_thread(move || {
                        let state = app_for_state.state::<QuickFoxAppState>();
                        apply_completed_index_refresh(&state, generation, report, completed_at_ms);
                    })
                }
                Err(error) => {
                    let message = error.to_string();
                    let app_for_state = app_for_update.clone();
                    app_for_update.run_on_main_thread(move || {
                        let state = app_for_state.state::<QuickFoxAppState>();
                        apply_failed_index_refresh(&state, generation, message);
                    })
                }
            };
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
    report: IndexReport,
    completed_at_ms: i64,
) -> bool {
    let mut runtime = state
        .runtime
        .lock()
        .expect("quickfox runtime lock poisoned");
    if !runtime
        .index_lifecycle
        .complete_refresh(generation, report.entries.len(), completed_at_ms)
    {
        return false;
    }
    runtime.index = SearchIndex::from_entries(report.entries.clone());
    runtime.last_report = report;
    true
}

fn apply_failed_index_refresh(state: &QuickFoxAppState, generation: u64, message: String) -> bool {
    let mut runtime = state
        .runtime
        .lock()
        .expect("quickfox runtime lock poisoned");
    runtime.index_lifecycle.fail_refresh(generation, message)
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
    let entries = snapshot
        .as_ref()
        .map(|snapshot| snapshot.entries.clone())
        .unwrap_or_default();
    let index_lifecycle = snapshot
        .as_ref()
        .map(|snapshot| {
            IndexLifecycle::from_ready(snapshot.entries.len(), snapshot.completed_at_ms)
        })
        .unwrap_or_default();
    let report = IndexReport {
        entries: entries.clone(),
        failures: Vec::new(),
    };
    QuickFoxRuntime {
        config,
        index: SearchIndex::from_entries(entries),
        index_lifecycle,
        last_report: report,
    }
}

impl QuickFoxRuntime {
    fn index_status(&self) -> IndexStatus {
        self.index_lifecycle.status().clone()
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
    }
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

fn show_main_window<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    apply_launcher_window_effect(app, LauncherWindowEffect::ShowAndFocus);
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

fn key_press_from_global_event(event: &keytap::Event) -> Option<crate::core::platform::KeyPress> {
    match event.kind {
        EventKind::KeyDown(Key::ShiftLeft | Key::ShiftRight) => {
            Some(crate::core::platform::KeyPress::Shift)
        }
        EventKind::KeyDown(_) => Some(crate::core::platform::KeyPress::Other),
        EventKind::KeyUp(_) | EventKind::KeyRepeat(_) => None,
    }
}

fn start_global_double_shift_listener(app: tauri::AppHandle) {
    thread::Builder::new()
        .name("quickfox-global-hotkey".to_owned())
        .spawn(move || {
            let tap = match Tap::builder().macos_no_repeat_detection().build() {
                Ok(tap) => tap,
                Err(error) => {
                    eprintln!("QuickFox global hotkey listener disabled: {error}");
                    return;
                }
            };
            let mut hotkey_state = crate::core::platform::HotkeyState::default();

            for event in tap.iter() {
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
        })
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            use tauri::menu::{Menu, MenuItem};
            use tauri::tray::TrayIconBuilder;

            let settings = MenuItem::with_id(app, "settings", "设置", true, None::<&str>)?;
            let show = MenuItem::with_id(app, "show", "显示 QuickFox", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &settings, &quit])?;
            TrayIconBuilder::with_id("quickfox")
                .tooltip("QuickFox")
                .icon(TRAY_ICON.clone())
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "show" => show_main_window(app),
                    "settings" => {
                        show_main_window(app);
                        let _ = app.emit_to("main", "quickfox://open-settings", ());
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;

            start_global_double_shift_listener(app.handle().clone());
            let _ = start_background_index_refresh(app.handle().clone(), &app.state());

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            health_check,
            index_status,
            toggle_launcher_window,
            search,
            execute_action,
            refresh_index,
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
    fn runtime_reports_current_index_status() {
        let runtime = QuickFoxRuntime {
            config: QuickFoxConfig::default_with_index_dirs(vec!["/tmp".to_owned()]),
            index: SearchIndex::default(),
            last_report: IndexReport::default(),
            index_lifecycle: IndexLifecycle::default(),
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
                }],
            }),
        );

        assert_eq!(
            runtime.index_status().kind,
            crate::core::index::IndexStatusKind::Ready
        );
        assert_eq!(runtime.index_status().entry_count, 1);
        assert_eq!(runtime.index.entries()[0].name, "notes.md");
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
            entry_count: 0,
            message: None,
            generation: 1,
            completed_at_ms: None,
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
            },
            crate::core::index::IndexedEntry {
                path: "/tmp/Documents-2".to_owned(),
                name: "Documents-2".to_owned(),
                kind: crate::core::index::IndexedEntryKind::Directory,
            },
        ]);

        let results = perform_search(&config, &index, "doc");

        assert_eq!(results.len(), 1);
    }

    #[test]
    fn build_scan_options_adds_hidden_exclude_pattern() {
        let config = QuickFoxConfig::default_with_index_dirs(vec!["/tmp".to_owned()]);

        let options = build_scan_options(&config);

        assert!(options.exclude_patterns.contains(&".*".to_owned()));
    }

    #[test]
    fn build_terminal_command_returns_platform_specific_terminal_process() {
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
    fn global_hotkey_mapper_only_tracks_key_down_events() {
        assert_eq!(
            key_press_from_global_event(&keytap::Event {
                time: std::time::Instant::now(),
                kind: EventKind::KeyDown(Key::ShiftLeft),
            }),
            Some(crate::core::platform::KeyPress::Shift)
        );
        assert_eq!(
            key_press_from_global_event(&keytap::Event {
                time: std::time::Instant::now(),
                kind: EventKind::KeyDown(Key::A),
            }),
            Some(crate::core::platform::KeyPress::Other)
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
