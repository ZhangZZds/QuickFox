## ADDED Requirements

### Requirement: 托盘显示入口可靠唤醒窗口

系统 SHALL 通过统一窗口显示路径处理托盘“显示 QuickFox”，并在 Windows、macOS 和 Linux 上尽可能显示并聚焦主窗口。

#### Scenario: 托盘显示隐藏窗口

- **WHEN** QuickFox 主窗口隐藏且用户点击托盘菜单“显示 QuickFox”
- **THEN** 系统显示主窗口并请求聚焦搜索输入区域

#### Scenario: 托盘显示最小化窗口

- **WHEN** QuickFox 主窗口最小化且用户点击托盘菜单“显示 QuickFox”
- **THEN** 系统取消最小化、显示主窗口并请求聚焦

#### Scenario: 托盘显示同步窗口状态

- **WHEN** 用户通过托盘菜单显示 QuickFox
- **THEN** 系统内部窗口状态记录为可见且聚焦，后续 Shift+Shift 切换行为保持一致

### Requirement: Shift+Shift 全局唤醒状态可观察

系统 SHALL 使用低层键盘监听实现 Shift+Shift 全局唤醒，并在监听不可用时向用户展示可操作状态。

#### Scenario: 内置快捷键不支持双击 Shift

- **WHEN** 系统实现 Shift+Shift 全局唤醒
- **THEN** 不依赖只能注册普通 accelerator 的 Tauri 全局快捷键机制

#### Scenario: 全局监听权限缺失

- **WHEN** 低层键盘监听因为输入监控或设备权限缺失而无法启动
- **THEN** 设置页显示 Shift+Shift 全局唤醒不可用及需要授权的原因

#### Scenario: macOS 权限入口可打开

- **WHEN** macOS 上 Shift+Shift 全局监听因为输入监控权限缺失而不可用
- **THEN** 设置页提供打开系统输入监控权限设置的入口，并提示授权后需要重启 QuickFox

#### Scenario: 应用启动默认等待唤醒

- **WHEN** QuickFox 启动完成且用户尚未通过托盘或热键显示窗口
- **THEN** 主窗口默认隐藏，第一次有效 Shift+Shift 用于显示并聚焦快速启动窗口

### Requirement: 文件目录应用动作菜单按类型区分

系统 SHALL 根据搜索结果类型提供不同的右键动作菜单。

#### Scenario: 目录结果提供目录动作

- **WHEN** 用户右键点击目录结果
- **THEN** 动作菜单提供打开文件夹和复制路径，不提供选择打开方式

#### Scenario: 文件结果提供打开方式动作

- **WHEN** 用户右键点击文件结果
- **THEN** 动作菜单提供打开所在目录、复制路径和选择打开方式

#### Scenario: macOS 选择打开方式弹出应用选择

- **WHEN** 用户在 macOS 上点击文件结果的选择打开方式
- **THEN** 系统弹出应用选择流程，而不是直接用默认应用打开文件

#### Scenario: 应用结果只提供应用动作

- **WHEN** 用户右键点击应用结果
- **THEN** 动作菜单只提供打开应用和复制路径
