## ADDED Requirements

### Requirement: Windows 索引设置状态流发布验收

系统 SHALL 为影响索引配置状态流的发布维护 Windows 真实桌面验收记录，覆盖模式切换、整盘或大目录、不可访问 root、连续保存、关闭重开设置和重启恢复。

#### Scenario: fast 与 balanced 往返切换

- **WHEN** 维护者在 Windows 发布构建中从 `fast` 保存为 `balanced` 并在后台补全前重开设置
- **THEN** 控件仍显示 `balanced`
- **AND** 保存请求在不等待完整 D 盘扫描的情况下完成
- **AND** 后台状态和最近可用搜索范围可观察

#### Scenario: 大目录扫描被新配置取代

- **WHEN** 维护者在 D 盘大目录扫描中保存新的模式或目录范围
- **THEN** 旧 revision 不继续造成完整旧范围扫描的持续 I/O
- **AND** 只有最新 revision 最终显示为已应用

#### Scenario: 不可访问 root 不回滚设置

- **WHEN** 维护者断开盘符、移除目录或制造可诊断读取失败后保存设置
- **THEN** 已保存的模式和目录在重开设置后保持不变
- **AND** 旧索引继续可用
- **AND** 设置页显示失败 root 摘要与恢复动作

#### Scenario: 发布前检查状态流记录

- **WHEN** 准备发布包含索引设置、扫描或 revision 生命周期变更的版本
- **THEN** 维护者检查最近一次 Windows 状态流验收记录
- **AND** 若记录缺失则发布说明标记未完成验收且不得宣称 Windows 行为已验证
