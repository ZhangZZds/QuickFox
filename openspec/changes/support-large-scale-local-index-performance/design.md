## Context

QuickFox 当前索引管线已经有分阶段扫描、SQLite snapshot、字段查询和内容索引边界，但普通 name/path 搜索仍以 `SearchIndex.entries: Vec<IndexedEntry>` 为主路径。每次输入都会解析查询、线性遍历 entries，并在 matcher 中重复处理 name/path 字符串。这个模型在小索引下简单可靠，但在 Windows C/D 多盘、约 200 万文件的目标场景中，查询成本和内存占用都会随索引规模线性放大。

本变更把“200 万文件以内的 Windows 多盘开发机”作为正式产品基准。目标不是通过缩小默认索引范围来绕开问题，而是让 QuickFox 支持海量 name/path 索引，并在内容索引、状态事件和前端搜索调度上保持同样的性能纪律。

## Goals / Non-Goals

**Goals:**

- 普通 name/path 查询在 200 万 entry synthetic index 上 P95 小于 50ms，极端低命中查询小于 100ms。
- 200 万 entry 的常驻 name/path 索引目标小于 500MB，硬上限小于 800MB。
- 搜索主路径从全量扫描改为“候选召回 -> 小候选集评分 -> ranker 排序”。
- 扫描、快照加载、刷新进度和内容索引构建不保留与 entry 数量等比例的重复大对象。
- 建立自动化性能回归测试和 Windows 真实多盘验收清单，让后续改动不能悄悄退化。
- 保持现有用户可见语义：普通查询仍查 name/path，字段查询和 `content:` 仍按已有设计工作，排序仍由 QuickFox ranker 统一控制。

**Non-Goals:**

- 不在本变更中实现第三方插件 API。
- 不在本变更中扩展 PDF/Office 内容抽取。
- 不要求首次全盘扫描瞬间完成；全盘扫描可以后台渐进，但不能阻塞输入和非文件 Provider。
- 不以牺牲结果质量为代价简单截断搜索；精确文件名和高质量路径命中必须稳定靠前。

## Decisions

### 1. 采用紧凑内存索引作为普通搜索主路径

普通查询不再直接遍历完整 `Vec<IndexedEntry>`。新的 `CompactSearchIndex` 保存紧凑 entry table、字符串池和多个候选召回索引：

- `StringPool`: 去重保存 path、name、parent/root/path segment 等字符串。
- `EntryTable`: 使用整数 id 引用字符串池，保存 kind、extension id、depth、mtime/size 等轻量字段。
- `NameTokenIndex`: 文件名 token 到 entry id 列表的倒排索引。
- `PrefixIndex`: name/path segment 前缀到候选 entry id 列表，用于输入逐渐变长时快速收敛。
- `ExtensionIndex`: extension 到 entry id 列表，服务 `type:`。
- `PathSegmentIndex`: path segment 到 entry id 列表，服务普通路径词和 `dir:` 初筛。

查询时先由 `CandidateRetriever` 根据结构化 `FileQuery` 求交/求并得到有限候选，再对候选执行 fuzzy/contains/ranker。候选数量应有配置化硬上限和质量阈值；低命中查询也不应退化到全量扫描。

备选方案是继续优化 `Vec` 线性扫描，或让 SQLite/FTS 直接承载每次按键查询。线性扫描改动小但无法满足 200 万 entry 的低命中查询目标；SQLite/FTS 内存低但交互延迟和 fuzzy 排序控制更难稳定。紧凑内存索引更适合启动器的高频输入模型，SQLite 继续负责持久化和冷启动恢复。

### 2. 先做止血，再替换搜索主路径

第一阶段先移除确定的内存放大点：

- 不再为每个 accepted entry 长期保留 `EntryAccepted` scan event。
- `last_report` 不保留完整 entries，改为摘要和 failures。
- 刷新进度不 clone 整个 aggregate report 到主线程。
- 去掉 `IndexedEntry.search_text` 与 `SearchIndex.search_texts` 的重复持久化/内存副本，或迁移到紧凑索引内的派生字段。
- 启动 snapshot 加载避免同时持有 snapshot entries、report entries 和 index entries 多份副本。

这些改动不能替代搜索架构升级，但可以快速降低 1880MB 这类内存峰值，并减少刷新时 UI 卡顿风险。

