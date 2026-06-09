# 文件索引基线与依赖选择记录

## 基准夹具

本轮新增可复现的 Rust ignored test：

```bash
cargo test --manifest-path src-tauri/Cargo.toml indexing_baseline_fixture_reports_current_scanner_characteristics -- --ignored --nocapture
```

夹具覆盖：

- 小目录：`small/report-budget.md`、`small/notes.txt`
- 深目录：12 层 `deep/level-*`，每层一个文本文件
- Windows 多盘模拟：`windows-drives/C/.../Desktop` 与 `windows-drives/D/.../QuickFox`
- 大量排除目录：32 个 `excluded/node_modules_*`
- 文本内容文件：`content-text/body.txt` 中只在文件内容包含 `needle-from-file-content-only`

2026-06-08 在本机实际运行结果：

```text
QUICKFOX_INDEX_BASELINE scan_ms=3 entries=41 failures=0 ordinary_query_us=801 ordinary_results=2 content_query_results=0
```

这说明当前自研扫描器在小型夹具上可完成扫描，但 `content:` 查询仍只走 name/path 搜索路径；内容只在文件正文中出现时，当前基线结果为 0。

2026-06-09 完成本轮索引优化后，在同一基准夹具上重新运行：

```text
QUICKFOX_INDEX_BASELINE scan_ms=4 entries=41 failures=0 ordinary_query_us=408 ordinary_results=2 content_query_results=0
```

该 ignored test 仍使用 `SearchIndex::from_entries` 的 name/path-only 路径，因此 `content_query_results=0`
是预期结果；内容索引能力由 `content_index::tests` 覆盖，并通过显式内容索引入口验证。

## 依赖选择

已加入 `src-tauri/Cargo.toml` 并由 Cargo 解析锁定：

| 依赖                    | 版本   | License           | 本次用途                                        | 跨平台与打包影响                                                                                                                           |
| ----------------------- | ------ | ----------------- | ----------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ |
| `ignore`                | 0.4.26 | Unlicense OR MIT  | 替换自研递归扫描，提供 ignore/glob/并行遍历底座 | Rust 纯库为主；复用 ripgrep 生态，适合 macOS/Windows/Linux；无额外 Tauri 资源打包要求                                                      |
| `notify`                | 8.2.0  | CC0-1.0           | 运行期文件系统监听                              | 使用平台 watcher 后端；macOS 启用 FSEvents，Windows/Linux 由 crate 选择后端；需要 watcher 失败 fallback                                    |
| `notify-debouncer-mini` | 0.7.0  | MIT OR Apache-2.0 | 合并 create/remove/rename/write 抖动事件        | 只增加 Rust 依赖；不引入持久事件队列或额外打包资源                                                                                         |
| `tantivy`               | 0.26.1 | MIT               | `content:` 本地全文索引                         | 带来较大的 transitive 依赖和编译成本，包括压缩、mmap、tokenizer 相关 crate；Tantivy index 目录需要作为运行期 app data 管理，不应打进安装包 |
| `nucleo-matcher`        | 0.3.1  | MPL-2.0           | name/path fuzzy matcher 边界                    | License 与其源码文件级 copyleft 约束需在发布合规清单中保留；Rust 依赖，无额外运行期资源                                                    |
| `globset`               | 0.4.18 | Unlicense OR MIT  | `dir:` glob 与扫描排除规则边界                  | Rust 纯库；跨平台路径分隔符语义需在实现层统一                                                                                              |
| `content_inspector`     | 0.2.4  | MIT/Apache-2.0    | 文本/二进制内容抽取边界判断                     | Rust 纯库；仅用于安全判断文本优先 extractor                                                                                                |
| `tempfile`              | 3.27.0 | MIT OR Apache-2.0 | 基准夹具与后续 Rust 测试                        | 测试/工具依赖用途；当前作为普通 dependency 引入以匹配计划，后续可评估移到 dev-dependencies                                                 |

二进制体积确认边界：

- 本轮是可编译骨架，新增依赖尚未被生产路径调用；Rust release 链接器可能丢弃未使用代码，因此当前 binary delta 不代表最终接入后的真实体积。
- 体积风险主要来自 `tantivy` 及其压缩、mmap、tokenizer、fst 相关 transitive crates；后续 Task 5 接入内容索引后必须记录 release artifact 的 before/after。
- Tauri 打包不需要内置 Tantivy 索引文件；内容索引目录必须在运行期 app data 中按版本创建和迁移。

## 暂不进入本次实现的边界

PDF、Office 和其他需要专用 extractor 的二进制文档不进入本次实现。第一版只实现文本优先内容抽取边界：可安全识别的文本文件在大小限制内进入 Tantivy；PDF/Office 等文件继续参与 name/path 搜索，并通过 extractor trait/模块边界留给后续 OpenSpec 变更扩展。
