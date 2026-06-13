import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { App } from "./App";
import {
  appPaths,
  executeAction,
  globalHotkeyStatus,
  listenGlobalHotkeyStatus,
  listenIndexStatus,
  indexStatus,
  listenOpenSettings,
  loadConfig,
  openSettingsWindow,
  recentInputHistory,
  recordInputHistory,
  refreshIndex,
  saveConfig,
  search,
  type QuickFoxConfig,
  type SearchResult,
} from "./tauriClient";

vi.mock("./tauriClient", () => ({
  appPaths: vi.fn(),
  executeAction: vi.fn(),
  globalHotkeyStatus: vi.fn(),
  listenGlobalHotkeyStatus: vi.fn(),
  listenIndexStatus: vi.fn(),
  indexStatus: vi.fn(),
  listenOpenSettings: vi.fn(),
  loadConfig: vi.fn(),
  openSettingsWindow: vi.fn(),
  recentInputHistory: vi.fn(),
  recordInputHistory: vi.fn(),
  refreshIndex: vi.fn(),
  saveConfig: vi.fn(),
  search: vi.fn(),
}));

const fileResults: SearchResult[] = [
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

const webResults: SearchResult[] = [
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

const calculatorResults: SearchResult[] = [
  {
    id: "calculator:2+2",
    title: "2 + 2 = 4",
    detail: "按 Enter 复制结果",
    kind: "calculator",
    provider: "calculator",
    score: 950,
    mainAction: { type: "copyText", text: "4" },
    secondaryActions: [],
  },
];

const longPathResults: SearchResult[] = [
  {
    id: "path:/Users/frankzhang/workspace/QuickFox/src/components/DeeplyNestedFeature/VeryLongMatchingFileName.fixture.tsx",
    title: "VeryLongMatchingFileName.fixture.tsx",
    detail:
      "/Users/frankzhang/workspace/QuickFox/src/components/DeeplyNestedFeature/VeryLongMatchingFileName.fixture.tsx",
    kind: "file",
    provider: "files",
    score: 1000,
    mainAction: {
      type: "openPath",
      path: "/Users/frankzhang/workspace/QuickFox/src/components/DeeplyNestedFeature/VeryLongMatchingFileName.fixture.tsx",
    },
    secondaryActions: [],
  },
  {
    id: "path:C:\\Users\\frank\\Documents\\QuickFox\\fixtures\\reports\\VeryLongMatchingFileName.fixture.tsx",
    title: "VeryLongMatchingFileName.fixture.tsx",
    detail:
      "C:\\Users\\frank\\Documents\\QuickFox\\fixtures\\reports\\VeryLongMatchingFileName.fixture.tsx",
    kind: "file",
    provider: "files",
    score: 900,
    mainAction: {
      type: "openPath",
      path: "C:\\Users\\frank\\Documents\\QuickFox\\fixtures\\reports\\VeryLongMatchingFileName.fixture.tsx",
    },
    secondaryActions: [],
  },
];

const typedResults: SearchResult[] = [
  {
    id: "path:/Applications/Codex.app",
    title: "Codex.app",
    detail: "/Applications/Codex.app",
    kind: "application",
    provider: "files",
    score: 1100,
    mainAction: { type: "openPath", path: "/Applications/Codex.app" },
    secondaryActions: [
      { type: "openPath", path: "/Applications/Codex.app" },
      { type: "copyText", text: "/Applications/Codex.app" },
    ],
  },
  {
    id: "path:/tmp/report.md",
    title: "report.md",
    detail: "/tmp/report.md",
    kind: "file",
    provider: "files",
    score: 1000,
    mainAction: { type: "openPath", path: "/tmp/report.md" },
    secondaryActions: [
      { type: "openContainingFolder", path: "/tmp/report.md" },
      { type: "copyText", text: "/tmp/report.md" },
      {
        type: "openWithApplication",
        path: "/tmp/report.md",
        application: "systemChooser",
      },
    ],
  },
  {
    id: "path:/tmp/Documents",
    title: "Documents",
    detail: "/tmp/Documents",
    kind: "directory",
    provider: "files",
    score: 900,
    mainAction: { type: "openPath", path: "/tmp/Documents" },
    secondaryActions: [
      { type: "openPath", path: "/tmp/Documents" },
      { type: "copyText", text: "/tmp/Documents" },
    ],
  },
];

const contentSnippetResults: SearchResult[] = [
  {
    id: "path:/tmp/report.md",
    title: "report.md",
    detail: "/tmp/report.md",
    kind: "file",
    provider: "files",
    score: 1200,
    mainAction: { type: "openPath", path: "/tmp/report.md" },
    secondaryActions: [],
    snippet: {
      startLine: 40,
      lines: ["project alpha", "hello world appears here", "next action"],
      highlights: [
        {
          line: 41,
          startColumn: 0,
          endColumn: 11,
          matchedText: "hello world",
        },
      ],
    },
  },
];

const commandResults: SearchResult[] = [
  {
    id: "command:git status",
    title: "git status",
    detail: "需要确认后执行",
    kind: "command",
    provider: "commands",
    score: 1000,
    mainAction: { type: "executeCommand", command: "git status", requiresConfirmation: true },
    secondaryActions: [],
  },
];

const appConfig: QuickFoxConfig = {
  index: {
    include_dirs: ["/tmp"],
    exclude_dirs: [],
    exclude_patterns: [],
    performance_mode: "balanced",
    respect_project_ignores: true,
    content_include_dirs: ["/tmp/Documents"],
    content_max_file_bytes: 2097152,
    watcher_enabled: true,
  },
  query: {
    regex_prefix: "re:",
  },
  web_search: {
    engines: {
      g: { name: "Google", url: "https://www.google.com/search?q={query}" },
      bd: { name: "Baidu", url: "https://www.baidu.com/s?wd={query}" },
    },
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
  hotkey: {
    wake_shortcut: "Shift+Shift",
  },
};

describe("App", () => {
  beforeEach(() => {
    vi.mocked(appPaths).mockReset();
    vi.mocked(search).mockReset();
    vi.mocked(executeAction).mockReset();
    vi.mocked(globalHotkeyStatus).mockReset();
    vi.mocked(listenGlobalHotkeyStatus).mockReset();
    vi.mocked(listenIndexStatus).mockReset();
    vi.mocked(indexStatus).mockReset();
    vi.mocked(listenOpenSettings).mockReset();
    vi.mocked(loadConfig).mockReset();
    vi.mocked(openSettingsWindow).mockReset();
    vi.mocked(recentInputHistory).mockReset();
    vi.mocked(recordInputHistory).mockReset();
    vi.mocked(refreshIndex).mockReset();
    vi.mocked(saveConfig).mockReset();
    vi.mocked(search).mockResolvedValue([]);
    vi.mocked(appPaths).mockResolvedValue({
      configFilePath: "/Users/frank/Library/Application Support/QuickFox/config.json",
      indexSnapshotPath: "/Users/frank/Library/Application Support/QuickFox/index.snapshot.json",
    });
    vi.mocked(globalHotkeyStatus).mockResolvedValue({
      enabled: true,
      message: "Shift+Shift 全局唤醒可用",
      permissionSettingsUrl: null,
    });
    vi.mocked(listenGlobalHotkeyStatus).mockResolvedValue(() => undefined);
    vi.mocked(listenIndexStatus).mockResolvedValue(() => undefined);
    vi.mocked(listenOpenSettings).mockResolvedValue(() => undefined);
    vi.mocked(loadConfig).mockResolvedValue(appConfig);
    vi.mocked(openSettingsWindow).mockResolvedValue("completed");
    vi.mocked(indexStatus).mockResolvedValue({
      kind: "ready",
      entryCount: 12,
      message: null,
      generation: 1,
      completedAtMs: 100,
    });
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
    expect(screen.queryByRole("region", { name: "启动器状态" })).not.toBeInTheDocument();
  });

  it("uses localized placeholder text with compact syntax hints for empty input", () => {
    render(<App />);

    expect(
      screen.getByPlaceholderText("搜索文件、文件夹、计算器；g 关键词搜网页，re: 正则，> 命令"),
    ).toBeInTheDocument();
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
    expect(screen.queryByRole("region", { name: "启动器状态" })).not.toBeInTheDocument();
  });

  it("shows empty-state feedback when a non-empty query returns no results", async () => {
    vi.mocked(search).mockResolvedValueOnce([]);

    render(<App />);

    fireEvent.change(screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令"), {
      target: { value: "does-not-exist" },
    });

    expect(await screen.findByText("未找到结果")).toBeInTheDocument();
    expect(screen.queryByText(/文件索引/)).not.toBeInTheDocument();
  });

  it("shows first-run index preparation feedback with recovery actions", async () => {
    vi.mocked(indexStatus).mockResolvedValueOnce({
      kind: "unbuilt",
      entryCount: 0,
      message: null,
      generation: 0,
      completedAtMs: null,
    });
    vi.mocked(search).mockResolvedValueOnce([]);

    render(<App />);
    fireEvent.change(screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令"), {
      target: { value: "notes" },
    });

    const status = await screen.findByRole("region", { name: "启动器状态" });
    expect(status).toHaveTextContent("文件搜索正在准备");
    expect(status).toHaveTextContent("计算器和网页搜索仍可使用");
    expect(within(status).getByRole("button", { name: "刷新索引" })).toBeInTheDocument();
    expect(within(status).getByRole("button", { name: "打开设置" })).toBeInTheDocument();
  });

  it("shows index-building feedback without hiding other search behavior", async () => {
    vi.mocked(indexStatus).mockResolvedValueOnce({
      kind: "building",
      entryCount: 0,
      message: null,
      generation: 2,
      completedAtMs: null,
    });
    vi.mocked(search).mockResolvedValueOnce([]);

    render(<App />);
    fireEvent.change(screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令"), {
      target: { value: "notes" },
    });

    const status = await screen.findByRole("region", { name: "启动器状态" });
    expect(status).toHaveTextContent("文件索引正在建立");
    expect(status).toHaveTextContent("计算器和网页搜索仍可使用");
  });

  it("shows failed index feedback with retry and settings recovery actions", async () => {
    vi.mocked(indexStatus).mockResolvedValueOnce({
      kind: "failed",
      entryCount: 0,
      message: "权限不足，无法扫描目录",
      generation: 3,
      completedAtMs: null,
    });
    vi.mocked(search).mockResolvedValueOnce([]);

    render(<App />);
    fireEvent.change(screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令"), {
      target: { value: "notes" },
    });

    const status = await screen.findByRole("region", { name: "启动器状态" });
    expect(status).toHaveTextContent("文件索引构建失败");
    expect(status).toHaveTextContent("权限不足，无法扫描目录");
    fireEvent.click(within(status).getByRole("button", { name: "刷新索引" }));
    expect(refreshIndex).toHaveBeenCalledOnce();

    fireEvent.click(within(status).getByRole("button", { name: "打开设置" }));
    expect(openSettingsWindow).toHaveBeenCalledOnce();
    expect(screen.queryByRole("form", { name: "设置" })).not.toBeInTheDocument();
  });

  it("reloads index status after the launcher recovery refresh action", async () => {
    vi.mocked(indexStatus)
      .mockResolvedValueOnce({
        kind: "failed",
        entryCount: 0,
        message: "权限不足，无法扫描目录",
        generation: 3,
        completedAtMs: null,
      })
      .mockResolvedValueOnce({
        kind: "ready",
        entryCount: 18,
        message: "索引完成",
        generation: 4,
        completedAtMs: 200,
      });
    vi.mocked(search).mockResolvedValue([]);

    render(<App />);
    fireEvent.change(screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令"), {
      target: { value: "notes" },
    });
    const status = await screen.findByRole("region", { name: "启动器状态" });

    fireEvent.click(within(status).getByRole("button", { name: "刷新索引" }));

    await waitFor(() => expect(indexStatus).toHaveBeenCalledTimes(2));
    expect(await screen.findByText("未找到结果")).toBeInTheDocument();
    expect(screen.queryByText("文件索引构建失败")).not.toBeInTheDocument();
  });

  it("refreshes the current query when an index status event reports ready", async () => {
    let indexStatusHandler:
      | ((status: {
          kind: "ready";
          entryCount: number;
          message: string | null;
          generation: number;
          completedAtMs: number;
        }) => void)
      | undefined;
    vi.mocked(indexStatus).mockResolvedValueOnce({
      kind: "building",
      entryCount: 0,
      message: null,
      generation: 2,
      completedAtMs: null,
    });
    vi.mocked(listenIndexStatus).mockImplementation(async (handler) => {
      indexStatusHandler = handler as typeof indexStatusHandler;
      return () => undefined;
    });
    vi.mocked(search).mockResolvedValueOnce([]).mockResolvedValueOnce(fileResults);

    render(<App />);
    fireEvent.change(screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令"), {
      target: { value: "doc" },
    });
    expect(await screen.findByText("文件索引正在建立")).toBeInTheDocument();

    indexStatusHandler?.({
      kind: "ready",
      entryCount: 12,
      message: null,
      generation: 3,
      completedAtMs: 300,
    });

    expect(await screen.findByRole("option", { name: /Documents/ })).toBeInTheDocument();
    expect(search).toHaveBeenCalledTimes(2);
  });

  it("keeps web search results visible when the file index is unavailable", async () => {
    vi.mocked(indexStatus).mockResolvedValueOnce({
      kind: "unbuilt",
      entryCount: 0,
      message: null,
      generation: 0,
      completedAtMs: null,
    });
    vi.mocked(search).mockResolvedValueOnce(webResults);

    render(<App />);
    fireEvent.change(screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令"), {
      target: { value: "g 1234" },
    });

    expect(await screen.findByRole("option", { name: /Google: 1234/ })).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "启动器状态" })).not.toBeInTheDocument();
  });

  it("keeps calculator results visible when the file index is unavailable", async () => {
    vi.mocked(indexStatus).mockResolvedValueOnce({
      kind: "unbuilt",
      entryCount: 0,
      message: null,
      generation: 0,
      completedAtMs: null,
    });
    vi.mocked(search).mockResolvedValueOnce(calculatorResults);

    render(<App />);
    fireEvent.change(screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令"), {
      target: { value: "2 + 2" },
    });

    expect(await screen.findByRole("option", { name: /2 \+ 2 = 4/ })).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "启动器状态" })).not.toBeInTheDocument();
  });

  it("keeps command results visible when the file index is unavailable", async () => {
    vi.mocked(indexStatus).mockResolvedValueOnce({
      kind: "building",
      entryCount: 0,
      message: null,
      generation: 2,
      completedAtMs: null,
      stage: "configured-roots",
      currentRoot: "/tmp",
      scanned: 120,
      accepted: 90,
      skipped: 20,
      failures: 1,
    });
    vi.mocked(search).mockResolvedValueOnce(commandResults);

    render(<App commandEnabled />);
    fireEvent.change(screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令"), {
      target: { value: "> git status" },
    });

    expect(await screen.findByRole("region", { name: "命令预览" })).toHaveTextContent("git status");
    expect(screen.queryByRole("region", { name: "启动器状态" })).not.toBeInTheDocument();
  });

  it("shows lightweight index progress when file results are still empty", async () => {
    vi.mocked(indexStatus).mockResolvedValueOnce({
      kind: "building",
      entryCount: 0,
      message: null,
      generation: 2,
      completedAtMs: null,
      stage: "configured-roots",
      currentRoot: "/Users/frank/workspace",
      scanned: 120,
      accepted: 90,
      skipped: 20,
      failures: 1,
    });
    vi.mocked(search).mockResolvedValueOnce([]);

    render(<App />);
    fireEvent.change(screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令"), {
      target: { value: "notes" },
    });

    const status = await screen.findByRole("region", { name: "启动器状态" });
    expect(status).toHaveTextContent("configured-roots");
    expect(status).toHaveTextContent("/Users/frank/workspace");
    expect(status).toHaveTextContent("已扫描 120");
    expect(status).toHaveTextContent("收录 90");
    expect(status).toHaveTextContent("跳过 20");
    expect(status).toHaveTextContent("失败 1");
  });

  it("renders content snippets collapsed and expands line context on hover", async () => {
    vi.mocked(search).mockResolvedValueOnce(contentSnippetResults);

    render(<App />);
    fireEvent.change(screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令"), {
      target: { value: 'content:"hello world"' },
    });

    const result = await screen.findByRole("option", { name: /report.md/ });
    expect(within(result).getByText("命中 1 次")).toBeInTheDocument();
    expect(within(result).getByText("第 41 行")).toBeInTheDocument();
    expect(within(result).getByText("41")).toBeInTheDocument();
    expect(within(result).getByText("hello world")).toHaveProperty("tagName", "MARK");
    expect(within(result).queryByText("project alpha")).not.toBeInTheDocument();
    expect(within(result).queryByText("next action")).not.toBeInTheDocument();

    fireEvent.mouseEnter(result);

    expect(within(result).getByText("project alpha")).toBeInTheDocument();
    expect(within(result).getByText("next action")).toBeInTheDocument();
  });

  it("expands line context when arrow keys move onto a snippet result", async () => {
    vi.mocked(search).mockResolvedValueOnce([fileResults[0], contentSnippetResults[0]]);

    render(<App />);
    const input = screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令");
    fireEvent.change(input, {
      target: { value: 'content:"hello world"' },
    });

    await screen.findByRole("option", { name: /Documents/ });
    const snippetResult = screen.getByRole("option", { name: /report.md/ });
    expect(within(snippetResult).queryByText("project alpha")).not.toBeInTheDocument();

    fireEvent.keyDown(input, { key: "ArrowDown" });

    expect(snippetResult).toHaveAttribute("aria-selected", "true");
    expect(within(snippetResult).getByText("project alpha")).toBeInTheDocument();
    expect(within(snippetResult).getByText("next action")).toBeInTheDocument();
  });

  it("closes the secondary action menu when arrow keys move the result selection", async () => {
    vi.mocked(search).mockResolvedValueOnce(typedResults);

    render(<App />);
    const input = screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令");
    fireEvent.change(input, { target: { value: "doc" } });

    const firstResult = await screen.findByRole("option", { name: /report.md/ });
    fireEvent.contextMenu(firstResult);
    expect(screen.getByRole("menu")).toBeInTheDocument();

    fireEvent.keyDown(input, { key: "ArrowDown" });

    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    expect(screen.getByRole("option", { name: /Documents/ })).toHaveAttribute(
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

  it("executes the primary action when left-clicking a file result", async () => {
    const onExecuteAction = vi.fn();
    vi.mocked(search).mockResolvedValueOnce(typedResults);
    render(<App onExecuteAction={onExecuteAction} />);

    fireEvent.change(screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令"), {
      target: { value: "typed" },
    });

    fireEvent.click(await screen.findByRole("option", { name: /report\.md/ }));

    expect(onExecuteAction).toHaveBeenCalledWith({
      type: "openPath",
      path: "/tmp/report.md",
    });
  });

  it("executes type-specific primary actions when left-clicking directories and applications", async () => {
    const onExecuteAction = vi.fn();
    vi.mocked(search).mockResolvedValueOnce(typedResults);
    render(<App onExecuteAction={onExecuteAction} />);

    fireEvent.change(screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令"), {
      target: { value: "typed" },
    });

    fireEvent.click(await screen.findByRole("option", { name: /Codex\.app/ }));
    fireEvent.click(screen.getByRole("option", { name: /Documents/ }));

    expect(onExecuteAction).toHaveBeenNthCalledWith(1, {
      type: "openPath",
      path: "/Applications/Codex.app",
    });
    expect(onExecuteAction).toHaveBeenNthCalledWith(2, {
      type: "openPath",
      path: "/tmp/Documents",
    });
  });

  it("uses the same primary action for Enter and left-click on the same result", async () => {
    const keyboardExecuteAction = vi.fn();
    vi.mocked(search).mockResolvedValueOnce(fileResults);
    const { unmount } = render(<App onExecuteAction={keyboardExecuteAction} />);
    const input = screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令");

    fireEvent.change(input, { target: { value: "do" } });
    await screen.findByText("Documents");
    fireEvent.keyDown(input, { key: "ArrowDown" });
    fireEvent.keyDown(input, { key: "Enter" });

    expect(keyboardExecuteAction).toHaveBeenCalledWith({
      type: "openPath",
      path: "/tmp/Downloads",
    });

    unmount();

    const clickExecuteAction = vi.fn();
    vi.mocked(search).mockResolvedValueOnce(fileResults);
    render(<App onExecuteAction={clickExecuteAction} />);

    fireEvent.change(screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令"), {
      target: { value: "do" },
    });
    fireEvent.click(await screen.findByRole("option", { name: /Downloads/ }));

    expect(clickExecuteAction).toHaveBeenCalledWith({
      type: "openPath",
      path: "/tmp/Downloads",
    });
  });

  it("scrolls the selected search result into view when moving with arrow keys", async () => {
    const scrollIntoView = vi.fn();
    Element.prototype.scrollIntoView = scrollIntoView;
    vi.mocked(search).mockResolvedValueOnce(fileResults);
    render(<App />);
    const input = screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令");

    fireEvent.change(input, { target: { value: "do" } });
    await screen.findByText("Documents");
    scrollIntoView.mockClear();

    fireEvent.keyDown(input, { key: "ArrowDown" });

    expect(scrollIntoView).toHaveBeenCalledWith({ block: "nearest" });
  });

  it("selects a search result when hovering it", async () => {
    vi.mocked(search).mockResolvedValueOnce(fileResults);
    render(<App />);
    const input = screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令");

    fireEvent.change(input, { target: { value: "doc" } });
    const documents = await screen.findByRole("option", { name: /Documents/ });
    const downloads = screen.getByRole("option", { name: /Downloads/ });
    expect(documents).toHaveAttribute("aria-selected", "true");

    fireEvent.mouseEnter(downloads);

    expect(documents).toHaveAttribute("aria-selected", "false");
    expect(downloads).toHaveAttribute("aria-selected", "true");
    expect(downloads).toHaveClass("result-item--selected");
  });

  it("summarizes long POSIX and Windows paths while exposing full paths as tooltips", async () => {
    vi.mocked(search).mockResolvedValueOnce(longPathResults);
    render(<App />);

    fireEvent.change(screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令"), {
      target: { value: "fixture" },
    });

    expect(await screen.findAllByText("VeryLongMatchingFileName.fixture.tsx")).toHaveLength(2);
    const posixPath = screen.getByLabelText(
      "完整路径 /Users/frankzhang/workspace/QuickFox/src/components/DeeplyNestedFeature/VeryLongMatchingFileName.fixture.tsx",
    );
    expect(posixPath).toHaveTextContent(
      "/Users/frankzhang/.../DeeplyNestedFeature/VeryLongMatchingFileName.fixture.tsx",
    );
    expect(posixPath).toHaveAttribute(
      "title",
      "/Users/frankzhang/workspace/QuickFox/src/components/DeeplyNestedFeature/VeryLongMatchingFileName.fixture.tsx",
    );

    const windowsPath = screen.getByLabelText(
      "完整路径 C:\\Users\\frank\\Documents\\QuickFox\\fixtures\\reports\\VeryLongMatchingFileName.fixture.tsx",
    );
    expect(windowsPath).toHaveTextContent(
      "C:\\Users\\...\\reports\\VeryLongMatchingFileName.fixture.tsx",
    );
    expect(windowsPath).toHaveAttribute(
      "title",
      "C:\\Users\\frank\\Documents\\QuickFox\\fixtures\\reports\\VeryLongMatchingFileName.fixture.tsx",
    );
  });

  it("keeps result titles identifiable and selected rows semantically styled", async () => {
    vi.mocked(search).mockResolvedValueOnce(longPathResults);
    render(<App />);
    const input = screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令");

    fireEvent.change(input, { target: { value: "fixture" } });
    const firstResult = await screen.findByRole("option", {
      name: /VeryLongMatchingFileName\.fixture\.tsx.*DeeplyNestedFeature/,
    });
    expect(firstResult).toHaveClass("result-item--selected");
    expect(firstResult).toHaveAttribute("aria-selected", "true");
    expect(screen.getAllByTitle("VeryLongMatchingFileName.fixture.tsx")[0]).toHaveTextContent(
      "VeryLongMatchingFileName.fixture.tsx",
    );

    fireEvent.keyDown(input, { key: "ArrowDown" });

    const secondResult = screen.getByRole("option", {
      name: /VeryLongMatchingFileName\.fixture\.tsx.*reports/,
    });
    expect(firstResult).not.toHaveClass("result-item--selected");
    expect(secondResult).toHaveClass("result-item--selected");
    expect(secondResult).toHaveAttribute("aria-selected", "true");
  });

  it("shows long result titles with a middle summary that keeps the ending visible", async () => {
    vi.mocked(search).mockResolvedValueOnce([
      {
        ...longPathResults[0],
        title: "PROJECT_SUMMARY_WITH_A_VERY_LONG_PREFIX_THAT_WOULD_HIDE_THE_EXTENSION.md",
      },
    ]);
    render(<App />);

    fireEvent.change(screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令"), {
      target: { value: "summary" },
    });

    const title = await screen.findByTitle(
      "PROJECT_SUMMARY_WITH_A_VERY_LONG_PREFIX_THAT_WOULD_HIDE_THE_EXTENSION.md",
    );
    expect(title).toHaveTextContent("PROJECT_SUMMARY...THE_EXTENSION.md");
    expect(title.querySelector(".result-title-tail")).toHaveTextContent("THE_EXTENSION.md");
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

  it("offers a development open action from the result context menu", async () => {
    const onExecuteAction = vi.fn();
    vi.mocked(search).mockResolvedValueOnce([
      {
        ...longPathResults[0],
        title: "report.md",
        detail: "/tmp/report.md",
        secondaryActions: [
          { type: "openContainingFolder", path: "/tmp/report.md" },
          { type: "copyText", text: "/tmp/report.md" },
          {
            type: "openWithApplication",
            path: "/tmp/report.md",
            application: "systemChooser",
          },
        ],
      },
    ]);
    render(<App onExecuteAction={onExecuteAction} />);
    fireEvent.change(screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令"), {
      target: { value: "doc" },
    });

    fireEvent.contextMenu(await screen.findByRole("option", { name: /report\.md/ }));
    fireEvent.click(screen.getByRole("menuitem", { name: "选择打开方式" }));

    expect(onExecuteAction).toHaveBeenCalledWith({
      type: "openWithApplication",
      path: "/tmp/report.md",
      application: "systemChooser",
    });
  });

  it("renders type badges for applications files and directories", async () => {
    vi.mocked(search).mockResolvedValueOnce(typedResults);
    render(<App />);

    fireEvent.change(screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令"), {
      target: { value: "typed" },
    });

    expect(await screen.findByLabelText("应用")).toBeInTheDocument();
    expect(screen.getByLabelText("文件")).toBeInTheDocument();
    expect(screen.getByLabelText("目录")).toBeInTheDocument();
  });

  it("shows context actions by result type", async () => {
    vi.mocked(search).mockResolvedValueOnce(typedResults);
    render(<App />);

    fireEvent.change(screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令"), {
      target: { value: "typed" },
    });

    fireEvent.contextMenu(await screen.findByRole("option", { name: /Codex\.app/ }));
    expect(screen.getByRole("menuitem", { name: "打开应用" })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: "复制路径" })).toBeInTheDocument();
    expect(screen.queryByRole("menuitem", { name: "选择打开方式" })).not.toBeInTheDocument();
    fireEvent.keyDown(screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令"), {
      key: "Escape",
    });

    fireEvent.contextMenu(screen.getByRole("option", { name: /report\.md/ }));
    expect(screen.getByRole("menuitem", { name: "打开所在目录" })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: "复制路径" })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: "选择打开方式" })).toBeInTheDocument();
    fireEvent.keyDown(screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令"), {
      key: "Escape",
    });

    fireEvent.contextMenu(screen.getByRole("option", { name: /Documents/ }));
    expect(screen.getByRole("menuitem", { name: "打开文件夹" })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: "复制路径" })).toBeInTheDocument();
    expect(screen.queryByRole("menuitem", { name: "选择打开方式" })).not.toBeInTheDocument();
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

  it("opens the settings window from launcher recovery instead of rendering settings in-place", async () => {
    vi.mocked(indexStatus).mockResolvedValueOnce({
      kind: "failed",
      entryCount: 0,
      message: "权限不足，无法扫描目录",
      generation: 3,
      completedAtMs: null,
    });
    vi.mocked(search).mockResolvedValueOnce([]);

    render(<App />);
    fireEvent.change(screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令"), {
      target: { value: "notes" },
    });

    const status = await screen.findByRole("region", { name: "启动器状态" });
    fireEvent.click(within(status).getByRole("button", { name: "打开设置" }));

    expect(openSettingsWindow).toHaveBeenCalledOnce();
    expect(screen.queryByRole("form", { name: "设置" })).not.toBeInTheDocument();
  });

  it("does not offer a return-to-search action in the settings window", async () => {
    render(<App initialView="settings" />);

    expect(screen.getByRole("form", { name: "设置" })).toBeInTheDocument();
    expect(
      screen.queryByLabelText("搜索文件、目录、计算器、网页搜索或命令"),
    ).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "返回搜索" })).not.toBeInTheDocument();
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
    expect(screen.getByRole("form", { name: "设置" })).toBeInTheDocument();
  });

  it("loads app paths through the Tauri client contract for settings", async () => {
    render(<App initialView="settings" />);

    await waitFor(() => expect(appPaths).toHaveBeenCalledOnce());
    expect(await screen.findByText("配置文件位置")).toBeInTheDocument();
    expect(
      screen.getAllByText("/Users/frank/Library/Application Support/QuickFox/config.json")[0],
    ).toBeInTheDocument();
    expect(screen.getByText("索引快照位置")).toBeInTheDocument();
    expect(
      screen.getAllByText(
        "/Users/frank/Library/Application Support/QuickFox/index.snapshot.json",
      )[0],
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "配置文件位置完整路径" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "索引快照位置完整路径" })).toBeInTheDocument();
  });

  it("keeps a single save button in the left settings action area across tabs", async () => {
    render(<App initialView="settings" />);
    await screen.findByDisplayValue("/tmp");

    const actionArea = screen.getByRole("region", { name: "设置操作" });
    expect(screen.getAllByRole("button", { name: "保存设置" })).toHaveLength(1);
    expect(actionArea).toContainElement(screen.getByRole("button", { name: "保存设置" }));

    fireEvent.click(screen.getByRole("tab", { name: "网页搜索" }));
    expect(screen.getAllByRole("button", { name: "保存设置" })).toHaveLength(1);
    expect(actionArea).toContainElement(screen.getByRole("button", { name: "保存设置" }));

    fireEvent.click(screen.getByRole("tab", { name: "命令安全" }));
    expect(screen.getAllByRole("button", { name: "保存设置" })).toHaveLength(1);
    expect(actionArea).toContainElement(screen.getByRole("button", { name: "保存设置" }));
  });

  it("keeps settings sections inside a dedicated scrollable content region", async () => {
    render(<App initialView="settings" />);
    await screen.findByDisplayValue("/tmp");

    const contentRegion = screen.getByRole("region", { name: "设置内容" });
    expect(contentRegion).toHaveClass("settings-content");
    expect(contentRegion).toContainElement(screen.getByRole("group", { name: "索引" }));

    fireEvent.click(screen.getByRole("tab", { name: "外观" }));

    expect(contentRegion).toContainElement(screen.getByRole("group", { name: "外观与窗口" }));
  });

  it("shows global Shift+Shift hotkey status in appearance settings", async () => {
    vi.mocked(globalHotkeyStatus).mockResolvedValueOnce({
      enabled: false,
      message: "需要授予输入监控权限后才能使用 Shift+Shift 全局唤醒",
      permissionSettingsUrl:
        "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent",
    });

    const onExecuteAction = vi.fn();
    render(<App initialView="settings" onExecuteAction={onExecuteAction} />);
    await screen.findByDisplayValue("/tmp");
    fireEvent.click(screen.getByRole("tab", { name: "外观" }));

    expect(screen.getByText("全局唤醒")).toBeInTheDocument();
    expect(screen.getByText(/输入监控权限/)).toBeInTheDocument();
    expect(screen.getByText(/授权后需要重启 QuickFox/)).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "打开权限设置" }));

    expect(onExecuteAction).toHaveBeenCalledWith({
      type: "openUrl",
      url: "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent",
    });
  });

  it("records a custom global wake shortcut from appearance settings", async () => {
    render(<App initialView="settings" />);
    await screen.findByDisplayValue("/tmp");
    fireEvent.click(screen.getByRole("tab", { name: "外观" }));

    const recorder = screen.getByRole("button", { name: "Shift+Shift" });
    fireEvent.click(recorder);
    fireEvent.keyDown(recorder, { key: " ", ctrlKey: true });
    fireEvent.click(screen.getByRole("button", { name: "保存设置" }));

    expect(saveConfig).toHaveBeenCalledWith({
      ...appConfig,
      hotkey: {
        wake_shortcut: "Control+Space",
      },
    });
  });

  it("keeps recording the global wake shortcut when keydown lands on the document", async () => {
    render(<App initialView="settings" />);
    await screen.findByDisplayValue("/tmp");
    fireEvent.click(screen.getByRole("tab", { name: "外观" }));

    fireEvent.click(screen.getByRole("button", { name: "Shift+Shift" }));
    fireEvent.keyDown(document, { key: "k", metaKey: true, shiftKey: true });
    fireEvent.click(screen.getByRole("button", { name: "保存设置" }));

    expect(saveConfig).toHaveBeenCalledWith({
      ...appConfig,
      hotkey: {
        wake_shortcut: "Command+Shift+K",
      },
    });
  });

  it("records double Shift as the default wake shortcut from two Shift presses", async () => {
    vi.mocked(loadConfig).mockResolvedValueOnce({
      ...appConfig,
      hotkey: {
        wake_shortcut: "Control+Space",
      },
    });
    render(<App initialView="settings" />);
    await screen.findByDisplayValue("/tmp");
    fireEvent.click(screen.getByRole("tab", { name: "外观" }));

    const recorder = screen.getByRole("button", { name: "Control+Space" });
    fireEvent.click(recorder);
    fireEvent.keyDown(recorder, { key: "Shift" });
    expect(screen.getByText("再次按 Shift 可录制为 Shift+Shift。")).toBeInTheDocument();
    fireEvent.keyDown(recorder, { key: "Shift" });
    fireEvent.click(screen.getByRole("button", { name: "保存设置" }));

    expect(saveConfig).toHaveBeenCalledWith({
      ...appConfig,
      hotkey: {
        wake_shortcut: "Shift+Shift",
      },
    });
  });

  it("rejects a bare modifier while recording the global wake shortcut", async () => {
    render(<App initialView="settings" />);
    await screen.findByDisplayValue("/tmp");
    fireEvent.click(screen.getByRole("tab", { name: "外观" }));

    const recorder = screen.getByRole("button", { name: "Shift+Shift" });
    fireEvent.click(recorder);
    fireEvent.keyDown(recorder, { key: "Alt", altKey: true });

    expect(screen.getByText("请按一个修饰键加普通键，或连续按两次 Shift。")).toBeInTheDocument();
    expect(saveConfig).not.toHaveBeenCalled();
  });

  it("shows help icons for configurable settings fields", async () => {
    render(<App initialView="settings" />);
    await screen.findByDisplayValue("/tmp");

    expect(screen.getByRole("button", { name: "索引目录说明" })).toBeInTheDocument();
    expect(screen.getByText(/每行填写一个完整目录路径，例如/)).toBeInTheDocument();
    expect(screen.getByText(/type:pdf/)).toBeInTheDocument();
    expect(screen.getByText(/content:"hello world"/)).toBeInTheDocument();

    fireEvent.click(screen.getByRole("tab", { name: "外观" }));

    expect(screen.getByRole("button", { name: "全局唤醒键说明" })).toBeInTheDocument();
    expect(screen.getByText(/保存后新按键生效/)).toBeInTheDocument();
  });

  it("exposes full settings values and field guidance for truncated rows", async () => {
    render(<App initialView="settings" />);
    await screen.findByDisplayValue("/tmp");

    fireEvent.click(screen.getByRole("tab", { name: "网页搜索" }));

    const googleUrl = screen.getByText("https://www.google.com/search?q={query}");
    expect(googleUrl).toHaveAttribute("title", "https://www.google.com/search?q={query}");

    fireEvent.click(screen.getByRole("tab", { name: "历史" }));
    expect(screen.getByRole("button", { name: "输入历史条数说明" })).toBeInTheDocument();
    expect(screen.getByText(/0 表示不保留/)).toBeInTheDocument();

    fireEvent.click(screen.getByRole("tab", { name: "命令安全" }));
    expect(screen.getByRole("button", { name: "命令执行说明" })).toBeInTheDocument();
    expect(screen.getByText(/每次执行前仍会要求确认/)).toBeInTheDocument();
  });

  it("shows complete index status details in settings", async () => {
    vi.mocked(indexStatus).mockResolvedValueOnce({
      kind: "ready",
      entryCount: 240,
      message: "索引完成",
      generation: 7,
      completedAtMs: 1710000000000,
    });

    render(<App initialView="settings" />);

    expect(await screen.findByText("文件索引可用")).toBeInTheDocument();
    expect(screen.getByText("240 项")).toBeInTheDocument();
    expect(screen.getByText("第 7 代")).toBeInTheDocument();
    expect(screen.getByText(/最近完成:/)).toBeInTheDocument();
    expect(screen.getAllByText("索引完成").length).toBeGreaterThan(0);
  });

  it("shows the index settings as a layered workspace", async () => {
    render(<App initialView="settings" />);
    await screen.findByDisplayValue("/tmp");

    const indexSection = screen.getByRole("group", { name: "索引" });
    expect(within(indexSection).getByRole("region", { name: "索引状态摘要" })).toBeInTheDocument();
    expect(within(indexSection).getByRole("region", { name: "主规则编辑" })).toBeInTheDocument();
    expect(within(indexSection).getByRole("region", { name: "辅助信息" })).toBeInTheDocument();
  });

  it("keeps regex prefix and maintenance paths in the index auxiliary column", async () => {
    render(<App initialView="settings" />);
    await screen.findByDisplayValue("/tmp");

    const auxiliaryColumn = screen.getByRole("region", { name: "辅助信息" });
    expect(auxiliaryColumn).toContainElement(screen.getByLabelText("正则前缀"));
    expect(auxiliaryColumn).toContainElement(screen.getByText("配置文件位置"));
    expect(auxiliaryColumn).toContainElement(
      screen.getAllByText("/Users/frank/Library/Application Support/QuickFox/config.json")[0],
    );
    expect(auxiliaryColumn).toContainElement(screen.getByText("索引快照位置"));
    expect(auxiliaryColumn).toContainElement(
      screen.getAllByText(
        "/Users/frank/Library/Application Support/QuickFox/index.snapshot.json",
      )[0],
    );
  });

  it("renders maintenance paths as selectable full-width text in the auxiliary column", async () => {
    render(<App initialView="settings" />);
    await screen.findByDisplayValue("/tmp");

    const configPath = screen.getByLabelText("配置文件位置完整路径文本");
    expect(configPath).toHaveTextContent(
      "/Users/frank/Library/Application Support/QuickFox/config.json",
    );
    expect(configPath).toHaveClass("settings-full-path-value");

    const snapshotPath = screen.getByLabelText("索引快照位置完整路径文本");
    expect(snapshotPath).toHaveTextContent(
      "/Users/frank/Library/Application Support/QuickFox/index.snapshot.json",
    );
    expect(snapshotPath).toHaveClass("settings-full-path-value");
  });

  it("places refresh index in the index workspace header", async () => {
    render(<App initialView="settings" />);

    const header = await screen.findByRole("region", { name: "索引工作区标题" });
    fireEvent.click(within(header).getByRole("button", { name: "刷新索引" }));

    expect(refreshIndex).toHaveBeenCalledOnce();
    expect(await screen.findByText("索引已刷新")).toBeInTheDocument();
  });

  it("renders the basic settings view", () => {
    render(<App initialView="settings" />);

    expect(screen.getByRole("form", { name: "设置" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "设置" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "索引" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "网页搜索" })).toBeInTheDocument();
    expect(screen.getByRole("group", { name: "索引" })).toBeInTheDocument();
    expect(screen.getByRole("group", { name: "网页搜索" })).toBeInTheDocument();
    expect(screen.getByRole("group", { name: "历史" })).toBeInTheDocument();
    expect(screen.getByRole("group", { name: "命令执行" })).toBeInTheDocument();
    expect(screen.getByRole("group", { name: "外观与窗口" })).toBeInTheDocument();
    expect(screen.getByLabelText("索引目录")).toBeInTheDocument();
    expect(screen.getByLabelText("正则前缀")).toHaveValue("re:");
    expect(screen.getByLabelText("命令执行")).not.toBeChecked();
    expect(screen.getByLabelText("输入历史条数")).toHaveValue(15);
  });

  it("adds a DuckDuckGo web search engine from the settings wizard", async () => {
    render(<App initialView="settings" />);
    await screen.findByDisplayValue("/tmp");

    fireEvent.click(screen.getByRole("button", { name: "新增搜索引擎" }));
    fireEvent.change(screen.getByLabelText("搜索前缀"), { target: { value: "ddg" } });
    fireEvent.change(screen.getByLabelText("搜索名称"), { target: { value: "DuckDuckGo" } });
    fireEvent.change(screen.getByLabelText("URL 模板"), {
      target: { value: "https://duckduckgo.com/?q={query}" },
    });
    fireEvent.click(screen.getByRole("button", { name: "添加引擎" }));
    fireEvent.click(screen.getByRole("button", { name: "保存设置" }));

    await waitFor(() =>
      expect(saveConfig).toHaveBeenCalledWith({
        ...appConfig,
        web_search: {
          engines: {
            ...appConfig.web_search.engines,
            ddg: {
              name: "DuckDuckGo",
              url: "https://duckduckgo.com/?q={query}",
            },
          },
        },
      }),
    );
  });

  it("validates web search URL templates in the settings wizard", () => {
    render(<App initialView="settings" />);

    fireEvent.click(screen.getByRole("button", { name: "新增搜索引擎" }));
    fireEvent.change(screen.getByLabelText("搜索前缀"), { target: { value: "bad" } });
    fireEvent.change(screen.getByLabelText("搜索名称"), { target: { value: "Broken" } });
    fireEvent.change(screen.getByLabelText("URL 模板"), {
      target: { value: "https://example.com/search" },
    });
    fireEvent.click(screen.getByRole("button", { name: "添加引擎" }));

    expect(screen.getByText("URL 模板必须包含 {query}")).toBeInTheDocument();
  });

  it("saves index include and exclude rules from settings", async () => {
    render(<App initialView="settings" />);
    await screen.findByDisplayValue("/tmp");

    fireEvent.change(screen.getByLabelText("索引目录"), {
      target: { value: "/tmp\n/Users/frank/Documents" },
    });
    fireEvent.change(screen.getByLabelText("排除目录"), {
      target: { value: "/tmp/cache" },
    });
    fireEvent.change(screen.getByLabelText("排除模式"), {
      target: { value: "*.log\nnode_modules" },
    });
    fireEvent.change(screen.getByLabelText("索引性能模式"), {
      target: { value: "complete" },
    });
    fireEvent.click(screen.getByLabelText("尊重项目 ignore"));
    fireEvent.change(screen.getByLabelText("内容索引目录"), {
      target: { value: "/tmp/Documents\n/Users/frank/workspace" },
    });
    fireEvent.change(screen.getByLabelText("内容大小上限 MB"), {
      target: { value: "4" },
    });
    fireEvent.click(screen.getByLabelText("运行期文件监听"));
    fireEvent.click(screen.getByRole("button", { name: "保存设置" }));

    await waitFor(() =>
      expect(saveConfig).toHaveBeenCalledWith({
        ...appConfig,
        index: {
          include_dirs: ["/tmp", "/Users/frank/Documents"],
          exclude_dirs: ["/tmp/cache"],
          exclude_patterns: ["*.log", "node_modules"],
          performance_mode: "complete",
          respect_project_ignores: false,
          content_include_dirs: ["/tmp/Documents", "/Users/frank/workspace"],
          content_max_file_bytes: 4194304,
          watcher_enabled: false,
        },
      }),
    );
  });

  it("keeps pending blank lines while editing multiline index path fields", async () => {
    render(<App initialView="settings" />);
    await screen.findByDisplayValue("/tmp");

    const includeDirs = screen.getByLabelText("索引目录");
    fireEvent.change(includeDirs, {
      target: { value: "/tmp\n" },
    });

    expect(includeDirs).toHaveValue("/tmp\n");

    fireEvent.click(screen.getByRole("button", { name: "保存设置" }));

    await waitFor(() =>
      expect(saveConfig).toHaveBeenCalledWith({
        ...appConfig,
        index: {
          ...appConfig.index,
          include_dirs: ["/tmp"],
        },
      }),
    );
  });

  it("refreshes the index from the settings search group", async () => {
    render(<App initialView="settings" />);

    fireEvent.click(await screen.findByRole("button", { name: "刷新索引" }));

    expect(refreshIndex).toHaveBeenCalledOnce();
    expect(await screen.findByText("索引已刷新")).toBeInTheDocument();
  });

  it("shows recent input history in Shift history mode", async () => {
    vi.mocked(recentInputHistory).mockResolvedValueOnce(["g 1234", "notes"]);
    render(<App />);
    const input = screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令");

    await waitFor(() => expect(recentInputHistory).toHaveBeenCalledOnce());
    fireEvent.keyDown(input, { key: "Shift" });

    expect(screen.getByRole("list", { name: "输入历史" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "g 1234" })).toHaveAttribute("aria-selected", "true");
  });

  it("keeps history rows in a single-column layout", async () => {
    vi.mocked(recentInputHistory).mockResolvedValueOnce([
      "very long remembered input that should use the whole history row",
    ]);
    render(<App />);
    const input = screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令");

    await waitFor(() => expect(recentInputHistory).toHaveBeenCalledOnce());
    fireEvent.keyDown(input, { key: "Shift" });

    expect(
      screen.getByRole("option", {
        name: "very long remembered input that should use the whole history row",
      }),
    ).toHaveClass("history-item");
    expect(
      screen.getByRole("option", {
        name: "very long remembered input that should use the whole history row",
      }),
    ).not.toHaveClass("result-item");
  });

  it("selects an input history row when hovering it", async () => {
    vi.mocked(recentInputHistory).mockResolvedValueOnce(["g 1234", "notes"]);
    render(<App />);
    const input = screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令");

    await waitFor(() => expect(recentInputHistory).toHaveBeenCalledOnce());
    fireEvent.keyDown(input, { key: "Shift" });
    const first = screen.getByRole("option", { name: "g 1234" });
    const second = screen.getByRole("option", { name: "notes" });
    expect(first).toHaveAttribute("aria-selected", "true");

    fireEvent.mouseEnter(second);

    expect(first).toHaveAttribute("aria-selected", "false");
    expect(second).toHaveAttribute("aria-selected", "true");
  });

  it("keeps arrow keys on search results until Shift history mode is active", async () => {
    vi.mocked(recentInputHistory).mockResolvedValueOnce(["g 1234", "notes"]);
    vi.mocked(search).mockResolvedValueOnce(fileResults);
    render(<App />);
    const input = screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令");

    fireEvent.change(input, { target: { value: "doc" } });
    await screen.findByText("Documents");
    fireEvent.keyDown(input, { key: "ArrowDown" });

    expect(input).toHaveValue("doc");
    expect(screen.getByRole("option", { name: /Downloads/ })).toHaveAttribute(
      "aria-selected",
      "true",
    );
  });

  it("fills the input from history mode without executing immediately", async () => {
    const onExecuteAction = vi.fn();
    vi.mocked(recentInputHistory).mockResolvedValueOnce(["g 1234", "notes"]);
    render(<App onExecuteAction={onExecuteAction} />);
    const input = screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令");

    await waitFor(() => expect(recentInputHistory).toHaveBeenCalledOnce());
    fireEvent.keyDown(input, { key: "Shift" });
    fireEvent.keyDown(input, { key: "ArrowDown" });
    fireEvent.keyDown(input, { key: "Enter" });

    expect(input).toHaveValue("notes");
    expect(screen.queryByRole("list", { name: "输入历史" })).not.toBeInTheDocument();
    expect(onExecuteAction).not.toHaveBeenCalled();
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

  it("opens Baidu web search directly on Enter without waiting for rendered results", async () => {
    const onExecuteAction = vi.fn();
    vi.mocked(recordInputHistory).mockResolvedValueOnce("recorded");
    render(<App onExecuteAction={onExecuteAction} />);
    const input = screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令");

    fireEvent.change(input, { target: { value: "bd 1234" } });
    fireEvent.keyDown(input, { key: "Enter" });

    expect(onExecuteAction).toHaveBeenCalledWith({
      type: "openUrl",
      url: "https://www.baidu.com/s?wd=1234",
    });
    await waitFor(() => expect(recordInputHistory).toHaveBeenCalledWith("bd 1234"));
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