### 3. 查询调度支持取消和最新查询优先

前端防抖只负责减少请求数量，后端仍必须保证旧查询不会压住最新查询。搜索命令应带 query generation 或 request id；后端在候选召回/评分边界检查取消信号，前端忽略过期响应。索引状态事件不能绕过防抖触发昂贵搜索风暴；状态更新只标记 revision，当前查询按同一防抖策略重跑。

### 4. 内容索引不常驻全文副本

Tantivy 可以继续作为 `content:` 检索底座，但 QuickFox 不应在内存中为所有内容文件保存全文和 lines 副本。内容命中 snippet 应优先通过以下方式之一生成：

- 保存命中文档路径和必要 offset，按需读取小范围内容。
- 保存受限大小的 snippet cache，而不是全文 cache。
- 对内容索引目录建立单独内存预算；超过预算时仍保留 Tantivy 检索，但 snippet 降级为延迟生成或摘要反馈。

### 5. 测试先于实现，并覆盖最坏路径

本变更所有行为变化按 TDD 执行。测试不只覆盖“命中很早”的乐观场景，还必须覆盖低命中、命中靠后、路径段 fuzzy、字段过滤组合和连续输入取消。性能测试分层：

- 单元测试：query planner、candidate retriever、ranker、string pool、snapshot migration、取消逻辑。
- Synthetic benchmark：构造 10 万、50 万、200 万 entry 的 deterministic index，覆盖 `agents.md`、`agents.m`、`type:md agents`、`dir:workspace agents`、低命中随机词等查询。
- 内存预算测试：对 entry table、string pool 和倒排索引估算或采样，输出可比较指标。
- 前端测试：防抖、状态 revision、过期响应、空结果和搜索中状态。
- Windows 手工验收：C/D 多盘约 500GB、发布构建、真实输入录屏、任务管理器内存、索引状态和结果质量。

## Risks / Trade-offs

- [Risk] 紧凑索引实现复杂，容易引入排序或匹配语义变化。 → Mitigation: 保留现有 matcher/ranker 作为 oracle，在中小 fixture 上做新旧结果对比测试；只有候选召回负责缩小集合，最终排序仍由 QuickFox ranker 控制。
- [Risk] 倒排索引可能增加内存而不是减少内存。 → Mitigation: 每个索引结构必须有预算测试；优先使用整数 id、排序 vec、共享字符串池和按需构建的字段索引。
- [Risk] 200 万 entry benchmark 在普通 CI 上耗时过长。 → Mitigation: CI 跑 10 万/50 万阈值测试，200 万作为 ignored benchmark 和 Windows 发布前验收；性能输出保存在文档中做趋势比较。
- [Risk] Windows 文件系统和权限差异导致 synthetic benchmark 与真实机器不一致。 → Mitigation: synthetic 只验证查询/内存模型，真实扫描和 watcher 由 Windows 手工验收与可自动化小型集成测试覆盖。
- [Risk] 内容 snippet 按需读取可能遇到文件变化或权限失败。 → Mitigation: snippet 生成失败时返回内容命中摘要，不影响 name/path 搜索和基础 content 结果。

## Migration Plan

1. 增加基准夹具和当前实现的失败/退化证明，先记录现状。
2. 移除 scan event、report、clone 和 search text 重复存储等内存放大点。
3. 引入 `CompactSearchIndex` 的数据结构和构建测试，但先与旧 `SearchIndex` 并行。
4. 实现 `CandidateRetriever`，用 oracle 测试对比旧搜索结果。
5. 切换普通 name/path 搜索主路径，保留 feature boundary 或内部 fallback 方便回退。
6. 调整内容索引 snippet 内存模型。
7. 加入前端/后端查询取消和状态防抖测试。
8. 完成 Windows 多盘手工验收，更新 QA 文档和性能基线。

## Open Questions

- 200 万 entry benchmark 是否在专门的本地命令中运行，还是增加可选 CI job 手动触发？
- 内存预算采用 jemalloc/Windows API 采样，还是先用结构化估算加发布版任务管理器验收？
- `nucleo-matcher` 是否继续只用于小候选集评分，还是引入它的 snapshot/threadpool 模型作为候选召回的一部分？
