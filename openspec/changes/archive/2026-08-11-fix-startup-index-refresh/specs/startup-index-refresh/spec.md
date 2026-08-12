## ADDED Requirements

### Requirement: Setup releases initial indexing

QuickFox SHALL release the queued startup indexing worker once Tauri setup has completed, without requiring a later event-loop callback to occur first.

#### Scenario: Ready callback is delayed or absent

- **WHEN** QuickFox setup successfully queues the startup indexing worker
- **THEN** the worker begins the configured-root refresh after setup completes
- **AND** it does not remain blocked waiting for `RunEvent::Ready`

### Requirement: Startup refresh failures are observable

QuickFox SHALL expose a startup refresh failure through its existing index status and diagnostic output while preserving a previously recovered searchable baseline when one exists.

#### Scenario: Refresh cannot start

- **WHEN** startup refresh returns an error after setup
- **THEN** the index status reports degradation or failure
- **AND** the error is recorded in application diagnostics

### Requirement: Legacy runtime state is upgraded before recovery

QuickFox SHALL upgrade a legacy `index_runtime_state` table to the current journal schema before reading the active baseline. When legacy state lacks an active baseline identifier, the upgrade SHALL retain the latest completed baseline and initialize its baseline generation to zero.

#### Scenario: Existing pre-journal database starts

- **WHEN** the local database has an `index_runtime_state` row without `active_baseline_id` or `baseline_generation`
- **THEN** startup migrates the runtime-state table before recovery
- **AND** the persisted baseline remains readable
- **AND** background refresh can begin

### Requirement: Legacy delta journal is upgraded before runtime commits

QuickFox SHALL detect an installed delta-journal schema that cannot represent the current integrity fields before the runtime watcher starts committing changes. It SHALL preserve the active baseline while replacing only the incompatible incremental journal and invalidating manifest state that requires recalibration.

#### Scenario: Existing delta batches lack payload hashes

- **WHEN** the local database has an `index_delta_batches` table without `payload_hash`
- **THEN** startup recreates the delta batch and entry tables using the current schema
- **AND** the active baseline remains available for search
- **AND** a newly created empty file can be committed by the runtime watcher
