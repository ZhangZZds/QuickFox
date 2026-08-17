## MODIFIED Requirements

### Requirement: 图形化管理索引规则

系统 SHALL 允许用户在设置页管理索引包含目录、排除目录和排除模式；通过配置校验后 MUST 先持久化用户的期望配置并快速完成保存请求，再在后台应用新的索引 revision。索引目录、排除目录和排除模式 SHALL 作为索引分区的主规则编辑列展示，不被正则前缀、配置路径或其他辅助信息压缩。

#### Scenario: 保存索引目录触发后台应用

- **WHEN** 用户在设置页修改索引目录并保存
- **THEN** 系统先持久化配置并让后续设置窗口读取到新值
- **AND** 系统启动或排队后台索引 revision
- **AND** 保存请求不等待新目录完整遍历或 baseline 写入

#### Scenario: 后台应用失败不回滚设置

- **WHEN** 已保存的索引目录包含不可访问、离线或读取失败的路径
- **THEN** 系统保留已经持久化的期望配置
- **AND** 最近可用索引继续参与搜索
- **AND** 索引应用状态显示失败目录或失败摘要和恢复动作

#### Scenario: 排除规则保存后生效

- **WHEN** 用户新增排除目录或排除模式并保存
- **THEN** 后台索引 revision 按新规则构建
- **AND** revision 发布后跳过或移除匹配的目录与条目

#### Scenario: 主规则编辑优先展示

- **WHEN** 用户打开设置页索引分区
- **THEN** 索引目录、排除目录和排除模式显示在主规则编辑列中，且正则前缀、配置路径等辅助信息不打断该编辑流程

## ADDED Requirements

### Requirement: 索引性能模式在全部生命周期范围一致

系统 MUST 使用同一性能模式 active-root 语义生成 baseline 扫描、standby watcher、运行期 watcher 和校准范围，不得在保存或恢复路径中无条件重新加入当前模式排除的 configured root。

#### Scenario: fast 保存不触达配置大盘

- **WHEN** 用户配置 `D:\` 但选择并保存 `fast`
- **THEN** baseline、watcher 和校准只使用应用入口和用户热路径
- **AND** 保存或后台应用不遍历 `D:\`

#### Scenario: balanced 保存后后台补全

- **WHEN** 用户从 `fast` 切换并保存 `balanced`
- **THEN** `balanced` 立即成为持久化期望配置
- **AND** 系统先保持快速范围可用，再在后台补全配置目录

#### Scenario: 重启恢复已保存模式

- **WHEN** 进程在索引 revision 完成前退出并重新启动
- **THEN** 设置页仍显示最近成功保存的性能模式
- **AND** 启动索引按该模式继续应用而不是恢复旧模式
