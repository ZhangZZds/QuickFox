## ADDED Requirements

### Requirement: Windows 默认索引用户高价值目录

Windows 上系统 SHALL 在首次创建默认索引配置时选择当前用户实际存在的 Desktop、
Documents、Downloads、Projects 和 workspace 等高价值目录。若这些目录均不存在，
系统 SHALL 回退当前用户 profile。盘符根目录只有在用户显式配置时才进入
`balanced` 补全范围。

#### Scenario: Windows 存在 C 盘和 D 盘且文件很多

- **WHEN** 用户首次启动 QuickFox 且没有配置文件
- **THEN** 默认 `include_dirs` 只包含实际存在的用户高价值目录
- **AND** 不自动包含 `C:\` 或 `D:\` 盘符根目录

#### Scenario: 用户高价值目录均不存在

- **WHEN** Windows 用户 profile 下没有可识别的高价值目录
- **THEN** 默认索引范围回退当前用户 profile

#### Scenario: 现有配置仍是旧版自动生成的盘符范围

- **WHEN** 现有 Windows 索引配置与旧版所有盘符默认值及其余默认字段完全一致
- **THEN** 系统将 `include_dirs` 迁移为用户高价值目录
- **AND** 任一索引字段已被用户修改时不执行该迁移

### Requirement: 索引存储磁盘安全边界

系统 SHALL 在写完整 baseline 前估算存储需求，并保留至少 5 GiB 可用空间。单个
baseline 估算超过 8 GiB 时 SHALL 拒绝持久化并提示缩小索引目录，不得继续消耗磁盘。

#### Scenario: 配置范围会突破磁盘预算

- **WHEN** baseline 估算超过 8 GiB 或剩余空间不足估算值加 5 GiB
- **THEN** 系统终止该 baseline 写入
- **AND** 最近可用 baseline 保持不变
- **AND** 索引状态提供可诊断失败信息

## MODIFIED Requirements

### Requirement: 持久化索引快照

系统 SHALL 将成功构建的文件索引 baseline 以可识别完成状态的有界事务持久化到本地
存储，并将后续运行期变化保存为可重放 journal。恢复路径 MUST 只读取已完成
baseline；下次启动 MUST 先加载 active baseline，再重放所有已提交且未合并的 journal
batch。激活新 baseline 后系统 SHALL 清理失效完整批次，阶段 checkpoint 只保留
active baseline 和最新 checkpoint。

#### Scenario: 启动加载基线与增量

- **WHEN** QuickFox 启动且存在 active baseline 与 committed journal
- **THEN** 文件 Provider 使用 baseline 和重放后的增量视图提供搜索结果
- **AND** 自动增量准备或后台刷新不阻塞非文件 Provider

#### Scenario: 旧快照兼容迁移

- **WHEN** QuickFox 升级后只存在旧格式完整快照
- **THEN** 系统加载旧快照作为 baseline 提供搜索
- **AND** 系统在后台建立目录清单和新的增量状态

#### Scenario: 无快照时文件搜索不可用

- **WHEN** QuickFox 启动且没有任何完成的索引 baseline
- **THEN** 文件 Provider 不阻塞查询，并向前端暴露文件搜索暂不可用的状态

#### Scenario: 完整 baseline 写入中进程退出

- **WHEN** QuickFox 在完整 baseline 尚未写完时退出或崩溃
- **THEN** 半成品 baseline 不参与下次恢复
- **AND** 下次启动清理其已写入条目
- **AND** 最近已完成 baseline 继续可用

#### Scenario: 新 baseline 成功激活

- **WHEN** 新完整 baseline 已成功激活
- **THEN** 旧完整 baseline 和过期 checkpoint 被分块删除
- **AND** 新数据库可通过增量页回收归还未使用空间

## REMOVED Requirements

### Requirement: Windows 默认索引可用本地盘符

**Reason**: 在文件很多的双盘机器上，首次默认全盘扫描和完整 SQLite 快照会产生不可
接受的启动成本与系统盘写满风险。

**Migration**: 与旧版自动生成索引字段完全一致的配置自动改用用户高价值目录；任一
索引字段已自定义的配置保持不变。用户仍可在设置中把盘符根目录替换为具体目录。
