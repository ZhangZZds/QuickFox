# QuickFox 索引优化与内容搜索设计

日期：2026-06-08
状态：采访决策已写入 OpenSpec，等待维护者书面 review
对应 OpenSpec：`openspec/changes/optimize-file-indexing-performance/`

## 目标

这次演进把文件索引从“能用的原型”升级成可持续扩展的搜索基础设施。它同时解决四类问题：

- Windows 首次索引慢，尤其是多盘和大目录场景。
- 普通搜索只能靠文件名/路径模糊匹配，缺少 `type:`、`name:`、`dir:` 等精确收窄方式。
- 用户明确需要文件内容搜索，并希望通过 `content:` 前缀开启，结果中能看到命中上下文。
- 文件系统变化不能在应用运行期稳定增量更新。

本设计不把 PDF/Office 全文抽取塞进第一版内容索引。第一版建立 extractor 边界，优先支持文本文件，后续再接入专用 extractor。

## 已确认决策

- `content:` 是显式内容搜索前缀；没有该前缀时普通词仍只搜索 name/path。
- 支持字段查询：`type:pdf`、`name:test`、`dir:**/workspace`、`content:"hello world"`。
- 字段条件之间使用 AND；普通词、`type:`、`name:`、`dir:` 先筛候选，再执行内容搜索。
- `type:` 按扩展名精确匹配，忽略大小写。
- `name:` 对文件名做 contains 匹配。
- `dir:` 无通配符时 contains 匹配目录路径；有 glob 通配符时按 glob 匹配。
- 字段值支持双引号，允许空格和 Windows 路径。
- 内容索引用 Tantivy，短语/default 语义交给 Tantivy 默认 query parser。
- 内容命中结果返回命中行上下各 5 行，并返回结构化高亮范围。
- 内容索引默认大小限制 2MB，可配置；超限文件仍可按 name/path 搜索。
- 默认尊重 `.gitignore`/`.ignore`，但允许配置关闭。
- watcher 只要求运行期生效，不做跨重启事件队列。
- Windows 内容索引默认只对 Desktop/桌面生效；其他盘符默认只做 name/path，除非用户配置。
- macOS 内容索引默认覆盖 Desktop、Documents、Downloads、workspace 等常用用户目录。

## 架构

Rust core 增加独立的索引分层：

- `FileSystemScanner`：基于 `ignore::WalkBuilder` 遍历文件系统，处理强制排除、用户排除、项目 ignore 和扫描统计。
- `IndexEntry`/snapshot：保存 path、name、kind、parent、extension、depth、root、mtime、size、search text 和 content index state。
- `FileQuery` parser：把用户输入解析为普通词和字段条件，给 Provider 提供结构化查询。
- 普通 matcher：使用预计算 search text 和可替换 fuzzy matcher；可接入 `nucleo`，但不让它直接决定最终排序。
- Tantivy content index：独立于 SQLite 快照保存全文索引，通过 doc id/path 映射和版本管理同步。
- Runtime watcher：基于 `notify` 和 debounce 批量处理 create/remove/rename/write。

前端只负责展示结构化结果：字段查询仍从同一个搜索框输入；内容命中时展示片段和高亮；设置页解释多个目录、ignore、内容索引范围和隐私含义。

## 数据流

首次启动或刷新时，索引分阶段执行：

1. 应用入口。
2. 常用目录。
3. 用户配置目录。
4. Windows 剩余本地盘符。

每个阶段完成后更新内存索引和状态，让用户尽早得到可用结果。内容抽取作为后台低优先级任务运行，只处理配置范围内、大小限制内、可识别为文本的文件。

查询时：

1. 解析用户输入为 `FileQuery`。
2. 用普通词、`type:`、`name:`、`dir:` 在元信息索引里生成候选。
3. 如果存在 `content:`，在候选集合中查 Tantivy；没有候选条件时查全局内容索引。
4. 组合普通匹配分数、Tantivy score、类型优先级和历史信号。
5. 返回结果、片段和高亮范围。

## 错误处理

- 单个目录权限失败不阻塞其他目录扫描。
- watcher 初始化或运行失败时，状态中记录摘要，并回退后台分批刷新。
- 内容读取失败只标记该文件 content state，不影响 name/path 搜索。
- 旧 SQLite 快照可加载；缺少元信息时触发后台新格式刷新。
- Tantivy schema 或版本不兼容时重建内容索引，不影响基础文件搜索。

## 测试与验收

实现必须按 TDD 推进。优先补 Rust 单元测试覆盖扫描器、查询解析、字段过滤、内容索引、片段、高亮、快照迁移和 watcher debounce。前端测试覆盖设置说明、内容片段展示和索引状态。

自动验证至少运行 `npm run check` 和 `openspec validate optimize-file-indexing-performance --strict`。手工验收需要覆盖 macOS 大目录、Windows 多盘 name/path 默认、Windows Desktop-only 内容默认、`.gitignore` 行为和运行期 watcher 更新。
