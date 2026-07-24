# Targeted Scanner Review Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** 修复 targeted scanner 的取消粒度、missing subtree 复杂度、batch ignore 重复 IO，以及跨平台路径语义。

**Architecture:** 目录校准通过可取消 visitor 流式读取直接子项，并用规范化 path `HashSet` 按祖先段折叠缺失子树。Targeted batch 按 configured root 分组，共享扫描期 ignore matcher cache；单文件通过 ignore 判定后直接构造 entry，目录仍交给 `WalkBuilder`。path key 使用平台明确的比较模式，native 模式只在 Windows lower-case。

**Tech Stack:** Rust、`ignore`、标准库 `HashMap`/`HashSet`、现有 QuickFox scanner 测试夹具

---

## File structure

- `src-tauri/src/core/index_entry.rs`：平台明确的 path key 规范化与 segment-boundary helper。
- `src-tauri/src/core/index_scanner.rs`：configured root 匹配、batch ignore matcher cache、单文件直接扫描。
- `src-tauri/src/core/targeted_index_scanner.rs`：可取消目录 visitor、manifest 校准、missing subtree 折叠、按 root 分组的 batch scan 与测试。

### Task 1: 修正 path key 与 configured root 选择

**Files:**
- Modify: `src-tauri/src/core/index_entry.rs`
- Modify: `src-tauri/src/core/index_scanner.rs`
- Test: `src-tauri/src/core/targeted_index_scanner.rs`
- Test: `src-tauri/src/core/index_scanner.rs`

- [x] **Step 1: 写失败测试**

在非 Windows 断言 `A\\B` 与 `a\\b` 规范化后不相等；原 Windows 变体去重测试改用 `#[cfg(target_os = "windows")]`。增加 explicit Windows mode root fixture：`C:\\Users\\Frank` 必须匹配 `c:/users/frank/docs/a.md`，但不得匹配 `C:/Users/Frankish/a.md`；native POSIX `/Data` 不匹配 `/data/a.md`。

- [x] **Step 2: 运行确认 RED**

```bash
cargo test --manifest-path src-tauri/Cargo.toml unix_backslash_paths_preserve_case
cargo test --manifest-path src-tauri/Cargo.toml configured_root_matching_supports_explicit_windows_mode
```

Expected: heuristic 会错误 lower-case 非 Windows 反斜杠路径，原始 `Path::starts_with` 无法匹配 Windows case 变体。

- [x] **Step 3: 最小实现并确认 GREEN**

在 `index_entry.rs` 定义内部 `PathComparisonMode::{Native, Windows}`。`normalize_path_text_key_for_mode` 始终替换 `\\` 为 `/`、去掉尾 `/`，仅 Windows mode lower-case；公开函数使用目标平台 mode。增加 mode-aware segment-boundary descendant helper。`IndexPathRules::configured_root_for` 委托 mode-aware helper并选择最长 root；IgnoreScanner root 边界也复用该 helper。

```bash
cargo test --manifest-path src-tauri/Cargo.toml core::index_entry::tests
cargo test --manifest-path src-tauri/Cargo.toml core::index_scanner::tests
```

Expected: native/Windows mode、大小写和段边界测试全部通过。

### Task 2: 可取消 visitor 目录枚举

**Files:**
- Modify/Test: `src-tauri/src/core/targeted_index_scanner.rs`

- [x] **Step 1: 写失败测试**

增加固定子项 probe；它在每项前检查 cancellation 并记录已读数量。读取 3 项后取消，断言 `calibrate_manifest_cancellable` 返回 `TargetedScanError::Cancelled` 且计数为 3，因此无 partial delta 且第 4 项未读。

- [x] **Step 2: 运行确认 RED**

```bash
cargo test --manifest-path src-tauri/Cargo.toml calibration_cancellation_stops_directory_stream_without_partial_delta
```

Expected: 当前 `read_dir -> Vec` contract 无法在子项间取消。

- [x] **Step 3: 最小实现并确认 GREEN**

将 probe contract 改为：

```rust
fn visit_dir(
    &self,
    path: &Path,
    is_cancelled: &dyn Fn() -> bool,
    visitor: &mut dyn FnMut(FileSystemEntry),
) -> TargetedScanResultValue<()>;
```

真实实现逐项检查取消、读取该项 metadata、调用 visitor。校准函数只在 visitor 完整成功后 diff 局部 entries；取消用 `?` 丢弃整个 calibration result。

