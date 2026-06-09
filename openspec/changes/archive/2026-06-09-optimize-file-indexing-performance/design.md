## Overview

本设计把索引优化拆成六层：扫描管线、索引模型、查询解析、普通 name/path 匹配、内容全文索引、运行期增量更新。目标是在同一轮演进中同时解决 Windows 首次索引慢、查询表达力弱、缺少内容搜索、索引状态不可观察和运行时文件变化不能及时反映的问题。

## Current State

- 扫描器使用自研递归 `std::fs::read_dir`。
- 排除规则是简单文件名/目录名通配，已经包含系统噪音和构建缓存，但不支持 `.gitignore`/`.ignore`。
- 扫描每轮仍遍历配置范围；现有“增量刷新”只是复用未变化 entry 对象，不是真正局部刷新。
- `IndexedEntry` 只保存 `path/name/kind`，无法判断 mtime/size 是否变化。
- `SearchIndex` 有 `search_texts` 预计算字段，但普通查询仍主要重新 lower-case name/path。
- 文件搜索仍只支持普通 name/path 查询，不支持 `type:`、`name:`、`dir:` 或 `content:` 字段查询。

## Recommended Dependencies

### `ignore`

采用 `ignore::WalkBuilder` 作为扫描底座。它提供快速递归目录 iterator，支持 globs、file types、`.gitignore`/`.ignore` 过滤，并提供 `WalkParallel` 并行递归遍历能力。它是 ripgrep 生态的成熟组件，适合替换当前自研扫描器。

### `notify`

用于监听已索引根目录变化。直接使用原始事件容易抖动，设计上应搭配 debounce 层，例如 `notify-debouncer-mini` 或 `notify-debouncer-full`，将短时间内的 create/remove/rename/write 合并为批处理。本次只要求运行期 watcher，不要求跨重启持久事件队列。

### `nucleo`

用于替换或增强手写 fuzzy。`nucleo` 提供后台 threadpool matcher 和 snapshot 模式，适合启动器中“边输入边匹配”的模型。接入时必须把 QuickFox 的类型优先级、历史权重和内容命中权重保留在最终 ranker 中，避免 matcher 直接决定用户可见排序。

### `tantivy`

采用 Tantivy 作为 `content:` 的内容全文索引。Tantivy 索引与 SQLite 文件快照解耦：SQLite 保存文件路径、mtime、size、extension、root 等轻量元信息；Tantivy 保存可全文检索的文本内容、行偏移和必要的 doc id 映射。第一版只抽取 UTF-8/常见文本文件内容，PDF/Office 等二进制文档通过 extractor trait 后续扩展。

## Architecture

### Scanner Layer

新增 `FileSystemScanner` trait，当前实现可保留为 `StdFsScanner`，新实现为 `IgnoreScanner`。

`IgnoreScanner` 负责：

- 使用 `WalkBuilder` 添加所有 include roots。
- 应用 QuickFox 隐式排除规则和用户排除规则。
- 启用 `.gitignore`/`.ignore` 支持，但保留 QuickFox 的系统级强制排除。
- 按 root 汇总 scanned、accepted、skipped、failures。
- 对 `.app` bundle 做平台特殊处理，应用入口只记录 bundle 本身，不进入内部。

### Index Entry Model

扩展条目结构：

- `path`
- `name`
- `kind`
- `parent`
- `extension`
- `depth`
- `root`
- `modified_ms`
- `size_bytes`
- `search_text`
- `content_index_state`

`search_text` 可以直接持久化，也可以启动加载时重建。第一阶段建议启动加载时重建，以减少快照迁移风险；mtime/size/root 等元信息应持久化。`content_index_state` 记录该路径是否未尝试、已索引、因大小跳过、因类型跳过、因读取失败或等待后台抽取。

### Snapshot Schema

SQLite 增加 schema version。迁移策略：

- 旧 snapshot 缺少元信息时仍可加载，但标记为 `needs_full_refresh`。
- 新 snapshot 保存每个 entry 的轻量元信息。
- 启动时先加载旧快照提供搜索，再后台按新 schema 刷新。

### Phased Indexing

首次构建分为阶段：

