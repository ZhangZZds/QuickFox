## ADDED Requirements

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
