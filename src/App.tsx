import { type KeyboardEvent, useEffect, useState } from "react";

import { executeAction, search as searchResults } from "./tauriClient";

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
  onExecuteAction?: (action: LauncherAction) => void;
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
  commandEnabled = false,
  initialView = "launcher",
  onClose = () => undefined,
  onExecuteAction = executeAction,
}: AppProps) {
  const [view, setView] = useState<"launcher" | "settings">(initialView);
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<LauncherResult[]>([]);
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [menuResultId, setMenuResultId] = useState<string | null>(null);

  const isCommandQuery = query.trim().startsWith(">");
  const isCommandMode = isCommandQuery;
  const commandText = query.trim().slice(1).trim();
  const selectedResult = results[Math.min(selectedIndex, Math.max(results.length - 1, 0))];
  const menuResult = results.find((result) => result.id === menuResultId);

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
    setMenuResultId(null);
  };

  const executeSelected = () => {
    if (isCommandMode && commandText) {
      onExecuteAction({
        type: "executeCommand",
        command: commandText,
        requiresConfirmation: true,
      });
      return;
    }

    if (selectedResult) {
      onExecuteAction(selectedResult.primaryAction);
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
      setSelectedIndex((index) => Math.min(index + 1, Math.max(results.length - 1, 0)));
      return;
    }

    if (event.key === "ArrowUp") {
      event.preventDefault();
      setSelectedIndex((index) => Math.max(index - 1, 0));
      return;
    }

    if (event.key === "Enter") {
      event.preventDefault();
      executeSelected();
    }
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
          <form aria-label="基础设置" className="settings-form">
            <label>
              索引目录
              <textarea defaultValue={"~/Documents\n~/Downloads"} />
            </label>
            <label>
              正则前缀
              <input defaultValue="re:" />
            </label>
            <label className="toggle-row">
              <input aria-label="命令执行" type="checkbox" defaultChecked={commandEnabled} />
              <span>命令执行</span>
            </label>
            <label>
              命令历史条数
              <input type="number" defaultValue={15} min={0} />
            </label>
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
          <button type="button" className="toolbar-button" onClick={() => setView("settings")}>
            打开设置
          </button>
        </header>
        {isCommandMode ? (
          <section className="command-preview" aria-label="命令预览">
            <span className="result-title">{commandEnabled ? commandText : "命令执行未启用"}</span>
            <span className="result-detail">
              {commandEnabled ? "外部终端" : "设置中开启后可用"}
            </span>
            <button
              type="button"
              disabled={!commandEnabled || !commandText}
              onClick={executeSelected}
            >
              确认执行
            </button>
          </section>
        ) : (
          <ul className="result-list" aria-label="搜索结果">
            {results.map((result, index) => (
              <li
                aria-selected={index === selectedIndex}
                className="result-item"
                key={result.id}
                onContextMenu={(event) => {
                  event.preventDefault();
                  setMenuResultId(result.id);
                  setSelectedIndex(index);
                }}
                role="option"
              >
                <span className="result-title">{result.title}</span>
                <span className="result-detail">{result.detail ?? ""}</span>
              </li>
            ))}
          </ul>
        )}
        {menuResult ? (
          <div className="action-menu" role="menu">
            {menuResult.secondaryActions.map((item) => (
              <button
                key={item.label}
                role="menuitem"
                type="button"
                onClick={() => {
                  setMenuResultId(null);
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
