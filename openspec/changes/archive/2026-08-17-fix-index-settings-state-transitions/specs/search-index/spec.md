## MODIFIED Requirements

### Requirement: 后台构建索引

The system SHALL preserve the currently searchable index while a background refresh runs, publish progress after each completed scan stage, and replace the searchable baseline only after the final staged result is successfully persisted. After Tauri setup completes, every available active root selected by the current performance mode SHALL be scheduled for that background refresh even if the event-loop Ready callback is delayed or absent.

#### Scenario: Startup refresh runs without a Ready callback

- **WHEN** a persisted baseline exists and QuickFox finishes setup with indexing enabled
- **THEN** the persisted baseline remains searchable while refresh runs
- **AND** all available active roots selected by the current performance mode are included in the scheduled refresh

#### Scenario: Refresh fails after a quick index is available

- **WHEN** a background refresh fails after a persisted or staged index becomes available
- **THEN** file search continues against the last available index
- **AND** index status exposes the refresh failure
- **AND** a retry can rebuild the index later

#### Scenario: A new file arrives while content indexing is pending

- **WHEN** the durable filename/path baseline has been published and the optional content index is still building
- **THEN** the runtime watcher SHALL already observe active roots selected by the current performance mode
- **AND** a file created after that baseline becomes searchable by its filename or path without waiting for content indexing to finish
- **AND** content installation SHALL reconcile watcher changes before it replaces its baseline

#### Scenario: Excluded filesystem traffic is observed

- **WHEN** the native watcher reports changes under an excluded directory
- **THEN** those events SHALL be rejected before entering the bounded runtime queue
- **AND** they SHALL NOT cause an overflow recovery for otherwise valid active-root changes

#### Scenario: The native event uses a canonical alias of a configured root

- **WHEN** the operating system reports an event through a canonical path alias such as macOS `/private/var` for a configured `/var` root
- **THEN** QuickFox SHALL associate the event with the original active root
- **AND** exclusion patterns SHALL be evaluated only within that active-root boundary
- **AND** index and manifest entries SHALL retain one stable root identity

#### Scenario: The native watcher requests a rescan

- **WHEN** the native backend requests a rescan without reporting a concrete backend failure
- **THEN** QuickFox SHALL schedule targeted calibration for the affected active roots
- **AND** any discovered differences SHALL be committed as an incremental delta
- **AND** a successful calibration SHALL clear the affected degraded-root state without forcing a baseline refresh

## ADDED Requirements

### Requirement: 索引配置 revision 可取代且可取消

系统 SHALL 为每个已保存索引语义分配单调递增的 desired revision；后台扫描、持久化和发布 MUST 校验 revision 身份，新 revision MUST 使旧 revision 在遍历过程中协作式取消并不得覆盖新状态。

#### Scenario: 连续保存取代旧扫描

- **WHEN** 用户在大目录扫描期间再次保存不同索引配置
- **THEN** 旧扫描在 root 或 walker entry 取消边界停止
- **AND** 系统只排队并最终发布最新 desired revision

#### Scenario: 旧 revision 完成结果被丢弃

- **WHEN** 旧 worker 在新配置保存后才返回扫描或持久化结果
- **THEN** identity fence 拒绝该结果更新 runtime、baseline 或应用状态

### Requirement: 索引配置应用允许部分 root 降级

系统 SHALL 在单个 active root 不可访问或出现目录项失败时继续处理其他可用 root；只要存在可用搜索视图，失败 MUST 作为 degraded/partial 状态报告而不是回滚 desired config 或清空旧索引。

#### Scenario: 一个盘符离线时其他范围继续

- **WHEN** balanced 或 complete 模式包含多个 roots 且其中一个盘符离线
- **THEN** 可访问 roots 继续扫描并保持或进入搜索视图
- **AND** 离线 root 记录为 dirty 或 failed root
- **AND** 设置页提供重试或修改范围的恢复动作

#### Scenario: 所有新 roots 失败

- **WHEN** 新 revision 没有任何可用 root
- **THEN** 系统把该 revision 标记为应用失败
- **AND** 最近可用旧索引继续参与搜索
- **AND** 已保存的 desired config 不被回滚
