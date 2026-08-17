# Fix Index Settings State Transitions Implementation Plan

**Goal:** 修复 v1.6.x 索引设置同步阻塞、失败回滚、fast 范围泄漏、旧扫描不可取消和前端无反馈。

**Architecture:** `config.toml`/runtime config 表示 desired revision，最近可用索引表示 applied view；保存只提交 desired config，现有后台 refresh 通过 identity fence 异步发布 matching revision。scanner 增加协作式取消，所有生命周期共用 performance-mode active roots。

**Tech Stack:** Rust、Tauri、React/TypeScript、SQLite/rusqlite、ignore/notify、Vitest。

---

### Task 1: Desired config regression and commit

**Files:**

- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/src/core/config.rs`

- [x] 写失败测试：persist hook 在 scanner/worker 前执行，调度失败后 runtime/config store 仍为 balanced。
- [x] 运行定向测试确认 RED。
- [x] 实现轻量 desired config commit 和后台调度，删除同步 candidate 生产入口。
- [x] 运行定向测试确认 GREEN。

### Task 2: Revision apply state and stale fencing

**Files:**

- Modify: `src-tauri/src/core/index_entry.rs`
- Modify: `src-tauri/src/lib.rs`
- Modify: `src/tauriClient.ts`

- [x] 写失败测试：applying/applied/failed 状态和 stale revision 不得发布。
- [x] 运行定向测试确认 RED。
- [x] 实现 desired/applied revision 与 apply state，并接入所有完成/失败路径。
- [x] 运行定向测试确认 GREEN。

### Task 3: Active roots and cancellable scan

**Files:**

- Modify: `src-tauri/src/core/index_scanner.rs`
- Modify: `src-tauri/src/core/index.rs`
- Modify: `src-tauri/src/lib.rs`

- [x] 写失败测试：fast 不包含 D 盘 configured root、Desktop 热路径仍存在、supersede 在 entry 边界停止。
- [x] 运行定向测试确认 RED。
- [x] 实现统一 active roots 与 cancellable scan，后台 worker 传入 revision identity closure。
- [x] 运行定向测试确认 GREEN。

### Task 4: Partial root failure

**Files:**

- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/src/core/index_entry.rs`

- [x] 写失败测试：一个 root 失败、另一个成功时保留可用 entries 并报告 partial/dirty。
- [x] 运行定向测试确认 RED。
- [x] 实现 partial baseline/status；全失败保留旧 view 并标记 apply failed。
- [x] 运行定向测试确认 GREEN。

### Task 5: Settings feedback

**Files:**

- Modify: `src/App.tsx`
- Modify: `src/App.test.tsx`
- Modify: `src/tauriClient.ts`
- Modify: `src/styles.css`

- [x] 写失败测试：dirty、saving、saved、error、applying、failed、重复提交和 D 盘提示。
- [x] 运行前端定向测试确认 RED。
- [x] 实现状态反馈、按钮行为与一致文案。
- [x] 运行定向测试确认 GREEN。

### Task 6: Cleanup, docs and verification

**Files:**

- Modify: `docs/architecture.md`
- Modify: `docs/troubleshooting.md`
- Modify: `docs/windows-manual-qa.md`
- Modify: OpenSpec tasks and artifacts

- [x] 删除旧同步 candidate/rollback 生产路径并隔离历史 fence 回归夹具。
- [x] 更新 desired/applied revision 与 Windows 验收文档。
- [x] 运行 `npm run check`、OpenSpec strict validate 和 `git diff --check`。
- [ ] 记录真实 Windows 发布构建验收；没有 Windows 硬件时保持对应 OpenSpec task 未完成。
