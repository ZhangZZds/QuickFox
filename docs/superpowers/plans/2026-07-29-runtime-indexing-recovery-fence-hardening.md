# Runtime Indexing Recovery Fence Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 修复 revision journal 污染、重复 full refresh、baseline generation 拒绝、successor 非持久窗口、startup calibration、missing root Ready 和 spawn failure 漏斗。

**Architecture:** 在 `core/index_refresh_orchestrator.rs` 建立可测试的 refresh 状态机与依赖 seam；配置切换使用新 capture watcher + old service durable handoff 形成 generation fence；baseline 通过 journal materialization 以权威 generation 安装。

**Tech Stack:** Rust、Tauri、SQLite/rusqlite、notify、现有 layered index/journal。

---

### Task 1: Revision compatibility fence

**Files:**

- Create: `src-tauri/src/core/index_refresh_orchestrator.rs`
- Modify: `src-tauri/src/core/mod.rs`
- Modify: `src-tauri/src/lib.rs`

- [x] 写生产配置切换测试：旧 revision 在新 capture 开始后提交 delta，断言新 refresh tail 不包含旧语义 delta。
- [x] 运行定向测试确认旧实现 RED。
- [x] 实现 capture-before-handoff、old service join、highest-generation fence 与新 revision compatible tail。
- [x] 运行定向测试确认 GREEN。

### Task 2: Refresh request latch

**Files:**

- Modify: `src-tauri/src/core/index_refresh_orchestrator.rs`
- Modify: `src-tauri/src/lib.rs`

- [x] 写测试证明 refresh in flight 时后续 DeltaSafetyLimit 不设置 pending。
- [x] 运行测试确认 RED。
- [x] 区分 safety request 与 recovery fallback latch，并实现一次性 in-flight 语义。
- [x] 运行测试确认 GREEN。

### Task 3: Authoritative baseline generation

**Files:**

- Modify: `src-tauri/src/core/layered_index.rs`
- Modify: `src-tauri/src/core/index_refresh_orchestrator.rs`
- Modify: `src-tauri/src/lib.rs`

- [x] 写生产 apply 测试：runtime generation 已领先 scan-start，name/content baseline 仍以 materialized tail generation 安装。
- [x] 运行测试确认 RED。
- [x] 实现连续 journal tail materialization 与 baseline install/replay，缺 generation 时拒绝替换。
- [x] 运行测试确认 GREEN。

### Task 4: Durable successor capture and crash recovery

**Files:**

- Modify: `src-tauri/src/core/index_refresh_orchestrator.rs`
- Modify: `src-tauri/src/lib.rs`

- [x] 写真实 watcher 测试：successor event 到达后、activation 前模拟 crash，SQLite restart 可恢复。
- [x] 运行测试确认 RED。
- [x] 在 handoff barrier 后立即启动 successor journal worker，并保持 Preparing 状态直到 baseline 安装。
- [x] 运行测试确认 GREEN。

### Task 5: Startup manifest calibration and missing roots

**Files:**

- Modify: `src-tauri/src/core/index_refresh_orchestrator.rs`
- Modify: `src-tauri/src/lib.rs`

- [x] 写 startup production-path 测试：离线文件变化被 calibration 发现；missing configured root 不进入 Ready。
- [x] 运行测试确认 RED。
- [x] 恢复后执行 manifest calibration gate，失败 root 保持 Degraded/dirty retry。
- [x] 运行测试确认 GREEN。

### Task 6: Injectable background spawn seam

**Files:**

- Modify: `src-tauri/src/core/index_refresh_orchestrator.rs`
- Modify: `src-tauri/src/lib.rs`

- [x] 写测试通过生产 spawner seam 注入 spawn failure，并调用生产 failure funnel，断言 standby/handle 清理、结构化 fallback 与一次 retry。
- [x] 运行测试确认 RED。
- [x] 实现 `RefreshThreadSpawner` seam 并让生产 `thread::Builder` 与测试失败共用 failure funnel。
- [x] 运行测试确认 GREEN。

### Task 7: Cleanup, review, verification and commit

**Files:**

- Modify: `src-tauri/src/lib.rs`
- Modify: `docs/superpowers/specs/2026-07-29-runtime-indexing-recovery-fence-design.md`
- Modify: `docs/superpowers/plans/2026-07-29-runtime-indexing-recovery-fence-hardening.md`

- [x] 删除仅测试的平行 orchestration 和死接口，确保 `lib.rs` 只保留 Tauri wiring。
- [x] 运行所有定向测试、完整 Rust、前端 lint/test/build、OpenSpec strict validate 与 `git diff --check`。
- [x] 请求内部 reviewer 审查 revision fence、durability、generation、startup calibration、spawn failure 和锁序。
- [x] 提交单独 fix commit，不修改 OpenSpec tasks。
