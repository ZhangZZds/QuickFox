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
- **THEN** PathAdapter 返回当前可用盘符根目录作为默认索引根目录
- **AND** 未发现可用盘符时回退当前用户 profile

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

### Requirement: 用开发工具打开

QuickFox SHALL 提供用开发工具打开文件和目录结果的次要动作。

#### Scenario: 文件结果暴露开发工具打开动作

- **WHEN** 文件搜索结果返回
- **THEN** 结果包含一个在 UI 中标记为开发工具打开的次要动作
- **AND** 该动作由 Rust core 通过平台 Adapter 执行

#### Scenario: 开发工具打开 Adapter 选择可用工具

- **WHEN** 用户调用开发工具打开动作
- **THEN** QuickFox 优先选择已安装的代码编辑器，例如 VS Code 或 Cursor
- **AND** 使用平台特定 fallback，而不在 Provider 中嵌入 shell 逻辑

### Requirement: 托盘显示入口可靠唤醒窗口

系统 SHALL 通过统一窗口切换路径处理托盘“显示/隐藏 QuickFox”，并在 Windows、macOS 和 Linux 上尽可能显示并聚焦主窗口或隐藏已聚焦主窗口。

#### Scenario: 托盘显示隐藏窗口

- **WHEN** QuickFox 主窗口隐藏且用户点击托盘菜单“显示/隐藏 QuickFox”
- **THEN** 系统显示主窗口并请求聚焦搜索输入区域

#### Scenario: 托盘显示最小化窗口

- **WHEN** QuickFox 主窗口最小化且用户点击托盘菜单“显示/隐藏 QuickFox”
- **THEN** 系统取消最小化、显示主窗口并请求聚焦

#### Scenario: 托盘隐藏已聚焦窗口

- **WHEN** QuickFox 主窗口已显示且聚焦
- **AND** 用户点击托盘菜单“显示/隐藏 QuickFox”
- **THEN** 系统隐藏主窗口

#### Scenario: 托盘切换同步窗口状态

- **WHEN** 用户通过托盘菜单显示或隐藏 QuickFox
- **THEN** 系统内部窗口状态记录与真实窗口状态保持一致，后续全局唤醒键切换行为保持一致

### Requirement: Shift+Shift 全局唤醒状态可观察

系统 SHALL 使用低层键盘监听实现可配置全局唤醒，并在监听不可用时向用户展示可操作状态。默认唤醒键为 Shift+Shift，用户可在设置页录制并保存其他快捷键。

#### Scenario: 内置快捷键不支持双击 Shift

- **WHEN** 系统实现 Shift+Shift 全局唤醒
- **THEN** 不依赖只能注册普通 accelerator 的 Tauri 全局快捷键机制

#### Scenario: 全局监听权限缺失

- **WHEN** 低层键盘监听因为输入监控或设备权限缺失而无法启动
- **THEN** 设置页显示当前全局唤醒不可用及需要授权的原因

#### Scenario: macOS 权限入口可打开

- **WHEN** macOS 上全局监听因为输入监控权限缺失而不可用
- **THEN** 设置页提供打开系统输入监控权限设置的入口，并提示授权后需要重启 QuickFox

#### Scenario: 应用启动默认等待唤醒

- **WHEN** QuickFox 启动完成且用户尚未通过托盘或热键显示窗口
- **THEN** 主窗口默认隐藏，第一次有效全局唤醒用于显示并聚焦快速启动窗口

### Requirement: 文件目录应用动作菜单按类型区分

系统 SHALL 根据搜索结果类型提供不同的右键动作菜单。

#### Scenario: 目录结果提供目录动作

- **WHEN** 用户右键点击目录结果
- **THEN** 动作菜单提供打开文件夹和复制路径
- **AND** 动作菜单提供复制所在文件夹路径和打开所在文件夹
- **AND** 动作菜单不提供选择打开方式

#### Scenario: 文件结果提供打开方式动作

- **WHEN** 用户右键点击文件结果
- **THEN** 动作菜单提供打开所在目录、复制路径和选择打开方式
- **AND** 动作菜单提供复制所在文件夹路径和打开所在文件夹

#### Scenario: 所在文件夹动作使用父目录

- **WHEN** 搜索结果路径存在父目录
- **THEN** “复制所在文件夹路径”复制该父目录路径
- **AND** “打开所在文件夹”打开该父目录路径

### Requirement: Windows 托盘常驻后台形态

系统 SHALL 在 Windows 上作为托盘常驻工具运行，后台状态不依赖普通最小化窗口。

#### Scenario: Windows 后台只保留托盘入口

- **WHEN** QuickFox 在 Windows 上启动且用户尚未唤醒启动器
- **THEN** 系统只需要显示托盘小图标
- **AND** 不需要显示或最小化普通主窗口

#### Scenario: 关闭设置不退出后台

- **WHEN** 用户在 Windows 上关闭设置窗口
- **THEN** QuickFox 后台进程继续运行
- **AND** 托盘菜单和全局唤醒键仍可用

#### Scenario: 退出只能通过托盘菜单

- **WHEN** 用户希望结束 QuickFox 后台进程
- **THEN** 用户可以通过托盘菜单“退出”关闭应用

### Requirement: 托盘菜单切换启动器显示状态

系统 SHALL 通过托盘小图标右键菜单提供“显示/隐藏 QuickFox”，并使用与全局唤醒键一致的启动器切换逻辑。

#### Scenario: 托盘显示隐藏的启动器

- **WHEN** QuickFox 启动器隐藏
- **AND** 用户从托盘菜单选择“显示/隐藏 QuickFox”
- **THEN** 系统显示并聚焦启动器

#### Scenario: 托盘隐藏已聚焦的启动器

- **WHEN** QuickFox 启动器已显示且聚焦
- **AND** 用户从托盘菜单选择“显示/隐藏 QuickFox”
- **THEN** 系统隐藏启动器

#### Scenario: 托盘切换同步窗口状态

- **WHEN** 用户通过托盘菜单显示或隐藏 QuickFox
- **THEN** 系统内部窗口状态与真实窗口可见/聚焦状态保持一致

### Requirement: 托盘设置可靠打开设置窗口

系统 SHALL 保证托盘菜单“设置”可以可靠展示设置窗口，即使设置窗口曾被关闭、隐藏、最小化或销毁。

#### Scenario: 设置窗口存在时聚焦

- **WHEN** 设置窗口已存在但隐藏或最小化
- **AND** 用户从托盘菜单选择“设置”
- **THEN** 系统显示、取消最小化并聚焦该设置窗口

#### Scenario: 设置窗口不存在时重建

- **WHEN** 设置窗口不存在或已经被系统销毁
- **AND** 用户从托盘菜单选择“设置”
- **THEN** 系统重新创建设置窗口
- **AND** 显示并聚焦设置页

#### Scenario: 打开设置不显示启动器

- **WHEN** 用户从托盘菜单选择“设置”
- **THEN** 系统打开设置窗口
- **AND** 不同时显示快速启动器
