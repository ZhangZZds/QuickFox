# search-index Specification

## Purpose

TBD - created by archiving change build-quickfox-launcher. Update Purpose after archive.

## Requirements

### Requirement: 默认索引用户主目录

系统 SHALL 在 macOS/Linux 默认索引当前用户的主目录，在 Windows 默认索引当前可用盘符根目录，并只索引文件名、目录名和完整路径；系统 MUST 默认排除应用包内部、构建产物、缓存目录和系统隐藏噪音目录。

#### Scenario: 首次启动创建默认索引范围

- **WHEN** 用户首次启动 QuickFox 且没有自定义索引配置
- **THEN** macOS/Linux 使用当前用户主目录作为默认索引根目录
- **AND** Windows 使用当前可用盘符根目录作为默认索引根目录

#### Scenario: 索引不读取文件内容

- **WHEN** 系统刷新索引
- **THEN** 系统记录文件名、目录名和完整路径，但不读取文件内容

#### Scenario: macOS 应用包内部不进入普通索引

- **WHEN** 系统扫描 macOS `.app` 应用包
- **THEN** 系统将 `.app` 作为应用结果索引，并跳过 `.app/Contents` 内部文件

### Requirement: 可配置索引包含和排除规则

系统 SHALL 允许用户配置额外索引目录、排除目录和排除模式。

#### Scenario: 额外目录进入索引

- **WHEN** 配置文件包含额外索引目录
- **THEN** 系统在刷新索引时扫描该目录下的文件和目录名称

#### Scenario: 排除目录不进入索引

- **WHEN** 配置文件包含排除目录
- **THEN** 系统在刷新索引时跳过该目录

### Requirement: 手动刷新索引

系统 SHALL 提供手动增量刷新，并在部分目录失败时继续处理其他可用目录；手动刷新 MUST 优先提交待处理 watcher 事件并使用目录清单校准变化路径，只有索引语义配置改变、持久化状态无法恢复或校准无法建立可信差异时才升级为后台全量重建。

#### Scenario: 手动刷新默认执行增量校准

- **WHEN** 用户触发手动刷新且 baseline、journal 和目录清单可用
- **THEN** 系统先提交待处理事件并校准 dirty 或变化目录
- **AND** 后续搜索结果反映新增、删除或修改路径
- **AND** 系统不重新构造明确未变化的文件条目

#### Scenario: 索引语义变化触发全量重建

- **WHEN** 索引包含目录、排除目录、排除模式、项目 ignore 或内容索引范围改变
- **THEN** 系统启动后台全量重建
- **AND** 状态明确显示全量重建及触发原因

#### Scenario: 持久化状态异常触发全量重建

- **WHEN** journal、目录清单或 schema 无法恢复到可信增量状态
- **THEN** 系统保留最近可用 baseline
- **AND** 系统在后台全量重建索引
- **AND** 状态明确显示 fallback 原因

#### Scenario: 部分目录失败不阻塞刷新

- **WHEN** 增量校准中某个目录因权限或不存在而失败
- **THEN** 系统报告该目录失败并继续处理其他目录
- **AND** 未确认删除的 baseline 条目保持可用

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

The system SHALL preserve the currently searchable index while a background refresh runs, publish progress after each completed scan stage, and replace the searchable baseline only after the final staged result is successfully persisted. After Tauri setup completes, every available active root selected by the current performance mode SHALL be scheduled for that background refresh even if the event-loop Ready callback is delayed or absent.

#### Scenario: Startup refresh runs without a Ready callback

- **WHEN** a persisted baseline exists and QuickFox finishes setup with indexing enabled
- **THEN** the persisted baseline remains searchable while refresh runs
- **AND** all available active roots selected by the current performance mode are included in the scheduled refresh

#### Scenario: Refresh fails after a quick index is available

- **WHEN** a background refresh fails after a persisted or staged index becomes available
- **THEN** file search continues against the last available index
- **AND** index status exposes the refresh failure
- **AND** a retry can rebuild the index later

#### Scenario: A new file arrives while content indexing is pending

- **WHEN** the durable filename/path baseline has been published and the optional content index is still building
- **THEN** the runtime watcher SHALL already observe active roots selected by the current performance mode
- **AND** a file created after that baseline becomes searchable by its filename or path without waiting for content indexing to finish
- **AND** content installation SHALL reconcile watcher changes before it replaces its baseline

#### Scenario: Excluded filesystem traffic is observed

