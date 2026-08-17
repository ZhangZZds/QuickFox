# QuickFox 1.5.0

## Highlights

- 大规模本地文件索引正式优化到 200 万 entry 目标场景：普通 name/path 搜索改为 compact candidate retrieval，再交给既有 matcher/ranker 精判。
- 修复海量索引下输入越长越卡的问题，尤其是连续输入 `agents.md` 到 `m` 时的低命中/靠后命中路径。
- 降低索引刷新和启动快照的内存放大：扫描进度不再长期保留每个 accepted entry，refresh/report/snapshot 路径避免重复持有完整 entries。
- 内容索引 snippet 不再让所有文本正文和按行拆分结果常驻内存，改为按命中文档从 Tantivy stored content 生成。
- 前端索引状态触发搜索统一走 debounce，旧查询结果不会覆盖最新输入。
- 补充 Windows C/D 多盘、200 万文件级发布验收清单和发布流程规范。

## Performance

本版本包含 deterministic synthetic benchmark 和 ignored release threshold：

- 100,000 entry debug baseline：
  - `agents-exact`: 90us
  - `agents-prefix`: 11us
  - `agents-type-md`: 191us
  - `agents-dir-workspace`: 418us
  - `low-hit-random`: 74us
  - `high-hit-project`: 3765us
- 2,000,000 entry release threshold：
  - `agents-exact`: 79us
  - `agents-prefix`: 3us
  - `agents-type-md`: 695us
  - `agents-dir-workspace`: 730us
  - `low-hit-random`: 48us

Windows 真实 C/D 多盘发布构建验收已由维护者确认通过。

## Verification

- `npm run check`
- `cargo test --release --manifest-path src-tauri/Cargo.toml two_million_entry_search_stays_within_latency_budget -- --ignored --nocapture`
- Windows C/D 多盘发布构建手工验收通过

## Release Notes

这是 `v1.4.0` 后的大规模索引性能发布。重点是让 QuickFox 在真实大盘符和 200 万文件级数据下保持输入响应、结果可信和内存可控。macOS 与 Windows 安装包由 GitHub Actions release workflow 在推送 `v1.5.0` tag 后生成并上传到 GitHub Release。
