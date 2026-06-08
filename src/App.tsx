import { type KeyboardEvent, useEffect, useRef, useState } from "react";

import {
  appPaths,
  executeAction,
  globalHotkeyStatus,
  listenGlobalHotkeyStatus,
  indexStatus,
  listenIndexStatus,
  loadConfig,
  listenOpenSettings,
  openSettingsWindow,
  recentInputHistory,
  recordInputHistory,
  refreshIndex,
  returnToLauncherWindow,
  type AppPaths,
  type GlobalHotkeyStatus,
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
  | {
      type: "openWithApplication";
      path: string;
      application: "developmentTool" | "systemChooser";
    }
  | { type: "executeCommand"; command: string; requiresConfirmation: boolean };

type LauncherResult = {
  id: string;
  title: string;
  detail?: string | null;
  kind: BackendSearchResult["kind"];
  primaryAction: LauncherAction;
  secondaryActions: Array<{ label: string; action: LauncherAction }>;
};

type LauncherStatusFeedback = {
  title: string;
  message: string;
  detail?: string | null;
  actions: Array<"refreshIndex" | "openSettings">;
};

type LauncherPresentation =
  | { mode: "empty" }
  | { mode: "command" }
  | { mode: "history" }
  | { mode: "results" }
  | { mode: "status"; status: LauncherStatusFeedback };

type BackendSearchResult = {
  id: string;
  title: string;
  detail?: string | null;
  kind: "application" | "file" | "directory" | "calculator" | "webSearch" | "command" | "feedback";
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

const launcherInputPlaceholder = "搜索文件、文件夹、计算器；g 关键词搜网页，re: 正则，> 命令";

function labelForAction(action: LauncherAction, resultKind?: BackendSearchResult["kind"]) {
  switch (action.type) {
    case "openPath":
      if (resultKind === "application") {
        return "打开应用";
      }
      if (resultKind === "directory") {
        return "打开文件夹";
      }
      return "打开";
    case "openContainingFolder":
      return "打开所在目录";
    case "copyText":
      return "复制路径";
    case "openUrl":
      return "打开链接";
    case "openWithApplication":
      return "选择打开方式";
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
    kind: result.kind,
    primaryAction: result.mainAction,
    secondaryActions: result.secondaryActions.map((action) => ({
      label: labelForAction(action, result.kind),
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
  const [indexSearchRevision, setIndexSearchRevision] = useState(0);
  const [currentAppPaths, setCurrentAppPaths] = useState<AppPaths>({
    configFilePath: null,
    indexSnapshotPath: null,
  });
  const [currentHotkeyStatus, setCurrentHotkeyStatus] = useState<GlobalHotkeyStatus>({
    enabled: false,
    message: "Shift+Shift 全局唤醒监听启动中",
  });
  const [settingsSection, setSettingsSection] = useState<
    "index" | "web" | "history" | "command" | "appearance"
  >("index");
  const [engineWizardOpen, setEngineWizardOpen] = useState(false);
  const [engineDraft, setEngineDraft] = useState({ prefix: "", name: "", url: "" });
  const [engineError, setEngineError] = useState<string | null>(null);
  const resultRefs = useRef<Array<HTMLLIElement | null>>([]);
  const historyRefs = useRef<Array<HTMLLIElement | null>>([]);
  const settingsContentRef = useRef<HTMLDivElement | null>(null);
  const effectiveCommandEnabled = commandEnabled ?? config.command.enabled;
  const hotkeyPermissionSettingsUrl = currentHotkeyStatus.permissionSettingsUrl;

  const isCommandQuery = query.trim().startsWith(">");
  const isCommandMode = isCommandQuery;
  const commandText = query.trim().slice(1).trim();
  const selectedResult = results[Math.min(selectedIndex, Math.max(results.length - 1, 0))];
  const menuResult = results.find((result) => result.id === menuResultId);
  const launcherPresentation = buildLauncherPresentation({
    historyMode,
    indexStatus: currentIndexStatus,
    isCommandMode,
    query,
    results,
  });

  useEffect(() => {
    let cancelled = false;
    void Promise.all([loadConfig(), recentInputHistory(), indexStatus(), globalHotkeyStatus()])
      .then(([nextConfig, nextHistory, nextIndexStatus, nextHotkeyStatus]) => {
        if (!cancelled) {
          setConfig(nextConfig as QuickFoxConfig);
          setInputHistory(nextHistory as string[]);
          setCurrentIndexStatus(nextIndexStatus as IndexStatus);
          setCurrentHotkeyStatus(nextHotkeyStatus as GlobalHotkeyStatus);
        }
      })
      .catch(() => undefined);

    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    let cancelled = false;
    void appPaths()
      .then((paths) => {
        if (!cancelled) {
          setCurrentAppPaths(paths as AppPaths);
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
    let dispose: (() => void) | undefined;
    void listenGlobalHotkeyStatus(setCurrentHotkeyStatus).then((unlisten) => {
      dispose = unlisten;
    });

    return () => {
      dispose?.();
    };
  }, []);

  useEffect(() => {
    let dispose: (() => void) | undefined;
    void listenIndexStatus((nextStatus) => {
      setCurrentIndexStatus(nextStatus);
      setIndexSearchRevision((revision) => revision + 1);
    }).then((unlisten) => {
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
  }, [indexSearchRevision, isCommandQuery, query]);

  useEffect(() => {
    if (!historyMode && results.length > 0) {
      scrollElementIntoView(resultRefs.current[selectedIndex]);
    }
  }, [historyMode, results.length, selectedIndex]);

  useEffect(() => {
    if (historyMode && historyIndex !== null) {
      scrollElementIntoView(historyRefs.current[historyIndex]);
    }
  }, [historyIndex, historyMode]);

  useEffect(() => {
    const element = settingsContentRef.current;
    if (typeof element?.scrollTo === "function") {
      element.scrollTo({ top: 0 });
    }
  }, [settingsSection]);

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
    await refreshIndex();
    const nextStatus = (await indexStatus()) as IndexStatus;
    setCurrentIndexStatus(nextStatus);
    setIndexSearchRevision((revision) => revision + 1);
    setRefreshStatus("索引已刷新");
  };

  const openSettingsFromLauncher = async () => {
    try {
      await openSettingsWindow();
    } catch {
      setView("settings");
    }
  };

  const returnToLauncherFromSettings = async () => {
    try {
      await returnToLauncherWindow();
    } catch {
      setView("launcher");
    }
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

  const openHotkeyPermissionSettings = () => {
    if (!hotkeyPermissionSettingsUrl) {
      return;
    }

    void onExecuteAction({
      type: "openUrl",
      url: hotkeyPermissionSettingsUrl,
    });
  };

  if (view === "settings") {
    return (
      <main className="launcher-shell" aria-label="QuickFox launcher">
        <section className="launcher-panel settings-panel">
          <header className="panel-toolbar settings-toolbar">
            <button
              type="button"
              className="toolbar-button"
              onClick={() => void returnToLauncherFromSettings()}
            >
              返回搜索
            </button>
            <h1>设置</h1>
            <span aria-hidden="true" />
          </header>
          <form aria-label="设置" className="settings-form">
            <div className="settings-sidebar">
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
              <section aria-label="设置操作" className="settings-actions">
                <button
                  type="button"
                  className="primary-button"
                  onClick={() => void saveSettings()}
                >
                  保存设置
                </button>
              </section>
            </div>
            <div
              className="settings-content"
              role="region"
              aria-label="设置内容"
              ref={settingsContentRef}
            >
              <fieldset
                className={
                  settingsSection === "index"
                    ? "settings-section settings-section--index"
                    : "settings-section settings-section--index settings-section-muted"
                }
              >
                <legend>索引</legend>
                <header className="settings-index-header" role="region" aria-label="索引工作区标题">
                  <div>
                    <h2>索引</h2>
                    <p>管理文件搜索范围、排除规则和索引状态。</p>
                  </div>
                  <div className="settings-index-refresh">
                    <button type="button" onClick={() => void refreshSearchIndex()}>
                      刷新索引
                    </button>
                    {refreshStatus ? (
                      <span className="settings-status">{refreshStatus}</span>
                    ) : null}
                  </div>
                </header>
                <IndexStatusSummary status={currentIndexStatus} />
                <div className="settings-index-workspace">
                  <section aria-label="主规则编辑" className="settings-index-column">
                    <span className="settings-column-label">主规则编辑</span>
                    <label className="settings-field">
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
                    <label className="settings-field">
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
                    <label className="settings-field">
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
                  </section>
                  <section aria-label="辅助信息" className="settings-index-column">
                    <span className="settings-column-label">辅助信息</span>
                    <label className="settings-field">
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
                    <IndexAuxiliaryDetails status={currentIndexStatus} appPaths={currentAppPaths} />
                  </section>
                </div>
              </fieldset>
              <fieldset
                className={
                  settingsSection === "web" ? "settings-section" : "settings-section-muted"
                }
              >
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
              <fieldset
                className={
                  settingsSection === "history" ? "settings-section" : "settings-section-muted"
                }
              >
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
              <fieldset
                className={
                  settingsSection === "command" ? "settings-section" : "settings-section-muted"
                }
              >
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
              <fieldset
                className={
                  settingsSection === "appearance" ? "settings-section" : "settings-section-muted"
                }
              >
                <legend>外观与窗口</legend>
                <span>Compact</span>
                <div
                  className={
                    hotkeyPermissionSettingsUrl
                      ? "settings-meta-row settings-meta-row--action"
                      : "settings-meta-row"
                  }
                >
                  <span>全局唤醒</span>
                  <span>
                    {currentHotkeyStatus.message}
                    {hotkeyPermissionSettingsUrl ? (
                      <small>授权后需要重启 QuickFox，Shift+Shift 监听才会重新启动。</small>
                    ) : null}
                  </span>
                  {hotkeyPermissionSettingsUrl ? (
                    <button type="button" onClick={openHotkeyPermissionSettings}>
                      打开权限设置
                    </button>
                  ) : null}
                </div>
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
            </div>
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
            placeholder={launcherInputPlaceholder}
          />
        </header>
        {launcherPresentation.mode === "command" ? (
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
            {launcherPresentation.mode === "history" ? (
              <ul className="history-list" aria-label="输入历史">
                {inputHistory.map((item, index) => (
                  <li
                    aria-selected={index === (historyIndex ?? 0)}
                    className="history-item"
                    key={`${item}:${index}`}
                    onMouseEnter={() => setHistoryIndex(index)}
                    ref={(element) => {
                      historyRefs.current[index] = element;
                    }}
                    role="option"
                  >
                    {item}
                  </li>
                ))}
              </ul>
            ) : null}
            {launcherPresentation.mode === "results" || launcherPresentation.mode === "status" ? (
              <ul className="result-list" aria-label="搜索结果">
                {launcherPresentation.mode === "results" ? (
                  results.map((result, index) => (
                    <li
                      aria-selected={index === selectedIndex}
                      className={
                        index === selectedIndex
                          ? "result-item result-item--selected"
                          : "result-item"
                      }
                      key={result.id}
                      onContextMenu={(event) => {
                        event.preventDefault();
                        setMenuResultId(result.id);
                        setMenuPosition({ left: event.clientX, top: event.clientY });
                        setSelectedIndex(index);
                      }}
                      onMouseEnter={() => setSelectedIndex(index)}
                      ref={(element) => {
                        resultRefs.current[index] = element;
                      }}
                      role="option"
                    >
                      <span className="result-title-cell">
                        <ResultKindBadge kind={result.kind} />
                        <span className="result-title" title={result.title}>
                          <ResultTitle title={result.title} />
                        </span>
                      </span>
                      <span
                        aria-label={result.detail ? `完整路径 ${result.detail}` : undefined}
                        className="result-detail"
                        title={result.detail ?? undefined}
                      >
                        {summarizePath(result.detail)}
                      </span>
                    </li>
                  ))
                ) : (
                  <li className="empty-state" key="empty-state">
                    <LauncherStatus
                      status={launcherPresentation.status}
                      onOpenSettings={() => void openSettingsFromLauncher()}
                      onRefreshIndex={() => void refreshSearchIndex()}
                    />
                  </li>
                )}
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

function summarizePath(path?: string | null) {
  if (!path) {
    return "";
  }

  const separator = path.includes("\\") ? "\\" : "/";
  const parts = path.split(/[\\/]+/).filter(Boolean);
  if (parts.length <= 4) {
    return path;
  }

  const isAbsolutePosix = path.startsWith("/");
  const isUnc = path.startsWith("\\\\");
  const prefixCount = isUnc ? 2 : 2;
  const suffixCount = 2;
  const prefix = parts.slice(0, prefixCount).join(separator);
  const suffix = parts.slice(-suffixCount).join(separator);
  const leading = isAbsolutePosix ? separator : isUnc ? `${separator}${separator}` : "";

  return `${leading}${prefix}${separator}...${separator}${suffix}`;
}

function scrollElementIntoView(element: HTMLElement | null | undefined) {
  if (typeof element?.scrollIntoView === "function") {
    element.scrollIntoView({ block: "nearest" });
  }
}

function buildLauncherPresentation({
  historyMode,
  indexStatus,
  isCommandMode,
  query,
  results,
}: {
  historyMode: boolean;
  indexStatus: IndexStatus;
  isCommandMode: boolean;
  query: string;
  results: LauncherResult[];
}): LauncherPresentation {
  if (isCommandMode) {
    return { mode: "command" };
  }

  if (historyMode) {
    return { mode: "history" };
  }

  if (!query.trim()) {
    return { mode: "empty" };
  }

  if (results.length > 0) {
    return { mode: "results" };
  }

  return {
    mode: "status",
    status: launcherStatusForIndex(indexStatus),
  };
}

function launcherStatusForIndex(status: IndexStatus): LauncherStatusFeedback {
  switch (status.kind) {
    case "unbuilt":
      return {
        title: "文件搜索正在准备",
        message: "首次索引尚未建立，计算器和网页搜索仍可使用。",
        actions: ["refreshIndex", "openSettings"],
      };
    case "building":
      return {
        title: "文件索引正在建立",
        message: "请稍等片刻；计算器和网页搜索仍可使用。",
        actions: ["refreshIndex", "openSettings"],
      };
    case "refreshing":
      return {
        title: "文件索引正在更新",
        message: "正在刷新文件快照；计算器和网页搜索仍可使用。",
        actions: [],
      };
    case "failed":
      return {
        title: "文件索引构建失败",
        message: "计算器和网页搜索仍可使用，可刷新索引或打开设置调整范围。",
        detail: status.message,
        actions: ["refreshIndex", "openSettings"],
      };
    case "ready":
      return {
        title: "未找到结果",
        message: "可以换个关键词，或使用 g 关键词、re: 正则和 > 命令。",
        actions: [],
      };
  }
}

function LauncherStatus({
  onOpenSettings,
  onRefreshIndex,
  status,
}: {
  onOpenSettings: () => void;
  onRefreshIndex: () => void;
  status: LauncherStatusFeedback;
}) {
  return (
    <section aria-label="启动器状态" className="launcher-status">
      <div className="launcher-status-copy">
        <strong>{status.title}</strong>
        <span>{status.message}</span>
        {status.detail ? <small>{status.detail}</small> : null}
      </div>
      {status.actions.length > 0 ? (
        <div className="launcher-status-actions">
          {status.actions.includes("refreshIndex") ? (
            <button type="button" onClick={onRefreshIndex}>
              刷新索引
            </button>
          ) : null}
          {status.actions.includes("openSettings") ? (
            <button type="button" onClick={onOpenSettings}>
              打开设置
            </button>
          ) : null}
        </div>
      ) : null}
    </section>
  );
}

function ResultTitle({ title }: { title: string }) {
  const summary = summarizeTitle(title);
  if (!summary) {
    return title;
  }

  return (
    <>
      <span className="result-title-head">{summary.head}</span>
      <span className="result-title-ellipsis">...</span>
      <span className="result-title-tail">{summary.tail}</span>
    </>
  );
}

function ResultKindBadge({ kind }: { kind: BackendSearchResult["kind"] }) {
  const labels: Partial<Record<BackendSearchResult["kind"], string>> = {
    application: "应用",
    file: "文件",
    directory: "目录",
  };
  const label = labels[kind];
  if (!label) {
    return null;
  }

  return (
    <span aria-label={label} className={`result-kind result-kind--${kind}`}>
      {label.slice(0, 1)}
    </span>
  );
}

function summarizeTitle(title: string): { head: string; tail: string } | null {
  if (title.length <= 48) {
    return null;
  }

  const underscoreParts = title.split("_");
  const tail = underscoreParts.length >= 3 ? underscoreParts.slice(-2).join("_") : title.slice(-18);
  const head =
    underscoreParts.length >= 3 ? underscoreParts.slice(0, 2).join("_") : title.slice(0, 15);

  return { head, tail: tail.replace(/^_+/, "") };
}

function IndexStatusSummary({ status }: { status: IndexStatus }) {
  return (
    <section aria-label="索引状态摘要" className="settings-status-grid">
      <div className="settings-status-card">
        <span>状态</span>
        <strong>{indexStatusLabel(status)}</strong>
        {status.message ? <small>{status.message}</small> : null}
      </div>
      <div className="settings-status-card">
        <span>条目</span>
        <strong>{status.entryCount} 项</strong>
      </div>
      <div className="settings-status-card">
        <span>索引代次</span>
        <strong>第 {status.generation} 代</strong>
      </div>
      <div className="settings-status-card">
        <span>{status.kind === "failed" ? "失败摘要" : "最近完成"}</span>
        <strong>
          {status.completedAtMs
            ? `最近完成: ${formatCompletedAt(status.completedAtMs)}`
            : (status.message ?? "尚未完成")}
        </strong>
      </div>
    </section>
  );
}

function IndexAuxiliaryDetails({ status, appPaths }: { status: IndexStatus; appPaths: AppPaths }) {
  return (
    <div className="settings-auxiliary-list">
      <div className="settings-meta-row">
        <span>配置文件位置</span>
        <span title={appPaths.configFilePath ?? undefined}>
          {appPaths.configFilePath ?? "未找到"}
        </span>
      </div>
      <div className="settings-meta-row">
        <span>索引快照位置</span>
        <span title={appPaths.indexSnapshotPath ?? undefined}>
          {appPaths.indexSnapshotPath ?? "尚未创建"}
        </span>
      </div>
      <div className="settings-meta-row">
        <span>{status.kind === "failed" ? "失败摘要" : "状态摘要"}</span>
        <span title={status.message ?? undefined}>{status.message ?? "暂无失败"}</span>
      </div>
    </div>
  );
}

function formatCompletedAt(timestampMs: number) {
  return new Date(timestampMs).toLocaleString();
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
