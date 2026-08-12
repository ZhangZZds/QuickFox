## Context

The persisted baseline is from 2026-06-21 and lacks the user-configured `cann` directory. The desktop process starts successfully, but no `quickfox-index-refresh` thread remains alive. The startup job is queued in Tauri setup and blocked by a gate that is released only from `RunEvent::Ready`; failures from its subsequent refresh call are discarded. Direct diagnostics confirm that the same configured macOS roots can be registered by the native watcher.

The captured startup diagnostic identifies a second blocker: pre-journal databases contain an older `index_runtime_state` table without `active_baseline_id` and `baseline_generation`. SQLite `CREATE TABLE IF NOT EXISTS` does not add those columns, so the new recovery query fails before the refresh can start.

After that migration was fixed, an end-to-end runtime trace identified the remaining direct failure: the installed database still had the legacy `index_delta_batches(created_at_ms, committed_at_ms)` shape and no `payload_hash`. Every native file event reached the runtime service, but its journal commit failed with `no such column: payload_hash`, requested dirty-root recovery, and made newly created empty files appear unavailable until another baseline refresh.

## Goals / Non-Goals

**Goals:**

- Start the queued refresh as soon as setup has completed, independently of whether a later `RunEvent::Ready` callback is delivered.
- Retain the existing Ready-event retry for an initial worker-spawn failure.
- Make a startup refresh failure visible through the existing runtime status and diagnostic output.
- Ensure a successful refresh installs a baseline that includes configured directory contents and starts the runtime watcher.
- Upgrade an incompatible legacy delta journal before accepting native file events.
- Keep excluded-path event storms out of the bounded watcher queue and recover native rescan signals with targeted calibration.

**Non-Goals:**

- Changing user include/exclude configuration or disabling project ignore rules.
- Replacing the notify backend or adding an external index daemon.

## Decisions

- Release the existing `StartupIndexingGate` at the end of `setup`, after all setup work and initial scheduling have completed. This is the lifecycle point the gate was intended to protect and is available even when the later event callback is not observed.
- Keep `RunEvent::Ready` only as an idempotent release plus retry hook. This preserves recovery for a scheduling failure without making it the sole start signal.
- Route an error returned by `start_background_index_refresh` to the existing startup failure status path and emit a concise diagnostic. The application continues serving any recovered baseline while exposing degradation.
- Rebuild only the small singleton runtime-state table when it lacks the current journal columns or foreign key. Preserve its generation/status values, associate a legacy row with the latest completed baseline, and initialize the missing baseline generation to zero.
- Detect legacy delta-batch columns during migration. Because the old rows do not contain the current payload hash and therefore cannot satisfy current journal integrity checks, discard only that incompatible incremental journal, recreate the current delta tables, reset the last generation to the active baseline generation, and clear the directory manifest so the next baseline safely recalibrates it. The active baseline itself remains intact and searchable.
- Complete the durable successor handoff and start the production watcher as soon as the filename/path baseline is persisted. Keep the refresh lifecycle open for the content phase; when that phase finishes, fence the live watcher again, merge its journal tail into the content baseline, and restart it. This keeps newly created files searchable during a long content build without losing updates at either handoff.
- Derive the watcher callback filter from the same `IndexPathRules` used by the targeted scanner. Reject excluded paths before bounded-channel enqueue so application/database traffic cannot consume queue capacity.
- Treat canonical filesystem paths as aliases only when deciding which configured root owns a native event. Evaluate exclusion globs relative to that root boundary and retain the user's original root identity in index entries and manifests; this covers macOS `/var` → `/private/var` event paths without creating duplicate roots or generations.
- Interpret a native watcher rescan signal without a concrete backend failure as a dirty-root calibration request. Scan only affected configured roots, commit any differences as a delta, and clear the degraded root after a successful calibration.

## Risks / Trade-offs

- [Refresh begins shortly before the event loop's Ready callback] → The worker remains outside setup and all UI mutations continue through `run_on_main_thread`.
- [Repeated release calls] → `release_after_setup` is idempotent and retry claiming remains atomic.
- [Large configured roots take time to scan] → Existing staged checkpoints and status updates remain unchanged.
- [Legacy databases hold a stale active-baseline value] → Migration selects the latest completed baseline only when the old table has no active-baseline column.
- [Content build overlaps filesystem changes] → The production watcher journals changes while content builds, and the content installation consumes the second fenced tail before replacing its baseline.
- [Legacy delta rows cannot be authenticated under the new schema] → Drop only the incompatible delta journal and force manifest recalibration while preserving the active baseline.
- [A native backend emits a broad rescan event] → Calibrate the configured roots through the targeted scanner instead of immediately scheduling another full refresh.

## Migration Plan

1. Ship the lifecycle fix plus runtime-state and delta-journal migrations; no user configuration changes are required.
2. On next startup, rebuild and replace stale baselines for configured roots.
3. Roll back by reverting the lifecycle change; existing persisted baselines remain readable.
