## Why

Windows 默认多盘索引会显著放大首次扫描成本；当前自研递归扫描、线性内存匹配和“重新扫描式增量刷新”已经开始影响启动后的可用速度和用户信任。同时，QuickFox 现在只能按文件名/路径找文件，无法用 `type:`、`dir:`、`name:` 精确收窄，也无法在用户明确输入 `content:` 时搜索文件内容。需要把文件索引从原型级扫描升级为可观测、可分批、可增量、可全文扩展的索引管线。

## What Changes

- 将索引扫描从自研 `fs::read_dir` 递归迁移到成熟目录遍历组件，优先采用 `ignore::WalkBuilder`，利用并行遍历、忽略规则和 `.gitignore`/`.ignore` 支持。
- 扩展索引条目元信息，保存用于增量判断、字段查询和排序的轻量属性，例如 mtime、size、parent、extension、depth、root/drive。
- 将首次索引改为分批可用：优先索引应用入口和常用目录，再后台补齐配置目录和 Windows 剩余本地盘符。
- 增强索引状态可观测性，暴露 scanned/accepted/skipped/failure counts、当前阶段和当前根目录摘要。
- 接入运行期 `notify` watcher 与 debounce 队列，在应用运行时对已索引根目录进行批量增量更新；监听失败时回退后台刷新。
- 引入结构化查询解析，支持普通词、双引号字段值，以及 `type:`、`name:`、`dir:`、`content:` 等字段条件组合。
- 引入 Tantivy 作为内容全文索引。默认先做文本文件抽取边界，PDF/Office 等二进制文档留给后续 extractor 扩展；`content:` 命中结果返回匹配片段上下 5 行和结构化高亮范围。
- 使用 `nucleo` 或等价可替换 matcher 优化普通 name/path fuzzy 匹配，但最终排序仍由 QuickFox ranker 结合类型、历史和内容分数控制。

## Capabilities

### New Capabilities

- `search-index`: 新增字段查询和 `content:` 文件内容搜索能力。

### Modified Capabilities

- `search-index`: 索引扫描、增量刷新、状态进度、默认优先级和搜索性能要求变化。
- `configuration-and-history`: 索引相关配置需要支持扫描性能参数、优先目录、忽略规则来源、内容索引范围和内容索引大小限制。

## Impact

- Rust core: `src-tauri/src/core/index.rs`、`storage.rs`、`providers.rs`、`lib.rs` 的索引管线、状态 payload、快照 schema。
- Dependencies: 计划新增 `ignore`、`notify`/debounce、`tantivy`，并评估/接入 `nucleo` 作为普通 fuzzy matcher。
- Storage: SQLite 文件索引快照 schema 需要迁移，Tantivy 内容索引需要独立目录和版本管理。
- Frontend: 设置页/启动器索引状态需要展示分批进度、当前阶段摘要、字段查询说明和内容片段。
- QA: Windows 多盘、大文件树、权限失败、网络/外接盘、`.gitignore` 行为、内容索引范围和字段查询组合需要补充手工验收。
