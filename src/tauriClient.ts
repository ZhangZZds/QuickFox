import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type FrontendAction =
  | { type: "openPath"; path: string }
  | { type: "openContainingFolder"; path: string }
  | { type: "copyText"; text: string }
  | { type: "openUrl"; url: string }
  | { type: "openWithApplication"; path: string; application: "developmentTool" }
  | { type: "executeCommand"; command: string; requiresConfirmation: boolean };

export function search(query: string) {
  return invoke("search", { query });
}

export function executeAction(action: FrontendAction) {
  return invoke("execute_action", { action });
}

export function refreshIndex() {
  return invoke("refresh_index");
}

export function loadConfig() {
  return invoke("load_config");
}

export function saveConfig(config: unknown) {
  return invoke("save_config", { config });
}

export function clearCommandHistory() {
  return invoke("clear_command_history");
}

export function recordInputHistory(input: string) {
  return invoke("record_input_history", { input });
}

export function recentInputHistory() {
  return invoke("recent_input_history");
}

export function clearInputHistory() {
  return invoke("clear_input_history");
}

export function listenOpenSettings(handler: () => void) {
  return listen("quickfox://open-settings", handler);
}

export type QuickFoxConfig = {
  index: {
    include_dirs: string[];
    exclude_dirs: string[];
    exclude_patterns: string[];
  };
  query: {
    regex_prefix: string;
  };
  web_search: {
    engines: Record<string, { name: string; url: string }>;
  };
  command: {
    prefix: string;
    enabled: boolean;
  };
  history: {
    input_history_enabled: boolean;
    input_max_entries: number;
    file_history_enabled: boolean;
    calculator_history_enabled: boolean;
    web_search_history_enabled: boolean;
    command_history_enabled: boolean;
    command_max_entries: number;
  };
  results: {
    limit: number;
  };
};
