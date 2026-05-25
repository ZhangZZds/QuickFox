## ADDED Requirements

### Requirement: DuckDuckGo 网页搜索配置

系统 SHALL 支持通过配置注册 DuckDuckGo 搜索引擎，并用配置前缀生成搜索 URL。

#### Scenario: DuckDuckGo 前缀生成搜索 URL

- **WHEN** 配置 `ddg` 前缀指向 `https://duckduckgo.com/?q={query}` 且用户输入 `ddg tauri`
- **THEN** 网页搜索 Provider 返回打开 DuckDuckGo 搜索 URL 的结果

#### Scenario: DuckDuckGo 查询编码

- **WHEN** 用户输入 `ddg rust tauri`
- **THEN** 网页搜索 Provider 生成包含 URL 编码查询的 DuckDuckGo 地址

## MODIFIED Requirements

### Requirement: 网页搜索 Provider

系统 SHALL 只在查询使用已配置网页搜索前缀时返回网页搜索结果，并使用当前有效配置中的 URL 模板生成动作。

#### Scenario: 配置前缀生成搜索 URL

- **WHEN** 配置 `g` 前缀指向 `https://www.google.com/search?q={query}` 且用户输入 `g: tauri`
- **THEN** 网页搜索 Provider 返回打开 Google 搜索 URL 的结果

#### Scenario: 无本地结果不自动网页搜索

- **WHEN** 用户输入普通查询且本地没有匹配结果
- **THEN** 系统不自动生成网页搜索结果

#### Scenario: 删除前缀后不再触发

- **WHEN** 用户从配置中删除 `ddg` 前缀
- **THEN** `ddg tauri` 不再触发 DuckDuckGo 网页搜索 Provider
