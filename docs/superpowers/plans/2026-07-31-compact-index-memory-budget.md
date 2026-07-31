# Compact Index Memory Budget Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 2,000,000 entry 的常驻 name/path 搜索索引完整估算低于 500 MiB 目标，并始终低于 800 MiB 硬上限，同时保持查询延迟与结果质量门槛。

**Architecture:** `CompactCandidateIndex` 改为唯一权威 name/path 存储：字符串放入 packed byte arena，entry 使用 `u32` range/ID，name、parent、extension、root 优先引用 path 子区间；构建期 interner 不进入常驻结构。候选索引用按规范化键排序的 `u32` posting/range 或固定宽度 fingerprint posting，查询后用 arena 原文复核碰撞；不再为每个字符保留 `BTreeMap<String, Vec<EntryId>>`。`SearchIndex` 只在返回受限结果、兼容快照或少量 overlay 操作时按需展开 `IndexedEntry`，`LayeredSearchIndex` 复用 compact exact-path/entry-kind 查询，不复制 baseline path。

**Tech Stack:** Rust、Tauri core、现有 `FileQuery`/`FileMatcher`、Cargo ignored release benchmark、OpenSpec strict validation。

---

### Task 1: 建立完整且会失败的内存门槛

**Files:**
- Modify: `src-tauri/src/core/compact_index.rs`
- Modify: `src-tauri/src/core/index.rs`
- Modify: `src-tauri/src/core/layered_index.rs`

- [x] **Step 1: Write the failing SearchIndex budget test**

在 `SearchIndexMemoryEstimate` 增加 `total_resident_bytes()`，总和必须包含结构体、arena、所有 vector 槽位/capacity、key/posting、lookup 和可选缓存；在 2M ignored fixture 中断言：

```rust
let estimate = search_index.memory_estimate();
assert!(estimate.total_resident_bytes() < 500 * 1024 * 1024, "{estimate:#?}");
assert!(estimate.total_resident_bytes() < 800 * 1024 * 1024, "{estimate:#?}");
```

- [x] **Step 2: Verify RED**

Run: `QUICKFOX_LARGE_INDEX_SCALE=2000000 cargo test --release synthetic_large_index_baseline_reports_current_linear_search_characteristics -- --ignored --nocapture --test-threads=1`

Expected: FAIL；旧结构完整下界至少为 `1,652,411,283 B`，而不是由于测试拼写或 fixture 构造失败。

- [x] **Step 3: Add focused structural RED tests**

增加测试并确认分别失败：

```rust
assert_eq!(estimate.legacy_entry_bytes, 0);
assert_eq!(estimate.duplicate_search_text_bytes, 0);
assert_eq!(estimate.retained_build_interner_bytes, 0);
assert!(stats.prefix_key_count <= stats.entry_count.saturating_mul(2));
```

另为 `LayeredSearchIndex` 断言 2M baseline 的 `baseline_path_metadata_bytes == 0`，以及 `baseline + 10k delta` 总估算低于 500 MiB。

- [x] **Step 4: Commit the RED tests and plan**

```bash
git add docs/superpowers/plans/2026-07-31-compact-index-memory-budget.md src-tauri/src/core/compact_index.rs src-tauri/src/core/index.rs src-tauri/src/core/layered_index.rs
git commit -m "test: enforce compact index memory budget"
```

### Task 2: 将 EntryTable 迁移为唯一 packed 存储

**Files:**
- Modify: `src-tauri/src/core/compact_index.rs`
- Modify: `src-tauri/src/core/index.rs`
- Modify: `src-tauri/src/core/file_matcher.rs`
- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/src/core/storage.rs`

- [ ] **Step 1: Add packed-entry behavior tests**

测试 `EntryTable` 对 path/name/parent/extension/root/kind/mtime/size/content state 的往返；name、parent、extension、root 是 path 子串时不得再次写入 arena；自定义 `search_text` 仅在不等于标准 `name + path` 时进入例外 arena。确认测试先失败。

- [ ] **Step 2: Implement fixed-width ranges and build-only interning**

使用 `u32::MAX` 表示缺失 range，arena 总长度或 entry 数超过 `u32` 时返回明确构建错误；`CompactEntry` 只保存 fixed-width metadata 与 arena range。构建完成后丢弃用于去重的 `HashMap`，常驻 estimate 中 `retained_build_interner_bytes` 必须为 0。

- [ ] **Step 3: Remove SearchIndex legacy copies**

删除常驻 `entries: Vec<IndexedEntry>` 与 `search_texts: Vec<String>`。普通查询从 compact table 借用 path/name，昂贵 matcher 只接收候选；最终 `entry_to_result`、content mixed 查询和 regex compatibility 才展开需要的 entry。把生产代码中的 `entries()` slice 调用改为显式 `materialized_entries()` 或 streaming iterator，快照保存不得把展开结果重新存回 `SearchIndex`。

- [ ] **Step 4: Verify focused compatibility**

Run: `cargo test core::compact_index core::index core::storage -- --test-threads=1`

Expected: packed round-trip、legacy SQLite snapshot、content-only/mixed、regex、取消和既有搜索测试全部 PASS。

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/core/compact_index.rs src-tauri/src/core/index.rs src-tauri/src/core/file_matcher.rs src-tauri/src/lib.rs src-tauri/src/core/storage.rs
git commit -m "refactor: make packed index authoritative"
```

