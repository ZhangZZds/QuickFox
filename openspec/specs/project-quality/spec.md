# project-quality Specification

## Purpose

TBD - created by archiving change build-quickfox-launcher. Update Purpose after archive.

## Requirements

### Requirement: 中文维护文档

系统 SHALL 提供中文维护文档，说明依赖安装、本地运行、测试、构建、架构和
开发扩展方式。

#### Scenario: README 说明常用操作

- **WHEN** 维护者打开 `README.md`
- **THEN** 文档说明如何安装依赖、运行开发环境、执行测试和构建应用

#### Scenario: 开发文档说明扩展方式

- **WHEN** 维护者打开 `docs/development.md`
- **THEN** 文档说明如何新增 Provider、Action 或平台 Adapter

### Requirement: 项目级 agent 规则

系统 SHALL 在仓库根目录提供 `AGENTS.md`，记录长期工程规则。

#### Scenario: Agent 读取项目规则

- **WHEN** Codex 或其他 agent 在仓库中工作
- **THEN** `AGENTS.md` 提供中文沟通、流程、架构边界、测试和安全规则

### Requirement: 本地质量检查

系统 SHALL 提供统一本地检查命令，覆盖 Rust 格式化、clippy、测试，以及
前端 lint、格式化、测试和构建。

#### Scenario: 本地检查通过

- **WHEN** 维护者运行统一检查命令
- **THEN** 系统执行 Rust 和前端的格式、lint、测试及构建检查

### Requirement: GitHub Actions Windows/Linux CI

系统 SHALL 使用标准 GitHub-hosted Windows 和 Linux runner 执行普通 push/PR
检查，不使用 larger runner。

#### Scenario: Pull request 触发 CI

- **WHEN** 有 pull request 打开或更新
- **THEN** GitHub Actions 在 Windows 和 Linux 标准 runner 上运行核心检查

#### Scenario: 普通 CI 不发布安装包

- **WHEN** 普通 push 或 pull request 触发 CI
- **THEN** workflow 不构建或上传发布安装包

### Requirement: 无死代码规则

系统 SHALL 不保留死代码、废弃代码或未使用的生成示例代码。

#### Scenario: 删除未使用生成示例

- **WHEN** 项目脚手架生成与 QuickFox 无关的示例代码
- **THEN** 实现任务删除或替换这些示例代码

### Requirement: TDD 和完成前验证

系统 SHALL 对行为变化使用测试驱动开发，并在声称完成前执行验证命令。

#### Scenario: 新功能先有失败测试

- **WHEN** 实现新的 Provider、Action、配置或平台规则
- **THEN** 先添加能覆盖预期行为的测试，再实现生产代码

#### Scenario: 完成前验证

- **WHEN** 准备声明 OpenSpec 任务完成
- **THEN** 运行相关测试、构建、lint 和 `openspec validate`

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

### Requirement: 启动器视觉可用性验收

系统 SHALL 为快速启动器关键状态提供视觉可用性验收，覆盖单元测试难以发现的真实布局、
窗口外壳、背景污染、文本溢出和状态可读性问题。

#### Scenario: 启动器首屏截图验收

- **WHEN** 维护者完成启动器首屏相关变更
- **THEN** 验收记录包含启动器空输入状态截图或等价视觉检查
- **AND** 该检查确认启动器是紧凑浮层且没有被桌面背景文字或设置页窗口外壳干扰

#### Scenario: 索引异常状态截图验收

- **WHEN** 维护者完成索引未建立、构建中或失败状态相关变更
- **THEN** 验收记录包含至少一个索引异常状态截图或等价视觉检查
- **AND** 该检查确认状态文案、恢复动作和输入框没有重叠或溢出

#### Scenario: 有结果状态截图验收

- **WHEN** 维护者完成结果列表相关变更
- **THEN** 验收记录包含有结果状态截图或等价视觉检查
- **AND** 该检查确认结果列表紧凑、选中态明显且路径/标题文本可辨认

### Requirement: 本地开发页面降级可观察

系统 SHALL 让维护者在 Vite 浏览器页面中调试前端时能够看到可控降级状态，
而不是被 Tauri API 初始化错误淹没。

