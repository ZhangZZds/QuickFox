# search-index Specification

## Purpose

TBD - created by archiving change build-quickfox-launcher. Update Purpose after archive.

## Requirements

### Requirement: 默认索引用户主目录

系统 SHALL 默认索引当前用户的主目录或 profile 目录，并只索引文件名、目录
名和完整路径；系统 MUST 默认排除应用包内部、构建产物、缓存目录和系统隐藏
噪音目录。

#### Scenario: 首次启动创建默认索引范围

- **WHEN** 用户首次启动 QuickFox 且没有自定义索引配置
- **THEN** 系统使用当前用户主目录作为默认索引根目录

#### Scenario: 索引不读取文件内容

- **WHEN** 系统刷新索引
- **THEN** 系统记录文件名、目录名和完整路径，但不读取文件内容

#### Scenario: macOS 应用包内部不进入普通索引

- **WHEN** 系统扫描 macOS `.app` 应用包
- **THEN** 系统将 `.app` 作为应用结果索引，并跳过 `.app/Contents` 内部文件

#### Scenario: Windows 应用入口作为应用结果

- **WHEN** 系统扫描 Windows `.exe` 或开始菜单 `.lnk` 入口
- **THEN** 系统将其作为应用结果索引

#### Scenario: Linux desktop 文件作为应用结果

- **WHEN** 系统扫描 Linux `.desktop` 文件
- **THEN** 系统将其作为应用结果索引

### Requirement: 可配置索引包含和排除规则

系统 SHALL 允许用户配置额外索引目录、排除目录和排除模式。

#### Scenario: 额外目录进入索引

- **WHEN** 配置文件包含额外索引目录
- **THEN** 系统在刷新索引时扫描该目录下的文件和目录名称

#### Scenario: 排除目录不进入索引

- **WHEN** 配置文件包含排除目录
- **THEN** 系统在刷新索引时跳过该目录

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

### Requirement: 默认模糊搜索

系统 SHALL 对普通查询使用有最低相关性阈值的模糊搜索，并返回文件、目录或
应用名称和路径匹配的结果。

#### Scenario: 普通输入匹配路径结果

- **WHEN** 用户输入不带特殊前缀的查询文本
- **THEN** 系统返回名称或路径与查询模糊匹配且达到最低相关性阈值的结果

#### Scenario: 不存在查询不返回明显无关结果

- **WHEN** 用户输入 `Openspec_123` 且索引中没有相关文件、目录或应用
- **THEN** 系统不返回 PyCharm 内部资源或其他明显无关路径

### Requirement: 显式正则搜索

系统 SHALL 通过可配置正则前缀触发正则搜索，默认前缀为 `re:`。

#### Scenario: 正则前缀匹配 PDF

- **WHEN** 用户输入 `re:.*\.pdf$`
- **THEN** 系统返回路径或名称匹配该正则表达式的结果

#### Scenario: 无效正则显示反馈

- **WHEN** 用户输入带正则前缀但表达式无效的查询
- **THEN** 系统显示无效正则反馈且启动器保持可用

### Requirement: 搜索排序受历史影响

系统 SHALL 在排序时结合结果类型优先级、匹配质量、路径信号和文件/目录/
应用使用历史。

#### Scenario: 应用结果优先于文件和目录

- **WHEN** 应用、文件和目录结果的匹配质量相近
- **THEN** 系统按应用程序、文件、目录的顺序排序

#### Scenario: 最近打开结果排序提升

- **WHEN** 两个同类型结果匹配质量相近且其中一个最近被打开过
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

### Requirement: 索引状态驱动启动器实时反馈

系统 SHALL 在后台索引状态变化时更新启动器可见状态，并在需要时刷新当前查询结果。

#### Scenario: 后台索引完成后更新启动器状态

- **WHEN** 启动器显示文件索引正在建立或尚未建立
- **AND** 后台索引成功完成
- **THEN** 启动器更新为索引可用状态
- **AND** 如果用户当前查询仍未变化，系统重新执行该查询并显示最新结果

#### Scenario: 后台索引失败后更新失败反馈

- **WHEN** 后台索引刷新失败
- **THEN** 启动器和设置页可观察到失败状态
- **AND** 启动器在普通文件查询状态下显示失败摘要和恢复动作

#### Scenario: 使用旧索引刷新时保持结果可用

- **WHEN** 后台索引正在刷新且存在旧索引快照
- **THEN** 文件搜索继续使用旧索引返回结果
- **AND** 启动器以轻量状态提示索引正在更新

### Requirement: 索引状态反馈保留非文件 Provider 可用性

系统 SHALL 在文件索引不可用或刷新中时继续允许非文件 Provider 返回结果，
并在启动器状态反馈中表达这种可用性。

