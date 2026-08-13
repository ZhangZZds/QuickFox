## ADDED Requirements

### Requirement: 桌面应用单实例运行

QuickFox SHALL 在桌面平台只保留一个运行实例。重复启动 SHALL 通知已有实例显示并
聚焦 launcher，然后结束新进程，不得创建第二套托盘、快捷键监听或索引任务。

#### Scenario: Windows 后台实例已存在

- **WHEN** QuickFox 已在 Windows 后台运行且用户再次启动应用
- **THEN** 已有实例显示并聚焦 launcher
- **AND** 新进程退出
- **AND** 系统中仍只有一个 QuickFox 索引任务和一个托盘实例
