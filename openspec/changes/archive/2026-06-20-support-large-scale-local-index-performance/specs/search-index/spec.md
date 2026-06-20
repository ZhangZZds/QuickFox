## ADDED Requirements

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
