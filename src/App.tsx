import { type KeyboardEvent, useEffect, useState } from "react";

import {
  executeAction,
  indexStatus,
  loadConfig,
  listenOpenSettings,
  recentInputHistory,
  recordInputHistory,
  refreshIndex,
  type IndexStatus,
  type QuickFoxConfig,
  saveConfig,
  search as searchResults,
} from "./tauriClient";

type LauncherAction =
  | { type: "openPath"; path: string }
  | { type: "openContainingFolder"; path: string }
  | { type: "copyText"; text: string }
  | { type: "openUrl"; url: string }
  | { type: "openWithApplication"; path: string; application: "developmentTool" }
  | { type: "executeCommand"; command: string; requiresConfirmation: boolean };

type LauncherResult = {
  id: string;
  title: string;
  detail?: string | null;
  primaryAction: LauncherAction;
  secondaryActions: Array<{ label: string; action: LauncherAction }>;
};

type BackendSearchResult = {
  id: string;
  title: string;
  detail?: string | null;
  mainAction: LauncherAction;
  secondaryActions: LauncherAction[];
};

type AppProps = {
  commandEnabled?: boolean;
  initialView?: "launcher" | "settings";
  onClose?: () => void;
  onExecuteAction?: (action: LauncherAction) => unknown;
};

