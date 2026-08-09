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

`QUICKFOX_LARGE_INDEX_MEMORY` 的 `compact` 明细会额外输出 `name_ngram_bytes`。这是 1–3 字符 name n-gram 的 delta-varint postings；它覆盖短查询和数字子串候选，不允许以整表扫描替代。`candidates` 是 matcher 前的候选数，`full_scan=true` 仅适用于显式不支持的路径/glob 收窄，不能作为普通 name/path 查询的常规回退。

2,000,000 阈值测试会输出 `QUICKFOX_LARGE_INDEX_THRESHOLD`，其中 `elapsed_us` 是每个查询的耗时，`results` 是返回结果数量。

运行期分层 benchmark 还会输出：

- `QUICKFOX_LAYERED_INDEX`：`scale`、`delta`、查询名称、轮数、`p95_us`、结果数、baseline/overlay/tombstone 数量和 `delta_bytes`。
- `QUICKFOX_INCREMENTAL_BATCH`：batch 条目数、总提交、journal 提交、内存 layer apply 耗时、分层数量、查询轮数、查询 P95、结果数和估算 delta bytes。

这两个 fixture 的 10,000 条 delta 各包含 5,000 条 overlay 与 5,000 条 tombstone，并覆盖新增、覆盖、删除子树和低命中查询。普通增量不得改变 compact baseline build identity。

## 运行期增量 release benchmark

2,000,000 baseline + 10,000 delta 的 P95 门槛：

```bash
/usr/bin/time -l cargo test --release --manifest-path src-tauri/Cargo.toml \
  two_million_baseline_with_runtime_delta_stays_within_latency_budget \
  -- --ignored --nocapture
```

10,000 条 durable batch 的提交、journal、layer apply、查询和内存字段：

```bash
/usr/bin/time -l cargo test --release --manifest-path src-tauri/Cargo.toml \
  incremental_batch_benchmark_reports_commit_layers_query_p95_and_memory \
  -- --ignored --nocapture
```

纯 2,000,000 baseline 的单样本 compact candidate 阈值：

```bash
/usr/bin/time -l cargo test --release --manifest-path src-tauri/Cargo.toml \
  two_million_entry_search_stays_within_latency_budget \
  -- --ignored --nocapture
```

每条命令必须使用 release 构建并保存完整输出、exit code、`test result`、wall time 和平台内存观测。`/usr/bin/time -l` 的 maximum resident set size 与 peak memory footprint 是 macOS 的两种计数口径；整个 test 进程还包含 synthetic fixture 构造、Rust test harness 和销毁阶段，不能把 maximum RSS 直接当作 QuickFox 稳态索引常驻内存。

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

## 2026-07-31 运行期增量 release 验证记录

环境：macOS arm64 开发机，Rust release synthetic benchmark；benchmark 对应文档提交前实现 HEAD `85a6818`。三个命令均 exit 0，均为 `1 passed`。fixture 不读取真实 Windows C/D 文件树，也不替代 macOS/Windows 真实桌面 watcher 验收。

### 2,000,000 baseline + 10,000 runtime delta

分层为 baseline 2,000,000、overlay 5,000、tombstone 5,000，估算增量状态 `20,177,198` bytes。

| query                  | P95 (µs) |
| ---------------------- | -------: |
| overlay-exact          |        3 |
| overlay-prefix         |        5 |
| overlay-field-filtered |      423 |
| baseline-overwrite     |        7 |
| subtree-tombstone      |        2 |
| low-hit                |        3 |

最大 P95 为 423 µs（0.423 ms），满足不超过 50 ms 的门槛。wall time 为 47.27 s，test 自报 25.92 s。`/usr/bin/time -l` maximum RSS 为 `5,696,045,056` bytes，peak memory footprint 为 `116,228,720` bytes。

maximum RSS 与 peak footprint 差异来自 macOS 计数口径，且覆盖完整 synthetic fixture 生命周期；它们不等同于应用 ready 后的常驻索引内存门槛。真实常驻内存仍须在发布构建中用平台进程工具验收。

### 100,000 baseline + 10,000 durable batch

