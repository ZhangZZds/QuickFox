## 1. Startup lifecycle

- [x] 1.1 Add a regression test for releasing the queued startup worker at setup completion without a Ready event.
- [x] 1.2 Release the startup indexing gate after setup while retaining idempotent Ready-event retry behavior.

## 2. Legacy storage migration

- [x] 2.1 Add a failing regression test for a pre-journal runtime-state table.
- [x] 2.2 Migrate that singleton state table to current active-baseline columns and preserve legacy state.
- [x] 2.3 Add a failing regression test for a legacy delta-batch table without `payload_hash`.
- [x] 2.4 Recreate only the incompatible incremental journal, preserve the active baseline, and allow the next runtime delta commit.

## 3. Failure handling

- [x] 3.1 Record and publish startup refresh failures instead of discarding them.
- [x] 3.2 Run focused Rust tests and the full repository check; validate the OpenSpec change.

## 4. Manual acceptance

- [x] 4.1 Add a failing regression test that a persisted filename/path baseline starts runtime watching while content indexing remains pending.
- [x] 4.2 Finalize the durable handoff and start the watcher before the content phase; retain tail reconciliation for content installation.
- [x] 4.3 Filter excluded paths before bounded-channel enqueue, preserve configured-root identity across canonical native-event aliases, and add regression coverage.
- [x] 4.4 Convert watcher rescan notifications into targeted calibration and add regression coverage.
- [x] 4.5 Exercise the native watcher with an empty file created after startup.
- [x] 4.6 Verify against the real user database that empty-file create and removal commit as incremental deltas without changing the baseline generation.
- [x] 4.7 Launch exactly one final QuickFox instance and ask the maintainer to confirm a newly created file is searchable.
