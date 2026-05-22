const initialResults = [
  { id: "documents", title: "Documents", detail: "~/Documents" },
  { id: "downloads", title: "Downloads", detail: "~/Downloads" },
  { id: "command", title: "> git status", detail: "命令执行默认关闭" },
];

export function App() {
  return (
    <main className="launcher-shell" aria-label="QuickFox launcher">
      <section className="launcher-panel">
        <input
          className="search-input"
          aria-label="搜索文件、目录、计算器、网页搜索或命令"
          autoFocus
          placeholder="Search files, folders, calculator, web prefixes..."
        />
        <ul className="result-list" aria-label="搜索结果">
          {initialResults.map((result) => (
            <li className="result-item" key={result.id}>
              <span className="result-title">{result.title}</span>
              <span className="result-detail">{result.detail}</span>
            </li>
          ))}
        </ul>
      </section>
    </main>
  );
}
