## Why

QuickFox 已经有后台索引、分阶段扫描和状态展示，但 Windows 首次启动面对 `C:\Users`、`D:\` 这类真实常用大目录时，仍会长期占用磁盘并在阶段边界产生卡顿感。短期需要在不更换底层索引技术的前提下，让首次体验先“快速可用”，再把完整索引放到后台补齐。

## What Changes

- 让现有 `fast` / `balanced` / `complete` 索引性能模式真正影响扫描阶段、默认范围和首次索引行为。
- 将首次索引拆成快速可用阶段和后台补全阶段：应用入口、热路径和用户最近关心的路径优先；大根目录后置。
- 降低阶段边界卡顿：阶段进度更新不应每次都写入完整聚合快照或重建重量级内容索引。
- 延后内容索引：基础 name/path 索引优先可用，`content:` 索引作为低优先级后续工作。
- 改进启动器和设置页反馈：明确当前处于快速可用、后台补全或内容索引阶段，并提供调整范围/模式的入口。
- 优化索引已建立后的输入搜索体验：连续打字时不应因为每个字符触发重量级搜索、全量索引 clone 或过期结果回写而卡顿。
- 不引入 Everything、Windows Search、NTFS USN/MFT 或新的平台索引 Provider；这些留给后续中长期变更。

## Capabilities

### New Capabilities

### Modified Capabilities

- `search-index`: 明确首次索引快速可用、后台补全、快照节流、内容索引延后和状态反馈要求。
- `configuration-and-history`: 明确 `fast` / `balanced` / `complete` 的配置语义和设置页说明。

## Impact

- Rust core: 索引计划构建、后台刷新流程、快照保存策略、内容索引触发时机、索引状态 payload。
- Rust query path: 搜索命令、ProviderRegistry/FileProvider 的索引持有方式、查询耗时观测和候选限制。
- Frontend: 启动器轻量状态、设置页索引模式说明、进度/阶段文案、输入搜索节流/过期结果处理。
- Tests/docs: Rust 单元测试覆盖性能模式计划、快照节流和大索引查询不 clone；前端测试覆盖新状态文案与连续输入搜索体验；Windows QA 覆盖大目录首次启动和索引已建立后的打字体感。
