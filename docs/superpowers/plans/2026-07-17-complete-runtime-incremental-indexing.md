# Complete Runtime Incremental Indexing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 QuickFox 在 macOS/Windows 上默认自动消费文件系统事件，以 baseline + overlay − tombstone 的分层视图在 10 秒内反映普通变化，并用 SQLite journal 与目录清单保证手动增量刷新和崩溃恢复。

**Architecture:** 平台 watcher 只向 8192 容量的非阻塞通道发送标准化事件，纯 Rust coordinator 负责 5 秒静默/10 秒硬上限的合并和 dirty-root 降级。Targeted scanner 与目录 manifest 生成 delta，SQLite 先事务提交 journal，再原子切换 `LayeredSearchIndex` generation；overlay 达到 50,000 条或 64 MiB 时复用现有后台全量刷新生成新 baseline。

**Tech Stack:** Rust, Tauri 2, `notify`, `ignore`, SQLite/`rusqlite`, Tantivy, React 19, TypeScript, Vitest, OpenSpec

---

## Source of truth

- Design: `docs/superpowers/specs/2026-07-16-performance-experience-roadmap-design.md`
- OpenSpec proposal: `openspec/changes/complete-runtime-incremental-indexing/proposal.md`
- OpenSpec design: `openspec/changes/complete-runtime-incremental-indexing/design.md`
- Delta specs: `openspec/changes/complete-runtime-incremental-indexing/specs/`
- OpenSpec checklist: `openspec/changes/complete-runtime-incremental-indexing/tasks.md`

## File structure

### New Rust modules

- `src-tauri/src/core/index_update_coordinator.rs` — debounce state machine、event batch 输出、dirty-root 与线程生命周期；不访问 SQLite 或 Tauri。
- `src-tauri/src/core/targeted_index_scanner.rs` — 单路径/局部子树扫描、目录指纹与 manifest 校准；复用现有过滤规则。
- `src-tauri/src/core/layered_index.rs` — baseline、overlay、tombstone、generation view 与分层查询合并。
- `src-tauri/src/core/index_journal.rs` — journal/manifest repository contract、恢复结果与 runtime commit 协调；SQLite 细节仍在 `storage.rs`。
- `src-tauri/src/core/runtime_indexing.rs` — 把 watcher、scanner、journal、layered view 和 Tauri-neutral callback 组合成后台服务。

### Modified Rust modules

- `src-tauri/src/core/mod.rs` — 注册上述模块。
- `src-tauri/src/core/index_watcher.rs` — watcher 自持有 bounded receiver，非阻塞 overflow 记录，保持平台 adapter 职责。
- `src-tauri/src/core/index_scanner.rs` — 提取可复用的路径过滤/单 root 扫描边界。
- `src-tauri/src/core/index.rs` — 增加 visibility-aware 查询与 `FileSearchIndex` trait；不再用全量 `apply_update_batch` 承载运行期更新。
- `src-tauri/src/core/compact_index.rs` — 仅增加测试用 build counter 和必要的可见性入口，不实现可变全量索引。
- `src-tauri/src/core/storage.rs` — schema v3、delta journal、directory manifest、runtime state 与清理 API。
- `src-tauri/src/core/index_entry.rs` — 增量状态字段和错误 code 的序列化 contract。
- `src-tauri/src/core/providers.rs` — FileProvider 借用 `dyn FileSearchIndex`。
- `src-tauri/src/core/config.rs` — 保持 `watcher_enabled` 默认 true，增加索引语义配置比较 helper。
- `src-tauri/src/core/content_index.rs` — 暴露独立的内容 delta 更新结果，不让失败回滚 name/path。
- `src-tauri/src/lib.rs` — runtime 服务启动/停止、后台刷新成功后的 baseline 切换、命令接线和 Tauri status emit。

### Frontend and docs

- `src/tauriClient.ts` — 扩展 `IndexStatus` 类型。
- `src/App.tsx` — 复用现有 watcher 开关，增加基础自动增量/降级状态文案。
- `src/App.test.tsx`、`src/tauriClient.test.ts` — contract、状态展示和防抖回归。
- `docs/architecture.md`、`docs/large-index-performance.md`、`docs/troubleshooting.md` — 运行期增量架构、阈值和恢复说明。
- `docs/macos-manual-qa.md`、`docs/windows-manual-qa.md` — 真实 watcher 验收步骤。

## Task 1: Bounded watcher inbox and deterministic batching

**Files:**

- Modify: `src-tauri/src/core/index_watcher.rs`
- Create: `src-tauri/src/core/index_update_coordinator.rs`
- Modify: `src-tauri/src/core/mod.rs`

- [ ] **Step 1: Write failing tests for bounded delivery and retained receiver**

Add these tests to `index_watcher.rs` using a test-only inbox capacity of one:

```rust
#[test]
fn watcher_inbox_retains_receiver_and_marks_overflow_without_blocking() {
    let root = PathBuf::from("/tmp/quickfox-watch");
    let (sink, mut inbox) = watcher_inbox(vec![root.clone()], 1);

    assert_eq!(
        sink.try_send(IndexWatchEvent::Create(root.join("first.md"))),
        WatchSendOutcome::Queued
    );
    assert_eq!(
        sink.try_send(IndexWatchEvent::Create(root.join("second.md"))),
        WatchSendOutcome::Overflowed
    );
    assert_eq!(
        inbox.try_recv(),
        Some(IndexWatchEvent::Create(root.join("first.md")))
    );
    assert_eq!(inbox.take_dirty_roots(), vec![root]);
    sink.record_failure(WatcherFailure::new(PathBuf::new(), "backend stopped"));
    assert_eq!(
        inbox.take_failure().unwrap().message,
        "watcher failed for : backend stopped; falling back to background refresh"
    );
}
```

- [ ] **Step 2: Run the focused watcher test and verify RED**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml watcher_inbox_retains_receiver_and_marks_overflow_without_blocking
```

Expected: compile failure because `watcher_inbox`, `WatchSendOutcome` and retained inbox do not exist.

- [ ] **Step 3: Implement the bounded inbox and make RuntimeIndexWatcher own it**

Use these public contracts in `index_watcher.rs`:

```rust
pub const DEFAULT_WATCH_CHANNEL_CAPACITY: usize = 8_192;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchSendOutcome {
    Queued,
    Overflowed,
    Disconnected,
}

pub struct WatchEventInbox {
    receiver: std::sync::mpsc::Receiver<IndexWatchEvent>,
    dirty_roots: std::sync::Arc<std::sync::Mutex<std::collections::BTreeSet<PathBuf>>>,
    latest_failure: std::sync::Arc<std::sync::Mutex<Option<WatcherFailure>>>,
}

