import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import {
  clearCommandHistory,
  clearInputHistory,
  executeAction,
  globalHotkeyStatus,
  indexStatus,
  listenGlobalHotkeyStatus,
  listenOpenSettings,
  loadConfig,
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

const invokeMock = vi.mocked(invoke);
const listenMock = vi.mocked(listen);

describe("tauriClient", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    listenMock.mockReset();
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
    await clearCommandHistory();
    await recordInputHistory("g 1234");
    await recentInputHistory();
    await clearInputHistory();

    expect(invokeMock).toHaveBeenNthCalledWith(1, "refresh_index");
    expect(invokeMock).toHaveBeenNthCalledWith(2, "index_status");
    expect(invokeMock).toHaveBeenNthCalledWith(3, "global_hotkey_status");
    expect(invokeMock).toHaveBeenNthCalledWith(4, "load_config");
    expect(invokeMock).toHaveBeenNthCalledWith(5, "save_config", { config });
    expect(invokeMock).toHaveBeenNthCalledWith(6, "clear_command_history");
    expect(invokeMock).toHaveBeenNthCalledWith(7, "record_input_history", {
      input: "g 1234",
    });
    expect(invokeMock).toHaveBeenNthCalledWith(8, "recent_input_history");
    expect(invokeMock).toHaveBeenNthCalledWith(9, "clear_input_history");
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
});
