# large-scale-search-performance Specification

## Purpose

定义 QuickFox 在 200 万文件级 Windows 多盘场景下的查询延迟、内存预算、基准测试和验收要求。

## Requirements

### Requirement: 200 万文件级搜索体验预算

QuickFox SHALL treat a Windows C/D multi-drive development machine with up to 2,000,000 indexed file-system entries as a supported performance target.

#### Scenario: 普通查询延迟满足预算

- **WHEN** 系统使用 2,000,000 entry 的 deterministic synthetic index 执行普通 name/path 查询
- **THEN** 查询 P95 延迟小于 50ms
- **AND** 极端低命中或命中靠后的查询延迟小于 100ms

#### Scenario: 精确文件名查询稳定靠前

- **WHEN** synthetic index 中存在名为 `AGENTS.md` 或 `agents.md` 的文件
- **AND** 用户输入 `agents.md`、`agents.m` 或 `agents`
- **THEN** 目标文件出现在结果列表前 5 位
- **AND** 搜索不退化为全量 entry 线性扫描

### Requirement: 大索引内存预算

QuickFox SHALL keep the resident name/path search index memory bounded for large local file indexes.

#### Scenario: 200 万 entry 常驻内存受控

- **WHEN** 系统加载 2,000,000 entry 的 name/path index
- **THEN** 常驻搜索索引目标内存小于 500MB
- **AND** 常驻搜索索引硬上限小于 800MB

#### Scenario: 刷新过程不保留重复大对象

- **WHEN** 后台索引刷新正在聚合多个扫描阶段
- **THEN** 系统不长期保留完整 entries 的多份 clone
- **AND** 系统不为每个 accepted entry 长期保留包含完整路径的进度事件

### Requirement: 候选召回优先的查询执行

QuickFox SHALL execute ordinary file search through bounded candidate retrieval before fuzzy scoring and final ranking.

#### Scenario: 低命中查询不会全量扫描

- **WHEN** 用户输入一个只有少量结果或没有结果的普通查询
- **THEN** 系统通过 token、prefix、extension 或 path segment 索引召回有限候选
- **AND** 系统不会对所有 indexed entries 执行完整 matcher

#### Scenario: 字段过滤缩小候选集合

- **WHEN** 用户输入 `type:md agents` 或 `dir:workspace agents`
- **THEN** 系统先使用字段索引缩小候选集合
- **AND** 再对候选集合执行普通匹配和排序

### Requirement: 最新查询优先

QuickFox SHALL keep launcher input responsive during rapid typing by making stale searches cancellable or ignorable across frontend and backend boundaries.

#### Scenario: 连续输入只展示最新查询结果

- **WHEN** 用户快速连续输入 `a`、`ag`、`age`、`agen`、`agents.m`
- **THEN** 前端只展示最后一次查询对应的结果
- **AND** 较旧查询完成后不会覆盖最新结果

#### Scenario: 索引状态更新不触发搜索风暴

- **WHEN** 后台索引在用户输入期间连续发送状态更新
- **THEN** 启动器按统一防抖策略刷新当前查询
- **AND** 状态更新不会绕过防抖立即启动多次昂贵搜索

### Requirement: 大规模性能基准可复现

QuickFox SHALL provide deterministic performance fixtures and commands for large-index query, memory, and regression testing.

#### Scenario: synthetic benchmark 覆盖多个规模

- **WHEN** 维护者运行大索引 benchmark 命令
- **THEN** 测试覆盖至少 100,000、500,000 和 2,000,000 entry 规模
- **AND** 输出 query latency、candidate count、result count 和 memory estimate

#### Scenario: CI 覆盖可承受规模

- **WHEN** pull request 触发普通 CI
- **THEN** CI 至少运行可承受规模的大索引回归测试
- **AND** 2,000,000 entry benchmark 可作为 ignored test 或手动 workflow 运行

### Requirement: 大索引增量可见延迟预算

QuickFox SHALL 在 2,000,000 entry baseline 上以分层增量视图处理普通运行期文件变化，不得因单个小批次重建完整 compact baseline。

#### Scenario: 普通增量十秒内可搜索

- **WHEN** watcher 向 coordinator 交付普通创建、修改、删除或重命名事件
- **THEN** 对应 name/path 变化在 10 秒内进入可搜索视图
- **AND** 该批次不重建 2,000,000 entry compact baseline

#### Scenario: 小型增量层保持查询预算

- **WHEN** 2,000,000 entry baseline 同时存在不超过 10,000 条 overlay 与 tombstone
- **THEN** deterministic 普通 name/path 查询 P95 仍小于等于 50ms
- **AND** 查询结果正确应用 overlay 覆盖与 tombstone 过滤

### Requirement: 增量运行期资源有界

QuickFox SHALL 对 watcher channel、delta overlay、tombstone、journal 状态和失败摘要设置明确上限，并在达到安全阈值时降级到 dirty-root 校准或后台 baseline 刷新。

#### Scenario: watcher channel 不无限增长

- **WHEN** 文件系统事件产生速度超过 coordinator 消费速度
- **THEN** 内存中的 watcher channel 不超过 8192 个标准化事件
- **AND** 系统把受影响 root 标记为 dirty，而不是无限分配队列内存

#### Scenario: 增量层达到安全阈值

- **WHEN** overlay 与 tombstone 合计达到 50,000 条或估算内存达到 64 MiB
- **THEN** 系统安排后台 baseline 全量刷新
- **AND** 刷新完成前旧 baseline 与当前增量视图继续提供搜索

#### Scenario: 增量状态性能数据可复现

- **WHEN** 维护者运行大索引增量 benchmark
- **THEN** 输出 batch 条目数、提交耗时、baseline/overlay/tombstone 数量、查询延迟和估算内存
- **AND** fixture 覆盖创建、覆盖、删除子树和事件风暴降级
