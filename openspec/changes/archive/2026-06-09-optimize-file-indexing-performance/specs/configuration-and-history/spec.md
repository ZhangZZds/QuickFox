## ADDED Requirements

### Requirement: 索引性能配置

系统 SHALL 提供内部可扩展的索引性能配置边界，用于控制扫描阶段、项目忽略规则、内容索引范围和运行期文件监听策略。

#### Scenario: 配置是否尊重项目忽略文件

- **WHEN** 配置启用尊重项目忽略规则
- **THEN** 扫描器应用 `.gitignore` 和 `.ignore`
- **AND** 该配置默认启用

#### Scenario: 配置关闭项目忽略文件

- **WHEN** 配置关闭尊重项目忽略规则
- **THEN** 扫描器只使用 QuickFox 用户排除规则和系统强制排除规则

#### Scenario: 配置索引性能模式

- **WHEN** 配置选择快速、均衡或完整索引模式
- **THEN** 系统按对应模式调整索引阶段和默认扫描范围

#### Scenario: 配置内容索引大小限制

- **WHEN** 用户配置内容索引最大文件大小
- **THEN** 系统只读取大小不超过该限制的文件内容
- **AND** 默认限制为 2MB

#### Scenario: Windows 默认只对桌面内容索引

- **WHEN** QuickFox 在 Windows 上使用默认配置
- **THEN** 内容索引默认范围只包含用户 Desktop/桌面
- **AND** 其他本地盘符默认仍可参与 name/path 索引

#### Scenario: macOS 默认索引常用目录内容

- **WHEN** QuickFox 在 macOS 上使用默认配置
- **THEN** 内容索引默认范围包含 Desktop、Documents、Downloads 和 workspace 等常用用户目录中存在的目录

#### Scenario: 设置说明内容索引隐私含义

- **WHEN** 用户在设置页查看内容索引配置说明
- **THEN** 系统说明 content 搜索会读取并在本机索引已配置范围内的文本文件内容
- **AND** 系统说明超出大小限制或暂不支持 extractor 的文件仍可按文件名和路径搜索
