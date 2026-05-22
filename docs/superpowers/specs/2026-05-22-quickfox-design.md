# QuickFox 设计文档

日期：2026-05-22
状态：讨论稿已确认，等待书面 review

## 目标

QuickFox 是一个受 Listary 启发的跨平台快速启动软件。目标平台是
Windows 和 Linux；macOS 作为主要开发环境，用来做日常开发和大部分核心
逻辑验证。

第一版要先成为一个真正可用的键盘优先启动器，同时保持代码结构清楚、
文档齐全，让不熟悉 Rust 或 Tauri 的维护者也能理解项目怎么运行、怎么
测试、怎么扩展。

QuickFox 通过 `Shift+Shift` 唤起，显示一个紧凑搜索框。它可以按文件名、
目录名和路径搜索本机文件与目录，支持显式正则查询、计算器表达式、可配置
网页搜索前缀，以及受控的命令执行。

## 产品范围

第一版采用“模块化 MVP 内核”，不是完整的公开插件平台。

第一版包含：

- Compact 风格启动窗口：搜索框 + 结果列表。
- `Shift+Shift` 全局快捷键。
- 按文件名、目录名和路径搜索文件/目录。
- 默认索引用户主目录。
- 支持配置额外索引目录。
- 支持配置排除目录和排除模式。
- 支持手动刷新索引。
- 默认模糊搜索。
- 通过可配置前缀触发正则搜索，默认前缀为 `re:`。
- 常用增强计算器。
- 通过可配置前缀和 URL 模板触发网页搜索。
- 通过可配置前缀触发受控命令执行，默认关闭。
- 文件/目录结果动作：打开、打开所在目录、复制路径。
- 通过右键或键盘快捷键打开结果动作列表。
- 持久化文件/目录使用历史和最近命令。
- GitHub Actions CI，覆盖 Windows 和 Linux 核心检查。

第一版暂不包含：

- 文件内容搜索。
- 公开第三方插件 API。
- QuickFox 内嵌终端输出。
- 每次 push 都构建发布安装包。
- 高级计算器能力，例如变量、汇率、日期计算。
- 完整主题和视觉自定义系统。

## 架构

QuickFox 使用 Tauri：前端采用 TypeScript，后端采用 Rust。

前端负责展示层：

- Compact 启动窗口。
- 搜索输入框。
- 结果列表。
- 右键/快捷键动作菜单。
- 命令预览与确认视图。
- 基础设置页。

Rust core 负责行为层：

- 查询路由。
- Provider 注册。
- Action 分发。
- 配置加载与保存。
- 索引管理。
- 历史管理。
- 搜索排序。

平台差异集中在 Adapter 层：

- `HotkeyAdapter`：注册全局快捷键。
- `OpenAdapter`：打开文件、目录和 URL。
- `TerminalAdapter`：调用外部终端执行命令。
- `PathAdapter`：处理应用数据路径和默认索引路径。

大部分业务逻辑必须能在 macOS 上测试，但不能把 macOS 假装成 Windows 或
Linux。Windows 和 Linux 行为通过 CI 验证；Windows 桌面行为通过用户的
Windows 机器手工验收。

## Provider 模型

每个 Provider 接收标准化后的查询，返回统一的 `SearchResult`。Provider
只负责产出候选结果，不直接执行动作。

第一版内置 Provider：

- `FileProvider`：搜索名称/路径索引，支持模糊模式和正则模式。
- `CalculatorProvider`：计算常用表达式。
- `WebSearchProvider`：处理配置过的网页搜索前缀。
- `CommandProvider`：在命令执行启用后处理命令前缀查询。

Provider 边界是内部扩展边界。它应该足够清楚，方便未来升级成公开插件
API；但第一版不开放第三方插件加载。

## Action 模型

每个结果提供一个主动作和若干次要动作。用户按 Enter 执行主动作，通过
右键或键盘快捷键打开次要动作列表。

第一版动作：

- 打开文件。
- 打开目录。
- 打开所在目录。
- 复制路径。
- 打开 URL。
- 复制计算器结果。
- 确认后在外部终端执行命令。