const fallbackConfig: QuickFoxConfig = {
  index: {
    include_dirs: [],
    exclude_dirs: [],
    exclude_patterns: [],
  },
  query: {
    regex_prefix: "re:",
  },
  web_search: {
    engines: {
      g: { name: "Google", url: "https://www.google.com/search?q={query}" },
      ddg: { name: "DuckDuckGo", url: "https://duckduckgo.com/?q={query}" },
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
};

function labelForAction(action: LauncherAction) {
  switch (action.type) {
    case "openPath":
      return "打开";
    case "openContainingFolder":
      return "打开所在目录";
    case "copyText":
      return "复制路径";
    case "openUrl":
      return "打开链接";
    case "openWithApplication":
      return "用开发工具打开";
    case "executeCommand":
      return "确认执行";
  }
}

function buildWebSearchAction(
  query: string,
  engines: QuickFoxConfig["web_search"]["engines"],
): LauncherAction | null {
  const trimmed = query.trim();
  const separator = trimmed.search(/\s/);
  if (separator <= 0) {
    return null;
  }

  const prefix = trimmed.slice(0, separator);
  const searchText = trimmed.slice(separator).trim();
  const engine = engines[prefix];
  if (!engine || !searchText || !engine.url.includes("{query}")) {
    return null;
  }

  return {
    type: "openUrl",
    url: engine.url.replace("{query}", encodeURIComponent(searchText)),
  };
}

function toLauncherResults(results: BackendSearchResult[]): LauncherResult[] {
  return results.map((result) => ({
    id: result.id,
    title: result.title,
    detail: result.detail,
    primaryAction: result.mainAction,
    secondaryActions: result.secondaryActions.map((action) => ({
      label: labelForAction(action),
      action,
    })),
  }));
}

export function App({
  commandEnabled,
  initialView = "launcher",
  onClose = () => undefined,
  onExecuteAction = executeAction,
}: AppProps) {
  const [view, setView] = useState<"launcher" | "settings">(initialView);
  const [config, setConfig] = useState<QuickFoxConfig>(fallbackConfig);
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<LauncherResult[]>([]);
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [menuResultId, setMenuResultId] = useState<string | null>(null);
  const [menuPosition, setMenuPosition] = useState<{ left: number; top: number } | null>(null);
  const [inputHistory, setInputHistory] = useState<string[]>([]);
  const [historyIndex, setHistoryIndex] = useState<number | null>(null);
  const [historyMode, setHistoryMode] = useState(false);
  const [refreshStatus, setRefreshStatus] = useState<string | null>(null);
  const [currentIndexStatus, setCurrentIndexStatus] = useState<IndexStatus>({
    kind: "unbuilt",
    entryCount: 0,
    message: null,
    generation: 0,
    completedAtMs: null,
  });
  const [settingsSection, setSettingsSection] = useState<
    "index" | "web" | "history" | "command" | "appearance"
  >("index");
  const [engineWizardOpen, setEngineWizardOpen] = useState(false);
  const [engineDraft, setEngineDraft] = useState({ prefix: "", name: "", url: "" });
  const [engineError, setEngineError] = useState<string | null>(null);
  const effectiveCommandEnabled = commandEnabled ?? config.command.enabled;

  const isCommandQuery = query.trim().startsWith(">");
  const isCommandMode = isCommandQuery;
  const commandText = query.trim().slice(1).trim();
  const selectedResult = results[Math.min(selectedIndex, Math.max(results.length - 1, 0))];
  const menuResult = results.find((result) => result.id === menuResultId);

  useEffect(() => {
    let cancelled = false;
    void Promise.all([loadConfig(), recentInputHistory(), indexStatus()])
      .then(([nextConfig, nextHistory, nextIndexStatus]) => {
        if (!cancelled) {
          setConfig(nextConfig as QuickFoxConfig);
          setInputHistory(nextHistory as string[]);
          setCurrentIndexStatus(nextIndexStatus as IndexStatus);
        }
      })
      .catch(() => undefined);

    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    let dispose: (() => void) | undefined;
    void listenOpenSettings(() => setView("settings")).then((unlisten) => {
      dispose = unlisten;
    });
    return () => {
      dispose?.();
    };
  }, []);

  useEffect(() => {
    if (!query.trim() || isCommandQuery) {
      setResults([]);
      return;
    }

    let cancelled = false;
    void searchResults(query)
      .then((nextResults) => {
        if (!cancelled) {
          setResults(toLauncherResults(nextResults as BackendSearchResult[]));
        }
      })
      .catch(() => {
        if (!cancelled) {
          setResults([]);
        }
      });

    return () => {
      cancelled = true;
    };
  }, [isCommandQuery, query]);

  const updateQuery = (value: string) => {
    setQuery(value);
    setSelectedIndex(0);
    setHistoryIndex(null);
    setHistoryMode(false);
    setMenuResultId(null);
    setMenuPosition(null);
  };

  const executeSelected = async () => {
    const executedInput = query.trim();
    const webSearchAction = buildWebSearchAction(query, config.web_search.engines);
    if (webSearchAction) {
      await onExecuteAction(webSearchAction);
      if (executedInput) {
        await recordInputHistory(executedInput);
      }
      return;
    }

    if (isCommandMode && commandText) {
      if (!effectiveCommandEnabled) {
        return;
      }

      await onExecuteAction({
        type: "executeCommand",
        command: commandText,
        requiresConfirmation: true,
      });
      if (executedInput) {
        await recordInputHistory(executedInput);
      }
      return;
    }

    if (selectedResult) {
      await onExecuteAction(selectedResult.primaryAction);
      if (executedInput) {
        await recordInputHistory(executedInput);
      }
    }
  };

  const handleKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Shift") {
      if (inputHistory.length > 0) {
        event.preventDefault();
        setHistoryMode(true);
        setHistoryIndex((index) => index ?? 0);
      }
      return;
    }

    if (event.key === "Escape") {
      event.preventDefault();
      if (historyMode) {
        setHistoryMode(false);
        setHistoryIndex(null);
        return;
      }
      onClose();
      return;
    }

    if (event.key === "ArrowDown") {
      event.preventDefault();
      if (historyMode && inputHistory.length > 0) {
        const nextIndex =
          historyIndex === null ? 0 : Math.min(historyIndex + 1, inputHistory.length - 1);
        setHistoryIndex(nextIndex);
        return;
      }
      setSelectedIndex((index) => Math.min(index + 1, Math.max(results.length - 1, 0)));
      return;
    }

    if (event.key === "ArrowUp") {
      event.preventDefault();
      if (historyMode && inputHistory.length > 0) {
        const nextIndex = historyIndex === null ? 0 : Math.max(historyIndex - 1, 0);
        setHistoryIndex(nextIndex);
        return;
      }
      setSelectedIndex((index) => Math.max(index - 1, 0));
      return;
    }

    if (event.key === "Enter") {
      event.preventDefault();
      if (historyMode) {
        const selectedHistory = inputHistory[historyIndex ?? 0];
        if (selectedHistory) {
          setQuery(selectedHistory);
        }
        setHistoryMode(false);
        setHistoryIndex(null);
        return;
      }
      void executeSelected();
    }
  };

  const refreshSearchIndex = async () => {
    setRefreshStatus(null);
    const nextStatus = (await refreshIndex()) as IndexStatus;
    setCurrentIndexStatus(nextStatus);
    setRefreshStatus("索引已刷新");
  };

  const addEngineFromWizard = () => {
    const prefix = engineDraft.prefix.trim();
    const name = engineDraft.name.trim();
    const url = engineDraft.url.trim();
    if (!url.includes("{query}")) {
      setEngineError("URL 模板必须包含 {query}");
      return;
    }
    if (!prefix || !name) {
      setEngineError("前缀和名称不能为空");
      return;
    }

    setConfig((current) => ({
      ...current,
      web_search: {
        engines: {
          ...current.web_search.engines,
          [prefix]: { name, url },
        },
      },
    }));
    setEngineWizardOpen(false);
    setEngineDraft({ prefix: "", name: "", url: "" });
    setEngineError(null);
  };

  const removeEngine = (prefix: string) => {
    setConfig((current) => {
      const engines = { ...current.web_search.engines };
      delete engines[prefix];
      return {
        ...current,
        web_search: { engines },
      };
    });
  };

  const saveSettings = async () => {
    await saveConfig(config);
    const nextStatus = (await indexStatus()) as IndexStatus;
    setCurrentIndexStatus(nextStatus);
    setView("launcher");
  };

  if (view === "settings") {
    return (
      <main className="launcher-shell" aria-label="QuickFox launcher">
        <section className="launcher-panel settings-panel">
          <header className="panel-toolbar">
            <button type="button" className="toolbar-button" onClick={() => setView("launcher")}>
              返回搜索
            </button>
          </header>
          <form aria-label="设置" className="settings-form">
            <nav aria-label="设置分区" className="settings-tabs" role="tablist">
              {[
                ["index", "索引"],
                ["web", "网页搜索"],
                ["history", "历史"],
                ["command", "命令安全"],
                ["appearance", "外观"],
              ].map(([id, label]) => (
                <button
                  aria-selected={settingsSection === id}
                  key={id}
                  onClick={() => setSettingsSection(id as typeof settingsSection)}
                  role="tab"
                  type="button"
                >
                  {label}
                </button>
              ))}
            </nav>
            <fieldset className={settingsSection === "index" ? "" : "settings-section-muted"}>
              <legend>搜索与索引</legend>
              <div className="settings-status-row">
                <span>{indexStatusLabel(currentIndexStatus)}</span>
                <span>{currentIndexStatus.entryCount} 项</span>
              </div>
              <label>
                索引目录
                <textarea
                  value={config.index.include_dirs.join("\n")}
                  onChange={(event) =>
                    setConfig((current) => ({
                      ...current,
                      index: {
                        ...current.index,
                        include_dirs: event.target.value
                          .split("\n")
                          .map((item) => item.trim())
                          .filter(Boolean),
                      },
                    }))
                  }
                />
              </label>
              <label>
                排除目录
                <textarea
                  value={config.index.exclude_dirs.join("\n")}
                  onChange={(event) =>
                    setConfig((current) => ({
                      ...current,
                      index: {
                        ...current.index,
                        exclude_dirs: linesFromTextarea(event.target.value),
                      },
                    }))
                  }
                />
              </label>
              <label>
                排除模式
                <textarea
                  value={config.index.exclude_patterns.join("\n")}
                  onChange={(event) =>
                    setConfig((current) => ({
                      ...current,
                      index: {
                        ...current.index,
                        exclude_patterns: linesFromTextarea(event.target.value),
                      },
                    }))
                  }
                />
              </label>
              <label>
                正则前缀
                <input
                  value={config.query.regex_prefix}
                  onChange={(event) =>
                    setConfig((current) => ({
                      ...current,
                      query: {
                        ...current.query,
                        regex_prefix: event.target.value,
                      },
                    }))
                  }
                />
              </label>
              <button type="button" onClick={() => void refreshSearchIndex()}>
                刷新索引
              </button>
              {refreshStatus ? <span className="settings-status">{refreshStatus}</span> : null}
            </fieldset>
            <fieldset className={settingsSection === "web" ? "" : "settings-section-muted"}>
              <legend>网页搜索</legend>
              <div className="engine-list">
                {Object.entries(config.web_search.engines).map(([prefix, engine]) => (
                  <div className="engine-row" key={prefix}>
                    <span>{prefix}</span>
                    <span>{engine.name}</span>
                    <span>{engine.url}</span>
                    <button type="button" onClick={() => removeEngine(prefix)}>
                      删除
                    </button>
                  </div>
                ))}
              </div>
              <button type="button" onClick={() => setEngineWizardOpen(true)}>
                新增搜索引擎
              </button>
            </fieldset>
            <fieldset className={settingsSection === "history" ? "" : "settings-section-muted"}>
              <legend>历史</legend>
              <label>
                输入历史条数
                <input
                  type="number"
                  value={config.history.input_max_entries}
                  min={0}
                  onChange={(event) =>
                    setConfig((current) => ({
                      ...current,
                      history: {
                        ...current.history,
                        input_max_entries: Number(event.target.value),
                      },
                    }))
                  }
                />
              </label>
            </fieldset>
            <fieldset className={settingsSection === "command" ? "" : "settings-section-muted"}>
              <legend>命令执行</legend>
              <label className="toggle-row">
                <input
                  aria-label="命令执行"
                  type="checkbox"
                  checked={effectiveCommandEnabled}
                  onChange={(event) =>
                    setConfig((current) => ({
                      ...current,
                      command: {
                        ...current.command,
                        enabled: event.target.checked,
                      },
                    }))
                  }
                />
                <span>命令执行</span>
              </label>
            </fieldset>
            <fieldset className={settingsSection === "appearance" ? "" : "settings-section-muted"}>
              <legend>外观与窗口</legend>
              <span>Compact</span>
            </fieldset>
            {engineWizardOpen ? (
              <section aria-label="新增搜索引擎" className="settings-dialog" role="dialog">
                <label>
                  搜索前缀
                  <input
                    value={engineDraft.prefix}
                    onChange={(event) =>
                      setEngineDraft((current) => ({ ...current, prefix: event.target.value }))
                    }
                  />
                </label>
                <label>
                  搜索名称
                  <input
                    value={engineDraft.name}
                    onChange={(event) =>
                      setEngineDraft((current) => ({ ...current, name: event.target.value }))
                    }
                  />
                </label>
                <label>
                  URL 模板
                  <input
                    value={engineDraft.url}
                    onChange={(event) =>
                      setEngineDraft((current) => ({ ...current, url: event.target.value }))
                    }
                  />
                </label>
                {engineError ? <span className="settings-error">{engineError}</span> : null}
                <div className="dialog-actions">
                  <button type="button" onClick={addEngineFromWizard}>
                    添加引擎
                  </button>
                  <button type="button" onClick={() => setEngineWizardOpen(false)}>
                    取消
                  </button>
                </div>
              </section>
            ) : null}
            <button type="button" className="primary-button" onClick={() => void saveSettings()}>
              保存设置
            </button>
          </form>
        </section>
      </main>
    );
  }

  return (
    <main className="launcher-shell" aria-label="QuickFox launcher">
      <section className="launcher-panel">
        <header className="panel-toolbar">
          <input
            className="search-input"
            aria-label="搜索文件、目录、计算器、网页搜索或命令"
            autoFocus
            value={query}
            onChange={(event) => updateQuery(event.target.value)}
            onKeyDown={handleKeyDown}
            placeholder="Search files, folders, calculator, web prefixes..."
          />
        </header>
        {isCommandMode ? (
          <section className="command-preview" aria-label="命令预览">
            <span className="result-title">
              {effectiveCommandEnabled ? commandText : "命令执行未启用"}
            </span>
            <span className="result-detail">
              {effectiveCommandEnabled ? "外部终端" : "设置中开启后可用"}
            </span>
            <button
              type="button"
              disabled={!effectiveCommandEnabled || !commandText}
              onClick={executeSelected}
            >
              确认执行
            </button>
          </section>
        ) : (
          <>
            {historyMode ? (
              <ul className="history-list" aria-label="输入历史">
                {inputHistory.map((item, index) => (
                  <li
                    aria-selected={index === (historyIndex ?? 0)}
                    className="history-item"
                    key={`${item}:${index}`}
                    role="option"
                  >
                    {item}
                  </li>
                ))}
              </ul>
            ) : null}
            {query.trim() ? (
              <ul className="result-list" aria-label="搜索结果">
                {results.length > 0
                  ? results.map((result, index) => (
                      <li
                        aria-selected={index === selectedIndex}
                        className="result-item"
                        key={result.id}
                        onContextMenu={(event) => {
                          event.preventDefault();
                          setMenuResultId(result.id);
                          setMenuPosition({ left: event.clientX, top: event.clientY });
                          setSelectedIndex(index);
                        }}
                        role="option"
                      >
                        <span className="result-title">{result.title}</span>
                        <span className="result-detail">{result.detail ?? ""}</span>
                      </li>
                    ))
                  : [
                      <li className="empty-state" key="empty-state">
                        {fileSearchStatusText(currentIndexStatus) ?? "未找到结果"}
                      </li>,
                    ]}
              </ul>
            ) : null}
          </>
        )}
        {menuResult ? (
          <div
            className="action-menu"
            role="menu"
            style={
              menuPosition
                ? {
                    left: `${menuPosition.left}px`,
                    top: `${menuPosition.top}px`,
                  }
                : undefined
            }
          >
            {menuResult.secondaryActions.map((item) => (
              <button
                key={item.label}
                role="menuitem"
                type="button"
                onClick={() => {
                  setMenuResultId(null);
                  setMenuPosition(null);
                  onExecuteAction(item.action);
                }}
              >
                {item.label}
              </button>
            ))}
          </div>
        ) : null}
      </section>
    </main>
  );
}

function linesFromTextarea(value: string) {
  return value
    .split("\n")
    .map((item) => item.trim())
    .filter(Boolean);
}

function indexStatusLabel(status: IndexStatus) {
  switch (status.kind) {
    case "building":
      return "文件索引正在建立";
    case "refreshing":
      return "文件索引正在更新";
    case "ready":
      return "文件索引可用";
    case "failed":
      return status.message ?? "文件索引构建失败";
    case "unbuilt":
      return "文件索引尚未建立";
  }
}

function fileSearchStatusText(status: IndexStatus) {
  if (status.kind === "building") {
    return "文件索引正在建立";
  }
  if (status.kind === "refreshing") {
    return "文件索引正在更新";
  }
  if (status.kind === "failed") {
    return status.message ?? "文件索引构建失败";
  }
  if (status.kind === "unbuilt") {
    return "文件索引尚未建立";
  }
  return null;
}