| 字段                  |                 实测值 |
| --------------------- | ---------------------: |
| entries               |                 10,000 |
| commit                | 79,655 µs（79.655 ms） |
| journal commit        | 20,357 µs（20.357 ms） |
| layer apply           | 59,297 µs（59.297 ms） |
| query P95             |                  14 µs |
| baseline              |                100,000 |
| overlay               |                  5,000 |
| tombstones            |                  5,000 |
| estimated delta bytes |             20,177,198 |

wall time 为 1.66 s；maximum RSS 为 `451,756,032` bytes，peak memory footprint 为 `115,311,168` bytes。query P95 满足 50 ms 门槛，且 journal 与内存 layer apply 均被单独计时。

### 纯 2,000,000 baseline compact threshold

| query                   | 单次耗时 (µs) |
| ----------------------- | ------------: |
| agents-exact            |           135 |
| agents-prefix-extension |             3 |
| agents-prefix           |             4 |
| agents-type-md          |           690 |
| agents-dir-workspace    |           889 |
| low-hit-random          |            96 |

最大单次耗时为 889 µs（0.889 ms），满足该测试每个查询小于 250 ms 的单样本阈值。该命令每类只测一次，不应称为 P95；50 ms P95 门槛由上面的 layered 和 batch 多轮 benchmark 覆盖。

wall time 为 49.92 s；maximum RSS 为 `6,605,078,528` bytes，peak memory footprint 为 `116,277,824` bytes。同样应按完整 fixture 生命周期和 macOS 双重内存口径解释，不能替代 Windows 任务管理器的真实发布构建常驻内存记录。

### 尚未解除的发布验证项

- macOS 真实桌面 watcher：create/write/rename/delete、1000-file checkout、休眠/唤醒、root 断开、失败降级、10 秒硬上限和 UI 截图。
- Windows NTFS C:/D: 跨目录 rename、盘符断开/重连、junction 安全回归、真实 200 万文件级 RSS 与 UI 响应。
- 上述项目记录在 `docs/runtime-incremental-indexing-manual-qa-results-2026-07-31.md`，完成前不能把 synthetic benchmark 等同于发布验收通过。

## 2026-08-09 compact candidate 与增量 release 复验

环境：macOS arm64 开发机，Rust release synthetic benchmark，代码提交 `9fd39b1`。所有以下命令 exit 0、各报告 `1 passed`；数据只证明进程内候选/分层逻辑，不替代真实 Windows 文件系统或桌面 watcher 验收。

### 纯 2,000,000 baseline

`synthetic_large_index_baseline_reports_current_linear_search_characteristics` 输出常驻估算 `462,092,513` bytes（约 441 MiB），低于 500 MiB 目标和 800 MiB 硬上限。`name_ngram_bytes` 为 `95,500,991` bytes，compact path-prefix ID 表为 `8,000,000` bytes；它替代了 LayeredSearchIndex 的重复 baseline 路径字符串 map。

| query                         | 耗时 (µs) | candidates | full_scan |
| ----------------------------- | --------: | ---------: | --------- |
| agents-exact                  |        35 |          1 | false     |
| agents-prefix-extension       |         2 |          1 | false     |
| agents-prefix                 |         1 |          1 | false     |
| agents-type-md                |    48,409 |          1 | false     |
| agents-dir-workspace          |       589 |          1 | false     |
| low-hit-random                |         2 |          0 | false     |
| path-segment-fuzzy (`wrkspc`) |       404 |    500,001 | false     |
| high-hit-project              |    13,835 |    636,364 | false     |

同一 release 阈值测试的最大受测耗时为 `49,505` µs（agents-type-md），低于每查询 250 ms 门槛。`wrkspc` 的 500,001 候选是同一 `workspace` 路径段的真实 fuzzy 命中，首批 20 个结果在 404 µs 返回；不是整表回退。

### 2,000,000 baseline + 10,000 runtime delta

`two_million_baseline_with_runtime_delta_stays_within_latency_budget` 的最大 P95 是 overlay-field-filtered `41,208` µs（41.208 ms），满足 ≤50 ms 门槛。该 fixture 使用 baseline 2,000,000、overlay 5,000、tombstone 5,000，估算 delta 为 `4,801,540` bytes。

`incremental_batch_benchmark_reports_commit_layers_query_p95_and_memory`：10,000 entries 的 commit `45,017` µs、journal `20,611` µs、layer apply `24,405` µs、查询 P95 `3,586` µs，估算 delta 同为 `4,781,540` bytes。