所有动作都通过 Rust core 执行，让平台适配和安全检查集中在一个地方。

## 索引与搜索

默认索引根目录：

- Windows：用户 profile 目录。
- Linux：用户 home 目录。
- macOS 开发环境：用户 home 目录，仅用于本地开发验证。

用户可以配置额外索引目录、排除目录和排除模式。第一版支持手动刷新索引。
自动文件监听暂不做，等手动刷新路径稳定后再作为后续能力加入。

第一版索引内容：

- 文件名。
- 目录名。
- 完整路径。

第一版不索引文件内容。未来如果做内容搜索，应作为独立 Provider 加入，
避免拖慢启动器主路径。

普通输入使用模糊匹配。正则模式必须显式触发，默认前缀为 `re:`，例如：

```text
re:.*\.pdf$
```

无效正则必须显示为查询反馈，不能导致应用崩溃。

## 计算器

计算器 Provider 支持常用增强表达式：

- 四则运算。
- 括号。
- 小数。
- 百分比。
- 指数运算，例如 `2^10`。
- 常用函数，例如 `sqrt(9)`。
- 进制字面量，例如 `0xff`。

计算结果可以复制；是否进入历史由配置控制。

## 网页搜索

网页搜索只通过显式前缀触发。QuickFox 不会在本地无结果时自动发起网页
搜索。

每个搜索引擎由前缀、显示名称和 URL 模板配置。例如：

```toml
[web_search.engines.g]
name = "Google"
url = "https://www.google.com/search?q={query}"

[web_search.engines.gh]
name = "GitHub"
url = "https://github.com/search?q={query}"

[web_search.engines.bd]
name = "Baidu"
url = "https://www.baidu.com/s?wd={query}"
```

URL 模板必须包含 `{query}` 占位符。替换前，查询文本必须先进行 URL 编码。

## 命令执行

命令执行进入第一版，但默认关闭。

启用后：

- 通过可配置命令前缀触发，默认前缀为 `>`。
- 结果列表切换为 preview/确认样式。
- 输入命令时不会立即执行。
- 用户确认后才执行。
- 第一版使用外部终端执行。

Windows 第一版使用 Windows Terminal，即 `wt.exe`。Linux 使用通用终端
fallback 适配。QuickFox 内嵌命令输出暂不做，但架构不应阻碍后续加入。

必要安全规则：

- 启用命令执行前显示风险提示。
- 每次执行命令前都要求确认。
- 识别明显危险命令，并要求更强确认或在第一版直接阻止。
- 提供关闭命令执行的设置。
- 提供清空命令历史的设置。

危险命令识别只是防护栏，不是安全沙箱。文档必须明确：shell 命令会以
当前用户权限运行。

## 历史

文件和目录使用历史默认记录，并用于结果排序。

网页搜索和计算器历史可配置。

命令历史默认持久化保存最近 15 条命令。在命令模式中，用户可以用上/下
方向键循环这些命令。设置中必须支持：

- 清空命令历史。
- 关闭命令历史。
- 修改命令历史最大条数。

由于命令可能包含敏感路径、token 或参数，设置页和文档必须说明隐私取舍。

## 配置

配置使用人可读格式，优先采用 TOML。

配置内容包括：

- 索引包含目录。
- 排除目录和排除模式。
- 正则前缀。
- 网页搜索引擎和前缀。
- 命令前缀。
- 是否启用命令执行。
- 历史策略。
- 结果数量限制。
- 支持的平台终端偏好。

首次启动时，QuickFox 创建默认配置文件。设置页覆盖常用配置；高级用户可以
直接编辑配置文件。无效配置必须给出可操作错误，并在可能时安全回退。

## 项目规范

项目必须适合 Rust/Tauri 新手维护。

长期工程规则放在仓库根目录 `AGENTS.md`。本设计文档只记录 QuickFox 第一版
的产品和架构决策；实现时必须同时遵守 `AGENTS.md`。

必需文档：

- `README.md`：依赖安装、本地运行、测试、构建和基础使用。
- `docs/architecture.md`：模块职责和数据流。
- `docs/development.md`：项目结构、常用命令、如何新增 Provider/Action。
- 排错文档：Windows Terminal、Linux 终端 fallback、全局快捷键、索引刷新。

