import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { App } from "./App";
import { executeAction, loadConfig, saveConfig, search } from "./tauriClient";

vi.mock("./tauriClient", () => ({
  executeAction: vi.fn(),
  loadConfig: vi.fn(),
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
    vi.mocked(loadConfig).mockReset();
    vi.mocked(saveConfig).mockReset();
    vi.mocked(loadConfig).mockResolvedValue(appConfig);
    vi.mocked(saveConfig).mockResolvedValue("saved");
  });

  it("renders the compact launcher shell", () => {
    render(<App />);

    expect(screen.getByRole("main", { name: "QuickFox launcher" })).toBeInTheDocument();
    expect(screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令")).toBeInTheDocument();
    expect(screen.getByRole("list", { name: "搜索结果" })).toBeInTheDocument();
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

    fireEvent.click(screen.getByRole("button", { name: "打开设置" }));

    expect(screen.getByRole("form", { name: "基础设置" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "返回搜索" })).toBeInTheDocument();
  });

  it("saves updated command settings through the Tauri config command", async () => {
    render(<App />);

    fireEvent.click(screen.getByRole("button", { name: "打开设置" }));
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

    expect(screen.getByRole("form", { name: "基础设置" })).toBeInTheDocument();
    expect(screen.getByLabelText("索引目录")).toBeInTheDocument();
    expect(screen.getByLabelText("正则前缀")).toHaveValue("re:");
    expect(screen.getByLabelText("命令执行")).not.toBeChecked();
    expect(screen.getByLabelText("命令历史条数")).toHaveValue(15);
  });
});
