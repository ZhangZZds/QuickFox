## 1. 基准、依赖与风险确认

- [x] 1.1 增加索引基准夹具，覆盖小目录、深目录、Windows 多盘模拟、大量排除目录和文本内容文件
- [x] 1.2 记录当前扫描器在基准夹具上的扫描耗时、条目数、失败数、普通查询耗时和内容查询缺失基线
- [x] 1.3 引入并确认 `ignore`、`notify`/debounce、`tantivy`、`nucleo` 的 license、二进制体积、跨平台行为和 Tauri 打包影响
- [x] 1.4 写入依赖选择说明，记录 PDF/Office extractor 暂不进入本次实现的边界

## 2. 扫描器重构

- [x] 2.1 先写 `FileSystemScanner` trait、扫描事件、扫描统计和错误降级的 Rust 单元测试
- [x] 2.2 定义 `FileSystemScanner` trait 和扫描事件/统计结构
- [x] 2.3 保留当前扫描器作为测试对照或 fallback
- [x] 2.4 实现基于 `ignore::WalkBuilder` 的扫描器，支持 QuickFox 强制排除、用户排除、项目 ignore 和并行遍历
- [x] 2.5 覆盖 `.app` bundle、`.exe`、`.lnk`、`.desktop` 应用入口行为
- [x] 2.6 覆盖权限失败、单个目录项失败、重复根目录和慢速/不可用根目录行为

## 3. 索引模型、快照与阶段

- [x] 3.1 先写 `IndexedEntry` 轻量元信息、旧快照兼容和阶段进度 payload 的测试
- [x] 3.2 扩展 `IndexedEntry` 或新增内部 entry model，包含 parent、extension、depth、root、mtime、size、search text、content index state
- [x] 3.3 更新 SQLite schema version 和迁移逻辑，旧快照可加载并触发后台新格式刷新
- [x] 3.4 实现索引阶段：应用入口、常用目录、配置目录、Windows 剩余本地盘符
- [x] 3.5 每个阶段完成后更新内存索引、快照和可观察状态
- [x] 3.6 扩展 `IndexStatus` payload，包含阶段、当前 root、scanned/accepted/skipped/failures

## 4. 普通查询性能与字段查询

- [x] 4.1 先写查询解析测试，覆盖普通词、`type:`、`name:`、`dir:`、`content:`、双引号值和 Windows 路径
- [x] 4.2 实现结构化 `FileQuery` 解析器，字段条件使用 AND 语义
- [x] 4.3 将预计算搜索文本真正用于普通查询匹配，避免重复 lower-case path/name
- [x] 4.4 接入 `nucleo` 或可替换 matcher，保证普通 name/path 查询语义稳定
- [x] 4.5 实现 `type:` 扩展名过滤、`name:` 文件名 contains、`dir:` contains/glob 过滤
- [x] 4.6 增加大索引查询候选上限测试和性能回归测试

## 5. Tantivy 内容索引

- [x] 5.1 先写内容索引测试，覆盖文本文件入库、超限跳过、二进制跳过、内容查询和片段上下 5 行
- [x] 5.2 设计 Tantivy schema、doc id 映射、index 目录版本和与 SQLite 快照的同步策略
- [x] 5.3 实现文本优先 extractor boundary，只读取安全文本文件
- [x] 5.4 实现内容大小限制，默认 2MB，超限文件仍参与 name/path 搜索
- [x] 5.5 实现 `content:` 查询，使用 Tantivy 默认 query parser 处理短语/默认语义
- [x] 5.6 实现候选约束后的内容查询：普通词、`type:`、`name:`、`dir:` 先筛候选，再查内容
- [x] 5.7 返回内容命中片段上下 5 行和结构化高亮范围
- [x] 5.8 实现混合排序：普通候选优先，内容命中加权；纯 `content:` 主要按 Tantivy score

## 6. 运行期增量更新

- [x] 6.1 先写 snapshot-assisted refresh 和 watcher debounce 的 Rust 测试
- [x] 6.2 使用 mtime/size/root 元信息实现 snapshot-assisted refresh，跳过明确未变化路径
- [x] 6.3 接入 `notify` watcher，对已索引根目录建立运行期递归监听
- [x] 6.4 实现 debounce 队列，将 create/remove/rename/write 合并为批处理
- [x] 6.5 批处理更新受影响路径的 name/path 索引、SQLite 快照和 Tantivy 内容索引
- [x] 6.6 watcher 失败时回退后台分批刷新，并在状态中暴露失败摘要

## 7. 配置、前端与说明

- [x] 7.1 先写配置默认值测试，覆盖 Windows 内容索引只默认 Desktop、macOS 默认常用目录、ignore 默认启用、大小默认 2MB
- [x] 7.2 扩展配置 schema，支持索引性能模式、尊重项目 ignore、内容索引目录、内容大小限制和 watcher 开关边界
- [x] 7.3 更新设置页索引配置和提示说明，明确多个目录配置方式、content 隐私含义和字段查询示例
- [x] 7.4 更新启动器结果类型，展示内容片段、上下文和高亮
- [x] 7.5 更新启动器轻量索引状态展示，避免覆盖计算器、网页搜索或命令模式
- [x] 7.6 更新项目文档，说明字段查询语法：`type:pdf`、`name:test`、`dir:**/workspace`、`content:"hello world"`

## 8. 验证

- [x] 8.1 运行 `npm run check`
- [x] 8.2 运行索引基准并记录 before/after
- [ ] 8.3 在 macOS 上手工验证大目录启动、设置页进度、普通字段查询、content 片段和 watcher 更新
- [ ] 8.4 在 Windows 上手工验证多盘 name/path 默认、Desktop-only 内容默认、Program Files/Start Menu 应用入口、系统目录排除和首次索引耗时
- [x] 8.5 验证 `.gitignore`/`.ignore` 默认尊重，可配置关闭
- [x] 8.6 运行 `openspec validate optimize-file-indexing-performance --strict`
