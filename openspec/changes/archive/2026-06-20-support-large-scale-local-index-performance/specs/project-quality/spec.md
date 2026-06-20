## ADDED Requirements

### Requirement: 大规模性能回归测试

系统 SHALL 为文件搜索提供可重复运行的大规模性能回归测试，覆盖查询延迟、候选数量、结果质量和内存预算。

#### Scenario: 性能测试包含最坏查询路径

- **WHEN** 维护者运行文件搜索性能测试
- **THEN** 测试覆盖高命中、低命中、命中靠后、路径段 fuzzy、`agents.md` 精确文件名和字段过滤组合
- **AND** 测试输出每类查询的耗时和候选数量

#### Scenario: 性能退化阻止完成声明

- **WHEN** 变更影响索引、matcher、ranker、snapshot 或内容索引
- **THEN** 完成前验证必须运行相关性能回归测试或明确说明未运行原因
- **AND** 若性能阈值失败，不得声明该变更完成

### Requirement: Windows 多盘手工验收记录

系统 SHALL 为 200 万文件级 Windows 多盘场景维护手工验收记录，覆盖真实发布构建下的输入体验、内存和索引状态。

#### Scenario: Windows 大索引验收记录核心指标

- **WHEN** 维护者执行 Windows C/D 多盘手工验收
- **THEN** 验收记录包含索引 entry 数、磁盘规模、QuickFox 内存占用、`agents.md` 输入录屏或等价观察、查询响应和索引状态截图

#### Scenario: 发布前检查大索引风险

- **WHEN** 准备发布包含搜索索引变更的版本
- **THEN** 维护者检查最近一次 Windows 多盘验收记录
- **AND** 若记录缺失或指标明显退化，发布说明必须标记风险或阻止发布
