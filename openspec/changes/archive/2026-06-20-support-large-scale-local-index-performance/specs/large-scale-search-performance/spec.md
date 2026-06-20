## ADDED Requirements

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