```bash
cargo test --manifest-path src-tauri/Cargo.toml core::targeted_index_scanner::tests::calibration
cargo test --manifest-path src-tauri/Cargo.toml core::targeted_index_scanner::tests::changed_directory
```

Expected: cancellation 与 calibration/direct diff 测试通过。

### Task 3: 线性折叠 missing subtree

**Files:**
- Modify/Test: `src-tauri/src/core/targeted_index_scanner.rs`

- [x] **Step 1: 写失败测试**

构造 2,000 个同级 missing paths，调用带 comparison counter 的折叠 helper，断言输出 2,000 个 root 且祖先 membership probe 不超过 `4 * N`；父子 missing 仍只输出父路径。

- [x] **Step 2: 运行确认 RED**

```bash
cargo test --manifest-path src-tauri/Cargo.toml missing_subtree_collapse_has_linear_ancestor_probes
```

Expected: 当前 `Vec::iter().any` 约执行 `N²/2` 次比较。

- [x] **Step 3: 最小实现并确认 GREEN**

建立所有 normalized missing keys 的 `HashSet<String>`；每个 path 只沿 `/` 段逐级向上执行 set membership probe。helper 返回 `(collapsed_paths, probe_count)`，生产调用只使用 paths。

```bash
cargo test --manifest-path src-tauri/Cargo.toml missing_subtree_collapse_has_linear_ancestor_probes
cargo test --manifest-path src-tauri/Cargo.toml calibration_collapses_missing_descendants_into_one_subtree_removal
```

Expected: probe 数线性有界且折叠语义不变。

### Task 4: batch ignore cache 与单文件直接扫描

**Files:**
- Modify: `src-tauri/src/core/index_scanner.rs`
- Modify/Test: `src-tauri/src/core/targeted_index_scanner.rs`

- [x] **Step 1: 写失败 IO 测试**

同一 root 下创建 128 个 sibling 文件并一次调用 `scan_changed_paths`。断言全部 upsert、`read_dir_calls == 0`，且 `read_file_calls <= root.ancestors().count() * 3`，证明固定 ignore IO 与 depth 相关而非乘以 N。

- [x] **Step 2: 运行确认 RED**

```bash
cargo test --manifest-path src-tauri/Cargo.toml targeted_batch_reuses_ancestor_ignore_matchers_for_sibling_files
```

Expected: 当前每个 path 重建 evaluator，读取次数约为 `N * depth`。

- [x] **Step 3: 最小实现并确认 GREEN**

增加 batch 生命周期 cache：按 directory 缓存 `.git` stat，按 `(matcher_root, ignore_file)` 缓存已编译 `Option<Gitignore>`。`TargetedIndexScanner` 先按 configured root 分组，每个 root 使用一个 batch scanner。单文件 ignore 通过后直接由 metadata 构造 entry，不启动第二个 `WalkBuilder`；目录仍使用 subtree walker。

```bash
cargo test --manifest-path src-tauri/Cargo.toml targeted_batch_reuses_ancestor_ignore_matchers_for_sibling_files
cargo test --manifest-path src-tauri/Cargo.toml core::targeted_index_scanner::tests
cargo test --manifest-path src-tauri/Cargo.toml core::index_scanner::tests
```

Expected: IO 随 depth 有界，ignore priority/negation/git exclude/global/cancel 与 direct diff 全部通过。

### Task 5: 完整验证与提交

**Files:**
- Verify: `docs/superpowers/plans/2026-07-25-targeted-scanner-review-hardening.md`
- Verify: `src-tauri/src/core/index_entry.rs`
- Verify: `src-tauri/src/core/index_scanner.rs`
- Verify: `src-tauri/src/core/targeted_index_scanner.rs`

- [x] **Step 1: 完整门禁**

```bash
cargo test --manifest-path src-tauri/Cargo.toml
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
git diff --check
```

Expected: 全部非 ignored 测试、fmt、clippy、diff 通过。

- [x] **Step 2: 范围审计并提交**

```bash
git diff -- openspec/changes/complete-runtime-incremental-indexing/tasks.md
git add docs/superpowers/plans/2026-07-25-targeted-scanner-review-hardening.md src-tauri/src/core/index_entry.rs src-tauri/src/core/index_scanner.rs src-tauri/src/core/targeted_index_scanner.rs
git commit -m "fix: harden targeted scanner batches"
```

Expected: `tasks.md` 无差异，新 commit 只含计划和三个 scanner/path 文件。
