# Runtime Indexing Calibration Barrier Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make startup calibration asynchronous and close every watcher, revision, activation, monitor, and persistence failure gap without sacrificing the recovered live index.

**Architecture:** A core calibration/recovery state machine owns phases, generation fences, and retry latches. Tauri wiring creates watcher-first sessions, performs authoritative calibration, and commits config or baseline only after a drain barrier. Root recovery uses an owned cancel-and-join monitor handle.

**Tech Stack:** Rust, Tauri, notify watcher abstraction, rusqlite, existing layered index/journal modules, TypeScript contract tests.

---

### Task 1: Core session and failure state

**Files:**

- Modify: `src-tauri/src/core/index_refresh_orchestrator.rs`
- Modify: `src-tauri/src/core/mod.rs`
- Test: `src-tauri/src/core/index_refresh_orchestrator.rs`

- [ ] **Step 1: Write failing tests for phase order and retry latches**

Add tests proving `Watching` is rejected before capture, calibration, and fence complete; a failure
transitions to `Degraded`; and one revision can claim only one recovery until completion clears it.

- [ ] **Step 2: Run the focused tests and verify RED**

Run: `cd src-tauri && cargo test core::index_refresh_orchestrator::tests --lib`
Expected: compile/test failures because the session and failure application APIs do not exist.

- [ ] **Step 3: Implement the minimal pure state machine**

Add `CalibrationPhase`, `RuntimeCalibrationSession`, `RuntimeFailureApplication`, and
`RevisionRecoveryLatch`. Keep filesystem, SQLite, threads, and Tauri outside this module.

- [ ] **Step 4: Run focused tests and verify GREEN**

Run: `cd src-tauri && cargo test core::index_refresh_orchestrator::tests --lib`
Expected: all orchestrator tests pass.

### Task 2: Non-blocking watcher-first startup

**Files:**

- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/src/core/index_refresh_orchestrator.rs`
- Test: `src-tauri/src/lib.rs`

- [ ] **Step 1: Write failing startup production-entry tests**

Add tests proving runtime construction only replays baseline+journal, returns with `Preparing`, does
not enumerate roots, and a setup-posted worker creates capture before calibrating all roots returned by
`build_scan_plans`, including applications, hot paths, remaining drive, and explicit roots.

- [ ] **Step 2: Run focused startup tests and verify RED**

Run: `cd src-tauri && cargo test startup_ --lib -- --nocapture`
Expected: existing synchronous calibration is observed and/or required APIs are missing.

- [ ] **Step 3: Implement minimal asynchronous startup wiring**

Make `build_runtime_with_startup_calibration` recovery-only. Post the calibration worker after setup
returns; deduplicate full scan-plan roots and current indexed roots. Keep state `Preparing` until the
capture-calibrate-fence sequence succeeds, otherwise apply the common degraded failure.

- [ ] **Step 4: Run focused startup tests and verify GREEN**

Run: `cd src-tauri && cargo test startup_ --lib -- --nocapture`
Expected: all startup tests pass without fixed sleeps.

### Task 3: Two-phase config revision transition

**Files:**

- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/src/core/index_refresh_orchestrator.rs`
- Test: `src-tauri/src/lib.rs`

- [ ] **Step 1: Write failing interleaving and storage-failure tests**

Use controlled capture/storage probes to prove old service remains usable while candidate calibration
runs; an event between watcher registration and calibration is included; storage fence read failure
returns an error, keeps disk/in-memory config and old service unchanged, marks Degraded, and claims at
most one recovery.

- [ ] **Step 2: Run focused revision tests and verify RED**

Run: `cd src-tauri && cargo test revision_ --lib -- --nocapture`
Expected: fixed 500ms heuristic or early config persistence violates assertions.

- [ ] **Step 3: Implement candidate-session commit**

Move config store save after candidate capture+calibration+fence success. Remove
`wait_for_revision_capture_registration`. Commit memory config and service under the refresh fence;
on any error dispose candidate and apply the unified failure to the untouched old runtime.

