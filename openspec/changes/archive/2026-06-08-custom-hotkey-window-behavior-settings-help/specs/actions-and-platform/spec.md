## ADDED Requirements

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

## MODIFIED Requirements

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
