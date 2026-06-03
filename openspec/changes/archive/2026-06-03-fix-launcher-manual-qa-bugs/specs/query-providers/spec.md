## MODIFIED Requirements

### Requirement: 网页搜索 Provider

系统 SHALL 只在查询使用已配置网页搜索前缀加空格的语法时返回网页搜索结果。

#### Scenario: Google 前缀生成搜索 URL

- **WHEN** 配置 `g` 前缀指向 `https://www.google.com/search?q={query}` 且用户输入 `g 1234`
- **THEN** 网页搜索 Provider 返回打开 Google 搜索 URL 的结果

#### Scenario: 百度前缀生成搜索 URL

- **WHEN** 配置 `bd` 前缀指向 `https://www.baidu.com/s?wd={query}` 且用户输入 `bd 1234`
- **THEN** 网页搜索 Provider 返回打开百度搜索 URL 的结果

#### Scenario: 无查询文本不触发网页搜索

- **WHEN** 用户只输入网页搜索前缀但没有输入空格后的查询文本
- **THEN** 网页搜索 Provider 不返回网页搜索结果

#### Scenario: 无本地结果不自动网页搜索

- **WHEN** 用户输入普通查询且本地没有匹配结果
- **THEN** 系统不自动生成网页搜索结果