#### Scenario: 索引不可用时网页搜索仍可执行

- **WHEN** 文件索引尚未建立
- **AND** 用户输入有效网页搜索前缀和查询文本
- **THEN** 系统返回网页搜索结果或执行对应打开链接动作
- **AND** 不用文件索引不可用状态覆盖网页搜索结果

#### Scenario: 索引不可用时计算器仍可执行

- **WHEN** 文件索引尚未建立
- **AND** 用户输入可计算表达式
- **THEN** 系统返回计算器结果
- **AND** 不用文件索引不可用状态覆盖计算器结果

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

### Requirement: 设置页显示索引存储位置

系统 SHALL 在设置页显示文件索引快照的本地存储路径，并展示完整索引状态信息；这些信息 SHALL 以状态摘要和辅助信息方式展示，不干扰主规则编辑。

#### Scenario: 索引快照路径可见

- **WHEN** 用户打开设置页索引分区
- **THEN** 系统显示当前索引快照文件位置

#### Scenario: 索引状态信息完整

- **WHEN** 用户打开设置页索引分区
- **THEN** 系统显示索引状态、条目数量、最近完成时间或失败摘要中的可用信息

#### Scenario: 索引状态摘要优先可见

- **WHEN** 用户打开设置页索引分区
- **THEN** 索引状态、条目数量、索引代次和最近完成时间或失败摘要显示在规则编辑区域之前

#### Scenario: 索引快照路径位于辅助信息列

- **WHEN** 用户打开设置页索引分区
- **THEN** 索引快照路径显示在辅助信息列中，而不是夹在索引目录、排除目录或排除模式字段之间

### Requirement: 普通搜索避免路径父级噪音

系统 SHALL 避免普通搜索仅因父级路径中间字符片段命中而返回无关子文件。

#### Scenario: 父级路径中间片段不带出无关文件

- **WHEN** 用户搜索 `Test` 且某个文件名不包含 `test`，仅父级路径 `ComputeStates` 中间包含 `test`
- **THEN** 系统不返回该文件

#### Scenario: 路径段前缀和紧凑缩写仍可命中

- **WHEN** 用户搜索路径段前缀或紧凑缩写，例如 `quickfox` 或 `qfx`
- **THEN** 系统仍可返回该路径段下的相关结果

#### Scenario: 完整路径精确查询命中

- **WHEN** 用户输入一个完整文件路径
- **THEN** 系统返回该路径对应的结果

### Requirement: Windows 默认索引可用本地盘符

Windows 上系统 SHALL 在首次创建默认索引配置时发现可用本地盘符，并把可访问盘符根目录作为默认索引范围；如果没有发现可用盘符，系统 SHALL 回退到当前用户 profile 目录。

#### Scenario: 多个可用盘符进入默认索引范围

- **WHEN** Windows 机器存在 `C:\` 和 `D:\` 等可访问盘符
- **THEN** 默认索引范围包含这些可访问盘符根目录
- **AND** 不只包含当前用户 profile 目录

#### Scenario: 无可用盘符时回退用户目录

- **WHEN** Windows 盘符发现没有返回可访问目录
- **THEN** 系统使用当前用户 profile 目录作为默认索引范围

### Requirement: 索引扫描跳过系统噪音和构建缓存

系统 SHALL 默认跳过系统目录、回收站、隐藏噪音目录、构建产物、缓存目录和特殊系统文件；用户未显式排除的普通数据目录仍 SHALL 被索引。

#### Scenario: 隐式排除 Windows 系统噪音

- **WHEN** 系统构建默认扫描选项
- **THEN** 排除规则包含 `Windows`、`System Volume Information`、`$Recycle.Bin`、`AppData` 等系统噪音目录

#### Scenario: 隐式排除构建和缓存目录

- **WHEN** 系统构建默认扫描选项
- **THEN** 排除规则包含 `node_modules`、`target`、`.git`、`.cache`、`__pycache__`、`dist`、`build` 等构建和缓存目录

#### Scenario: 排除匹配不区分大小写

- **WHEN** 文件或目录名称大小写与排除规则不同
- **THEN** 扫描器仍跳过该文件或目录

### Requirement: 索引扫描减少无意义重复工作

系统 SHALL 在扫描前去重索引根目录，并在遇到单个目录项读取失败时记录失败并继续处理其他可用路径。

#### Scenario: 重复根目录只扫描一次

- **WHEN** 配置中出现重复索引根目录
- **THEN** 扫描器只索引该根目录一次

#### Scenario: 单个目录项失败不中断整轮扫描

- **WHEN** 扫描某个目录项时读取元数据失败
- **THEN** 系统记录该路径失败
- **AND** 继续扫描同级其他文件和目录

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