- **WHEN** the native watcher reports changes under an excluded directory
- **THEN** those events SHALL be rejected before entering the bounded runtime queue
- **AND** they SHALL NOT cause an overflow recovery for otherwise valid active-root changes

#### Scenario: The native event uses a canonical alias of a configured root

- **WHEN** the operating system reports an event through a canonical path alias such as macOS `/private/var` for a configured `/var` root
- **THEN** QuickFox SHALL associate the event with the original active root
- **AND** exclusion patterns SHALL be evaluated only within that active-root boundary
- **AND** index and manifest entries SHALL retain one stable root identity

#### Scenario: The native watcher requests a rescan

- **WHEN** the native backend requests a rescan without reporting a concrete backend failure
- **THEN** QuickFox SHALL schedule targeted calibration for the affected active roots
- **AND** any discovered differences SHALL be committed as an incremental delta
- **AND** a successful calibration SHALL clear the affected degraded-root state without forcing a baseline refresh

### Requirement: 持久化索引快照

系统 SHALL 将成功构建的文件索引 baseline 以可识别完成状态的有界事务持久化到本地
存储，并将后续运行期变化保存为可重放 journal。恢复路径 MUST 只读取已完成
baseline；下次启动 MUST 先加载 active baseline，再重放所有已提交且未合并的 journal
batch。激活新 baseline 后系统 SHALL 清理失效完整批次，阶段 checkpoint 只保留
active baseline 和最新 checkpoint。

#### Scenario: 启动加载基线与增量

- **WHEN** QuickFox 启动且存在 active baseline 与 committed journal
- **THEN** 文件 Provider 使用 baseline 和重放后的增量视图提供搜索结果
- **AND** 自动增量准备或后台刷新不阻塞非文件 Provider

#### Scenario: 旧快照兼容迁移

- **WHEN** QuickFox 升级后只存在旧格式完整快照
- **THEN** 系统加载旧快照作为 baseline 提供搜索
- **AND** 系统在后台建立目录清单和新的增量状态

#### Scenario: 无快照时文件搜索不可用

- **WHEN** QuickFox 启动且没有任何完成的索引 baseline
- **THEN** 文件 Provider 不阻塞查询，并向前端暴露文件搜索暂不可用的状态

#### Scenario: 完整 baseline 写入中进程退出

- **WHEN** QuickFox 在完整 baseline 尚未写完时退出或崩溃
- **THEN** 半成品 baseline 不参与下次恢复
- **AND** 下次启动清理其已写入条目
- **AND** 最近已完成 baseline 继续可用

#### Scenario: 新 baseline 成功激活

- **WHEN** 新完整 baseline 已成功激活
- **THEN** 旧完整 baseline 和过期 checkpoint 被分块删除
- **AND** 新数据库可通过增量页回收归还未使用空间

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

### Requirement: 紧凑 name/path 搜索索引

系统 SHALL 为普通文件搜索维护紧凑内存索引，避免以完整 `IndexedEntry` 字符串集合和重复 search text 作为每次查询的主要工作集。

#### Scenario: 索引字段使用共享存储

- **WHEN** 系统从 snapshot 或扫描结果构建搜索索引
- **THEN** path、name、parent、root、extension 和 path segment 使用共享或去重后的表示
- **AND** 系统避免为同一 entry 长期保存多份等价 search text 字符串

#### Scenario: 搜索结果按需展开

- **WHEN** 候选召回和排序完成
- **THEN** 系统只为最终返回的受限结果展开完整路径、标题、动作和 snippet payload

### Requirement: 普通查询候选召回

系统 SHALL 使用候选召回索引处理普通 name/path 查询，并在小候选集上执行 fuzzy 匹配和 QuickFox ranker 排序。

#### Scenario: 文件名 token 召回候选

- **WHEN** 用户输入普通文件名词项
- **THEN** 系统使用文件名 token 或 prefix 索引召回候选 entry id
- **AND** 只对候选 entry 执行更昂贵的 matcher

#### Scenario: 路径段召回候选

- **WHEN** 用户输入可匹配路径段的普通词项
- **THEN** 系统使用 path segment 索引召回候选 entry id
- **AND** 不需要遍历所有 entry 的完整路径字符串

#### Scenario: 候选召回保留排序语义

- **WHEN** 新候选召回路径与旧线性搜索路径在同一 fixture 上比较
- **THEN** 精确名称、前缀、字段过滤和高质量 fuzzy 结果保持等价或更优
- **AND** 应用、文件、目录类型优先级和历史信号仍由 QuickFox ranker 控制