#### Scenario: 浏览器开发环境不产生未处理 Tauri 监听错误

- **WHEN** 维护者通过普通浏览器打开 Vite 开发页面
- **THEN** 前端不产生未处理的 Tauri event listen 初始化错误
- **AND** 页面显示可用于布局调试的降级状态或测试数据入口

### Requirement: 发布打包

QuickFox SHALL 为 1.0.0 及后续版本提供由 tag 触发的 GitHub Release workflow。

#### Scenario: Tag 创建 macOS 和 Windows 发布资产

- **WHEN** 推送 `v1.0.0` tag
- **THEN** GitHub Actions 构建 macOS 和 Windows Tauri bundle
- **AND** 将生成的安装器产物上传到 GitHub Release

#### Scenario: 普通 CI 只做验证

- **WHEN** 分支 push 或 pull request 触发普通 CI
- **THEN** CI 运行检查和测试
- **AND** 不发布 release 资产

### Requirement: 大规模性能回归测试

系统 SHALL 为文件搜索提供可重复运行的大规模性能回归测试，覆盖查询延迟、候选数量、结果质量和内存预算。

#### Scenario: 性能测试包含最坏查询路径

- **WHEN** 维护者运行文件搜索性能测试
- **THEN** 测试覆盖高命中、低命中、命中靠后、路径段 fuzzy、`agents.md` 精确文件名和字段过滤组合
- **AND** 测试输出每类查询的耗时和候选数量

#### Scenario: 性能退化阻止完成声明

- **WHEN** 变更影响索引、matcher、ranker、snapshot 或内容索引
- **THEN** 完成前验证必须运行相关性能回归测试或明确说明未运行原因
- **AND** 若性能阈值失败，不得声明该变更完成

### Requirement: Windows 多盘手工验收记录

系统 SHALL 为 200 万文件级 Windows 多盘场景维护手工验收记录，覆盖真实发布构建下的输入体验、内存和索引状态。

#### Scenario: Windows 大索引验收记录核心指标

- **WHEN** 维护者执行 Windows C/D 多盘手工验收
- **THEN** 验收记录包含索引 entry 数、磁盘规模、QuickFox 内存占用、`agents.md` 输入录屏或等价观察、查询响应和索引状态截图

#### Scenario: 发布前检查大索引风险

- **WHEN** 准备发布包含搜索索引变更的版本
- **THEN** 维护者检查最近一次 Windows 多盘验收记录
- **AND** 若记录缺失或指标明显退化，发布说明必须标记风险或阻止发布

### Requirement: macOS 与 Windows 增量索引发布验收

系统 SHALL 为影响运行期索引的发布维护 macOS 与 Windows 真实桌面验收记录，覆盖普通文件变化、事件风暴、休眠唤醒、失败降级和恢复路径。

#### Scenario: 普通文件变化验收

- **WHEN** 维护者在 macOS 或 Windows 发布构建的已监听目录中创建、修改、重命名和删除文件
- **THEN** 验收记录包含变化进入或离开搜索结果的耗时
- **AND** 普通变化在 watcher 到达后不超过 10 秒生效

#### Scenario: 事件风暴和休眠唤醒验收

- **WHEN** 维护者执行批量 Git checkout、依赖安装或大目录移动，或者让系统休眠后唤醒
- **THEN** 验收记录确认 QuickFox 窗口和查询保持响应
- **AND** watcher overflow 或重启状态可观察
- **AND** dirty-root 校准或后台刷新最终恢复一致结果

#### Scenario: watcher 失败降级验收

- **WHEN** 维护者模拟监听权限失败、root 断开或 watcher 初始化失败
- **THEN** 文件搜索继续使用最近可用 baseline
- **AND** 设置或状态反馈显示结构化失败摘要和可用恢复动作

#### Scenario: 发布前检查双平台记录

- **WHEN** 准备发布包含运行期索引变更的版本
- **THEN** 维护者检查最近一次 macOS 与 Windows 增量索引验收记录
- **AND** 记录缺失或门槛失败时不得声明该能力完成
