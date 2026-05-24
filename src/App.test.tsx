import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { App } from "./App";
import {
  executeAction,
  listenOpenSettings,
  loadConfig,
  recentInputHistory,
  recordInputHistory,
  refreshIndex,
  saveConfig,
  search,
} from "./tauriClient";

vi.mock("./tauriClient", () => ({
  executeAction: vi.fn(),
  listenOpenSettings: vi.fn(),
  loadConfig: vi.fn(),
  recentInputHistory: vi.fn(),
  recordInputHistory: vi.fn(),
  refreshIndex: vi.fn(),
  saveConfig: vi.fn(),
  search: vi.fn(),
}));

const fileResults = [
  {
    id: "path:/tmp/Documents",
    title: "Documents",
    detail: "/tmp/Documents",
    kind: "directory",
    provider: "files",
    score: 1000,
    mainAction: { type: "openPath", path: "/tmp/Documents" },
    secondaryActions: [
      { type: "openContainingFolder", path: "/tmp/Documents" },
      { type: "copyText", text: "/tmp/Documents" },
    ],
  },
  {
    id: "path:/tmp/Downloads",
    title: "Downloads",
    detail: "/tmp/Downloads",
    kind: "directory",
    provider: "files",
    score: 900,
    mainAction: { type: "openPath", path: "/tmp/Downloads" },
    secondaryActions: [
      { type: "openContainingFolder", path: "/tmp/Downloads" },
      { type: "copyText", text: "/tmp/Downloads" },
    ],
  },
];

const webResults = [
  {
    id: "web:g:1234",
    title: "Google: 1234",
    detail: "https://www.google.com/search?q={query}",
    kind: "webSearch",
    provider: "web-search",
    score: 800,
    mainAction: { type: "openUrl", url: "https://www.google.com/search?q=1234" },
    secondaryActions: [],
  },
];

const appConfig = {
  index: {
    include_dirs: ["/tmp"],
    exclude_dirs: [],
    exclude_patterns: [],
  },
  query: {
    regex_prefix: "re:",
  },
  web_search: {
    engines: {},
  },
  command: {
    prefix: ">",
    enabled: false,
  },
  history: {
    input_history_enabled: true,
    input_max_entries: 15,
    file_history_enabled: true,
    calculator_history_enabled: false,
    web_search_history_enabled: false,
    command_history_enabled: true,
    command_max_entries: 15,
  },
  results: {
    limit: 20,
  },
};

