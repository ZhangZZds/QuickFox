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

系统 SHALL 通过配置控制索引目录、排除规则、正则前缀、网页搜索引擎、命令
前缀、命令执行开关、输入历史策略和结果数量限制。

#### Scenario: 修改正则前缀

- **WHEN** 用户将正则前缀配置为 `regex:`
- **THEN** 系统使用 `regex:` 触发正则搜索

#### Scenario: 配置网页搜索引擎

- **WHEN** 用户配置一个新的网页搜索前缀和 URL 模板
- **THEN** 系统允许该前缀通过 `prefix query` 语法触发对应网页搜索结果

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

### Requirement: 历史隐私控制

系统 SHALL 提供清空输入历史、关闭输入历史和调整最大条数的设置。

#### Scenario: 清空输入历史

- **WHEN** 用户在设置中清空输入历史
- **THEN** 系统删除已保存的输入历史

#### Scenario: 关闭输入历史

- **WHEN** 用户关闭输入历史
- **THEN** 系统不再持久化后续确认执行的输入

### Requirement: 图形化管理网页搜索引擎

系统 SHALL 允许用户在设置页新增、编辑和删除网页搜索引擎配置。

#### Scenario: 新增 DuckDuckGo 搜索

- **WHEN** 用户新增前缀 `ddg`、名称 `DuckDuckGo`、URL 模板 `https://duckduckgo.com/?q={query}`
- **THEN** 系统保存配置，并允许 `ddg privacy` 触发 DuckDuckGo 搜索结果

#### Scenario: 模板缺少占位符时阻止保存

- **WHEN** 用户新增或编辑网页搜索引擎且 URL 模板缺少 `{query}`
- **THEN** 设置页显示校验错误并阻止保存该引擎

### Requirement: 图形化管理索引规则

系统 SHALL 允许用户在设置页管理索引包含目录、排除目录和排除模式，并在保存影响索引的配置后触发后台刷新；索引目录、排除目录和排除模式 SHALL 作为索引分区的主规则编辑列展示，不被正则前缀、配置路径或其他辅助信息压缩。

#### Scenario: 保存索引目录触发后台刷新

- **WHEN** 用户在设置页修改索引目录并保存
- **THEN** 系统保存配置并启动后台索引刷新

#### Scenario: 排除规则保存后生效

- **WHEN** 用户新增排除目录或排除模式并保存
- **THEN** 后续索引刷新跳过匹配的目录或条目

#### Scenario: 主规则编辑优先展示

- **WHEN** 用户打开设置页索引分区
- **THEN** 索引目录、排除目录和排除模式显示在主规则编辑列中，且正则前缀、配置路径等辅助信息不打断该编辑流程

### Requirement: 输入历史默认持久化最近 15 条

系统 SHALL 默认持久化最近 15 条确认执行过的输入，并在搜索框中支持上/下
方向键召回。

#### Scenario: 打开文件后记录输入

- **WHEN** 用户输入查询并按 Enter 打开一个文件结果
- **THEN** 系统记录该次输入历史

#### Scenario: 打开目录后记录输入

- **WHEN** 用户输入查询并按 Enter 打开一个目录结果
- **THEN** 系统记录该次输入历史

#### Scenario: 执行命令后记录输入

- **WHEN** 用户输入命令查询并按 Enter 确认执行命令
- **THEN** 系统记录该次输入历史

#### Scenario: 打开网页搜索后记录输入

- **WHEN** 用户输入网页搜索查询并按 Enter 打开搜索 URL
- **THEN** 系统记录该次输入历史

#### Scenario: 未执行不记录输入

- **WHEN** 用户输入查询后关闭窗口、按 Esc 或未按 Enter 执行任何结果
- **THEN** 系统不记录该次输入历史

#### Scenario: 输入历史最多 15 条

- **WHEN** 用户确认执行第 16 条不同输入且最大条数为默认值
- **THEN** 系统移除最旧输入并保留最近 15 条

#### Scenario: 方向键召回输入历史

- **WHEN** 搜索框聚焦且用户按上/下方向键进入历史召回
- **THEN** 系统按时间顺序在最近输入中切换并显示到搜索框

### Requirement: 设置页显示配置文件位置

系统 SHALL 在设置页显示当前配置文件的完整路径，便于维护者定位和排查配置问题；配置文件路径 SHALL 位于索引分区的辅助信息列。

#### Scenario: 配置文件路径可见

- **WHEN** 用户打开设置页的索引分区或辅助信息列
- **THEN** 系统显示 QuickFox 当前使用的配置文件路径

#### Scenario: 配置文件路径位于辅助信息列

