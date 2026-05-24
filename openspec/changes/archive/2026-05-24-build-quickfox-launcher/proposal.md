## Why

QuickFox 是一个跨平台桌面启动器，第一版同时涉及平台适配、文件索引、
命令执行、配置、历史、测试和 CI。先用 OpenSpec 固化范围和契约，可以让
MVP 保持可用，同时避免项目变成难维护的一次性原型。

## What Changes

- 搭建 Tauri 桌面应用，前端使用 TypeScript，后端使用 Rust。
- 新增通过 `Shift+Shift` 唤起的 Compact 风格启动窗口。
- 新增统一的结果模型和动作模型，覆盖文件、目录、计算器、网页搜索和命令。
- 新增文件/目录名称与路径索引：默认索引用户主目录，支持可配置包含/排除
  目录、手动刷新、模糊搜索和显式正则搜索。
- 新增常用增强计算器能力。
- 新增可配置网页搜索前缀和 URL 模板。
- 新增受控命令执行：显式前缀触发、默认关闭、preview/确认、安全检查、
  最近命令持久化，并通过外部终端执行。
- 新增人可读配置、历史存储，以及受使用历史影响的排序。
- 新增中文维护文档、项目规则、测试、lint/format 检查和 GitHub Actions，
  覆盖 Windows 与 Linux 核心验证。
- 本变更不实现文件内容搜索、公开第三方插件 API、QuickFox 内嵌终端输出，
  也不在每次 push 时自动构建发布安装包。

## Capabilities

### New Capabilities

- `launcher-shell`: Compact Tauri 启动器 UI、全局快捷键、结果导航、主动作
  执行和次要动作菜单。
- `search-index`: 文件/目录名称与路径索引、手动刷新、模糊搜索、正则搜索
  和排序。
- `query-providers`: Provider 模型，以及文件、计算器、网页搜索、命令四类
  内置 Provider。
- `actions-and-platform`: 统一 Action 分发，以及打开文件/目录/URL、外部
  终端命令执行的平台 Adapter。
- `configuration-and-history`: TOML 配置、默认配置创建、历史持久化、命令
  历史导航和隐私控制。
- `project-quality`: 可维护性文档、无死代码规则、测试、本地检查和
  GitHub Actions CI。

### Modified Capabilities

- 无。这是 QuickFox 的第一个 OpenSpec 变更。

## Impact

- 创建初始 Tauri/Rust/TypeScript 应用结构。
- 新增 Rust core 模块，用于启动器核心逻辑、配置、索引/存储、Provider
  执行、平台 Adapter 和测试。
- 新增前端组件、状态管理、设置视图和 UI 测试。
- 新增本地持久化数据，用于索引状态和历史，预期通过 Rust storage layer
  使用 SQLite。
- 新增人可读配置，预期采用 TOML。
- 新增使用标准 Windows/Linux runner 的 GitHub Actions workflow。
- 新增面向用户和维护者的中文文档。
