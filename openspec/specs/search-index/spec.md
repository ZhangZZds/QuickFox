# search-index Specification

## Purpose

TBD - created by archiving change build-quickfox-launcher. Update Purpose after archive.

## Requirements

### Requirement: 默认索引用户主目录

系统 SHALL 默认索引当前用户的主目录或 profile 目录，并只索引文件名、目录
名和完整路径。

#### Scenario: 首次启动创建默认索引范围

- **WHEN** 用户首次启动 QuickFox 且没有自定义索引配置
- **THEN** 系统使用当前用户主目录作为默认索引根目录

#### Scenario: 索引不读取文件内容

- **WHEN** 系统刷新索引
- **THEN** 系统记录文件名、目录名和完整路径，但不读取文件内容

### Requirement: 可配置索引包含和排除规则

系统 SHALL 允许用户配置额外索引目录、排除目录和排除模式。

#### Scenario: 额外目录进入索引

- **WHEN** 配置文件包含额外索引目录
- **THEN** 系统在刷新索引时扫描该目录下的文件和目录名称

#### Scenario: 排除目录不进入索引

- **WHEN** 配置文件包含排除目录
- **THEN** 系统在刷新索引时跳过该目录

### Requirement: 手动刷新索引

系统 SHALL 提供手动刷新索引能力，在后台执行刷新，并在部分目录失败时继续处理其他可用目录。

#### Scenario: 手动刷新更新结果

- **WHEN** 用户触发手动刷新索引
- **THEN** 系统在后台重新扫描配置范围，并在刷新完成后更新后续搜索结果

#### Scenario: 部分目录失败不阻塞索引

- **WHEN** 刷新索引时某个目录因权限或不存在而失败
- **THEN** 系统报告该目录失败并继续处理其他目录

### Requirement: 默认模糊搜索

系统 SHALL 对普通查询使用模糊搜索，并返回文件/目录名称和路径匹配的结果。

#### Scenario: 普通输入匹配路径结果

- **WHEN** 用户输入不带特殊前缀的查询文本
- **THEN** 系统返回名称或路径与查询模糊匹配的文件/目录结果

### Requirement: 显式正则搜索

系统 SHALL 通过可配置正则前缀触发正则搜索，默认前缀为 `re:`。

#### Scenario: 正则前缀匹配 PDF

- **WHEN** 用户输入 `re:.*\.pdf$`
- **THEN** 系统返回路径或名称匹配该正则表达式的结果

#### Scenario: 无效正则显示反馈

- **WHEN** 用户输入带正则前缀但表达式无效的查询
- **THEN** 系统显示无效正则反馈且启动器保持可用

### Requirement: 搜索排序受历史影响

系统 SHALL 在排序时结合匹配质量、路径信号和文件/目录使用历史。

#### Scenario: 最近打开结果排序提升

- **WHEN** 两个结果匹配质量相近且其中一个最近被打开过
- **THEN** 最近打开过的结果排序更靠前

### Requirement: 后台构建索引

系统 SHALL 在后台构建或刷新文件索引，不阻塞 QuickFox 启动、托盘菜单、窗口显示或非文件 Provider 查询。

#### Scenario: 首次启动索引后台运行

- **WHEN** 用户首次启动 QuickFox 且磁盘文件很多
- **THEN** QuickFox 显示启动窗口和托盘能力，文件索引在后台继续构建

#### Scenario: 非文件 Provider 在索引中可用

- **WHEN** 文件索引仍在后台构建
- **THEN** 计算器、网页搜索和命令模式仍可返回结果或预览

### Requirement: 持久化索引快照

系统 SHALL 将成功构建的文件索引快照持久化到本地存储，并在下次启动时先加载最近完成的可用快照。

#### Scenario: 启动加载旧索引

- **WHEN** QuickFox 启动且存在最近完成的索引快照
- **THEN** 文件 Provider 使用该快照提供搜索结果，同时后台刷新索引

#### Scenario: 无快照时文件搜索不可用

- **WHEN** QuickFox 启动且没有任何完成的索引快照
- **THEN** 文件 Provider 不阻塞查询，并向前端暴露文件搜索暂不可用的状态

### Requirement: 索引状态可观察

系统 SHALL 暴露索引状态，至少区分未建立、构建中、可用、使用旧索引刷新中和失败。

#### Scenario: 设置页显示构建中

- **WHEN** 后台索引正在构建
- **THEN** 设置页显示构建中状态和已知条目数量或进度摘要

#### Scenario: 索引失败可恢复

- **WHEN** 后台索引因权限或 IO 错误失败
- **THEN** 系统保留应用可用性，显示失败原因摘要，并允许用户重新触发索引

### Requirement: 大索引搜索性能

系统 SHALL 对文件搜索使用预计算的匹配字段和结果上限，避免每次查询对全量条目重复拼接路径文本或重复大小写转换。

#### Scenario: 查询复用预计算字段

- **WHEN** 用户输入普通文件查询
- **THEN** 文件 Provider 使用索引中的预计算搜索文本进行匹配

#### Scenario: 结果构造受限制

- **WHEN** 大量文件匹配同一查询
- **THEN** 系统只构造和排序受配置上限约束的候选结果，避免无界结果分配
