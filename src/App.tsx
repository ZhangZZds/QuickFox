import { type KeyboardEvent, useEffect, useState } from "react";

import {
  executeAction,
  loadConfig,
  listenOpenSettings,
  recentInputHistory,
  recordInputHistory,
  refreshIndex,
  type QuickFoxConfig,
  saveConfig,
  search as searchResults,
} from "./tauriClient";

type LauncherAction =
  | { type: "openPath"; path: string }
  | { type: "openContainingFolder"; path: string }
  | { type: "copyText"; text: string }
  | { type: "openUrl"; url: string }
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
    case "executeCommand":
      return "确认执行";
  }
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
  const [refreshStatus, setRefreshStatus] = useState<string | null>(null);
  const effectiveCommandEnabled = commandEnabled ?? config.command.enabled;

  const isCommandQuery = query.trim().startsWith(">");
  const isCommandMode = isCommandQuery;
  const commandText = query.trim().slice(1).trim();
  const selectedResult = results[Math.min(selectedIndex, Math.max(results.length - 1, 0))];
  const menuResult = results.find((result) => result.id === menuResultId);

  useEffect(() => {
    let cancelled = false;
    void Promise.all([loadConfig(), recentInputHistory()])
      .then(([nextConfig, nextHistory]) => {
        if (!cancelled) {
          setConfig(nextConfig as QuickFoxConfig);
          setInputHistory(nextHistory as string[]);
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
    setMenuResultId(null);
    setMenuPosition(null);
  };

  const executeSelected = async () => {
    const executedInput = query.trim();
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
    if (event.key === "Escape") {
      event.preventDefault();
      onClose();
      return;
    }

    if (event.key === "ArrowDown") {
      event.preventDefault();
      if (results.length === 0 && inputHistory.length > 0) {
        const nextIndex =
          historyIndex === null ? inputHistory.length - 1 : Math.max(historyIndex - 1, 0);
        setHistoryIndex(nextIndex);
        setQuery(inputHistory[nextIndex]);
        return;
      }
      setSelectedIndex((index) => Math.min(index + 1, Math.max(results.length - 1, 0)));
      return;
    }

    if (event.key === "ArrowUp") {
      event.preventDefault();
      if (results.length === 0 && inputHistory.length > 0) {
        const nextIndex =
          historyIndex === null ? 0 : Math.min(historyIndex + 1, inputHistory.length - 1);
        setHistoryIndex(nextIndex);
        setQuery(inputHistory[nextIndex]);
        return;
      }
      setSelectedIndex((index) => Math.max(index - 1, 0));
      return;
    }

    if (event.key === "Enter") {
      event.preventDefault();
      void executeSelected();
    }
  };

  const refreshSearchIndex = async () => {
    setRefreshStatus(null);
    await refreshIndex();
    setRefreshStatus("索引已刷新");
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
            <fieldset>
              <legend>搜索与索引</legend>
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
            <fieldset>
              <legend>网页搜索</legend>
              <span>g Google</span>
              <span>bd Baidu</span>
            </fieldset>
            <fieldset>
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
            <fieldset>
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
            <fieldset>
              <legend>外观与窗口</legend>
              <span>Compact</span>
            </fieldset>
            <button
              type="button"
              className="primary-button"
              onClick={() => void saveConfig(config).then(() => setView("launcher"))}
            >
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
                        未找到结果
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
