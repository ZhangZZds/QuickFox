import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { beforeEach, describe, expect, it, vi } from "vitest";

import {
  clearCommandHistory,
  clearInputHistory,
  currentWindowLabel,
  executeAction,
  globalHotkeyStatus,
  hideCurrentWindow,
  indexStatus,
  listenGlobalHotkeyStatus,
  listenIndexStatus,
  listenOpenSettings,
  loadConfig,
  openSettingsWindow,
  recentInputHistory,
  recordInputHistory,
  refreshIndex,
  saveConfig,
  search,
} from "./tauriClient";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(),
}));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: vi.fn(),
}));

const invokeMock = vi.mocked(invoke);
const listenMock = vi.mocked(listen);
const getCurrentWindowMock = vi.mocked(getCurrentWindow);

describe("tauriClient", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    listenMock.mockReset();
    getCurrentWindowMock.mockReset();
  });

  it("calls the search command with the query text", async () => {
    invokeMock.mockResolvedValueOnce([]);

    await search("notes");

    expect(invokeMock).toHaveBeenCalledWith("search", { query: "notes" });
  });

  it("calls the execute action command with the selected action", async () => {
    const action = { type: "openPath", path: "~/Downloads" } as const;
    invokeMock.mockResolvedValueOnce("completed");

    await executeAction(action);

    expect(invokeMock).toHaveBeenCalledWith("execute_action", { action });
  });

  it("calls refresh, config, and history commands with stable names", async () => {
    const config = { query: { regex_prefix: "re:" } };
    invokeMock.mockResolvedValue(undefined);

    await refreshIndex();
    await indexStatus();
    await globalHotkeyStatus();
    await loadConfig();
    await saveConfig(config);
    await openSettingsWindow();
    await clearCommandHistory();
    await recordInputHistory("g 1234");
    await recentInputHistory();
    await clearInputHistory();

    expect(invokeMock).toHaveBeenNthCalledWith(1, "refresh_index");
    expect(invokeMock).toHaveBeenNthCalledWith(2, "index_status");
    expect(invokeMock).toHaveBeenNthCalledWith(3, "global_hotkey_status");
    expect(invokeMock).toHaveBeenNthCalledWith(4, "load_config");
    expect(invokeMock).toHaveBeenNthCalledWith(5, "save_config", { config });
    expect(invokeMock).toHaveBeenNthCalledWith(6, "open_settings_window");
    expect(invokeMock).toHaveBeenNthCalledWith(7, "clear_command_history");
    expect(invokeMock).toHaveBeenNthCalledWith(8, "record_input_history", {
      input: "g 1234",
    });
    expect(invokeMock).toHaveBeenNthCalledWith(9, "recent_input_history");
    expect(invokeMock).toHaveBeenNthCalledWith(10, "clear_input_history");
  });

  it("reports the current Tauri window label when it is available", () => {
    getCurrentWindowMock.mockReturnValueOnce({ label: "settings" } as ReturnType<
      typeof getCurrentWindow
    >);

    expect(currentWindowLabel()).toBe("settings");
  });

  it("returns null for current window label outside Tauri", () => {
    getCurrentWindowMock.mockImplementationOnce(() => {
      throw new Error("Tauri window bridge unavailable");
    });

    expect(currentWindowLabel()).toBeNull();
  });

  it("hides the current Tauri window when available", async () => {
    const hide = vi.fn().mockResolvedValueOnce(undefined);
    getCurrentWindowMock.mockReturnValueOnce({ label: "main", hide } as unknown as ReturnType<
      typeof getCurrentWindow
    >);

    await hideCurrentWindow();

    expect(hide).toHaveBeenCalledOnce();
  });

  it("listens for the tray settings event", async () => {
    const handler = vi.fn();
    const unlisten = vi.fn();
    listenMock.mockResolvedValueOnce(unlisten);

    await listenOpenSettings(handler);

    expect(listenMock).toHaveBeenCalledWith("quickfox://open-settings", handler);
  });

  it("listens for global hotkey status events", async () => {
    const handler = vi.fn();
    const unlisten = vi.fn();
    listenMock.mockResolvedValueOnce(unlisten);

    await listenGlobalHotkeyStatus(handler);

    expect(listenMock).toHaveBeenCalledWith(
      "quickfox://global-hotkey-status",
      expect.any(Function),
    );
  });

  it("listens for index status events", async () => {
    const handler = vi.fn();
    const unlisten = vi.fn();
    listenMock.mockResolvedValueOnce(unlisten);

    await listenIndexStatus(handler);

    expect(listenMock).toHaveBeenCalledWith("quickfox://index-status", expect.any(Function));
  });

  it("returns a noop unlisten when Tauri event listen is unavailable", async () => {
    listenMock.mockRejectedValueOnce(new Error("Tauri event bridge unavailable"));

    const unlisten = await listenOpenSettings(vi.fn());

    expect(unlisten()).toBeUndefined();
  });

  it("returns a noop unlisten when Tauri event listen throws synchronously", async () => {
    listenMock.mockImplementationOnce(() => {
      throw new Error("Tauri event bridge unavailable");
    });

    const unlisten = await listenOpenSettings(vi.fn());

    expect(unlisten()).toBeUndefined();
  });
});
