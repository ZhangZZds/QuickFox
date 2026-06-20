## Why

QuickFox 的目标不是小规模 demo，而是在 Windows C/D 多盘、约 200 万文件的开发机上仍保持稳定输入体验、可控内存和可信结果。当前文件搜索仍以大内存 `Vec` 和每次输入线性扫描为核心，已经在全盘索引场景暴露出输入卡顿和接近 2GB 的内存占用，需要把索引与查询内核升级为可基准验证的大规模本地搜索架构。

## What Changes

- 为文件搜索建立明确的大规模体验预算：200 万文件以内普通 name/path 查询 P95 小于 50ms，极端查询小于 100ms；常驻内存目标小于 500MB，硬上限小于 800MB。
- 将普通文件搜索从“每次查询全量遍历条目”改为候选召回加小候选集排序：先通过 name token、prefix、extension、path segment 等紧凑索引召回，再执行 fuzzy/ranker。
- 重构内存索引模型，减少重复字符串、重复 search text、全量 report、per-entry scan event 和刷新过程中的大对象 clone。
- 保留 Windows 多盘和大配置目录作为正式支持场景，而不是要求用户缩小索引范围才能获得可用体验。
- 将内容索引的常驻内存边界纳入设计：`content:` 仍可搜索文本内容，但内容正文和 snippet 不应以全量重复副本长期驻留内存。
- 建立大规模性能测试体系，包括可复现 synthetic fixture、matcher micro-benchmark、内存预算估算/采样、回归阈值和 Windows 手工验收清单。
- 调整查询执行与前端交互，使连续输入时旧查询不会压住最新查询，索引刷新状态更新不会绕过防抖触发昂贵搜索风暴。

## Capabilities

### New Capabilities

- `large-scale-search-performance`: 定义 QuickFox 在 200 万文件级别下的查询延迟、内存预算、基准测试和验收要求。

### Modified Capabilities

- `search-index`: 普通 name/path 搜索、索引内存模型、内容索引内存边界和 Windows 多盘索引行为需要升级到大规模数据场景。
- `project-quality`: 增加性能回归测试、基准夹具和跨平台验收作为发布质量门槛。

## Impact

- Rust core: `src-tauri/src/core/index.rs`、`file_matcher.rs`、`index_entry.rs`、`index_scanner.rs`、`content_index.rs`、`storage.rs`、`providers.rs` 和 `src-tauri/src/lib.rs` 的索引结构、查询路径、刷新进度、内容 snippet 和状态事件。
- Frontend: `src/App.tsx` 的搜索防抖、索引状态触发搜索、设置页性能模式文案和大索引状态反馈。
- Storage: SQLite snapshot schema 可能需要保存更紧凑的字段和索引元信息；内容索引目录与 snippet 数据需要版本化。
- Tests: Rust 单元测试、ignored benchmark、性能阈值测试、内存预算测试、前端防抖/过期结果测试，以及 Windows C/D 多盘手工验收。
- Dependencies: 可以继续使用 `ignore`、`tantivy`、`nucleo-matcher`，但需要评估是否引入额外紧凑索引结构或 benchmark 工具。
