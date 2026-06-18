# QuickFox 大规模本地索引性能设计

日期：2026-06-18
状态：已写入 OpenSpec，等待维护者 review
对应 OpenSpec：`openspec/changes/support-large-scale-local-index-performance/`

## 目标

QuickFox 以 Windows C/D 多盘、约 200 万文件的开发机作为正式支持基准。目标是在海量文件下仍保持稳定输入体验、可控内存和可信结果，而不是要求用户缩小索引范围才能避免卡顿。

核心预算：

- 普通 name/path 查询 P95 小于 50ms。
- 极端低命中或命中靠后查询小于 100ms。
- 200 万 entry 常驻 name/path 搜索索引目标小于 500MB，硬上限小于 800MB。
- `agents.md`、`agents.m`、`agents` 这类精确/前缀查询必须稳定把目标文件排到前 5 位。

## 架构方向

当前 `Vec<IndexedEntry>` 加每次输入线性扫描的模型不能满足 200 万文件目标。新的方向是紧凑内存索引加候选召回：

```text
SQLite Snapshot / Scanner
  ↓
CompactSearchIndex
  ├─ StringPool
  ├─ EntryTable
  ├─ NameTokenIndex
  ├─ PrefixIndex
  ├─ ExtensionIndex
  └─ PathSegmentIndex
        ↓
CandidateRetriever
        ↓
Small Candidate Set
        ↓
Matcher + QuickFox Ranker
        ↓
SearchResult
```

搜索主路径不再对所有 entry 执行完整 matcher。查询先解析成结构化 `FileQuery`，再通过 token、prefix、extension、path segment 等索引召回有限候选，最后只对候选做 fuzzy 和排序。

## 分阶段实现

第一阶段先止血，移除确定的内存放大点：per-entry accepted scan event、完整 entries report、刷新过程大对象 clone、重复 search text、启动 snapshot 多份 entries。

第二阶段引入 `CompactSearchIndex`，与旧搜索路径并行，通过 oracle 测试对比结果质量。

第三阶段切换普通 name/path 搜索主路径到候选召回架构，并加入后端查询取消、前端防抖一致化和状态事件节流。

第四阶段收紧内容索引内存模型，不再为所有文本文件长期保存全文和 lines 副本；snippet 改为按需读取或受限 cache。

## 测试策略

测试是本变更的核心交付，不是收尾工作。

- 单元测试覆盖 query planner、candidate retriever、string pool、entry table、ranker、snapshot migration、取消逻辑。
- Synthetic benchmark 覆盖 100,000、500,000、2,000,000 entry，查询包括 `agents.md`、`agents.m`、`type:md agents`、`dir:workspace agents`、低命中随机词和路径段 fuzzy。
- 内存测试输出 entry table、string pool、倒排索引和内容 snippet cache 的预算估算或采样。
- 前端测试覆盖连续输入、过期响应、索引状态 revision、防抖和搜索中状态。
- Windows 手工验收覆盖 C/D 多盘约 500GB、发布构建、任务管理器内存、真实输入录屏、索引状态截图和结果质量。

## 风险

紧凑索引会增加实现复杂度，因此必须用旧搜索路径做 oracle，确保精确名称、前缀、字段过滤和高质量 fuzzy 结果不退化。200 万 benchmark 不适合每次普通 CI 都跑完整规模，因此 CI 跑可承受规模，2,000,000 entry 作为 ignored benchmark 或手动 workflow 加 Windows 发布前验收。
