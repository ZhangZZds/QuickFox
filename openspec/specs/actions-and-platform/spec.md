# actions-and-platform Specification

## Purpose

TBD - created by archiving change build-quickfox-launcher. Update Purpose after archive.

## Requirements

### Requirement: Action dispatcher 集中执行动作

系统 SHALL 通过 Rust core 的 Action dispatcher 执行动作，不允许 Provider 或
前端绕过 dispatcher 直接调用平台能力。

#### Scenario: 前端请求执行主动作

- **WHEN** 前端请求执行某个结果的主动作
- **THEN** Rust core 通过 Action dispatcher 路由该动作

### Requirement: 打开文件目录和 URL

系统 SHALL 通过平台 Adapter 打开文件、目录、所在目录和 URL。

#### Scenario: 打开文件

- **WHEN** 用户执行文件结果的打开动作
- **THEN** 系统通过当前平台默认方式打开该文件

#### Scenario: 打开所在目录

- **WHEN** 用户执行文件结果的“打开所在目录”动作
- **THEN** 系统通过当前平台文件管理器打开该文件所在目录

#### Scenario: 打开 URL

- **WHEN** 用户执行网页搜索结果的打开动作
- **THEN** 系统通过当前平台默认浏览器打开 URL

### Requirement: 外部终端执行命令

系统 SHALL 通过 TerminalAdapter 在外部终端执行确认后的命令。

#### Scenario: Windows 使用 wt.exe

- **WHEN** 当前平台是 Windows 且用户确认执行命令
- **THEN** TerminalAdapter 构造并启动 Windows Terminal `wt.exe` 命令

#### Scenario: Linux 使用终端 fallback

- **WHEN** 当前平台是 Linux 且用户确认执行命令
- **THEN** TerminalAdapter 按配置或 fallback 顺序选择可用终端执行命令

### Requirement: 命令执行安全检查

系统 SHALL 在命令执行前应用安全检查，并对危险命令阻止或要求更强确认。

#### Scenario: 普通命令要求确认

- **WHEN** 用户输入普通命令并选择执行
- **THEN** 系统在执行前显示确认步骤

#### Scenario: 危险命令触发强确认或阻止

- **WHEN** 用户输入匹配危险规则的命令
- **THEN** 系统阻止执行或要求更强确认

### Requirement: 平台路径默认值

系统 SHALL 通过 PathAdapter 解析应用数据目录和默认索引根目录。

#### Scenario: Windows 默认索引路径

- **WHEN** 当前平台是 Windows 且没有自定义索引目录
- **THEN** PathAdapter 返回用户 profile 目录作为默认索引根目录

#### Scenario: Linux 默认索引路径

- **WHEN** 当前平台是 Linux 且没有自定义索引目录
- **THEN** PathAdapter 返回用户 home 目录作为默认索引根目录

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