describe("App", () => {
  beforeEach(() => {
    vi.mocked(search).mockReset();
    vi.mocked(executeAction).mockReset();
    vi.mocked(listenOpenSettings).mockReset();
    vi.mocked(loadConfig).mockReset();
    vi.mocked(recentInputHistory).mockReset();
    vi.mocked(recordInputHistory).mockReset();
    vi.mocked(refreshIndex).mockReset();
    vi.mocked(saveConfig).mockReset();
    vi.mocked(search).mockResolvedValue([]);
    vi.mocked(listenOpenSettings).mockResolvedValue(() => undefined);
    vi.mocked(loadConfig).mockResolvedValue(appConfig);
    vi.mocked(recentInputHistory).mockResolvedValue([]);
    vi.mocked(refreshIndex).mockResolvedValue({ entries: [], failures: [] });
    vi.mocked(recordInputHistory).mockResolvedValue("recorded");
    vi.mocked(saveConfig).mockResolvedValue("saved");
  });

  it("renders the compact launcher shell", () => {
    render(<App />);

    expect(screen.getByRole("main", { name: "QuickFox launcher" })).toBeInTheDocument();
    expect(screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令")).toBeInTheDocument();
  });

  it("does not render placeholder results before the user types a query", () => {
    render(<App />);

    expect(screen.queryAllByRole("option")).toHaveLength(0);
  });

  it("renders search results returned by the Tauri search command and marks the first item", async () => {
    vi.mocked(search).mockResolvedValueOnce([fileResults[1]]);

    render(<App />);

    fireEvent.change(screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令"), {
      target: { value: "down" },
    });

    expect(await screen.findByText("Downloads")).toBeInTheDocument();
    expect(screen.getByRole("option", { name: /Downloads/ })).toHaveAttribute(
      "aria-selected",
      "true",
    );
  });

  it("shows empty-state feedback when a non-empty query returns no results", async () => {
    vi.mocked(search).mockResolvedValueOnce([]);

    render(<App />);

    fireEvent.change(screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令"), {
      target: { value: "does-not-exist" },
    });

    expect(await screen.findByText("未找到结果")).toBeInTheDocument();
  });

  it("moves selection with arrow keys and executes the selected primary action with Enter", async () => {
    const onExecuteAction = vi.fn();
    vi.mocked(search).mockResolvedValueOnce(fileResults);
    render(<App onExecuteAction={onExecuteAction} />);
    const input = screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令");

    fireEvent.change(input, {
      target: { value: "do" },
    });
    await screen.findByText("Documents");
    fireEvent.keyDown(input, { key: "ArrowDown" });
    fireEvent.keyDown(input, { key: "Enter" });

    expect(onExecuteAction).toHaveBeenCalledWith({
      type: "openPath",
      path: "/tmp/Downloads",
    });
  });

  it("closes the launcher with Esc without executing an action", () => {
    const onClose = vi.fn();
    const onExecuteAction = vi.fn();
    render(<App onClose={onClose} onExecuteAction={onExecuteAction} />);

    fireEvent.keyDown(screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令"), {
      key: "Escape",
    });

    expect(onClose).toHaveBeenCalledOnce();
    expect(onExecuteAction).not.toHaveBeenCalled();
  });

  it("opens the action menu from context menu and executes secondary actions", async () => {
    const onExecuteAction = vi.fn();
    vi.mocked(search).mockResolvedValueOnce(fileResults);
    render(<App onExecuteAction={onExecuteAction} />);
    fireEvent.change(screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令"), {
      target: { value: "doc" },
    });

    fireEvent.contextMenu(await screen.findByRole("option", { name: /Documents/ }));
    fireEvent.click(screen.getByRole("menuitem", { name: "复制路径" }));

    expect(onExecuteAction).toHaveBeenCalledWith({
      type: "copyText",
      text: "/tmp/Documents",
    });
  });

  it("positions the action menu near the context-clicked result", async () => {
    vi.mocked(search).mockResolvedValueOnce(fileResults);
    render(<App />);
    fireEvent.change(screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令"), {
      target: { value: "doc" },
    });

    fireEvent.contextMenu(await screen.findByRole("option", { name: /Documents/ }), {
      clientX: 80,
      clientY: 120,
    });

    expect(screen.getByRole("menu")).toHaveStyle({ left: "80px", top: "120px" });
  });

  it("uses the Tauri action client by default", async () => {
    vi.mocked(search).mockResolvedValueOnce(fileResults);
    render(<App />);
    const input = screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令");

    fireEvent.change(input, {
      target: { value: "doc" },
    });
    await screen.findByText("Documents");
    fireEvent.keyDown(input, { key: "Enter" });

    expect(executeAction).toHaveBeenCalledWith({
      type: "openPath",
      path: "/tmp/Documents",
    });
  });

  it("shows command preview when command mode is enabled", () => {
    render(<App commandEnabled />);

    fireEvent.change(screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令"), {
      target: { value: "> git status" },
    });

    expect(screen.getByRole("region", { name: "命令预览" })).toBeInTheDocument();
    expect(screen.getByText("git status")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "确认执行" })).toBeInTheDocument();
  });

  it("shows command disabled feedback for command queries when command mode is off", () => {
    render(<App />);

    fireEvent.change(screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令"), {
      target: { value: "> ls" },
    });

    expect(screen.getByRole("region", { name: "命令预览" })).toBeInTheDocument();
    expect(screen.getByText("命令执行未启用")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "确认执行" })).toBeDisabled();
  });

  it("opens settings from the launcher toolbar", () => {
    render(<App />);

    expect(screen.queryByRole("button", { name: "打开设置" })).not.toBeInTheDocument();
  });

  it("opens settings when the tray settings event is received", async () => {
    let openSettings: (() => void) | undefined;
    vi.mocked(listenOpenSettings).mockImplementation(async (handler) => {
      openSettings = handler;
      return () => undefined;
    });
    render(<App />);

    openSettings?.();

    expect(await screen.findByRole("form", { name: "设置" })).toBeInTheDocument();
  });

  it("saves updated command settings through the Tauri config command", async () => {
    render(<App initialView="settings" />);
    await screen.findByDisplayValue("/tmp");
    fireEvent.click(screen.getByLabelText("命令执行"));
    fireEvent.click(screen.getByRole("button", { name: "保存设置" }));

    expect(saveConfig).toHaveBeenCalledWith({
      ...appConfig,
      command: {
        ...appConfig.command,
        enabled: true,
      },
    });
  });

  it("renders the basic settings view", () => {
    render(<App initialView="settings" />);

    expect(screen.getByRole("form", { name: "设置" })).toBeInTheDocument();
    expect(screen.getByRole("group", { name: "搜索与索引" })).toBeInTheDocument();
    expect(screen.getByRole("group", { name: "网页搜索" })).toBeInTheDocument();
    expect(screen.getByRole("group", { name: "历史" })).toBeInTheDocument();
    expect(screen.getByRole("group", { name: "命令执行" })).toBeInTheDocument();
    expect(screen.getByRole("group", { name: "外观与窗口" })).toBeInTheDocument();
    expect(screen.getByLabelText("索引目录")).toBeInTheDocument();
    expect(screen.getByLabelText("正则前缀")).toHaveValue("re:");
    expect(screen.getByLabelText("命令执行")).not.toBeChecked();
    expect(screen.getByLabelText("输入历史条数")).toHaveValue(15);
  });

  it("refreshes the index from the settings search group", async () => {
    render(<App initialView="settings" />);

    fireEvent.click(await screen.findByRole("button", { name: "刷新索引" }));

    expect(refreshIndex).toHaveBeenCalledOnce();
    expect(await screen.findByText("索引已刷新")).toBeInTheDocument();
  });

  it("recalls recent confirmed input history with arrow keys", async () => {
    vi.mocked(recentInputHistory).mockResolvedValueOnce(["g 1234", "notes"]);
    render(<App />);
    const input = screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令");

    await waitFor(() => expect(recentInputHistory).toHaveBeenCalledOnce());
    fireEvent.keyDown(input, { key: "ArrowUp" });

    expect(input).toHaveValue("g 1234");
  });

  it("records input history only after Enter executes an action", async () => {
    vi.mocked(search).mockResolvedValueOnce(fileResults);
    vi.mocked(executeAction).mockResolvedValueOnce("completed");
    render(<App />);
    const input = screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令");

    fireEvent.change(input, { target: { value: "doc" } });
    await screen.findByText("Documents");
    expect(recordInputHistory).not.toHaveBeenCalled();
    fireEvent.keyDown(input, { key: "Enter" });

    await waitFor(() => expect(recordInputHistory).toHaveBeenCalledWith("doc"));
  });

  it("does not record input history when Esc closes without executing", () => {
    const onClose = vi.fn();
    render(<App onClose={onClose} />);
    const input = screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令");

    fireEvent.change(input, { target: { value: "notes" } });
    fireEvent.keyDown(input, { key: "Escape" });

    expect(onClose).toHaveBeenCalledOnce();
    expect(recordInputHistory).not.toHaveBeenCalled();
  });

  it("records web search input history after Enter opens the URL", async () => {
    vi.mocked(search).mockResolvedValueOnce(webResults);
    vi.mocked(executeAction).mockResolvedValueOnce("completed");
    render(<App />);
    const input = screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令");

    fireEvent.change(input, { target: { value: "g 1234" } });
    await screen.findByText("Google: 1234");
    fireEvent.keyDown(input, { key: "Enter" });

    await waitFor(() => expect(recordInputHistory).toHaveBeenCalledWith("g 1234"));
    expect(executeAction).toHaveBeenCalledWith({
      type: "openUrl",
      url: "https://www.google.com/search?q=1234",
    });
  });

  it("does not render a result list when the query is empty", () => {
    render(<App />);

    expect(screen.queryByRole("list", { name: "搜索结果" })).not.toBeInTheDocument();
  });

  it("uses the internal result list scroller for non-empty results", async () => {
    vi.mocked(search).mockResolvedValueOnce(fileResults);
    render(<App />);

    fireEvent.change(screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令"), {
      target: { value: "doc" },
    });

    expect(await screen.findByRole("list", { name: "搜索结果" })).toHaveClass("result-list");
  });
});