### Requirement: 扫描进度摘要有界

系统 SHALL 以有界摘要表达扫描进度，不因大文件树中每个 accepted entry 都保留完整路径事件而造成内存线性膨胀。

#### Scenario: 大扫描不保留 per-entry accepted 日志

- **WHEN** 扫描阶段接受大量文件和目录
- **THEN** 长期保留的状态只包含阶段、当前 root、scanned、accepted、skipped、failures 和有限失败摘要
- **AND** 不长期保留每个 accepted entry 的完整 path 事件

#### Scenario: 失败摘要可观察

- **WHEN** 扫描过程中部分目录失败
- **THEN** 系统保留有限失败摘要供设置页和日志展示
- **AND** 失败摘要不会随成功 entry 数量增长

### Requirement: 内容索引 snippet 内存受控

系统 SHALL 避免为所有 content-indexed 文本文件在内存中长期保存全文和按行拆分副本。

#### Scenario: 内容命中按需生成 snippet

- **WHEN** 用户执行 `content:` 查询并产生内容命中
- **THEN** 系统可按需读取命中文件或使用受限 snippet cache 生成片段
- **AND** 不要求所有已索引文本正文常驻内存

#### Scenario: snippet 失败不影响基础搜索

- **WHEN** 内容命中文件已移动、权限变化或 snippet 读取失败
- **THEN** 系统仍返回可用的内容命中或降级反馈
- **AND** 普通 name/path 搜索不受影响

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

### Requirement: 索引扫描跳过系统噪音和构建缓存

系统 SHALL 默认跳过系统目录、回收站、隐藏噪音目录、构建产物、缓存目录和特殊系统文件；用户未显式排除的普通数据目录仍 SHALL 被索引。

#### Scenario: 隐式排除 Windows 系统噪音

- **WHEN** 系统构建 Windows 默认扫描选项
- **THEN** 排除规则包含 `Windows`、`ProgramData`、`PerfLogs`、`System Volume Information`、`$Recycle.Bin`、`Recovery`、`AppData`、Windows 升级目录和虚拟内存文件
- **AND** 排除规则不排除 `Users`、Desktop、Documents 等普通用户数据目录

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

系统 SHALL 在应用运行期间监听已索引根目录变化，通过有界队列和 debounce 批量更新受影响路径；普通事件从 watcher 到达后 MUST 在 10 秒内进入可搜索视图，平台失败或事件溢出 MUST 转为 dirty-root 校准而不是静默丢失一致性。

#### Scenario: 文件创建后进入索引

- **WHEN** 用户在已监听根目录下创建符合索引规则的文件
- **THEN** watcher 将事件发送到有界批处理队列
- **AND** 系统在事件到达后 10 秒内更新该路径的 name/path 索引
- **AND** 若文件符合内容索引范围和大小限制，系统独立更新内容索引

#### Scenario: 文件删除后从索引移除

- **WHEN** 用户删除已索引文件或目录
- **THEN** 系统在事件到达后 10 秒内使用 tombstone 从搜索视图移除对应路径或子树
- **AND** 系统将删除操作写入 committed journal

#### Scenario: 文件重命名原子折叠

- **WHEN** 平台 watcher 报告文件或目录重命名
- **THEN** batcher 将其折叠为旧路径 tombstone 与新路径 targeted scan
- **AND** 查询不同时显示重命名前后的重复结果

#### Scenario: 事件风暴转为 dirty-root 校准

- **WHEN** 平台报告 overflow 或有界 channel 无法接收更多事件
- **THEN** 系统将可识别的受影响 root 标记为 dirty
- **AND** 系统安排目录清单校准或带原因的后台全量刷新
- **AND** 最近可用搜索视图保持可用

#### Scenario: watcher 失败时降级

- **WHEN** 平台 watcher 初始化或运行失败
- **THEN** 系统暴露结构化失败 code 与用户可读摘要
- **AND** 系统回退到手动增量刷新或后台刷新
- **AND** 启动器基础搜索能力仍保持可用

### Requirement: 首次索引快速可用

系统 SHALL 将首次文件索引区分快速可用阶段和后台补全阶段。系统 MUST 在应用入口和用户热路径完成后允许文件 Provider 使用已收录条目返回 name/path 搜索结果，而不等待所有配置大目录扫描完成。

#### Scenario: 快速阶段完成后文件搜索部分可用

