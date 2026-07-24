# Layered Content Memory Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 消除无内容索引时的全量 path lookup，并让 layered content-only 可见性过滤成本只与 delta 相关。

**Architecture:** `SearchIndex` 只在持有 `ContentIndex` 时建立仅含 `ContentIndexState::Indexed` entry 的稀疏 lookup，并通过 attach/update/detach 生命周期统一同步。`LayeredSearchIndex` 在 `apply_delta` 时生成不可变 `Arc<ContentVisibilitySnapshot>`，查询时 O(1) 克隆并把 path predicate 直接交给 Tantivy fast-field collector。

**Tech Stack:** Rust、Tantivy、现有 QuickFox compact/layered index、Cargo tests

---

### Task 1: Content lookup lifecycle

**Files:**
- Modify: `src-tauri/src/core/index.rs`
- Test: `src-tauri/src/core/index.rs`

- [x] **Step 1: Write failing lifecycle and memory tests**

新增测试覆盖：`SearchIndex::from_entries` 的 lookup count/bytes 为 0；注入内容索引后只包含 `ContentIndexState::Indexed` 路径；内容增量更新后同步；detach 与 `replace_entries` 后归零；无内容 overlay rebuild 不分配 lookup。

- [x] **Step 2: Run tests and verify RED**

Run: `cargo test --manifest-path src-tauri/Cargo.toml core::index::tests::content_path_lookup -- --nocapture`

Expected: FAIL，因为当前 `from_entries` 无条件构建所有路径，且不存在 attach/detach 生命周期 API。

- [x] **Step 3: Implement sparse optional lookup**

将字段改为：

```rust
content_entry_index_by_path: Option<HashMap<String, usize>>
```

构建 helper 只收 `entry.content_index_state == ContentIndexState::Indexed`；`from_entries_with_content_index` 复用 attach；内容增量后重建；detach/replace 清空。内存估算新增 lookup count，并在 `None` 时 bytes/count 都为 0。

- [x] **Step 4: Run focused tests and verify GREEN**

Run: `cargo test --manifest-path src-tauri/Cargo.toml core::index::tests::content_path_lookup -- --nocapture`

Expected: PASS。

### Task 2: Delta-sized layered content visibility snapshot

**Files:**
- Modify: `src-tauri/src/core/index.rs`
- Modify: `src-tauri/src/core/layered_index.rs`
- Test: `src-tauri/src/core/layered_index.rs`
- Test: `src-tauri/src/core/index.rs`

- [x] **Step 1: Write failing visibility construction probe test**

建立大 baseline + 一个目录 tombstone 的 content fixture；重置 probe 后执行 content-only 查询，断言构建 filter 时 baseline scans 与 normalized allocations 为 0，快照 `Arc` identity 不变，并继续断言隐藏的高分文档不会占据 TopDocs 截断。

- [x] **Step 2: Run test and verify RED**

Run: `cargo test --manifest-path src-tauri/Cargo.toml core::layered_index::tests::content_visibility_snapshot -- --nocapture`

Expected: FAIL，因为当前 content-only 查询遍历 baseline 并克隆所有 hidden paths。

- [x] **Step 3: Add immutable snapshot and filter-aware search contract**

新增不可变快照：

```rust
struct ContentVisibilitySnapshot {
    overlay_paths: BTreeSet<String>,
    exact_tombstones: BTreeSet<String>,
    directory_tombstones: BTreeSet<String>,
}
```

`apply_delta`/`replace_baseline` 更新 `Arc<ContentVisibilitySnapshot>`；`LayeredSearchIndex::search` O(1) 克隆 Arc，并调用新的 `SearchIndex` filter-aware content search contract。命中判断先查 exact，再沿父路径段查 directory tombstone；不遍历 baseline。

- [x] **Step 4: Run focused visibility tests and verify GREEN**

Run: `cargo test --manifest-path src-tauri/Cargo.toml core::layered_index::tests::content_visibility_snapshot -- --nocapture`

Expected: PASS，且现有 >10k content-only/mixed 回归继续通过。

### Task 3: Verification and commit

**Files:**
- Modify: `openspec/changes/complete-runtime-incremental-indexing/design.md`
- Do not modify: `openspec/changes/complete-runtime-incremental-indexing/tasks.md`

- [x] **Step 1: Run focused suites**

```bash
cargo test --manifest-path src-tauri/Cargo.toml core::layered_index::tests
cargo test --manifest-path src-tauri/Cargo.toml core::index::tests
cargo test --manifest-path src-tauri/Cargo.toml core::content_index::tests
cargo test --manifest-path src-tauri/Cargo.toml core::providers::tests
```

- [x] **Step 2: Run full verification**

```bash
cargo test --manifest-path src-tauri/Cargo.toml
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
git diff --check
```

- [x] **Step 3: Commit reviewed scope**

```bash
git add docs/superpowers/plans/2026-07-24-layered-content-memory-hardening.md \
  openspec/changes/complete-runtime-incremental-indexing/design.md \
  src-tauri/src/core/index.rs src-tauri/src/core/layered_index.rs
git commit -m "fix: bound layered content lookup memory"
```
