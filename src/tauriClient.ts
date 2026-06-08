import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

export type FrontendAction =
  | { type: "openPath"; path: string }
  | { type: "openContainingFolder"; path: string }
  | { type: "copyText"; text: string }
  | { type: "openUrl"; url: string }
  | {
      type: "openWithApplication";
      path: string;
      application: "developmentTool" | "systemChooser";
    }
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

export function indexStatus() {
  return invoke("index_status");
}

export function appPaths() {
  return invoke("app_paths");
}

export function globalHotkeyStatus() {
  return invoke("global_hotkey_status");
}

export function openSettingsWindow() {
  return invoke("open_settings_window");
}

export function returnToLauncherWindow() {
  return invoke("return_to_launcher_window");
}

export function currentWindowLabel() {
  try {
    return getCurrentWindow().label;
  } catch {
    return null;
  }
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
  return safeListen("quickfox://open-settings", handler);
}

export function listenGlobalHotkeyStatus(handler: (status: GlobalHotkeyStatus) => void) {
  return safeListen<GlobalHotkeyStatus>("quickfox://global-hotkey-status", (event) =>
    handler(event.payload),
  );
}

export function listenIndexStatus(handler: (status: IndexStatus) => void) {
  return safeListen<IndexStatus>("quickfox://index-status", (event) => handler(event.payload));
}

function safeListen<T>(
  event: string,
  handler: Parameters<typeof listen<T>>[1],
): Promise<() => void> {
  try {
    return listen<T>(event, handler).catch(() => () => undefined);
  } catch {
    return Promise.resolve(() => undefined);
  }
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

export type IndexStatus = {
  kind: "unbuilt" | "building" | "ready" | "refreshing" | "failed";
  entryCount: number;
  message?: string | null;
  generation: number;
  completedAtMs?: number | null;
};

export type AppPaths = {
  configFilePath?: string | null;
  indexSnapshotPath?: string | null;
};

export type GlobalHotkeyStatus = {
  enabled: boolean;
  message: string;
  permissionSettingsUrl?: string | null;
};
