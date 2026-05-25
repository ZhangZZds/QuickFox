## ADDED Requirements

### Requirement: 跨平台索引与后台行为验收

系统 SHALL 在维护文档中记录 Windows、macOS 和 Linux 的索引性能、后台常驻和唤醒验收点。

#### Scenario: Windows 大文件树验收

- **WHEN** 维护者执行 Windows 手工验收
- **THEN** 文档要求验证大文件树启动不被索引阻塞、后台索引状态可见、发布版不弹出 cmd 窗口

#### Scenario: macOS 行为验收

- **WHEN** 维护者执行 macOS 手工验收
- **THEN** 文档要求验证权限提示、托盘、Shift+Shift 和后台索引状态

#### Scenario: Linux 行为验收

- **WHEN** 维护者执行 Linux 手工验收
- **THEN** 文档要求验证托盘、窗口唤醒、终端 fallback 和后台索引状态
