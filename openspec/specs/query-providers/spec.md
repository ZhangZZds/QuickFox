# query-providers Specification

## Purpose
TBD - created by archiving change build-quickfox-launcher. Update Purpose after archive.
## Requirements
### Requirement: Provider 返回统一结果

系统 SHALL 通过 Provider registry 调用内置 Provider，并将 Provider 输出统一
为 `SearchResult` 供前端展示。

#### Scenario: 多 Provider 合并结果

- **WHEN** 查询同时命中文件 Provider 和计算器 Provider
- **THEN** 系统返回统一格式的合并结果列表

### Requirement: 文件 Provider

系统 SHALL 提供文件 Provider，从名称/路径索引中返回文件和目录结果。

#### Scenario: 文件查询返回打开动作

- **WHEN** 用户输入普通文件查询
- **THEN** 文件 Provider 返回包含主打开动作的文件/目录结果

### Requirement: 计算器 Provider

系统 SHALL 支持常用增强计算器表达式，包括四则运算、括号、小数、百分比、
指数、常用函数和进制字面量。

#### Scenario: 计算指数表达式

- **WHEN** 用户输入 `2^10`
- **THEN** 计算器 Provider 返回结果 `1024`

#### Scenario: 计算函数表达式

- **WHEN** 用户输入 `sqrt(9)`
- **THEN** 计算器 Provider 返回结果 `3`

#### Scenario: 计算进制字面量

- **WHEN** 用户输入 `0xff`
- **THEN** 计算器 Provider 返回十进制结果 `255`

### Requirement: 网页搜索 Provider

系统 SHALL 只在查询使用已配置网页搜索前缀时返回网页搜索结果。

#### Scenario: 配置前缀生成搜索 URL

- **WHEN** 配置 `g` 前缀指向 `https://www.google.com/search?q={query}` 且用户输入 `g: tauri`
- **THEN** 网页搜索 Provider 返回打开 Google 搜索 URL 的结果

#### Scenario: 无本地结果不自动网页搜索

- **WHEN** 用户输入普通查询且本地没有匹配结果
- **THEN** 系统不自动生成网页搜索结果

### Requirement: 命令 Provider

系统 SHALL 仅在命令执行已启用且查询使用命令前缀时返回命令结果。

#### Scenario: 命令执行未启用

- **WHEN** 命令执行未启用且用户输入 `> git status`
- **THEN** 命令 Provider 返回启用提示或设置入口，而不是可执行命令结果

#### Scenario: 命令执行已启用

- **WHEN** 命令执行已启用且用户输入 `> git status`
- **THEN** 命令 Provider 返回需要确认的命令结果