### Task 3: 用有界候选结构替换全字符 BTreeMap

**Files:**
- Modify: `src-tauri/src/core/compact_index.rs`
- Modify: `src-tauri/src/core/index.rs`

- [ ] **Step 1: Add candidate equivalence and growth RED tests**

保留现有 `agents.md`、`agents.m`、`agents`、`type:md agents`、`dir:workspace agents`、低命中和 path fuzzy oracle；新增 4KiB 长名称/路径测试，断言索引 key 数不随字符数线性增长，且 lookup 不标记 full scan。确认旧 `name_prefixes`/`segment_prefixes` 实现失败。

- [ ] **Step 2: Implement sorted/fingerprint postings**

name prefix 使用规范化名称排序后的 `Vec<u32>` 做 range lookup；token、leading trigram、extension、exact path 与 path segment 使用固定宽度 fingerprint + `u32 EntryId` posting，碰撞候选必须用 arena 原文复核。path segment 只保存一次 path 分段引用，不重复遍历 parent，也不物化每字符 prefix key。

- [ ] **Step 3: Keep candidate semantics bounded**

字段过滤先取交集，普通 term 取 token/name-prefix/path-segment/fuzzy 的有界并集；低命中不得回退为所有 entry 的完整 matcher。候选 ID 排序、去重并在每 1024 个候选检查取消。

- [ ] **Step 4: Verify quality and 100k CI fixture**

Run: `cargo test core::compact_index core::index::tests::synthetic_large_index -- --test-threads=1`

Expected: oracle 结果等价或更优，候选统计无全量退化，100k 门槛 PASS。

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/core/compact_index.rs src-tauri/src/core/index.rs
git commit -m "perf: bound compact candidate storage"
```

### Task 4: 移除 Layered baseline 路径副本

**Files:**
- Modify: `src-tauri/src/core/compact_index.rs`
- Modify: `src-tauri/src/core/index.rs`
- Modify: `src-tauri/src/core/layered_index.rs`

- [ ] **Step 1: Add exact-path and descendant RED tests**

覆盖文件替换、目录 tombstone、rename、Windows 大小写、overlay 覆盖、baseline 不可见和 `materialized_entries`；断言 exact-path/kind/descendant 查询直接来自 baseline compact storage，`baseline_path_metadata_bytes == 0`。

- [ ] **Step 2: Reuse authoritative baseline lookup**

删除 `baseline_by_path`；向 `SearchIndex` 暴露只读 `entry_id_by_normalized_path`、`kind_by_path` 和 ordered path iteration，供 visible count、目录 descendant 与 watched roots 使用。overlay/tombstone 仍保持现有有界 BTreeMap，并继续报告其完整 estimate。

- [ ] **Step 3: Verify layered behavior**

Run: `cargo test core::layered_index core::index_journal -- --test-threads=1`

Expected: delta、replay、compaction、content visibility、generation 与恢复测试全部 PASS。

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/core/compact_index.rs src-tauri/src/core/index.rs src-tauri/src/core/layered_index.rs
git commit -m "perf: reuse compact baseline path metadata"
```

### Task 5: 发布级验证、文档与 OpenSpec 收口

**Files:**
- Modify: `docs/large-index-performance.md`
- Modify: `docs/architecture.md`
- Modify: `docs/runtime-incremental-indexing-manual-qa-results-2026-07-31.md`
- Modify: `openspec/changes/complete-runtime-incremental-indexing/tasks.md`

- [ ] **Step 1: Run formatter, lints and full suite**

Run: `npm run check`

Expected: frontend TypeScript/ESLint/Prettier/tests 和 Rust fmt/clippy/tests 全部 PASS，0 warning。

- [ ] **Step 2: Run all release gates**

依次运行 100k、500k、2M deterministic memory/query benchmarks、2M runtime overlay P95 benchmark、10k durable batch benchmark；记录 total resident estimate、各分类 bytes、candidate count、result count、P95、wall time 和平台进程观测。2M 必须 `<500 MiB` 目标且 `<800 MiB` 硬上限，P95 `<50ms`，极端查询 `<100ms`，目标文件前 5。

- [ ] **Step 3: Update documentation truthfully**

写入实际 commit、命令、计数和结果；macOS/Windows 真实桌面验收没有证据时继续标注 release blocker，不得用 synthetic 或 macOS runner 替代 Windows 任务管理器记录。

- [ ] **Step 4: Validate OpenSpec and checklist**

Run: `openspec validate complete-runtime-incremental-indexing --strict`

Expected: PASS。仅在自动化门槛真实通过、手工 blocker 已明确记录后勾选 7.5、8.1–8.5。

- [ ] **Step 5: Commit**

```bash
git add docs/large-index-performance.md docs/architecture.md docs/runtime-incremental-indexing-manual-qa-results-2026-07-31.md openspec/changes/complete-runtime-incremental-indexing/tasks.md
git commit -m "docs: close incremental indexing validation"
```
