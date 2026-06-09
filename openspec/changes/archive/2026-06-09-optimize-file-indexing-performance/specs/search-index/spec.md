## ADDED Requirements

### Requirement: 分阶段构建文件索引

系统 SHALL 将首次索引拆分为多个阶段，并在高价值阶段完成后让文件搜索尽早可用。

#### Scenario: 应用入口优先可用

- **WHEN** QuickFox 首次启动且索引尚未完成
- **THEN** 系统优先索引平台应用入口
- **AND** 应用搜索结果可在剩余磁盘扫描完成前出现

#### Scenario: 常用用户目录优先可用

- **WHEN** 用户存在 Desktop、Documents、Downloads 或 workspace 等常用目录
- **THEN** 系统在全盘剩余目录前优先索引这些目录

#### Scenario: 阶段完成后更新可搜索快照

- **WHEN** 某个索引阶段成功完成
- **THEN** 系统更新内存索引
- **AND** 后续查询可以使用已完成阶段的结果

### Requirement: 索引扫描使用成熟遍历和忽略规则

系统 SHALL 使用支持并行遍历、忽略文件和 glob 规则的扫描器实现，替代自研递归扫描作为主要扫描路径。

#### Scenario: 扫描器尊重 QuickFox 强制排除

- **WHEN** 扫描器遍历系统目录、回收站、构建产物或缓存目录
- **THEN** 系统跳过这些路径
- **AND** 不因项目 ignore 配置重新包含这些路径

#### Scenario: 扫描器支持项目忽略文件

- **WHEN** 索引目录中存在 `.gitignore` 或 `.ignore`
- **THEN** 系统按配置决定是否尊重这些忽略规则

#### Scenario: 并行扫描不中断可用性

- **WHEN** 扫描器并行遍历多个目录
- **THEN** QuickFox 启动器、托盘和非文件 Provider 仍保持响应

### Requirement: 索引条目保存轻量元信息

系统 SHALL 为索引条目保存用于增量判断、排序和诊断的轻量文件系统元信息。

#### Scenario: 条目包含增量判断元信息

- **WHEN** 系统保存文件或目录索引条目
- **THEN** 条目包含路径、名称、类型、父目录、扩展名、深度、所属根目录、修改时间和大小中平台可取得的信息

#### Scenario: 旧快照兼容加载

- **WHEN** QuickFox 启动且只存在旧格式索引快照
- **THEN** 系统仍加载旧快照提供搜索
- **AND** 后台刷新生成新格式快照

### Requirement: 索引进度可观察

系统 SHALL 暴露索引进度摘要，使用户可以知道当前正在扫描哪个阶段和大致处理量。

#### Scenario: 设置页显示当前阶段

- **WHEN** 索引正在运行
- **THEN** 设置页显示当前阶段，例如应用入口、常用目录、配置目录或剩余盘符

#### Scenario: 设置页显示处理计数

- **WHEN** 索引正在运行
- **THEN** 设置页显示已扫描、已接受、已跳过和失败数量中的可用信息

#### Scenario: 启动器显示轻量进度

- **WHEN** 用户在索引运行时打开启动器
- **THEN** 启动器以轻量状态提示索引仍在后台进行
- **AND** 不覆盖计算器、网页搜索或命令模式结果

### Requirement: 结构化文件查询语法

系统 SHALL 支持普通词和字段查询组合，用于按文件名、目录、扩展名和内容收窄搜索结果。

#### Scenario: 普通词仍搜索文件名和路径

- **WHEN** 用户输入 `report budget`
- **THEN** 文件 Provider 将 `report` 和 `budget` 作为普通 name/path 查询词处理
- **AND** 不要求文件内容包含这些词

#### Scenario: type 字段按扩展名过滤

- **WHEN** 用户输入 `type:pdf`
- **THEN** 系统只返回扩展名为 `.pdf` 的文件结果
- **AND** 扩展名匹配不区分大小写

#### Scenario: name 字段按文件名 contains 匹配

- **WHEN** 用户输入 `name:test`
- **THEN** 系统返回文件名包含 `test` 的文件或目录结果
- **AND** 不要求父目录路径包含 `test`

#### Scenario: dir 字段支持 contains

- **WHEN** 用户输入 `dir:workspace`
- **THEN** 系统返回父目录路径或目录路径段包含 `workspace` 的结果

#### Scenario: dir 字段支持 glob

- **WHEN** 用户输入 `dir:**/workspace`
- **THEN** 系统使用 glob 语义匹配父目录路径

#### Scenario: 字段条件使用 AND 组合

- **WHEN** 用户输入 `budget type:pdf dir:workspace`
- **THEN** 系统先按普通词、扩展名和目录条件生成候选集合
- **AND** 只返回同时满足这些条件的结果

#### Scenario: 字段值支持双引号

- **WHEN** 用户输入 `name:"project report" dir:"D:\My Projects"`
- **THEN** 查询解析器将引号内空格和 Windows 路径作为字段值的一部分

### Requirement: content 前缀搜索文件内容

系统 SHALL 在用户显式输入 `content:` 时搜索文件内容，并返回可解释的上下文片段。

#### Scenario: content 查询触发全文索引

