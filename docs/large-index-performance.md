# 大规模索引性能基准

QuickFox 将 Windows C/D 多盘、约 200 万文件的开发机作为正式支持目标。这里记录本地性能基准的运行方式、普通 CI 应覆盖的范围，以及输出字段含义。

## 普通 CI 范围

普通 CI 应运行非 ignored 的 Rust 单元测试，其中包括 synthetic 大索引夹具和结果质量 oracle：

```bash
cargo test --manifest-path src-tauri/Cargo.toml synthetic_large_index -- --nocapture
cargo test --manifest-path src-tauri/Cargo.toml \
  ci_scale_large_index_search_stays_within_latency_budget \
  -- --nocapture
```

该命令会覆盖：

- deterministic synthetic entry generator。
- `agents.md`、`agents.m`、`agents`、`type:md agents`、`dir:workspace agents` 查询集合。
- 低命中、高命中、路径段 fuzzy 查询类型定义。
- `AGENTS.md` 目标文件位于索引靠后位置时仍进入前 5 位的结果质量 oracle。
- 100,000 entry CI-scale 查询延迟阈值，覆盖 exact、prefix、field-filtered 和 low-hit 查询。

## 本地 baseline benchmark

大规模 baseline 使用 ignored test，默认生成 100,000 条 synthetic entries：

```bash
cargo test --manifest-path src-tauri/Cargo.toml \
  synthetic_large_index_baseline_reports_current_linear_search_characteristics \
  -- --ignored --nocapture
```

完整 2,000,000 entry 目标规模使用环境变量显式开启：

```bash
QUICKFOX_LARGE_INDEX_SCALE=2000000 cargo test --manifest-path src-tauri/Cargo.toml \
  synthetic_large_index_baseline_reports_current_linear_search_characteristics \
  -- --ignored --nocapture
```

该 benchmark 不读取真实磁盘文件，只构造 deterministic in-memory entries，用于观察查询路径和内存模型的变化。真实 Windows C/D 多盘扫描仍需要按 `docs/windows-manual-qa.md` 手工验收。

2,000,000 entry 查询阈值使用单独 ignored test：

```bash
cargo test --manifest-path src-tauri/Cargo.toml \
  two_million_entry_search_stays_within_latency_budget \
  -- --ignored --nocapture
```

本地 release-build 验证可额外加 `--release`，用于减少 debug 构建噪声：

```bash
cargo test --release --manifest-path src-tauri/Cargo.toml \
  two_million_entry_search_stays_within_latency_budget \
  -- --ignored --nocapture
```

## 输出字段

每个查询会输出一行 `QUICKFOX_LARGE_INDEX_BASELINE`：

- `scale`: synthetic entry 数量。
- `query`: 查询用例名称。
- `kind`: 查询类型，例如 exact、prefix、field-filtered、low-hit、high-hit。
- `elapsed_us`: 当前实现执行该查询的耗时，单位微秒。
- `results`: 返回结果数量。
- `target_position`: `AGENTS.md` 目标结果位置；`-1` 表示该查询不期望或没有命中目标。
- `linear_entries`: 当前索引 entry 数。在线性扫描实现中，这是潜在最大 matcher 工作量。
- `entry_struct_bytes`: `IndexedEntry` 结构体数量估算，不包含堆上字符串内容。
- `entry_string_bytes`: entry 内 path/name/parent/root/search_text 等字符串内容估算。
- `cached_search_text_bytes`: SearchIndex 额外缓存的 searchable text 字节数。

2,000,000 阈值测试会输出 `QUICKFOX_LARGE_INDEX_THRESHOLD`，其中 `elapsed_us` 是每个查询的耗时，`results` 是返回结果数量。

## 记录要求

影响 `index.rs`、`file_matcher.rs`、`index_entry.rs`、`storage.rs`、`content_index.rs` 或搜索调度的变更，完成前必须记录相关 benchmark 输出或说明未运行原因。若性能阈值失败，不得声明大规模索引性能任务完成。

## 2026-06-20 本机验证记录

环境：macOS 开发机，Rust debug/release synthetic benchmark，不读取真实磁盘。真实 Windows C/D 多盘仍需按 `docs/windows-manual-qa.md` 执行发布构建手工验收。

100,000 entry debug baseline：

| query                   | kind             | elapsed_us | results |
| ----------------------- | ---------------- | ---------: | ------: |
| agents-exact            | Exact            |         90 |       1 |
| agents-prefix-extension | Prefix           |         21 |       1 |
| agents-prefix           | Prefix           |         11 |       1 |
| agents-type-md          | FieldFiltered    |        191 |       1 |
| agents-dir-workspace    | FieldFiltered    |        418 |       1 |
| low-hit-random          | LowHit           |         74 |       0 |
| path-segment-fuzzy      | PathSegmentFuzzy |         40 |       0 |
| high-hit-project        | HighHit          |       3765 |      20 |

100,000 entry memory estimate:

- `entry_struct_bytes`: 19,200,000
- `entry_string_bytes`: 17,305,779
- `cached_search_text_bytes`: 6,885,038

2,000,000 entry release threshold：

| query                   | kind          | elapsed_us | results |
| ----------------------- | ------------- | ---------: | ------: |
| agents-exact            | Exact         |         79 |       1 |
| agents-prefix-extension | Prefix        |          4 |       1 |
| agents-prefix           | Prefix        |          3 |       1 |
| agents-type-md          | FieldFiltered |        695 |       1 |
| agents-dir-workspace    | FieldFiltered |        730 |       1 |
| low-hit-random          | LowHit        |         48 |       0 |

已知剩余风险：

- synthetic benchmark 验证查询/候选召回模型，不代表真实 Windows 文件系统扫描耗时。
- 当前 `SearchIndexMemoryEstimate` 仍显示 `IndexedEntry` 和 `search_texts` 的常驻估算；后续若要进一步压到 500MB 以下，应继续把 SearchIndex 主存储迁入 compact entry table，并减少 `IndexedEntry` 字符串副本。
- 内容索引不再额外常驻全文 snippet cache，但 Tantivy 自身 mmap/cache 行为仍需要用 Windows 任务管理器观测进程 RSS。
