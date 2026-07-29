# Runtime Incremental Indexing Quality Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 消除运行期增量索引在 journal 失败、事件风暴、服务重启和全量刷新交接期间的数据丢失与旧状态污染。

**Architecture:** 将有界事件接收、失败恢复和 watcher 交接留在 `core/runtime_indexing.rs`；Tauri 层只管理带 epoch/config revision 的服务身份与 full-refresh revision fence。全量刷新以 scan-start generation 为基线，standby watcher 覆盖交接窗口，旧服务 drain 后保留并重放 journal tail，再启动 standby service。

**Tech Stack:** Rust、Tauri、SQLite/rusqlite、notify、现有 layered index 与 journal。

---

### Task 1: Journal failure recovery and bounded draining

**Files:**

- Modify: `src-tauri/src/core/index_update_coordinator.rs`
- Modify: `src-tauri/src/core/runtime_indexing.rs`

- [x] **Step 1: Write failing tests**

新增测试，要求 journal commit 失败后只发布一次 `BaselineRefreshRequired::DirtyRoots`、generation 不前进，并由模拟 full scan/baseline recovery 找回失败路径；持续 producer 下 `RuntimeIndexingHandle::stop()` 在 250ms 内返回，且 coordinator pending unique paths 从不超过 8192。

- [x] **Step 2: Verify RED**

Run: `cargo test --manifest-path src-tauri/Cargo.toml journal_failure_requests_recoverable_fallback bounded_drain -- --nocapture`

Expected: journal failure 没有 fallback，或持续 producer 令 stop 超时/pending 超界。

- [x] **Step 3: Implement the bounds and recovery latch**

加入：

```rust
pub const MAX_PENDING_UNIQUE_PATHS: usize = 8192;
const MAX_INBOX_DRAIN_PER_TICK: usize = 256;

pub enum CoordinatorPushOutcome {
    Queued,
    CapacityReached,
}
```

`drain_inbox` 每项检查 shutdown 且单轮最多 256；capacity reached 时清逐项事件、将相关 configured roots 标 dirty、发布 `ChannelOverflow` 并 latch 一次 `DirtyRoots` fallback。journal commit failure 使用相同 dirty/fallback 路径且不递增 generation。

- [x] **Step 4: Verify GREEN**

Run: `cargo test --manifest-path src-tauri/Cargo.toml journal_failure bounded_drain coordinator -- --nocapture`

Expected: 新测试和现有 coordinator/runtime tests 全部通过。

### Task 2: Service epoch and stale callback semantics

**Files:**

- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/src/core/runtime_indexing.rs`

- [x] **Step 1: Write failing tests**

新增确定性测试：service B 激活后，service A 排队的 `Watching`/fallback 不得改变状态或触发 refresh；A 已 committed 的 delta 在 generation 新于当前 view 时必须通过 journal handoff/recovery应用，不能因 epoch 不匹配而丢弃。

- [x] **Step 2: Verify RED**

Run: `cargo test --manifest-path src-tauri/Cargo.toml stale_runtime_service -- --nocapture`

Expected: 旧 status/fallback 污染当前 runtime。

- [x] **Step 3: Implement service identity**

加入：

```rust
struct RuntimeServiceIdentity {
    epoch: u64,
    config_revision: u64,
}

struct RuntimeIndexingEnvelope {
    service: RuntimeServiceIdentity,
    event: RuntimeIndexingEvent,
}
```

每次 start 分配递增 epoch；Status/Fallback 只接受 active identity。旧 `DeltaCommitted` 仅在 generation 新于 view 且 config revision 兼容时幂等应用，否则保留 journal 并由 active refresh/recovery吸收。

- [x] **Step 4: Verify GREEN**

Run: `cargo test --manifest-path src-tauri/Cargo.toml stale_runtime_service -- --nocapture`

Expected: 两个 stale callback tests 通过。

### Task 3: Gap-free full-refresh handoff

**Files:**

- Modify: `src-tauri/src/core/runtime_indexing.rs`
- Modify: `src-tauri/src/core/storage.rs`
- Modify: `src-tauri/src/lib.rs`

- [x] **Step 1: Write failing interleaving tests**

测试在 full scan 中创建文件，以及 scan 完成后、standby 启动前创建文件。交接后与进程重启恢复均必须可搜索，generation 连续，manifest 覆盖 roots 且不使用过期 fingerprint。

- [x] **Step 2: Verify RED**

Run: `cargo test --manifest-path src-tauri/Cargo.toml full_refresh_handoff -- --nocapture`

Expected: 当前扫描前停止 watcher，至少一个交错文件丢失。

- [x] **Step 3: Implement barrier and journal-tail replay**

加入 handoff command，使旧 service 先停止 watcher capture、drain 已接收事件并 join；在停止旧服务前先构造 standby watcher。记录 scan-start generation，旧服务 quiesce 后读取 `committed_index_deltas_after(scan_start_generation)` 与权威 manifest；activation 只清理 `<= scan_start_generation`，运行时 baseline 切换后按 generation 重放 tail，并以 tail 后 generation 启动 standby service消费已排队事件。

- [x] **Step 4: Verify GREEN**

Run: `cargo test --manifest-path src-tauri/Cargo.toml full_refresh_handoff -- --nocapture`

Expected: scan 中、barrier 两侧事件在 live view 与 recovery 中均一致。

### Task 4: Centralized restart degradation

**Files:**

- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/src/core/runtime_indexing.rs`

- [x] **Step 1: Write failing injected-failure tests**

分别注入 watcher、rules、storage unavailable、worker spawn failure；都必须清空 handle、写 `Degraded` 与结构化 code、emit status，并只请求一次 recoverable fallback。missing configured root 必须保持 degraded/dirty retry，不能宣称 manifest ready。

- [x] **Step 2: Verify RED**

Run: `cargo test --manifest-path src-tauri/Cargo.toml runtime_restart_failure missing_configured_root -- --nocapture`

Expected: rules/storage/spawn 仅返回字符串或 missing root 被忽略。

- [x] **Step 3: Implement one failure funnel**

所有 restart stage 返回统一 `RuntimeRestartFailure`，由单一函数清 handle、设置 `WatcherInitializationFailed`/`FullRefreshFallback`、失效 manifest、emit 并 latch recovery。`persist_and_activate_baseline` 在 storage unavailable 时返回 `Err`；configured root 不存在时保留 dirty root 并保持 Preparing/Degraded，等待后续 retry。

- [x] **Step 4: Verify GREEN**

Run: `cargo test --manifest-path src-tauri/Cargo.toml runtime_restart_failure missing_configured_root -- --nocapture`

Expected: 全部 failure matrix tests 通过。

### Task 5: Full verification and review

**Files:**

- Modify: `docs/superpowers/plans/2026-07-25-runtime-indexing-quality-hardening.md`

- [x] **Step 1: Run complete verification**

Run: `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check && cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings && cargo test --manifest-path src-tauri/Cargo.toml && npm run lint && npm run test && npm run build && openspec validate complete-runtime-incremental-indexing --strict && git diff --check`

Expected: 所有命令退出 0，Rust tests 0 failed。

- [x] **Step 2: Request final concurrency review**

审查 fence→runtime 锁序、handoff event coverage、epoch stale callback、journal generation/manifest recovery、所有 failure exits。

- [x] **Step 3: Commit**

Run: `git add src-tauri/src docs/superpowers/plans/2026-07-25-runtime-indexing-quality-hardening.md && git commit -m "fix: harden runtime indexing handoff"`