impl WatchEventInbox {
    pub fn recv_timeout(&self, timeout: std::time::Duration)
        -> Result<IndexWatchEvent, std::sync::mpsc::RecvTimeoutError>;
    pub fn try_recv(&mut self) -> Option<IndexWatchEvent>;
    pub fn take_dirty_roots(&self) -> Vec<PathBuf>;
    pub fn take_failure(&self) -> Option<WatcherFailure>;
}

pub struct RuntimeIndexWatcher {
    _watcher: RecommendedWatcher,
    watched_roots: Vec<PathBuf>,
    inbox: Option<WatchEventInbox>,
}

impl RuntimeIndexWatcher {
    pub fn watch_roots(roots: Vec<PathBuf>) -> Result<Self, WatcherFailure>;
    pub fn take_inbox(&mut self) -> Option<WatchEventInbox>;
}
```

The notify callback MUST call `try_send`; on `Full`, map the event path to the longest matching watched root and insert that root into `dirty_roots`. On a notify backend error, replace `latest_failure` with a `WatcherFailure`; the coordinator consumes it and publishes `WatcherRuntimeFailed`. `WatcherInitializationFailed` remains reserved for `watch_roots` startup errors. Do not log full paths in user-visible state.

- [ ] **Step 4: Add failing coordinator deadline tests**

Create `index_update_coordinator.rs` with tests first:

```rust
#[test]
fn batch_flushes_after_quiet_window_or_hard_deadline() {
    let start = Instant::now();
    let policy = CoordinatorPolicy::production();
    let root = PathBuf::from("/tmp/root");
    let mut state = CoordinatorState::default();

    state.push_at(IndexWatchEvent::Create(root.join("a.md")), start);
    assert!(!state.should_flush_at(start + Duration::from_secs(4), policy));
    assert!(state.should_flush_at(start + Duration::from_secs(5), policy));

    state.push_at(IndexWatchEvent::Create(root.join("a.md")), start);
    state.push_at(
        IndexWatchEvent::Write(root.join("a.md")),
        start + Duration::from_secs(8),
    );
    assert!(state.should_flush_at(start + Duration::from_secs(10), policy));
}

#[test]
fn rename_and_followup_write_collapse_to_remove_plus_changed_path() {
    let root = PathBuf::from("/tmp/root");
    let mut state = CoordinatorState::default();
    state.push(IndexWatchEvent::Rename {
        from: root.join("old.md"),
        to: root.join("new.md"),
    });
    state.push(IndexWatchEvent::Write(root.join("new.md")));

    assert_eq!(
        state.drain(),
        CoordinatorBatch {
            changed_paths: vec![root.join("new.md")],
            removed_paths: vec![root.join("old.md")],
            dirty_roots: Vec::new(),
        }
    );
}
```

- [ ] **Step 5: Implement the pure coordinator state machine**

Use exact production policy and output types:

```rust
#[derive(Debug, Clone, Copy)]
pub struct CoordinatorPolicy {
    pub quiet_window: Duration,
    pub max_latency: Duration,
}

