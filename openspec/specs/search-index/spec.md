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

系统 SHALL 提供手动刷新索引能力，并在部分目录失败时继续处理其他可用目录。

#### Scenario: 手动刷新更新结果

- **WHEN** 用户触发手动刷新索引
- **THEN** 系统重新扫描配置范围并更新后续搜索结果

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