1. `applications`: 平台应用入口，例如 Windows Start Menu、Program Files、macOS Applications、Linux desktop entries。
2. `user-hot-paths`: Desktop、Documents、Downloads、Projects/workspace 等常用用户目录。
3. `configured-roots`: 用户显式 include dirs。
4. `remaining-drives`: Windows 其他可用盘符的剩余普通目录。

每个阶段完成后可以更新内存索引和快照，让用户尽早搜到高价值结果。

内容索引采用更保守的默认范围：

- Windows 默认只对 Desktop/桌面进行内容索引；其他磁盘默认只做 name/path 索引，除非用户配置内容索引目录。
- macOS 默认对 Desktop、Documents、Downloads、workspace 等常用用户目录进行内容索引。
- 内容文件大小上限默认 2MB，可配置；超限文件仍参与 name/path 搜索，但不进入内容索引。

### Incremental Strategy

第一阶段做 snapshot-assisted refresh：

- 对目录记录 mtime 或平台可得的变化信号。
- 对文件记录 mtime/size。
- 未变化路径复用旧 entry。
- 删除路径从新报告中移除。

运行期接 `notify`：

- 对根目录建立递归 watcher。
- 事件进入 debounce 队列。
- 批处理更新受影响路径及子树。
- watcher 失败时降级到后台全量/分批刷新。
- 不持久化 watcher 事件队列；应用重启后从快照加载，再按阶段 refresh。

### Query Strategy

查询解析输出结构化 `FileQuery`：

- ordinary terms: 没有字段前缀的普通词，继续用于 name/path fuzzy 搜索。
- `type:<ext>`: 按扩展名精确过滤，`type:pdf` 匹配 `.pdf`。
- `name:<text>`: 对文件名做 contains 匹配。
- `dir:<text-or-glob>`: 无通配符时对父目录路径/路径段做 contains；包含 `*`、`?` 或 `[` 时按 glob 匹配目录。
- `content:<query>`: 使用 Tantivy 默认 query parser 解析内容查询；支持双引号短语。
- 字段值支持双引号，允许空格和 Windows 路径，例如 `name:"project report"`、`dir:"D:\My Projects"`。

执行顺序：

1. 普通词、`type:`、`name:`、`dir:` 先在文件元信息索引上生成候选集合。
2. 如果存在 `content:`，再在候选集合中执行内容检索；没有候选条件时则对全局内容索引检索。
3. 字段条件之间使用 AND。
4. `content:` 命中结果必须返回匹配片段，上下各 5 行，并返回结构化高亮范围供前端渲染。

普通 name/path 匹配：

- 普通查询使用 `search_texts` 或 entry 内预计算 lower-case 字段，避免每次 lower-case path/name。
- 保持候选上限，在匹配阶段尽早 stop 或保留 top candidates。
- 继续避免父目录中间片段带出无关子文件。
- 引入 `nucleo` 对 name/path candidates 做 fuzzy score。
- 保留 QuickFox 类型优先级和历史排序作为 final ranker 的输入。

排序规则：

- 有普通词、`type:`、`name:` 或 `dir:` 时，普通候选相关性先决定主排序，`content:` 命中和 Tantivy score 作为加权信号。
- 只有 `content:` 时，主要按 Tantivy 内容相关性排序，再叠加类型优先级和历史信号。

## Risks

- Windows 多盘根目录遍历可能扫到慢速外接盘或网络映射盘；第一阶段应只发现本地可访问盘符，并在状态中显示当前 root。
- `.gitignore` 语义可能排除用户期望搜索的项目文件；QuickFox 应允许关闭“尊重项目 ignore”或提供说明。
- `notify` 在不同平台行为差异大，必须 debounce，并提供全量刷新 fallback。
- 快照 schema 迁移必须兼容旧版本，不能导致启动时文件搜索完全不可用。
- 内容索引可能读取敏感文件内容；默认范围必须保守，设置页必须说明内容索引会读取并本地索引文件文本。
- Tantivy index schema 和 SQLite snapshot schema 版本需要分别管理，避免升级后旧内容索引污染结果。

## Open Questions

- PDF/Office extractor 何时进入后续变更，本次只保留扩展边界。
- 内容索引默认目录是否需要在首次启用时弹出隐私提示，本次先要求设置页清晰说明。
