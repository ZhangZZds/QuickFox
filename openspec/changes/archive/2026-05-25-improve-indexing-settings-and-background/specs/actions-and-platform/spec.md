## ADDED Requirements

### Requirement: Windows 后台常驻无控制台窗口

系统 SHALL 在 Windows 发布构建中作为 GUI 应用后台常驻，不显示额外 cmd 或 console 窗口。

#### Scenario: Windows 发布版启动无 cmd

- **WHEN** 用户在 Windows 上启动发布版 QuickFox
- **THEN** 系统显示托盘或启动器能力，但不弹出 cmd 或 console 窗口

#### Scenario: Windows 后台等待唤醒

- **WHEN** QuickFox 在 Windows 后台运行且主窗口隐藏
- **THEN** 用户可通过 Shift+Shift 唤醒启动器

### Requirement: 跨平台后台生命周期

系统 SHALL 在 Windows、macOS 和 Linux 上保持后台常驻、托盘入口和窗口唤醒行为的可用性，平台差异必须隔离在平台层或编译配置中。

#### Scenario: macOS 保持托盘和热键

- **WHEN** QuickFox 在 macOS 后台运行
- **THEN** 托盘菜单和 Shift+Shift 唤醒行为保持可用

#### Scenario: Linux 保持托盘和 fallback

- **WHEN** QuickFox 在 Linux 后台运行
- **THEN** 托盘菜单、窗口显示和可用终端 fallback 行为保持可用
