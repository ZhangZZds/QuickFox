# configuration-and-history Specification

## Purpose

TBD - created by archiving change build-quickfox-launcher. Update Purpose after archive.

## Requirements

### Requirement: 创建默认 TOML 配置

系统 SHALL 在首次启动且配置不存在时创建默认 TOML 配置文件。

#### Scenario: 首次启动生成配置

- **WHEN** QuickFox 启动且未找到配置文件
- **THEN** 系统创建包含默认索引、前缀、历史和命令执行设置的 TOML 配置

### Requirement: 配置可修改核心行为

系统 SHALL 通过配置和设置页控制索引目录、排除规则、正则前缀、网页搜索引擎、命令前缀、命令执行开关、历史策略和结果数量限制。

#### Scenario: 修改正则前缀

- **WHEN** 用户将正则前缀配置为 `regex:`
- **THEN** 系统使用 `regex:` 触发正则搜索

#### Scenario: 配置网页搜索引擎

- **WHEN** 用户配置一个新的网页搜索前缀和 URL 模板
- **THEN** 系统允许该前缀触发对应网页搜索结果

### Requirement: 配置错误可恢复

系统 SHALL 对无效配置提供可操作错误，并在可能时安全回退。

#### Scenario: 网页搜索模板缺少 query 占位符

- **WHEN** 配置的网页搜索 URL 模板缺少 `{query}`
- **THEN** 系统报告配置错误且不注册该搜索引擎

### Requirement: 文件目录历史影响排序

系统 SHALL 默认记录文件/目录打开历史，并将其用于搜索排序。

#### Scenario: 打开文件后记录历史

- **WHEN** 用户打开一个文件结果
- **THEN** 系统记录该文件的使用历史

### Requirement: 命令历史默认持久化最近 15 条

系统 SHALL 默认持久化最近 15 条命令，并在命令模式中支持上/下方向键循环。

#### Scenario: 保存最近命令

- **WHEN** 用户确认执行一条命令
- **THEN** 系统将该命令写入最近命令历史

#### Scenario: 命令历史最多 15 条

- **WHEN** 用户执行第 16 条不同命令且最大条数为默认值
- **THEN** 系统移除最旧命令并保留最近 15 条

#### Scenario: 方向键循环命令历史

- **WHEN** 用户处于命令模式并按上/下方向键
- **THEN** 系统按时间顺序在最近命令中切换

### Requirement: 历史隐私控制

系统 SHALL 提供清空命令历史、关闭命令历史和调整最大条数的设置。

#### Scenario: 清空命令历史

- **WHEN** 用户在设置中清空命令历史
- **THEN** 系统删除已保存的命令历史

#### Scenario: 关闭命令历史

- **WHEN** 用户关闭命令历史
- **THEN** 系统不再持久化后续执行的命令

### Requirement: 图形化管理网页搜索引擎

系统 SHALL 允许用户在设置页新增、编辑和删除网页搜索引擎配置。

#### Scenario: 新增 DuckDuckGo 搜索

- **WHEN** 用户新增前缀 `ddg`、名称 `DuckDuckGo`、URL 模板 `https://duckduckgo.com/?q={query}`
- **THEN** 系统保存配置，并允许 `ddg privacy` 触发 DuckDuckGo 搜索结果

#### Scenario: 模板缺少占位符时阻止保存

- **WHEN** 用户新增或编辑网页搜索引擎且 URL 模板缺少 `{query}`
- **THEN** 设置页显示校验错误并阻止保存该引擎

### Requirement: 图形化管理索引规则

系统 SHALL 允许用户在设置页管理索引包含目录、排除目录和排除模式，并在保存影响索引的配置后触发后台刷新。

#### Scenario: 保存索引目录触发后台刷新

- **WHEN** 用户在设置页修改索引目录并保存
- **THEN** 系统保存配置并启动后台索引刷新

#### Scenario: 排除规则保存后生效

- **WHEN** 用户新增排除目录或排除模式并保存
- **THEN** 后续索引刷新跳过匹配的目录或条目