- [ ] **Step 4: Run focused revision tests and verify GREEN**

Run: `cd src-tauri && cargo test revision_ --lib -- --nocapture`
Expected: all revision tests pass without sleeping.

### Task 4: Authoritative manifest activation

**Files:**

- Modify: `src-tauri/src/core/targeted_index_scanner.rs`
- Modify: `src-tauri/src/core/storage.rs`
- Modify: `src-tauri/src/lib.rs`
- Test: `src-tauri/src/core/storage.rs`
- Test: `src-tauri/src/lib.rs`

- [ ] **Step 1: Write failing activation/crash/restart tests**

Build a scanned baseline, commit directory creation or deletion in the durable tail, activate through
the production path, reopen storage as a simulated restart, and assert watcher-first calibration
recovers the correct manifest and entries while status is never prematurely `Ready`.

- [ ] **Step 2: Run focused activation tests and verify RED**

Run: `cd src-tauri && cargo test activation_ --lib -- --nocapture`
Expected: full-table replacement loses the durable manifest mutation.

- [ ] **Step 3: Recompute entries and manifest at the quiescent fence**

Drain durable successor before activation; apply its deltas and manifest mutations to both snapshots;
validate the final manifest tree, activate at the authoritative generation, then promote the already
registered standby watcher to durable service.

- [ ] **Step 4: Run focused activation tests and verify GREEN**

Run: `cd src-tauri && cargo test activation_ --lib -- --nocapture`
Expected: create and delete crash/restart cases pass.

### Task 5: Managed missing-root monitor and common failure funnel

**Files:**

- Create: `src-tauri/src/core/root_availability_monitor.rs`
- Modify: `src-tauri/src/core/mod.rs`
- Modify: `src-tauri/src/lib.rs`
- Test: `src-tauri/src/core/root_availability_monitor.rs`
- Test: `src-tauri/src/lib.rs`

- [ ] **Step 1: Write failing lifecycle and production failure tests**

Test cancellation+join, spawn failure, probe failure, main-thread dispatch failure, root recovery, and
baseline persistence failure with a no-op successor. Assert latches clear, live view remains usable,
state is `Degraded`, and one automatic recovery is scheduled.

- [ ] **Step 2: Run focused monitor/failure tests and verify RED**

Run: `cd src-tauri && cargo test root_monitor --lib -- --nocapture && cargo test persistence_failure --lib -- --nocapture`
Expected: naked thread/latch and no-op successor behavior fail assertions.

- [ ] **Step 3: Implement owned lifecycle and unified failure application**

Return a `RootAvailabilityMonitorHandle` containing cancel state and join handle. Store it in refresh
control, stop+join on revision/recovery/exit, and route every failure boundary through the core failure
application. Remove both test-only watcher-failure helpers.

- [ ] **Step 4: Run focused tests and verify GREEN**

Run: `cd src-tauri && cargo test root_monitor --lib -- --nocapture && cargo test persistence_failure --lib -- --nocapture`
Expected: all focused tests pass.

### Task 6: Refactor and full verification

**Files:**

- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/src/core/index_refresh_orchestrator.rs`
- Modify: `docs/superpowers/specs/2026-07-29-runtime-indexing-calibration-barrier-design.md`

- [ ] **Step 1: Remove duplicate test-only/runtime helpers**

Delete `record_watcher_failure*`, fixed registration waits, and duplicated retry/lifecycle decisions;
keep only Tauri adapter calls in `lib.rs`.

- [ ] **Step 2: Run Rust focused and full verification**

Run: `cd src-tauri && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test --all-targets`
Expected: exit 0, no warnings, all non-ignored tests pass.

- [ ] **Step 3: Run frontend and repository verification**

Run: `npm test -- --run && npm run check`
Expected: exit 0 and all tests/checks pass.

- [ ] **Step 4: Verify scope and commit**

Run: `git diff --check && git status --short && git diff -- openspec/changes/complete-runtime-incremental-indexing/tasks.md`
Expected: no whitespace errors and no OpenSpec task changes. Commit the complete batch as a new
separate commit.
