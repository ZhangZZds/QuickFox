import { invoke } from "@tauri-apps/api/core";

export type FrontendAction =
  | { type: "openPath"; path: string }
  | { type: "openContainingFolder"; path: string }
  | { type: "copyText"; text: string }
  | { type: "openUrl"; url: string }
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
