# QuickFox 1.3.0

## Highlights

- 重构文件索引管线：使用分阶段扫描、轻量元信息快照和后台状态事件，让应用入口与常用目录更早可搜索。
- 新增结构化文件查询：支持 `type:`、`name:`、`dir:` 和 `content:` 组合过滤，并兼容 `type: md` 这类带空格写法。
- 接入本机 Tantivy 内容索引：`content:"hello world"` 可搜索已配置范围内的文本文件内容，结果返回命中行上下各 5 行和高亮范围。
- 优化内容搜索结果预览：默认显示命中次数、命中行和当前命中行，鼠标悬停结果时懒展开上下文。
- 引入 `ignore` 扫描器、项目 ignore 配置、强制系统噪音排除和 watcher/debounce 更新，减少大目录重复扫描与无关结果。
- 扩展索引配置与设置页说明：包含性能模式、内容索引目录、内容文件大小限制、项目 ignore 和 watcher 开关。
- 整理搜索索引与配置历史设计文档。

## Verification

- `npm run check`
- `npm run rust:clippy`
- `npm run rust:test`
- 索引基准已记录在归档变更的 baseline/dependency notes 中。

## Release Notes

这是 `v1.2.1` 后的功能发布，重点提升 QuickFox 在大目录、本地文本内容和结构化文件查询场景下的可用性。macOS 和 Windows 安装包由 GitHub Actions release workflow 在 `v1.3.0` tag 推送后构建并上传到 GitHub Release。

Windows 多盘和 macOS 真实桌面行为仍建议在安装包产出后进行一次人工验收，结果继续回流到文档和回归测试。
