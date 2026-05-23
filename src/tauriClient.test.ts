import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";

import {
  clearCommandHistory,
  executeAction,
  loadConfig,
  refreshIndex,
  saveConfig,
  search,
} from "./tauriClient";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

const invokeMock = vi.mocked(invoke);

describe("tauriClient", () => {
  beforeEach(() => {
    invokeMock.mockReset();
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
    await loadConfig();
    await saveConfig(config);
    await clearCommandHistory();

    expect(invokeMock).toHaveBeenNthCalledWith(1, "refresh_index");
    expect(invokeMock).toHaveBeenNthCalledWith(2, "load_config");
    expect(invokeMock).toHaveBeenNthCalledWith(3, "save_config", { config });
    expect(invokeMock).toHaveBeenNthCalledWith(4, "clear_command_history");
  });
});
