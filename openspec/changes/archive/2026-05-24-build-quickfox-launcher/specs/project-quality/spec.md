## ADDED Requirements

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