impl CoordinatorPolicy {
    pub fn production() -> Self {
        Self {
            quiet_window: Duration::from_secs(5),
            max_latency: Duration::from_secs(10),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoordinatorBatch {
    pub changed_paths: Vec<PathBuf>,
    pub removed_paths: Vec<PathBuf>,
    pub dirty_roots: Vec<PathBuf>,
}
```

`CoordinatorState` must reuse `IndexEventBatcher`, track `first_event_at` and `last_event_at`, sort/deduplicate outputs, and reset both deadlines after `drain()`.

- [ ] **Step 6: Run the module tests and verify GREEN**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml core::index_watcher::tests
cargo test --manifest-path src-tauri/Cargo.toml core::index_update_coordinator::tests
```

Expected: all watcher and coordinator tests pass.

- [ ] **Step 7: Register the module and commit**

Add to `core/mod.rs`:

```rust
pub mod index_update_coordinator;
```

Then commit:

```bash
git add src-tauri/src/core/index_watcher.rs src-tauri/src/core/index_update_coordinator.rs src-tauri/src/core/mod.rs
git commit -m "feat: add bounded index event coordinator"
```

## Task 2: Layered search view without baseline rebuilds

**Files:**

- Create: `src-tauri/src/core/layered_index.rs`
- Modify: `src-tauri/src/core/index.rs`
- Modify: `src-tauri/src/core/compact_index.rs`
- Modify: `src-tauri/src/core/providers.rs`
- Modify: `src-tauri/src/core/mod.rs`

- [ ] **Step 1: Write failing overlay/tombstone behavior tests**

Create `layered_index.rs` and add:

```rust
#[test]
fn overlay_replaces_baseline_and_directory_tombstone_hides_descendants() {
    let root = PathBuf::from("/tmp/root");
    let old = entry(root.join("docs/readme.md"), &root, IndexedEntryKind::File);
    let sibling = entry(root.join("keep.md"), &root, IndexedEntryKind::File);
    let replacement = entry(root.join("docs/readme.md"), &root, IndexedEntryKind::File);
    let mut index = LayeredSearchIndex::from_baseline(vec![old, sibling]);

    index.apply_delta(CommittedIndexDelta {
        generation: 1,
        upserts: vec![replacement],
        removals: Vec::new(),
    });

    assert_eq!(index.search(&request("readme"), 20).len(), 1);

    index.apply_delta(CommittedIndexDelta {
        generation: 2,
        upserts: Vec::new(),
        removals: vec![root.join("docs")],
    });

    assert!(index.search(&request("readme"), 20).is_empty());
    assert_eq!(index.search(&request("keep"), 20).len(), 1);
}

#[test]
fn small_delta_does_not_rebuild_compact_baseline() {
    CompactCandidateIndex::reset_build_count();
    let root = PathBuf::from("/tmp/root");
    let mut index = LayeredSearchIndex::from_baseline(vec![entry(
        root.join("base.md"),
        &root,
        IndexedEntryKind::File,
    )]);
    let baseline_builds = CompactCandidateIndex::build_count();

    index.apply_delta(CommittedIndexDelta {
        generation: 1,
        upserts: vec![entry(root.join("new.md"), &root, IndexedEntryKind::File)],
        removals: Vec::new(),
    });

    assert_eq!(CompactCandidateIndex::build_count(), baseline_builds + 1);
    assert_eq!(index.baseline_build_count(), baseline_builds);
}
```

Add these test helpers in the same `tests` module:

```rust
fn entry(path: PathBuf, root: &Path, kind: IndexedEntryKind) -> IndexedEntry {
    IndexedEntry::from_path_metadata(path, root, kind)
}

fn request(text: &str) -> QueryRequest {
    QueryRequest::new(text, SearchMode::Normal)
}
```

The one additional build is the tiny overlay index; baseline build count captured by the layered type must remain unchanged.

- [ ] **Step 2: Run the layered test and verify RED**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml core::layered_index::tests --no-run
```

Expected: compile failure because `LayeredSearchIndex` and visibility-aware search do not exist.

- [ ] **Step 3: Add visibility-aware SearchIndex entry points**

Add to `SearchIndex`:

```rust
pub fn search_with_limit_visible(
    &self,
    query: &QueryRequest,
    limit: usize,
    is_visible: impl Fn(&IndexedEntry) -> bool,
) -> Vec<SearchResult> {
    self.search_with_limit_cancellable_visible(query, limit, || false, is_visible)
        .unwrap_or_default()
}
```

Thread `is_visible` into normal, regex and content-only searches. In normal search, apply it immediately after loading `entry` and before matcher/limit logic:

```rust
let Some(entry) = self.entries.get(index) else { continue };
if !is_visible(entry) {
    continue;
}
```

This placement is required so tombstoned candidates do not consume the result limit.

- [ ] **Step 4: Add a test-only compact build counter**

Mirror the existing `SEARCH_INDEX_CLONE_COUNT` pattern in `compact_index.rs`:

```rust
#[cfg(test)]
static COMPACT_INDEX_BUILD_COUNT: AtomicUsize = AtomicUsize::new(0);

impl CompactCandidateIndex {
    #[cfg(test)]
    pub fn reset_build_count() {
        COMPACT_INDEX_BUILD_COUNT.store(0, Ordering::Relaxed);
    }

    #[cfg(test)]
    pub fn build_count() -> usize {
        COMPACT_INDEX_BUILD_COUNT.load(Ordering::Relaxed)
    }
}
```

Increment it once at the top of `CompactCandidateIndex::from_entries` under `#[cfg(test)]`.

- [ ] **Step 5: Implement the layered types**

Use these public contracts in `layered_index.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommittedIndexDelta {
    pub generation: u64,
    pub upserts: Vec<IndexedEntry>,
    pub removals: Vec<PathBuf>,
}

#[derive(Debug)]
pub struct LayeredSearchIndex {
    baseline: SearchIndex,
    overlay_entries: BTreeMap<String, IndexedEntry>,
    overlay: SearchIndex,
    tombstones: PathTombstones,
    generation: u64,
    baseline_build_count: usize,
}

impl LayeredSearchIndex {
    pub fn from_baseline(entries: Vec<IndexedEntry>) -> Self;
    pub fn apply_delta(&mut self, delta: CommittedIndexDelta);
    pub fn replace_baseline(&mut self, entries: Vec<IndexedEntry>, generation: u64);
    pub fn search(&self, query: &QueryRequest, limit: usize) -> Vec<SearchResult>;
    pub fn entry_count(&self) -> usize;
    pub fn delta_entry_count(&self) -> usize;
    pub fn estimated_delta_bytes(&self) -> usize;
    pub fn generation(&self) -> u64;
}
```

Normalize keys without resolving symlinks: replace `\` with `/` on comparisons and lowercase only under `cfg(target_os = "windows")`. `PathTombstones::contains` must match exact paths and segment-boundary descendants, never the string prefix `/docs-old` for tombstone `/docs`.

Merge baseline and overlay results by `id`. Query each layer with the supplied bounded candidate budget, preserve each layer's retrieval order, and return at most `2 * candidate_budget` results to `ProviderRegistry`; the existing `Ranker` remains the only final scorer/sorter before the user-visible result limit. Do not pre-sort zero-score file results inside `LayeredSearchIndex`.

The overlay query MUST use the same `FileMatcher` and compact candidate retrieval as the baseline. Baseline visibility filtering happens before its per-layer limit, so tombstones cannot exhaust the baseline candidate budget.

- [ ] **Step 6: Make FileProvider accept the shared search trait**

Define in `index.rs`:

```rust
pub trait FileSearchIndex: Send + Sync {
    fn search_files(&self, query: &QueryRequest, limit: usize) -> Vec<SearchResult>;
    fn indexed_entry_count(&self) -> usize;
}
```

Implement it for `SearchIndex` and `LayeredSearchIndex`. Change `FileProviderIndex::Borrowed` to `Borrowed(&'a dyn FileSearchIndex)` while keeping the owned `SearchIndex` test constructor.

- [ ] **Step 7: Run oracle and provider tests**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml core::layered_index::tests
cargo test --manifest-path src-tauri/Cargo.toml core::providers::tests
cargo test --manifest-path src-tauri/Cargo.toml core::index::tests
```

Expected: all pass; existing search semantics remain green.

- [ ] **Step 8: Register and commit**

```bash
git add src-tauri/src/core/layered_index.rs src-tauri/src/core/index.rs src-tauri/src/core/compact_index.rs src-tauri/src/core/providers.rs src-tauri/src/core/mod.rs
git commit -m "feat: add layered runtime search index"
```

## Task 3: Targeted scanner and directory manifest calibration

**Files:**

- Create: `src-tauri/src/core/targeted_index_scanner.rs`
- Modify: `src-tauri/src/core/index_scanner.rs`
- Modify: `src-tauri/src/core/mod.rs`

- [ ] **Step 1: Write failing single-file and subtree tests**

Add tests first:

```rust
#[test]
fn scan_changed_file_preserves_configured_root_metadata() {
    let root = temp_dir("target-file");
    let file = root.join("docs/readme.md");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, "hello").unwrap();
    let scanner = TargetedIndexScanner::new(scan_rules(&root));

    let delta = scanner.scan_changed_paths(&[file.clone()]).unwrap();

    assert_eq!(delta.upserts.len(), 1);
    assert_eq!(delta.upserts[0].root, root.to_string_lossy());
    assert_eq!(delta.upserts[0].path, file.to_string_lossy());
}

#[test]
fn scan_new_directory_indexes_only_that_subtree() {
    let root = temp_dir("target-dir");
    fs::create_dir_all(root.join("new/nested")).unwrap();
    fs::write(root.join("new/nested/file.md"), "hello").unwrap();
    fs::write(root.join("outside.md"), "outside").unwrap();
    let scanner = TargetedIndexScanner::new(scan_rules(&root));

    let delta = scanner.scan_changed_paths(&[root.join("new")]).unwrap();

    assert!(delta.upserts.iter().any(|entry| entry.name == "file.md"));
    assert!(!delta.upserts.iter().any(|entry| entry.name == "outside.md"));
}
```

- [ ] **Step 2: Run the targeted scanner tests and verify RED**

```bash
cargo test --manifest-path src-tauri/Cargo.toml core::targeted_index_scanner::tests --no-run
```

Expected: module/type compile failures.

- [ ] **Step 3: Extract reusable scan rules from IndexScanPlan**

Add this boundary to `index_scanner.rs`:

```rust
#[derive(Debug, Clone)]
pub struct IndexPathRules {
    pub roots: Vec<PathBuf>,
    exclude_dirs: HashSet<PathBuf>,
    exclude_patterns: GlobSet,
    pub respect_project_ignores: bool,
}

impl IndexPathRules {
    pub fn from_plan(plan: &IndexScanPlan) -> Result<Self, std::io::Error>;
    pub fn configured_root_for(&self, path: &Path) -> Option<&Path>;
    pub fn is_forced_or_user_excluded(&self, path: &Path) -> bool;
}
```

Move only existing exclusion compilation into this type; do not duplicate forced exclusions. `configured_root_for` chooses the longest segment-boundary root prefix.

- [ ] **Step 4: Implement TargetedIndexScanner**

Use:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectoryFingerprint {
    pub path: String,
    pub parent: Option<String>,
    pub root: String,
    pub modified_ms: Option<i64>,
}

#[derive(Debug, Default)]
pub struct TargetedScanResult {
    pub upserts: Vec<IndexedEntry>,
    pub removals: Vec<PathBuf>,
    pub manifest_upserts: Vec<DirectoryFingerprint>,
    pub manifest_removals: Vec<PathBuf>,
    pub failures: Vec<IndexFailure>,
}

pub trait DirectoryManifestReader {
    fn directories_for_root(&self, root: &Path) -> Result<Vec<DirectoryFingerprint>, String>;
}
```

For a directory target, reuse `IgnoreScanner` through a new `scan_subtree(target, configured_root, rules)` method so parent `.gitignore`/`.ignore`, QuickFox forced exclusions and user globs remain active. Sort every returned vector by normalized path.

- [ ] **Step 5: Write failing manifest calibration tests**

```rust
#[test]
fn calibration_stats_every_known_directory_but_enumerates_only_changed_directories() {
    let fs = RecordingFileSystem::fixture([
        directory("/root", 10, ["/root/a", "/root/b"]),
        directory("/root/a", 20, ["/root/a/old.md"]),
        directory("/root/b", 30, ["/root/b/stable.md"]),
    ]);
    let manifest = manifest([
        fingerprint("/root", 10),
        fingerprint("/root/a", 19),
        fingerprint("/root/b", 30),
    ]);

    let result = calibrate_manifest(&fs, &manifest, Path::new("/root")).unwrap();

    assert_eq!(fs.statted_directories(), vec!["/root", "/root/a", "/root/b"]);
    assert_eq!(fs.enumerated_directories(), vec!["/root/a"]);
    assert_eq!(result.changed_directories, vec![PathBuf::from("/root/a")]);
}
```

Use a small `FileSystemProbe` trait so this test is deterministic and does not depend on OS timestamp granularity.

- [ ] **Step 6: Implement calibration and permission semantics**

`calibrate_manifest` must:

1. stat every known directory for the root;
2. mark missing directories as subtree removals;
3. `read_dir` only changed directories;
4. recursively scan newly discovered directories;
5. record permission failures without emitting removals;
6. continue other directories.

- [ ] **Step 7: Run scanner tests and commit**

```bash
cargo test --manifest-path src-tauri/Cargo.toml core::index_scanner::tests
cargo test --manifest-path src-tauri/Cargo.toml core::targeted_index_scanner::tests
git add src-tauri/src/core/index_scanner.rs src-tauri/src/core/targeted_index_scanner.rs src-tauri/src/core/mod.rs
git commit -m "feat: add targeted index scanning"
```

## Task 4: SQLite journal, manifest, and recovery repository

**Files:**

- Modify: `src-tauri/src/core/storage.rs`
- Create: `src-tauri/src/core/index_journal.rs`
- Modify: `src-tauri/src/core/mod.rs`

- [ ] **Step 1: Write a failing schema migration test**

Add to `storage.rs` tests:

```rust
#[test]
fn migration_creates_incremental_index_tables_without_losing_legacy_snapshot() {
    let path = temp_db_path("incremental-schema");
    create_legacy_snapshot(&path, indexed_entry("/root/legacy.md"));

    let storage = SqliteStorage::open(path).unwrap();

    assert!(storage.incremental_schema_is_ready().unwrap());
    assert_eq!(storage.latest_index_snapshot().unwrap().unwrap().entries.len(), 1);
}
```

- [ ] **Step 2: Run the migration test and verify RED**

```bash
cargo test --manifest-path src-tauri/Cargo.toml migration_creates_incremental_index_tables_without_losing_legacy_snapshot
```

Expected: missing incremental schema/API.

- [ ] **Step 3: Add schema v3**

Add these tables in the existing migration transaction and set `user_version = 3`:

```sql
CREATE TABLE IF NOT EXISTS index_delta_batches (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    generation INTEGER NOT NULL UNIQUE,
    status TEXT NOT NULL CHECK (status IN ('committed')),
    committed_at_ms INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS index_delta_entries (
    batch_id INTEGER NOT NULL,
    ordinal INTEGER NOT NULL,
    operation TEXT NOT NULL CHECK (operation IN ('upsert', 'remove')),
    path TEXT NOT NULL,
    entry_json TEXT,
    PRIMARY KEY (batch_id, ordinal),
    FOREIGN KEY (batch_id) REFERENCES index_delta_batches(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_index_delta_entries_path
    ON index_delta_entries(path);
CREATE TABLE IF NOT EXISTS index_directory_manifest (
    path TEXT PRIMARY KEY NOT NULL,
    parent TEXT,
    root TEXT NOT NULL,
    modified_ms INTEGER
);
CREATE INDEX IF NOT EXISTS idx_index_directory_manifest_root
    ON index_directory_manifest(root);
CREATE TABLE IF NOT EXISTS index_runtime_state (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    last_generation INTEGER NOT NULL,
    degradation_code TEXT,
    baseline_refresh_reason TEXT
);
```

- [ ] **Step 4: Write failing transaction and replay tests**

```rust
#[test]
fn committed_delta_round_trips_in_generation_order_and_replays_idempotently() {
    let storage = SqliteStorage::open(temp_db_path("delta-replay")).unwrap();
    storage.commit_incremental_batch(&delta(2, "two.md"), &[], &[]).unwrap();
    storage.commit_incremental_batch(&delta(1, "one.md"), &[], &[]).unwrap();

    let batches = storage.committed_index_deltas_after(0).unwrap();
    assert_eq!(batches.iter().map(|batch| batch.generation).collect::<Vec<_>>(), vec![1, 2]);

    let once = replay_deltas(empty_layers(), &batches).unwrap();
    let twice = replay_deltas(once.clone(), &batches).unwrap();
    assert_eq!(twice, once);
}

#[test]
fn malformed_journal_returns_recovery_failure_without_deleting_baseline() {
    let storage = storage_with_baseline_and_malformed_delta();
    let baseline = storage.latest_index_snapshot().unwrap().unwrap();

    let recovery = recover_layered_index(&storage, baseline.entries.clone());

    assert_eq!(recovery.baseline_entry_count(), baseline.entries.len());
    assert_eq!(recovery.degradation_code(), Some(IndexDegradationCode::JournalReplayFailed));
}
```

- [ ] **Step 5: Implement repository APIs and recovery types**

Add to `storage.rs`:

```rust
pub fn commit_incremental_batch(
    &self,
    delta: &CommittedIndexDelta,
    manifest_upserts: &[DirectoryFingerprint],
    manifest_removals: &[PathBuf],
) -> Result<(), StorageError>;
pub fn committed_index_deltas_after(&self, generation: u64)
    -> Result<Vec<CommittedIndexDelta>, StorageError>;
pub fn replace_directory_manifest(&self, root: &Path, rows: &[DirectoryFingerprint])
    -> Result<(), StorageError>;
pub fn directory_manifest_for_root(&self, root: &Path)
    -> Result<Vec<DirectoryFingerprint>, StorageError>;
pub fn clear_incremental_state_through(&self, generation: u64) -> Result<(), StorageError>;
```

Create `index_journal.rs` with:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IndexDegradationCode {
    WatcherInitializationFailed,
    WatcherRuntimeFailed,
    WatcherOverflow,
    ChannelOverflow,
    JournalWriteFailed,
    JournalReplayFailed,
    CalibrationFailed,
    FullRefreshFallback,
}

pub struct IndexRecovery {
    pub index: LayeredSearchIndex,
    pub degradation: Option<IndexDegradationCode>,
}

pub trait IndexJournalRepository {
    fn commit_incremental_batch(
        &mut self,
        delta: &CommittedIndexDelta,
        manifest_upserts: &[DirectoryFingerprint],
        manifest_removals: &[PathBuf],
    ) -> Result<(), String>;
    fn directory_manifest_for_root(
        &self,
        root: &Path,
    ) -> Result<Vec<DirectoryFingerprint>, String>;
    fn clear_incremental_state_through(&mut self, generation: u64) -> Result<(), String>;
}
```

Implement `IndexJournalRepository` for `SqliteStorage`. `commit_incremental_batch` MUST write the journal rows and manifest changes in one SQLite transaction. Use `serde_json` only for `IndexedEntry` payloads. Validate that an `upsert` has JSON and a `remove` does not require JSON; return `StorageError` on invalid combinations.

- [ ] **Step 6: Run storage and recovery tests**

```bash
cargo test --manifest-path src-tauri/Cargo.toml core::storage::tests
cargo test --manifest-path src-tauri/Cargo.toml core::index_journal::tests
```

Expected: all existing and new storage tests pass.

- [ ] **Step 7: Commit**

```bash
git add src-tauri/src/core/storage.rs src-tauri/src/core/index_journal.rs src-tauri/src/core/mod.rs
git commit -m "feat: persist incremental index journal"
```

## Task 5: Runtime indexing service and atomic commit order

**Files:**

- Create: `src-tauri/src/core/runtime_indexing.rs`
- Modify: `src-tauri/src/core/index_update_coordinator.rs`
- Modify: `src-tauri/src/core/index_entry.rs`
- Modify: `src-tauri/src/core/mod.rs`
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Write a failing runtime service test for receiver consumption**

Use injected fake watcher/scanner/journal implementations:

```rust
#[test]
fn watcher_event_is_committed_and_published_to_runtime_view() {
    let root = PathBuf::from("/root");
    let harness = RuntimeIndexingHarness::new(vec![entry("/root/base.md")]);
    harness.push(IndexWatchEvent::Create(root.join("new.md")));
    harness.advance(Duration::from_secs(5));

    let published = harness.take_published_delta().expect("delta published");
    assert_eq!(published.upserts[0].name, "new.md");
    assert_eq!(harness.journal_generations(), vec![1]);
    assert!(harness.search("new").iter().any(|result| result.title == "new.md"));
}
```

- [ ] **Step 2: Run the test and verify RED**

```bash
cargo test --manifest-path src-tauri/Cargo.toml watcher_event_is_committed_and_published_to_runtime_view
```

Expected: `runtime_indexing` service missing.

- [ ] **Step 3: Implement a Tauri-neutral background service**

Use these contracts:

```rust
pub struct RuntimeIndexingOptions {
    pub roots: Vec<PathBuf>,
    pub policy: CoordinatorPolicy,
    pub initial_generation: u64,
}

pub enum RuntimeIndexingEvent {
    DeltaCommitted(CommittedIndexDelta),
    Status(RuntimeIncrementalStatus),
    BaselineRefreshRequired { reason: BaselineRefreshReason },
}
```

Define the shared serialized status types in `index_entry.rs` so both the service and `IndexStatus` use one contract:

```rust

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IncrementalState {
    Disabled,
    Preparing,
    Watching,
    Degraded,
    Calibrating,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeIncrementalStatus {
    pub enabled: bool,
    pub state: IncrementalState,
    pub pending_events: usize,
    pub dirty_roots: usize,
    pub last_batch_entries: usize,
    pub last_batch_duration_ms: u64,
    pub degradation_code: Option<IndexDegradationCode>,
}

impl Default for RuntimeIncrementalStatus {
    fn default() -> Self {
        Self {
            enabled: true,
            state: IncrementalState::Preparing,
            pending_events: 0,
            dirty_roots: 0,
            last_batch_entries: 0,
            last_batch_duration_ms: 0,
            degradation_code: None,
        }
    }
}

```

Continue the service contract in `runtime_indexing.rs`:

```rust

pub struct RuntimeIndexingHandle {
    stop: SyncSender<()>,
    join: Option<JoinHandle<()>>,
}

impl RuntimeIndexingHandle {
    pub fn stop(mut self);
}

pub fn start_runtime_indexing(
    watcher: RuntimeIndexWatcher,
    scanner: TargetedIndexScanner,
    journal: Box<dyn IndexJournalRepository + Send>,
    options: RuntimeIndexingOptions,
    publish: impl Fn(RuntimeIndexingEvent) + Send + 'static,
) -> Result<RuntimeIndexingHandle, WatcherFailure>;
```

Initialize `let mut next_generation = options.initial_generation.saturating_add(1);` once before the worker event loop. The order for every normal batch MUST then be:

```rust
let scanned = scanner.scan_batch(batch)?;
let delta = CommittedIndexDelta {
    generation: next_generation,
    upserts: scanned.upserts,
    removals: scanned.removals,
};
journal.commit_incremental_batch(
    &delta,
    &scanned.manifest_upserts,
    &scanned.manifest_removals,
)?;
publish(RuntimeIndexingEvent::DeltaCommitted(delta));
next_generation = next_generation.saturating_add(1);
```

If journal commit fails, publish status `JournalWriteFailed` and never publish `DeltaCommitted`.

- [ ] **Step 4: Add a failing stop/leak test**

```rust
#[test]
fn stopping_runtime_indexing_disconnects_watcher_and_joins_worker() {
    let harness = RuntimeIndexingHarness::started();
    let worker = harness.worker_probe();
    harness.stop();
    assert!(worker.joined_within(Duration::from_secs(1)));
}
```

Implement stop with a bounded wakeup interval no greater than 250ms. Do not hold QuickFox runtime mutex while joining.

- [ ] **Step 5: Replace QuickFoxRuntime index/watcher fields**

Change the runtime core fields to:

```rust
struct QuickFoxRuntime {
    config: QuickFoxConfig,
    index: LayeredSearchIndex,
    last_report: IndexReport,
    index_lifecycle: IndexLifecycle,
    runtime_indexing: Option<RuntimeIndexingHandle>,
    incremental_status: RuntimeIncrementalStatus,
}
```

`QuickFoxRuntime::index_status()` must clone the lifecycle status and then assign `status.incremental = self.incremental_status.clone()` so the two state sources cannot diverge in serialized output.

Update `build_provider_registry`, `perform_search_with_index_status` and `file_index_is_available` to accept `&dyn FileSearchIndex`; availability uses `indexed_entry_count()`. The test-only `perform_search` may continue accepting `&SearchIndex` because it coerces to the trait object.

Update `build_runtime_from_snapshot` to call `recover_layered_index`; update FileProvider construction to borrow `&runtime.index` as `dyn FileSearchIndex`.

- [ ] **Step 6: Wire published events through run_on_main_thread**

Create one `publish` closure in `lib.rs` that clones `AppHandle`, calls `run_on_main_thread`, applies a committed delta under the runtime mutex, derives `IndexStatus`, releases the lock, then emits `quickfox://index-status`.

Do not call SQLite while holding `QuickFoxRuntime` mutex.

- [ ] **Step 7: Replace watcher startup after baseline refresh**

Delete the old `start_runtime_index_watcher` implementation that creates `(sender, _receiver)`. Add:

```rust
fn restart_runtime_incremental_indexing<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: &QuickFoxAppState,
) -> Result<(), String>;
```

Call it after a successful baseline switch and after recovered manifest readiness. Stop the previous handle outside the runtime lock before starting a replacement.

- [ ] **Step 8: Add the 50,000/64 MiB safety fallback**

After each committed delta:

```rust
if runtime.index.delta_entry_count() >= 50_000
    || runtime.index.estimated_delta_bytes() >= 64 * 1024 * 1024
{
    RuntimeIndexingEvent::BaselineRefreshRequired {
        reason: BaselineRefreshReason::DeltaSafetyLimit,
    }
}
```

The integration layer starts at most one existing background refresh; successful `replace_baseline` calls `clear_incremental_state_through(generation)`.

- [ ] **Step 9: Run runtime tests and commit**

```bash
cargo test --manifest-path src-tauri/Cargo.toml core::runtime_indexing::tests
cargo test --manifest-path src-tauri/Cargo.toml watcher_events_flow_into_runtime_index
git add src-tauri/src/core/runtime_indexing.rs src-tauri/src/core/index_update_coordinator.rs src-tauri/src/core/index_entry.rs src-tauri/src/core/mod.rs src-tauri/src/lib.rs
git commit -m "feat: connect runtime incremental indexing"
```

## Task 6: Manual calibration and configuration semantics

**Files:**

- Modify: `src-tauri/src/core/config.rs`
- Modify: `src-tauri/src/core/runtime_indexing.rs`
- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/src/core/storage.rs`

- [ ] **Step 1: Write failing config classification tests**

```rust
#[test]
fn watcher_toggle_does_not_change_index_semantics() {
    let before = config();
    let mut after = before.clone();
    after.index.watcher_enabled = false;
    assert_eq!(classify_index_config_change(&before.index, &after.index), IndexConfigChange::WatcherOnly);
}

#[test]
fn roots_or_filter_changes_require_full_rebuild() {
    let before = config();
    let mut after = before.clone();
    after.index.exclude_patterns.push("dist".to_owned());
    assert_eq!(classify_index_config_change(&before.index, &after.index), IndexConfigChange::IndexSemantics);
}
```

- [ ] **Step 2: Implement exact config classification**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexConfigChange {
    None,
    WatcherOnly,
    IndexSemantics,
}
```

`IndexSemantics` compares include/exclude dirs, patterns, performance mode, project ignores, content roots and max content bytes. Only `watcher_enabled` maps to `WatcherOnly`.

- [ ] **Step 3: Write failing manual refresh routing tests**

```rust
#[test]
fn manual_refresh_uses_manifest_calibration_when_state_is_trusted() {
    let decision = refresh_decision(&TrustedIncrementalState::ready(), IndexConfigChange::None);
    assert_eq!(decision, RefreshDecision::CalibrateAllRoots);
}

#[test]
fn missing_manifest_or_semantic_change_uses_full_refresh_with_reason() {
    assert_eq!(
        refresh_decision(&TrustedIncrementalState::missing_manifest(), IndexConfigChange::None),
        RefreshDecision::FullRefresh(BaselineRefreshReason::ManifestUnavailable)
    );
    assert_eq!(
        refresh_decision(&TrustedIncrementalState::ready(), IndexConfigChange::IndexSemantics),
        RefreshDecision::FullRefresh(BaselineRefreshReason::IndexConfigChanged)
    );
}
```

- [ ] **Step 4: Implement manual refresh routing**

Change the Tauri `refresh_index` command to request `FlushPendingThenCalibrateAllRoots` from the runtime service when trusted incremental state is ready. Only call `start_background_index_refresh` for an explicit `FullRefresh(reason)` decision.

- [ ] **Step 5: Change save_config behavior**

Capture `before` before assigning `runtime.config`. Then:

- `None`: only save config/hotkey.
- `WatcherOnly` off: take and stop runtime service outside lock.
- `WatcherOnly` on: start runtime service and immediately calibrate all roots.
- `IndexSemantics`: stop runtime service, start background full refresh with `IndexConfigChanged` reason.

Add command-level tests proving watcher-only saves do not increment full-refresh generation.

- [ ] **Step 6: Verify legacy default remains enabled**

Retain:

```rust
#[serde(default = "default_true")]
pub watcher_enabled: bool,
```

Add a TOML fixture without `watcher_enabled` and assert it deserializes to true.

- [ ] **Step 7: Run config/runtime tests and commit**

```bash
cargo test --manifest-path src-tauri/Cargo.toml core::config::tests
cargo test --manifest-path src-tauri/Cargo.toml manual_refresh
cargo test --manifest-path src-tauri/Cargo.toml save_config
git add src-tauri/src/core/config.rs src-tauri/src/core/runtime_indexing.rs src-tauri/src/core/storage.rs src-tauri/src/lib.rs
git commit -m "feat: route index refreshes incrementally"
```

## Task 7: Structured incremental status and frontend contract

**Files:**

- Modify: `src-tauri/src/core/index_entry.rs`
- Modify: `src-tauri/src/lib.rs`
- Modify: `src/tauriClient.ts`
- Modify: `src/App.tsx`
- Modify: `src/App.test.tsx`
- Modify: `src/tauriClient.test.ts`

- [ ] **Step 1: Write a failing Rust serialization test**

```rust
#[test]
fn index_status_serializes_incremental_summary_without_paths() {
    let mut status = IndexLifecycle::from_ready(20, 100).status().clone();
    status.incremental = RuntimeIncrementalStatus {
        enabled: true,
        state: IncrementalState::Degraded,
        pending_events: 12,
        dirty_roots: 1,
        last_batch_entries: 8,
        last_batch_duration_ms: 42,
        degradation_code: Some(IndexDegradationCode::WatcherOverflow),
    };
    let json = serde_json::to_value(status).unwrap();

    assert_eq!(json["incremental"]["pendingEvents"], 12);
    assert_eq!(json["incremental"]["degradationCode"], "watcherOverflow");
    assert!(!json.to_string().contains("/Users/"));
}
```

- [ ] **Step 2: Attach the runtime status to the serialized index status**

Reuse `IncrementalState` and `RuntimeIncrementalStatus` already defined in `index_entry.rs`. Add this field to `IndexStatus` and initialize it in every `IndexLifecycle` constructor:

```rust
#[serde(default)]
pub incremental: RuntimeIncrementalStatus,
```

Add `incremental: RuntimeIncrementalStatus` to `IndexStatus` with a serde default for old snapshots/tests.

- [ ] **Step 3: Extend TypeScript contract and defaults**

Add to `tauriClient.ts`:

```ts
export type RuntimeIncrementalStatus = {
  enabled: boolean;
  state: "disabled" | "preparing" | "watching" | "degraded" | "calibrating";
  pendingEvents: number;
  dirtyRoots: number;
  lastBatchEntries: number;
  lastBatchDurationMs: number;
  degradationCode?:
    | "watcherInitializationFailed"
    | "watcherRuntimeFailed"
    | "watcherOverflow"
    | "channelOverflow"
    | "journalWriteFailed"
    | "journalReplayFailed"
    | "calibrationFailed"
    | "fullRefreshFallback"
    | null;
};
```

- [ ] **Step 4: Write failing UI tests**

```tsx
it("shows automatic incremental indexing degradation without exposing paths", async () => {
  vi.mocked(indexStatus).mockResolvedValue(
    indexStatusWithIncremental({
      state: "degraded",
      degradationCode: "watcherOverflow",
      dirtyRoots: 1,
    }),
  );
  render(<App initialView="settings" />);

  expect(await screen.findByText("自动增量已降级，正在校准 1 个索引目录")).toBeInTheDocument();
  expect(screen.queryByText(/\/Users\//)).not.toBeInTheDocument();
});

it("keeps index status search refresh on the existing debounce path", async () => {
  let handler: ((status: IndexStatus) => void) | undefined;
  vi.mocked(listenIndexStatus).mockImplementation(async (next) => {
    handler = next;
    return () => undefined;
  });
  render(<App />);
  fireEvent.change(screen.getByLabelText("搜索文件、目录、计算器、网页搜索或命令"), {
    target: { value: "agents" },
  });
  await act(async () => vi.advanceTimersByTimeAsync(80));
  expect(search).toHaveBeenCalledTimes(1);
  handler?.(indexStatusWithIncremental({ pendingEvents: 4 }));
  handler?.(indexStatusWithIncremental({ pendingEvents: 2 }));
  await act(async () => vi.advanceTimersByTimeAsync(79));
  expect(search).toHaveBeenCalledTimes(1);
  await act(async () => vi.advanceTimersByTimeAsync(1));
  expect(search).toHaveBeenCalledTimes(2);
});
```

Add this helper once near the existing `readyIndexStatus` fixtures:

```ts
function indexStatusWithIncremental(overrides: Partial<RuntimeIncrementalStatus>): IndexStatus {
  return {
    kind: "ready",
    entryCount: 20,
    generation: 1,
    completedAtMs: 100,
    incremental: {
      enabled: true,
      state: "watching",
      pendingEvents: 0,
      dirtyRoots: 0,
      lastBatchEntries: 0,
      lastBatchDurationMs: 0,
      degradationCode: null,
      ...overrides,
    },
  };
}
```

- [ ] **Step 5: Render compact state beside the existing watcher switch**

Add a pure formatter in `App.tsx`:

```ts
function incrementalStatusText(status: RuntimeIncrementalStatus) {
  if (!status.enabled || status.state === "disabled") return "自动增量已关闭";
  if (status.state === "watching") return "自动增量运行中";
  if (status.state === "calibrating") return `正在校准 ${status.dirtyRoots} 个索引目录`;
  if (status.state === "degraded") {
    return `自动增量已降级，正在校准 ${status.dirtyRoots} 个索引目录`;
  }
  return "自动增量正在准备";
}
```

Do not add the full diagnostics panel in this change.

- [ ] **Step 6: Run frontend and contract tests**

```bash
npx vitest run src/tauriClient.test.ts src/App.test.tsx
```

Expected: all tests pass.

- [ ] **Step 7: Commit**

```bash
git add src-tauri/src/core/index_entry.rs src-tauri/src/lib.rs src/tauriClient.ts src/App.tsx src/App.test.tsx src/tauriClient.test.ts
git commit -m "feat: expose incremental index status"
```

## Task 8: Independent content-index delta updates

**Files:**

- Modify: `src-tauri/src/core/content_index.rs`
- Modify: `src-tauri/src/core/layered_index.rs`
- Modify: `src-tauri/src/core/runtime_indexing.rs`

- [ ] **Step 1: Write a failing partial-success test**

```rust
#[test]
fn content_failure_does_not_roll_back_committed_name_path_delta() {
    let harness = RuntimeIndexingHarness::with_failing_content_update();
    harness.commit(upsert("/root/new.md"));

    assert!(harness.search_name_path("new").iter().any(|result| result.title == "new.md"));
    assert_eq!(
        harness.entry("/root/new.md").unwrap().content_index_state,
        ContentIndexState::ReadFailed
    );
    assert_eq!(harness.last_degradation_code(), None);
}
```

- [ ] **Step 2: Expose a content delta outcome**

```rust
pub struct ContentDeltaOutcome {
    pub updated_states: BTreeMap<String, ContentIndexState>,
    pub failures: Vec<IndexFailure>,
}

pub fn apply_content_delta(
    &mut self,
    upserts: &mut [IndexedEntry],
    removals: &[PathBuf],
    options: &ContentIndexOptions,
) -> ContentDeltaOutcome;
```

Remove content documents for removals, then update eligible upserts. Convert extractor/read errors to per-entry `ReadFailed`; return no top-level error after the name/path journal has committed.

- [ ] **Step 3: Integrate after name/path publish**

The runtime sequence becomes:

1. targeted name/path scan;
2. journal commit;
3. layered name/path publish;
4. content delta attempt;
5. publish a second status/revision only if content states changed.

Do not rewrite the name/path journal on content failure.

- [ ] **Step 4: Run content and runtime tests**

```bash
cargo test --manifest-path src-tauri/Cargo.toml core::content_index::tests
cargo test --manifest-path src-tauri/Cargo.toml content_failure_does_not_roll_back_committed_name_path_delta
```

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/core/content_index.rs src-tauri/src/core/layered_index.rs src-tauri/src/core/runtime_indexing.rs
git commit -m "feat: update content index incrementally"
```

## Task 9: Performance, overflow, and recovery regression suite

**Files:**

- Modify: `src-tauri/src/core/index.rs`
- Modify: `src-tauri/src/core/layered_index.rs`
- Modify: `src-tauri/src/core/index_update_coordinator.rs`
- Modify: `src-tauri/src/core/storage.rs`
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Add the ignored 2M + 10K layered benchmark**

```rust
#[test]
#[ignore = "2,000,000 baseline plus 10,000 runtime delta release threshold"]
fn two_million_baseline_with_runtime_delta_stays_within_latency_budget() {
    let fixture = SyntheticLargeIndexFixture::new(2_000_000);
    let mut index = LayeredSearchIndex::from_baseline(fixture.entries);
    index.apply_delta(synthetic_delta(10_000));
    let parser = QueryParser::new(Default::default());

    for query in large_index_benchmark_queries() {
        let started = Instant::now();
        let results = index.search(&parser.parse(query.query), 20);
        let elapsed = started.elapsed();
        println!(
            "QUICKFOX_LAYERED_INDEX scale=2000000 delta=10000 query={} elapsed_us={} results={} delta_bytes={}",
            query.name,
            elapsed.as_micros(),
            results.len(),
            index.estimated_delta_bytes(),
        );
        assert!(elapsed.as_millis() <= 50, "{} took {elapsed:?}", query.name);
    }
}
```

- [ ] **Step 2: Add deterministic overflow and recovery tests**

Cover:

- 8,193 events in a capacity-8,192 inbox produce dirty root and bounded queue;
- repeated journal replay produces identical layered entries;
- malformed journal retains baseline and sets `JournalReplayFailed`;
- permission failure does not create removal;
- baseline refresh failure keeps old layered view;
- successful baseline replacement clears journal through the applied generation.

Each case must be a named test, not a single loop with silent branches.

- [ ] **Step 3: Run CI-scale tests**

```bash
cargo test --manifest-path src-tauri/Cargo.toml layered
cargo test --manifest-path src-tauri/Cargo.toml overflow
cargo test --manifest-path src-tauri/Cargo.toml journal
cargo test --manifest-path src-tauri/Cargo.toml calibration
```

Expected: all pass in debug mode.

- [ ] **Step 4: Run the release threshold**

```bash
cargo test --release --manifest-path src-tauri/Cargo.toml two_million_baseline_with_runtime_delta_stays_within_latency_budget -- --ignored --nocapture
```

Expected: every printed query is `<= 50ms`; save output for the final verification report.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/core/index.rs src-tauri/src/core/layered_index.rs src-tauri/src/core/index_update_coordinator.rs src-tauri/src/core/storage.rs src-tauri/src/lib.rs
git commit -m "test: cover incremental index performance"
```

## Task 10: Documentation, platform QA, and final verification

**Files:**

- Modify: `docs/architecture.md`
- Modify: `docs/large-index-performance.md`
- Modify: `docs/troubleshooting.md`
- Modify: `docs/macos-manual-qa.md`
- Modify: `docs/windows-manual-qa.md`
- Modify: `openspec/changes/complete-runtime-incremental-indexing/tasks.md`

- [ ] **Step 1: Update architecture and troubleshooting docs**

Document these exact user-visible rules:

- automatic incremental indexing defaults on;
- 5-second quiet window and 10-second maximum ordinary visibility;
- manual refresh flushes pending events then calibrates the manifest;
- config semantic changes and corrupted incremental state trigger full refresh;
- watcher/channel overflow preserves old search and marks roots dirty;
- 50,000 entries or 64 MiB triggers background baseline refresh;
- no paths or query text are sent to the frontend incremental status.

- [ ] **Step 2: Add macOS manual QA cases**

In `docs/macos-manual-qa.md`, add checkboxes for create/write/rename/delete, 1,000-file Git checkout, sleep/wake, watched root removal, 10-second timing, UI responsiveness and recovery status screenshot.

- [ ] **Step 3: Add Windows manual QA cases**

In `docs/windows-manual-qa.md`, add the same cases for NTFS C:/D: roots plus rename across directories, drive disconnect/reconnect when available, and task-manager memory observation.

- [ ] **Step 4: Run formatting and focused verification**

```bash
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
npm test
npm run build
```

Expected: all commands exit 0 with no warnings promoted by clippy.

- [ ] **Step 5: Run the full project check**

```bash
npm run check
```

Expected: Prettier, ESLint, Vitest, TypeScript/Vite build, rustfmt, clippy and Rust tests all pass.

- [ ] **Step 6: Run strict OpenSpec validation**

```bash
openspec validate complete-runtime-incremental-indexing --strict
```

Expected: `Change 'complete-runtime-incremental-indexing' is valid`.

- [ ] **Step 7: Update task checkboxes only with evidence**

Mark an OpenSpec task complete only after its tests or QA record exist. If Windows hardware is unavailable, leave task 7.5/8.4 unchecked and report the release blocker instead of claiming completion.

- [ ] **Step 8: Commit documentation and verification records**

```bash
git add docs/architecture.md docs/large-index-performance.md docs/troubleshooting.md docs/macos-manual-qa.md docs/windows-manual-qa.md openspec/changes/complete-runtime-incremental-indexing/tasks.md
git commit -m "docs: verify runtime incremental indexing"
```
