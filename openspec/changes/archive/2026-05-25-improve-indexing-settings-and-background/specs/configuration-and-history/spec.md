## ADDED Requirements

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

## MODIFIED Requirements

### Requirement: 配置可修改核心行为

系统 SHALL 通过配置和设置页控制索引目录、排除规则、正则前缀、网页搜索引擎、命令前缀、命令执行开关、历史策略和结果数量限制。

#### Scenario: 修改正则前缀

- **WHEN** 用户将正则前缀配置为 `regex:`
- **THEN** 系统使用 `regex:` 触发正则搜索

#### Scenario: 配置网页搜索引擎

- **WHEN** 用户配置一个新的网页搜索前缀和 URL 模板
- **THEN** 系统允许该前缀触发对应网页搜索结果
