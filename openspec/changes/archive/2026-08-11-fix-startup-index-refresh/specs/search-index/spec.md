## MODIFIED Requirements

### Requirement: 后台构建索引

The system SHALL preserve the currently searchable index while a background refresh runs, publish progress after each completed scan stage, and replace the searchable baseline only after the final staged result is successfully persisted. After Tauri setup completes, every available configured index root SHALL be scheduled for that background refresh even if the event-loop Ready callback is delayed or absent.

#### Scenario: Startup refresh runs without a Ready callback

- **WHEN** a persisted baseline exists and QuickFox finishes setup with indexing enabled
- **THEN** the persisted baseline remains searchable while refresh runs
- **AND** all available configured roots are included in the scheduled refresh

#### Scenario: Refresh fails after a quick index is available

- **WHEN** a background refresh fails after a persisted or staged index becomes available
- **THEN** file search continues against the last available index
- **AND** index status exposes the refresh failure
- **AND** a retry can rebuild the index later

#### Scenario: A new file arrives while content indexing is pending

- **WHEN** the durable filename/path baseline has been published and the optional content index is still building
- **THEN** the runtime watcher SHALL already observe configured roots
- **AND** a file created after that baseline becomes searchable by its filename or path without waiting for content indexing to finish
- **AND** content installation SHALL reconcile watcher changes before it replaces its baseline

#### Scenario: Excluded filesystem traffic is observed

- **WHEN** the native watcher reports changes under an excluded directory
- **THEN** those events SHALL be rejected before entering the bounded runtime queue
- **AND** they SHALL NOT cause an overflow recovery for otherwise valid configured-root changes

#### Scenario: The native event uses a canonical alias of a configured root

- **WHEN** the operating system reports an event through a canonical path alias such as macOS `/private/var` for a configured `/var` root
- **THEN** QuickFox SHALL associate the event with the original configured root
- **AND** exclusion patterns SHALL be evaluated only within that configured-root boundary
- **AND** index and manifest entries SHALL retain one stable configured-root identity

#### Scenario: The native watcher requests a rescan

- **WHEN** the native backend requests a rescan without reporting a concrete backend failure
- **THEN** QuickFox SHALL schedule targeted calibration for the affected configured roots
- **AND** any discovered differences SHALL be committed as an incremental delta
- **AND** a successful calibration SHALL clear the affected degraded-root state without forcing a baseline refresh