- **WHEN** 用户打开设置页索引分区
- **THEN** 配置文件路径显示在辅助信息列中，而不是夹在索引目录、排除目录或排除模式字段之间

### Requirement: 设置页级保存配置

系统 SHALL 将保存配置作为设置页级别操作，并保存所有分区的当前配置草稿。

#### Scenario: 统一保存所有分区改动

- **WHEN** 用户修改任意设置分区并点击统一保存设置
- **THEN** 系统保存当前配置草稿中的所有分区改动

### Requirement: 全局唤醒键配置持久化

系统 SHALL 在 QuickFox 配置中持久化全局唤醒键，并在启动、保存和运行时使用该配置。

#### Scenario: 默认配置包含双击 Shift

- **WHEN** QuickFox 首次创建默认配置
- **THEN** 配置包含全局唤醒键默认值 `Shift+Shift`

#### Scenario: 保存自定义唤醒键

- **WHEN** 用户在设置页录制 `Control+Space` 并保存设置
- **THEN** 配置文件持久化该全局唤醒键
- **AND** 后续启动继续使用该唤醒键

#### Scenario: 无效唤醒键回退默认值

- **WHEN** 配置文件包含无法解析的全局唤醒键
- **THEN** 系统使用 `Shift+Shift` 作为运行时回退
- **AND** 设置页显示可操作的状态提示

#### Scenario: 保存配置校验唤醒键

- **WHEN** 用户保存空唤醒键或单个非双击修饰键
- **THEN** 系统拒绝保存或回退为有效默认值
- **AND** 设置页提示用户录制组合键或双击 Shift

### Requirement: 索引性能配置

系统 SHALL 提供内部可扩展的索引性能配置边界，用于控制扫描阶段、项目忽略规则、内容索引范围和运行期文件监听策略。系统 MUST 让 `fast`、`balanced` 和 `complete` 三种索引性能模式产生可观察、可测试的扫描计划差异。

#### Scenario: 配置是否尊重项目忽略文件

- **WHEN** 配置启用尊重项目忽略规则
- **THEN** 扫描器应用 `.gitignore` 和 `.ignore`
- **AND** 该配置默认启用

#### Scenario: 配置关闭项目忽略文件

- **WHEN** 配置关闭尊重项目忽略规则
- **THEN** 扫描器只使用 QuickFox 用户排除规则和系统强制排除规则

#### Scenario: fast 模式只索引高价值快速范围

- **WHEN** 配置选择 `fast` 索引性能模式
- **THEN** 系统扫描应用入口和用户热路径
- **AND** 系统不自动补扫 `D:\` 等配置大根目录
- **AND** 设置页说明该模式优先首次可用但结果范围更窄

#### Scenario: balanced 模式先快速可用再后台补全配置范围

- **WHEN** 配置选择 `balanced` 索引性能模式
- **THEN** 系统先扫描应用入口和用户热路径
- **AND** 系统随后在后台补全用户配置的索引目录
- **AND** 该模式作为默认模式

#### Scenario: complete 模式覆盖完整配置范围

- **WHEN** 配置选择 `complete` 索引性能模式
- **THEN** 系统扫描应用入口、用户热路径和完整用户配置范围
- **AND** 设置页说明该模式首次索引更慢但结果覆盖更完整

#### Scenario: 保存性能模式触发索引计划更新

- **WHEN** 用户在设置页修改索引性能模式并保存设置
- **THEN** 系统保存该模式
- **AND** 后续后台索引按新模式生成扫描计划

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

### Requirement: 自动增量索引配置

系统 SHALL 提供自动增量索引开关，新安装和没有显式配置的现有安装 MUST 默认开启；开关只控制运行期 watcher/coordinator，不得删除或禁用现有文件搜索索引。

#### Scenario: 自动增量默认开启

- **WHEN** QuickFox 创建默认配置或加载缺少 watcher 配置的旧配置
- **THEN** 自动增量索引开关为开启状态

#### Scenario: 用户关闭自动增量

- **WHEN** 用户在设置中关闭自动增量索引并保存
- **THEN** 系统停止运行期 watcher 和新事件消费
- **AND** baseline、journal 和当前文件搜索结果保持可用
- **AND** 用户仍可触发手动增量刷新

#### Scenario: 用户重新开启自动增量

- **WHEN** 用户重新开启自动增量索引并保存
- **THEN** 系统启动 watcher/coordinator
- **AND** 系统先对配置 root 执行增量校准，再声明自动增量已正常运行

#### Scenario: watcher 开关变化不全量重建

- **WHEN** 用户只修改自动增量索引开关而未改变索引语义配置
- **THEN** 系统不得仅因该开关变化触发全量索引重建
