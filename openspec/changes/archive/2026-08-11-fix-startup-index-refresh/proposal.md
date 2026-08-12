## Why

QuickFox currently reports a persisted index as usable even when the startup refresh worker never starts. This leaves configured directories such as `/Users/frankzhang/workspace/cann` absent from search, and files created after launch are not observed by the unavailable watcher.

## What Changes

- Release the startup indexing worker after Tauri setup completes instead of depending solely on a later event-loop callback.
- Preserve the existing retry path for a failed initial worker spawn.
- Surface startup refresh failures rather than silently discarding them.
- Migrate legacy runtime-state rows before journal recovery reads the active baseline fields.
- Replace the incompatible legacy delta-journal schema before the runtime watcher commits its first change.
- Activate the runtime file watcher immediately after the durable filename/path baseline is published, even while the optional content index is still building.
- Filter excluded paths before watcher events enter the bounded queue, and treat native rescan notifications as targeted calibration instead of an unconditional full refresh.
- Add regression coverage for setup-time release, legacy schema upgrades, content-deferred watcher activation, queue filtering, rescan calibration, and a real empty-file create event.

## Capabilities

### New Capabilities

- `startup-index-refresh`: Reliable initial refresh and observable failure handling for configured index roots.

### Modified Capabilities

- `search-index`: Startup index lifecycle now guarantees that configured, available roots are scheduled for refresh after application setup.

## Impact

- Affected Rust application lifecycle code in `src-tauri/src/lib.rs`, SQLite migration code in `src-tauri/src/core/storage.rs`, and runtime watcher/indexing code.
- Affected runtime index availability, file-change ingestion, and refresh status visible to the frontend.