- **WHEN** QuickFox 首次启动且没有可用索引快照
- **AND** 应用入口和用户热路径阶段已完成
- **AND** 配置中的大目录仍在后台补全
- **THEN** 文件 Provider 使用已收录条目返回 name/path 结果
- **AND** 启动器显示文件搜索已部分可用且仍在后台补全

#### Scenario: 非文件 Provider 不等待任何索引阶段

- **WHEN** QuickFox 首次启动且文件索引尚未完成快速阶段
- **THEN** 计算器、网页搜索和命令模式仍可返回结果或预览
- **AND** 文件索引状态不覆盖这些非文件结果

#### Scenario: 大根目录后置补全

- **WHEN** 配置包含 `D:\` 或其他大根目录
- **AND** 索引模式允许后台补全
- **THEN** 系统先完成应用入口和用户热路径阶段
- **AND** 再以后台补全阶段扫描该大根目录

### Requirement: 索引阶段边界轻量化

系统 SHALL 避免在每个索引阶段边界重复执行完整聚合快照写入、内容索引构建或其他会造成明显卡顿的重量级工作。系统 SHOULD 将完整快照写入节流到快速可用检查点和最终完成检查点。

#### Scenario: 阶段进度不重复写完整聚合快照

- **WHEN** 后台索引完成一个中间阶段
- **THEN** 系统可以更新状态和内存索引
- **AND** 系统不对不断增长的完整聚合条目无条件写入新的完整 SQLite batch

#### Scenario: 快速可用检查点可持久化

- **WHEN** 快速可用阶段完成
- **THEN** 系统 MAY 保存一次可用快照
- **AND** 后续重启可先使用该快照提供部分文件搜索

#### Scenario: 最终补全保存完整快照

- **WHEN** 后台补全阶段成功完成
- **THEN** 系统保存完整索引快照
- **AND** 后续启动使用完整快照提供文件搜索

### Requirement: 内容索引延后执行

系统 SHALL 将内容索引与基础 name/path 索引解耦。系统 MUST 先让 name/path 文件搜索可用，再以低优先级构建已配置范围内的内容索引。

#### Scenario: name/path 索引不等待内容索引

- **WHEN** 基础文件条目扫描完成
- **AND** 内容索引尚未构建完成
- **THEN** 普通 name/path 文件搜索可用
- **AND** 系统不因内容索引未完成而把文件索引状态显示为不可用

#### Scenario: content 查询等待内容索引时显示反馈

- **WHEN** 用户输入 `content:` 查询
- **AND** 内容索引仍在准备或未启用
- **THEN** 启动器显示内容索引状态反馈
- **AND** 不返回仅按 name/path 命中的假内容结果

### Requirement: 索引补全状态反馈

系统 SHALL 在启动器和设置页中区分未建立、快速可用、后台补全、内容索引中、完整可用和失败状态。状态反馈 MUST 包含当前阶段、当前 root 和可用的扫描统计。

#### Scenario: 启动器提示部分可用

- **WHEN** 文件索引已完成快速可用阶段但后台补全仍在进行
- **THEN** 启动器显示文件搜索已部分可用
- **AND** 若普通文件查询无结果，启动器提示相关范围可能仍在索引

#### Scenario: 设置页显示后台补全详情

- **WHEN** 用户打开设置页索引分区
- **AND** 后台补全正在扫描大根目录
- **THEN** 设置页显示当前模式、阶段、当前 root、已扫描、已收录、已跳过和失败数量

#### Scenario: 补全失败保留已可用索引

- **WHEN** 后台补全阶段因权限、IO 或不可用路径失败
- **THEN** 系统保留快速可用阶段或最近成功快照
- **AND** 启动器和设置页显示补全失败摘要及恢复入口

### Requirement: 索引已建立后的输入搜索响应性

系统 SHALL 在索引已建立后保持启动器输入流畅。系统 MUST 避免每个字符输入都触发与完整索引大小成正比的大对象复制、无界候选构造或过期搜索结果回写。

#### Scenario: 大索引查询不复制完整索引

- **WHEN** 文件索引包含大量条目
- **AND** 用户输入普通文件查询
- **THEN** Rust 搜索路径不为该次查询复制完整 `SearchIndex` 条目集合
- **AND** 查询只构造受候选上限约束的结果

#### Scenario: 连续输入只展示最新查询结果

- **WHEN** 用户连续输入多个字符形成查询
- **AND** 较早查询晚于较新查询返回
- **THEN** 启动器忽略较早查询结果
- **AND** 只展示与当前输入一致的最新查询结果

#### Scenario: 连续输入不阻塞键入

- **WHEN** 用户在索引已建立后快速连续输入文件名
- **THEN** 启动器保持输入响应
- **AND** 系统可以短延迟或合并文件搜索请求以避免每个字符都触发重量级搜索

#### Scenario: 非文件即时反馈不被文件搜索节流破坏

- **WHEN** 用户输入计算器、网页搜索或命令查询
- **THEN** 对应非文件 Provider 的结果或预览仍及时显示
- **AND** 文件搜索节流不延迟这些模式的核心反馈

### Requirement: 分层运行期搜索视图

系统 SHALL 以不可变 compact baseline、可变 delta overlay 和删除 tombstone 组成运行期文件搜索视图；普通增量批次 MUST 只重建 delta 候选结构，不得重建完整 baseline。

#### Scenario: 新增文件只进入增量层

- **WHEN** watcher 批次包含一个符合索引规则的新文件
- **THEN** 系统将该文件加入 delta overlay
- **AND** 系统不重建完整 compact baseline

#### Scenario: 修改条目覆盖基线条目

- **WHEN** delta overlay 包含与 baseline 相同规范化路径的新条目
- **THEN** 查询只使用 overlay 中的新条目
- **AND** 结果中不出现同一路径的 baseline 重复项

#### Scenario: 删除目录屏蔽整个基线子树

- **WHEN** tombstone 标记一个已删除目录
- **THEN** 查询在最终候选截断前屏蔽该目录和全部后代路径
- **AND** 其他 baseline 与 overlay 结果仍可参与排序

#### Scenario: 分层结果保持既有排序语义

- **WHEN** baseline 与 overlay 同时返回匹配候选
- **THEN** 系统使用现有 matcher 和 ranker 语义合并、去重和排序
- **AND** 应用、文件、目录类型优先级与历史权重保持有效

### Requirement: 增量 journal 与崩溃恢复

系统 SHALL 在切换内存增量视图前事务提交可幂等重放的 delta journal，并在启动时从最近可用 baseline 重放所有已提交且未合并的 journal batch。

#### Scenario: 启动重放已提交增量

- **WHEN** QuickFox 启动且存在 baseline 与未合并的 committed journal batch
- **THEN** 系统按 generation 顺序重放 journal
- **AND** 文件 Provider 在重放后使用恢复的 layered view

#### Scenario: 未提交批次不进入恢复视图

- **WHEN** 应用在 journal batch 提交前退出或崩溃
- **THEN** 重启时系统忽略该未提交批次
- **AND** 最近完成的 baseline 与已提交 journal 保持可用

#### Scenario: 重复重放保持幂等

- **WHEN** 同一个 committed journal batch 因恢复重试被再次应用
- **THEN** 系统按规范化路径得到与单次应用相同的 overlay 和 tombstone
- **AND** 不产生重复搜索结果

#### Scenario: journal 损坏时保留基线

- **WHEN** 系统无法解析或一致重放 journal
- **THEN** 系统保留最近可用 baseline 提供搜索
- **AND** 状态暴露 journal 恢复失败和后台全量刷新 fallback 原因

### Requirement: 目录清单增量校准

系统 SHALL 持久化已知目录的轻量指纹和父子关系，并在手动刷新或 dirty-root 恢复时只枚举指纹变化、缺失或新发现的目录。

#### Scenario: 未变化目录不重新枚举文件

- **WHEN** 手动增量刷新发现已知目录的指纹未变化
- **THEN** 系统不对该目录执行 `read_dir` 或重新构造其文件条目
- **AND** 系统继续检查清单中的已知子目录指纹

#### Scenario: 变化目录只比较直接子项

- **WHEN** 已知目录的指纹发生变化
- **THEN** 系统枚举该目录直接子项并与持久化清单比较
- **AND** 只为新增、变化和缺失路径生成 delta 操作

#### Scenario: 新目录递归建立清单

- **WHEN** 校准在变化目录下发现新的子目录
- **THEN** 系统按现有包含和排除规则扫描该子树
- **AND** 系统为新子树保存目录清单和索引条目

### Requirement: Windows 默认索引用户高价值目录

Windows 上系统 SHALL 在首次创建默认索引配置时选择当前用户实际存在的 Desktop、
Documents、Downloads、Projects 和 workspace 等高价值目录。若这些目录均不存在，
系统 SHALL 回退当前用户 profile。盘符根目录只有在用户显式配置时才进入
`balanced` 补全范围。

#### Scenario: Windows 存在 C 盘和 D 盘且文件很多

- **WHEN** 用户首次启动 QuickFox 且没有配置文件
- **THEN** 默认 `include_dirs` 只包含实际存在的用户高价值目录
- **AND** 不自动包含 `C:\` 或 `D:\` 盘符根目录

#### Scenario: 用户高价值目录均不存在

- **WHEN** Windows 用户 profile 下没有可识别的高价值目录
- **THEN** 默认索引范围回退当前用户 profile

#### Scenario: 现有配置仍是旧版自动生成的盘符范围

- **WHEN** 现有 Windows 索引配置与旧版所有盘符默认值及其余默认字段完全一致
- **THEN** 系统将 `include_dirs` 迁移为用户高价值目录
- **AND** 任一索引字段已被用户修改时不执行该迁移

### Requirement: 索引存储磁盘安全边界

系统 SHALL 在写完整 baseline 前估算存储需求，并保留至少 5 GiB 可用空间。单个
baseline 估算超过 8 GiB 时 SHALL 拒绝持久化并提示缩小索引目录，不得继续消耗磁盘。

#### Scenario: 配置范围会突破磁盘预算

- **WHEN** baseline 估算超过 8 GiB 或剩余空间不足估算值加 5 GiB
- **THEN** 系统终止该 baseline 写入
- **AND** 最近可用 baseline 保持不变
- **AND** 索引状态提供可诊断失败信息

### Requirement: 索引配置 revision 可取代且可取消

系统 SHALL 为每个已保存索引语义分配单调递增的 desired revision；后台扫描、持久化和发布 MUST 校验 revision 身份，新 revision MUST 使旧 revision 在遍历过程中协作式取消并不得覆盖新状态。

#### Scenario: 连续保存取代旧扫描

- **WHEN** 用户在大目录扫描期间再次保存不同索引配置
- **THEN** 旧扫描在 root 或 walker entry 取消边界停止
- **AND** 系统只排队并最终发布最新 desired revision

#### Scenario: 旧 revision 完成结果被丢弃

- **WHEN** 旧 worker 在新配置保存后才返回扫描或持久化结果
- **THEN** identity fence 拒绝该结果更新 runtime、baseline 或应用状态

### Requirement: 索引配置应用允许部分 root 降级

系统 SHALL 在单个 active root 不可访问或出现目录项失败时继续处理其他可用 root；只要存在可用搜索视图，失败 MUST 作为 degraded/partial 状态报告而不是回滚 desired config 或清空旧索引。

#### Scenario: 一个盘符离线时其他范围继续

- **WHEN** balanced 或 complete 模式包含多个 roots 且其中一个盘符离线
- **THEN** 可访问 roots 继续扫描并保持或进入搜索视图
- **AND** 离线 root 记录为 dirty 或 failed root
- **AND** 设置页提供重试或修改范围的恢复动作

#### Scenario: 所有新 roots 失败

- **WHEN** 新 revision 没有任何可用 root
- **THEN** 系统把该 revision 标记为应用失败
- **AND** 最近可用旧索引继续参与搜索
- **AND** 已保存的 desired config 不被回滚

### Requirement: Windows 默认索引全部可用盘符

Windows 上系统 SHALL 在首次创建默认索引配置时选择当前可用盘符根目录。默认 `balanced` 模式 MUST 先发布应用入口和用户热路径，再在后台补全盘符范围；盘符或目录失败 MUST 使用 partial/retry 语义，不得撤销默认配置。

#### Scenario: Windows 存在 C 盘和 D 盘

- **WHEN** 用户首次启动 QuickFox 且没有配置文件
- **THEN** 默认 `include_dirs` 包含 `C:\` 和 `D:\`
- **AND** 系统目录仍由隐式规则排除

#### Scenario: 未发现可用盘符

- **WHEN** Windows 盘符发现没有返回任何可用根目录
- **THEN** 默认索引范围回退当前用户 profile

#### Scenario: 现有配置仍是 v1.6.1 自动热路径默认

- **WHEN** 现有 Windows 索引配置与 v1.6.1 自动生成的用户热路径范围及其余默认索引字段完全一致
- **THEN** 系统将 `include_dirs` 迁移为当前可用盘符根目录
- **AND** 任一索引字段已被用户修改时不执行该迁移

#### Scenario: 单个默认盘符不可访问

- **WHEN** 默认全盘索引中一个盘符或子目录不可访问而其他盘符可用
- **THEN** 系统发布其他盘符的可用结果并报告 partial/dirty 状态
- **AND** 默认盘符配置保持不变并提供重试/校准入口