代码规范：

- 不保留死代码或废弃代码。
- 不保留产品没有使用的生成示例代码。
- 模块小而清楚，每个模块有明确职责。
- Rust 后端逻辑不能堆在一个巨大的 `main.rs` 中。
- 前端 UI 不能堆在一个巨大的根组件中。
- 只在非显而易见的逻辑处写注释。

## 工具链

Rust：

- `rustfmt`。
- `clippy`。
- 核心逻辑单元测试。

前端：

- TypeScript。
- ESLint。
- Prettier。
- Vitest 单元测试。

项目级：

- 提供一个统一且有文档说明的本地检查命令，例如 `npm run check` 或
  `just check`。
- 配置 GitHub Actions，覆盖 Windows 和 Linux。

## CI 与验证

仓库计划为 public，因此 GitHub Actions 标准 GitHub-hosted runner 可免费
用于该仓库。workflow 必须只使用标准 runner，不使用 larger runner。

普通 push 和 pull request 检查：

- Rust format check。
- Rust clippy。
- Rust tests。
- Frontend lint。
- Frontend tests。
- Frontend build。
- Windows 和 Linux 平台矩阵核心检查。

发布安装包不在每次 push 时构建。发布打包后续通过手动 workflow 或 tag
release workflow 加入。

GitHub Actions 不能替代真实桌面验收。全局快捷键、窗口行为、Windows
Terminal 执行、系统权限弹窗等需要在 Windows 机器上手工测试。发现的问题
通过 GitHub issue 回流，并尽量补充回归测试。

## 错误处理

QuickFox 必须避免因为用户输入或配置错误而崩溃。

预期处理：

- 无效正则：显示查询反馈，启动器保持可用。
- 错误网页搜索模板：显示配置错误。
- 索引刷新失败：报告失败目录，并继续使用可用索引。
- 命令执行未启用：显示启用提示或设置入口。
- 终端不可用：显示清楚的平台相关错误。
- 危险命令：阻止执行或要求更强确认。

## 测试策略

核心单元测试：

- 查询解析。
- 前缀匹配。
- 正则模式解析和无效正则行为。
- 网页搜索 URL 模板替换。
- 计算器表达式。
- 命令历史长度限制和上下键导航。
- 配置加载和验证。
- 历史对搜索排序的影响。

集成测试：

- Provider registry 合并多个 Provider 的结果。
- Action dispatcher 将动作路由到 Adapter。
- 索引刷新更新可搜索条目。

平台 Adapter 测试：

- Windows Terminal 命令构造。
- Linux 终端 fallback 选择。
- Windows 和 Linux 默认路径。

UI 测试：

- 搜索输入渲染结果。
- 键盘上下选择结果。
- Enter 通过 mock 后端执行主动作。
- 右键或快捷键打开动作菜单。
- 命令模式显示 preview/确认，而不是普通文件结果列表。

## 实现默认决策

为了降低 OpenSpec 规划阶段的歧义，先采用以下默认决策：

- 前端使用 React、Vite 和 TypeScript，除非 Tauri 项目生成器给出明显更适合
  本项目的本地默认方案。
- 索引和历史通过 Rust storage layer 使用 SQLite 持久化。
- 搜索排序放在 `Ranker` 边界后。第一版排序结合模糊名称/路径匹配、精确
  子串加权、路径深度信号和历史加权。
- Linux 终端 fallback 顺序先尝试 `xdg-terminal-exec`，再尝试常见终端：
  `x-terminal-emulator`、`gnome-terminal`、`konsole`、`xfce4-terminal`、
  `alacritty`、`kitty`。
- 危险命令处理从可配置 deny/confirm 规则列表开始，覆盖破坏性文件操作、
  磁盘格式化、关机/重启和提权模式。
- 只有当任务运行器能明显简化命令时才引入额外工具；否则 package scripts
  和 Cargo 命令已经足够。

如果实现过程中发现某个默认决策不可行，必须更新 OpenSpec artifacts，而不是
只在代码里悄悄改变。