- **WHEN** 用户输入 `content:invoice`
- **THEN** 系统使用 Tantivy 内容索引搜索文件文本内容
- **AND** 不把 `invoice` 仅作为文件名查询词处理

#### Scenario: 普通条件先收窄候选再搜索内容

- **WHEN** 用户输入 `workspace type:md content:invoice`
- **THEN** 系统先使用普通词和字段过滤生成候选集合
- **AND** 再在候选集合中匹配内容索引

#### Scenario: 只有 content 时搜索全局内容索引

- **WHEN** 用户输入 `content:"hello world"`
- **THEN** 系统使用 Tantivy 默认 query parser 处理内容查询
- **AND** 在所有已进入内容索引的文件中搜索

#### Scenario: 内容命中返回上下文片段

- **WHEN** 文件内容命中 `content:` 查询
- **THEN** 搜索结果包含命中行上下各 5 行的片段
- **AND** 搜索结果包含结构化高亮范围，供前端渲染命中词

#### Scenario: 未进入内容索引的文件仍参与普通搜索

- **WHEN** 某个文件因大小、类型或读取失败未进入内容索引
- **THEN** 该文件仍可通过普通 name/path 查询返回
- **AND** 该文件不会作为 `content:` 命中结果返回

### Requirement: 文本优先的内容抽取边界

系统 SHALL 以文本文件内容抽取作为第一版内容索引能力，并为后续 PDF/Office extractor 保留扩展边界。

#### Scenario: 文本文件进入内容索引

- **WHEN** 已索引文件是 UTF-8 或可安全识别的常见文本文件
- **THEN** 系统在文件大小限制内读取文本并写入 Tantivy 内容索引

#### Scenario: 二进制文档暂不强制抽取

- **WHEN** 文件是 PDF、Office 或其他需要专用 extractor 的二进制文档
- **THEN** 第一版可以只建立 name/path 元信息索引
- **AND** extractor 边界允许后续在不改变查询语义的情况下加入内容抽取

### Requirement: 运行期文件系统监听

系统 SHALL 在应用运行期间监听已索引根目录变化，并通过 debounce 批量更新文件索引。

#### Scenario: 文件创建后进入索引

- **WHEN** 用户在已监听根目录下创建文件
- **THEN** watcher 将事件合并到批处理队列
- **AND** 系统更新受影响路径的 name/path 索引
- **AND** 若文件符合内容索引范围和大小限制，系统更新内容索引

#### Scenario: 文件删除后从索引移除

- **WHEN** 用户删除已索引文件
- **THEN** 系统从内存索引、快照和内容索引中移除该文件对应记录

#### Scenario: watcher 失败时降级

- **WHEN** 平台 watcher 初始化或运行失败
- **THEN** 系统记录失败状态
- **AND** 回退到后台分批刷新
- **AND** 启动器基础搜索能力仍保持可用

## MODIFIED Requirements

### Requirement: 手动刷新索引

系统 SHALL 提供手动刷新和增量刷新索引能力，并在部分目录失败时继续处理其他可用目录；增量刷新 SHOULD 利用已有快照元信息减少不必要的重复扫描。

#### Scenario: 手动刷新更新结果

- **WHEN** 用户触发手动刷新索引
- **THEN** 系统重新扫描配置范围并更新后续搜索结果

#### Scenario: 增量刷新更新变化路径

- **WHEN** 用户触发增量刷新索引且文件系统中存在新增、删除或修改的路径
- **THEN** 系统更新受影响路径的索引状态并保留未变化路径的历史信号
- **AND** 系统避免对明确未变化的路径重复构造索引条目

#### Scenario: 部分目录失败不阻塞索引

- **WHEN** 刷新索引时某个目录因权限或不存在而失败
- **THEN** 系统报告该目录失败并继续处理其他目录

### Requirement: 大索引搜索性能

系统 SHALL 对文件搜索使用预计算的匹配字段、候选上限和可替换的 fuzzy 匹配策略，避免每次查询对全量条目重复拼接路径文本、重复大小写转换或无界结果分配。

#### Scenario: 查询复用预计算字段

- **WHEN** 用户输入普通文件查询
- **THEN** 文件 Provider 使用索引中的预计算搜索文本进行匹配

#### Scenario: 结果构造受限制

- **WHEN** 大量文件匹配同一查询
- **THEN** 系统只构造和排序受配置上限约束的候选结果，避免无界结果分配

#### Scenario: fuzzy 匹配策略可替换

- **WHEN** 系统切换到底层 fuzzy matcher 实现
- **THEN** 文件搜索的用户可见语义保持稳定
- **AND** 类型优先级和历史排序仍由 QuickFox ranker 控制

#### Scenario: 混合查询排序保留普通候选优先级

- **WHEN** 查询同时包含普通词或字段过滤以及 `content:`
- **THEN** 系统以普通候选相关性作为主排序信号
- **AND** 内容命中和 Tantivy score 作为额外权重参与最终排序

#### Scenario: 纯 content 查询按全文相关性排序

- **WHEN** 查询只包含 `content:`
- **THEN** 系统主要按 Tantivy 内容相关性排序
- **AND** 类型优先级和历史信号仍可作为稳定排序补充
