## Context

QuickFox 当前是一个新项目，仓库还没有应用代码。已确认的产品方向是：
构建类似 Listary 的跨平台快速启动器，目标平台为 Windows 和 Linux，开发
环境主要是 macOS。维护者不熟悉 Rust/Tauri，因此项目必须从第一天开始保持
清晰目录结构、中文维护文档、测试和风格检查。

第一版不是公开插件平台，而是“模块化 MVP 内核”：先把启动器主路径做可用，
同时把 Provider、Action、Platform Adapter 等内部边界设计清楚，给后续
内容搜索、剪贴板历史、应用启动、脚本动作等能力留扩展空间。

## Goals / Non-Goals

**Goals:**

- 交付一个能通过 `Shift+Shift` 唤起的 Compact 桌面启动器。
- 支持文件/目录名称与路径搜索、手动索引刷新、显式正则搜索、计算器、网页
  搜索和受控命令执行。
- 让核心业务逻辑可测试、可维护，并尽量不依赖真实桌面环境。
- 将 Windows/Linux 差异隔离在薄 Adapter 层，避免散落在业务逻辑中。
- 建立中文 README、架构文档、开发文档、项目规则和 CI。

**Non-Goals:**

- 不实现文件内容搜索。
- 不开放第三方插件 API。
- 不实现 QuickFox 内嵌终端输出。
- 不在普通 push/PR 中构建发布安装包。
- 不把 macOS 测试结果当作 Windows/Linux 行为完成证据。

## Decisions

### 选择 Tauri + React/Vite/TypeScript + Rust

使用 Tauri 作为跨平台桌面壳，前端采用 React、Vite 和 TypeScript，后端采用
Rust。Tauri 能提供较小体积和系统集成能力，Rust 适合实现索引、搜索、配置、
历史和平台适配等核心逻辑。

备选方案是 Electron 或 Qt。Electron 开发快但常驻成本和分发体积更高；Qt
路线更传统但项目搭建、打包和跨平台维护成本更高。QuickFox 第一版更需要
轻量、可测试和可维护，因此选择 Tauri。

### 使用 Provider + Action + Adapter 分层

查询流程分三层：

- Provider 负责把输入转换为统一 `SearchResult`。
- Action 负责执行用户选择的动作。
- Adapter 负责系统差异，例如全局快捷键、打开文件、打开 URL、启动外部终端。

这样文件搜索、计算器、网页搜索和命令执行可以共享 UI 与动作机制。后续新增
功能时优先新增 Provider 或 Action，而不是改动启动器主窗口。

### 第一版只索引名称和路径

第一版只索引文件名、目录名和完整路径，不索引文件内容。内容搜索会显著增加
索引体积、权限问题、更新策略和性能风险；它更适合作为后续独立 Provider。

搜索默认使用模糊匹配，正则模式必须通过可配置前缀显式触发，默认 `re:`。
这可以避免普通用户输入特殊字符时被误判成正则。

### 使用 TOML 配置与 SQLite 持久化

用户可读配置采用 TOML，覆盖索引目录、排除规则、正则前缀、网页搜索前缀、
命令前缀、历史策略和终端偏好。

索引状态和历史采用 SQLite，通过 Rust storage layer 封装。SQLite 适合本地
桌面应用，便于查询、迁移和测试，也能避免手写复杂文件格式。

### 命令执行默认关闭并走外部终端

命令执行进入第一版，但默认关闭。启用后通过前缀触发，显示 preview/确认，
并由外部终端执行。Windows 主路径使用 Windows Terminal 的 `wt.exe`，Linux
通过终端 fallback 顺序选择可用终端。

不在第一版做内嵌输出，因为这会引入交互命令、输出流、取消、超时和终端仿真
问题。外部终端能先满足执行场景，同时降低第一版复杂度。

### CI 验证 Windows/Linux，真实桌面行为手工验收

GitHub Actions 使用标准 Windows/Linux runner，普通 push/PR 跑格式化、
lint、测试和构建检查。仓库 public，标准 runner 可免费使用。

CI 只能验证核心逻辑、平台分支和构建，不替代真实桌面验收。全局快捷键、窗口
置顶、Windows Terminal 实际行为、系统权限弹窗等由维护者在 Windows 机器上
手工验收，发现问题后通过 GitHub issue 回流。

## Risks / Trade-offs

- **风险：第一版模块边界比极简原型重。** → 通过清晰目录、文档和测试抵消
  初始结构成本，避免后续功能堆叠成难维护代码。
- **风险：macOS 开发环境无法真实代表 Windows/Linux。** → 把平台代码隔离到
  Adapter，自动化测试覆盖命令构造和路径分支，桌面行为交给 Windows 手工验收。
- **风险：索引用户主目录可能遇到权限或大目录性能问题。** → 支持排除规则、
  局部失败报告和手动刷新；第一版不做内容索引。
- **风险：命令执行可能带来安全和隐私问题。** → 默认关闭、启用提示、执行
  前确认、危险命令规则、命令历史清空/关闭设置，并在文档中说明非沙箱性质。
- **风险：SQLite 和 Tauri 打包在 Linux CI 上需要系统依赖。** → workflow 中
  明确安装 Tauri/Linux 所需依赖，普通 CI 不做完整发布打包。

## Migration Plan

这是初始应用开发，不涉及既有用户数据迁移。

实现顺序应先建立项目骨架、质量工具和可测试核心模块，再逐步加入 UI、平台
Adapter 和功能 Provider。若实现中发现已确认默认决策不可行，必须先更新
OpenSpec artifacts，再调整代码。

回滚策略是按任务拆分提交；若某个 Provider 或平台 Adapter 不稳定，应能在
配置或注册表中禁用，不影响其他 Provider 的核心搜索路径。

## Open Questions

无阻塞问题。Linux 终端 fallback 的精确行为、危险命令规则的首批模式、排序
权重等属于实现细节，应在任务执行中通过测试和小步调整落地。
