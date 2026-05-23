pub mod core;

use crate::core::actions::Action;
use crate::core::config::QuickFoxConfig;
use crate::core::index::{IndexReport, IndexScanOptions, IndexScanner, SearchIndex};
use crate::core::platform::{CommandSafetyChecker, CommandSafetyDecision};
use crate::core::search::{QueryParser, QueryParserConfig, SearchResult};

#[tauri::command]
fn health_check() -> &'static str {
    "ok"
}

#[tauri::command]
fn search(query: String) -> Vec<SearchResult> {
    let parser = QueryParser::new(QueryParserConfig::default());
    let request = parser.parse(&query);
    SearchIndex::default().search(&request)
}

#[tauri::command]
fn execute_action(action: Action) -> Result<&'static str, String> {
    if let Action::ExecuteCommand {
        command,
        requires_confirmation,
    } = &action
    {
        if !requires_confirmation {
            return Err("command requires confirmation".to_owned());
        }

        match CommandSafetyChecker.check(command) {
            CommandSafetyDecision::AllowWithConfirmation
            | CommandSafetyDecision::RequireStrongConfirmation { .. } => {}
            CommandSafetyDecision::Blocked { reason } => return Err(reason),
        }
    }

    Ok("completed")
}

#[tauri::command]
fn refresh_index() -> Result<IndexReport, String> {
    IndexScanner
        .scan(IndexScanOptions::default())
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn load_config() -> QuickFoxConfig {
    QuickFoxConfig::default_with_index_dirs(default_index_dirs())
}

#[tauri::command]
fn save_config(config: QuickFoxConfig) -> Result<&'static str, String> {
    let errors = config.validate();
    if let Some(error) = errors.first() {
        return Err(format!("{error:?}"));
    }

    Ok("saved")
}

#[tauri::command]
fn clear_command_history() -> &'static str {
    "cleared"
}

fn default_index_dirs() -> Vec<String> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .into_iter()
        .collect()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            health_check,
            search,
            execute_action,
            refresh_index,
            load_config,
            save_config,
            clear_command_history
        ])
        .run(tauri::generate_context!())
        .expect("error while running QuickFox");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execute_action_refuses_unconfirmed_commands() {
        let result = execute_action(Action::ExecuteCommand {
            command: "git status".to_owned(),
            requires_confirmation: false,
        });

        assert_eq!(result, Err("command requires confirmation".to_owned()));
    }

    #[test]
    fn execute_action_blocks_dangerous_commands() {
        let result = execute_action(Action::ExecuteCommand {
            command: "rm -rf /".to_owned(),
            requires_confirmation: true,
        });

        assert_eq!(result, Err("命令会递归删除根目录".to_owned()));
    }

    #[test]
    fn load_config_returns_default_config_for_tauri_command_contract() {
        let config = load_config();

        assert_eq!(config.query.regex_prefix, "re:");
        assert!(!config.command.enabled);
    }
}
