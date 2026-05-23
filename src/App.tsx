import { type KeyboardEvent, useMemo, useState } from "react";

type LauncherAction =
  | { type: "openPath"; path: string }
  | { type: "openContainingFolder"; path: string }
  | { type: "copyText"; text: string }
  | { type: "executeCommand"; command: string; requiresConfirmation: boolean };

type LauncherResult = {
  id: string;
  title: string;
  detail: string;
  keywords: string;
  primaryAction: LauncherAction;
  secondaryActions: Array<{ label: string; action: LauncherAction }>;
};

type AppProps = {
  commandEnabled?: boolean;
  initialView?: "launcher" | "settings";
  onClose?: () => void;
  onExecuteAction?: (action: LauncherAction) => void;
};

const initialResults: LauncherResult[] = [
  {
    id: "documents",
    title: "Documents",
    detail: "~/Documents",
    keywords: "documents docs",
    primaryAction: { type: "openPath", path: "~/Documents" },
    secondaryActions: [
      { label: "打开所在目录", action: { type: "openContainingFolder", path: "~/Documents" } },
      { label: "复制路径", action: { type: "copyText", text: "~/Documents" } },
    ],
  },
  {
    id: "downloads",
    title: "Downloads",
    detail: "~/Downloads",
    keywords: "downloads down",
    primaryAction: { type: "openPath", path: "~/Downloads" },
    secondaryActions: [
      { label: "打开所在目录", action: { type: "openContainingFolder", path: "~/Downloads" } },
      { label: "复制路径", action: { type: "copyText", text: "~/Downloads" } },
    ],
  },
  {
    id: "readme",
    title: "README.md",
    detail: "~/README.md",
    keywords: "readme markdown",
    primaryAction: { type: "openPath", path: "~/README.md" },
    secondaryActions: [
      {
        label: "打开所在目录",
        action: { type: "openContainingFolder", path: "~/README.md" },
      },
      { label: "复制路径", action: { type: "copyText", text: "~/README.md" } },
    ],
  },
];

export function App({
  commandEnabled = false,
  initialView = "launcher",
  onClose = () => undefined,
  onExecuteAction = () => undefined,
}: AppProps) {
  const [query, setQuery] = useState("");
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [menuResultId, setMenuResultId] = useState<string | null>(null);

  const isCommandMode = commandEnabled && query.trim().startsWith(">");
  const commandText = query.trim().slice(1).trim();
  const results = useMemo(() => {
    const normalized = query.trim().toLowerCase();
    if (!normalized || normalized.startsWith(">")) {
      return initialResults;
    }

    return initialResults.filter((result) =>
      `${result.title} ${result.detail} ${result.keywords}`.toLowerCase().includes(normalized),
    );
  }, [query]);
  const selectedResult = results[Math.min(selectedIndex, Math.max(results.length - 1, 0))];
  const menuResult = results.find((result) => result.id === menuResultId);

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

  if (initialView === "settings") {
    return (
      <main className="launcher-shell" aria-label="QuickFox launcher">
        <section className="launcher-panel settings-panel">
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
        <input
          className="search-input"
          aria-label="搜索文件、目录、计算器、网页搜索或命令"
          autoFocus
          value={query}
          onChange={(event) => updateQuery(event.target.value)}
          onKeyDown={handleKeyDown}
          placeholder="Search files, folders, calculator, web prefixes..."
        />
        {isCommandMode ? (
          <section className="command-preview" aria-label="命令预览">
            <span className="result-title">{commandText}</span>
            <span className="result-detail">外部终端</span>
            <button type="button" onClick={executeSelected}>
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
                <span className="result-detail">{result.detail}</span>
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
