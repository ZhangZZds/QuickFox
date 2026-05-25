## 1. Storage and Index Model

- [x] 1.1 Add Rust tests for loading an existing persisted index snapshot and for empty/no-snapshot startup behavior.
- [x] 1.2 Extend SQLite migrations with index entry and index batch/state storage, keeping existing history/config data intact.
- [x] 1.3 Add APIs to save a completed index batch transactionally and load the latest completed batch.
- [x] 1.4 Refactor `SearchIndex` to build searchable entries with cached normalized search text.
- [x] 1.5 Add Rust tests proving normal search uses cached matching data and respects configured candidate/result limits.

## 2. Background Index Lifecycle

- [x] 2.1 Add Rust tests for index status transitions: unbuilt, building, ready, refreshing with stale snapshot, failed.
- [x] 2.2 Introduce `IndexStatus` and runtime state commands for querying current index status/report.
- [x] 2.3 Change startup to load persisted index quickly and spawn background indexing without blocking Tauri setup.
- [x] 2.4 Change manual refresh and index-affecting config saves to trigger background refresh instead of synchronous scanning.
- [x] 2.5 Ensure stale background indexing work cannot overwrite a newer refresh request.

## 3. Providers and Query Behavior

- [x] 3.1 Add Rust tests showing calculator/web/command providers work while file index is unavailable or building.
- [x] 3.2 Make file Provider degrade gracefully when no file index is available, returning no file results plus status feedback for the UI path.
- [x] 3.3 Add DuckDuckGo web search config tests for `ddg` prefix URL generation and query encoding.
- [x] 3.4 Add or update default config to include DuckDuckGo if accepted, while preserving user-edited existing configs.

## 4. Frontend Settings and Status UX

- [x] 4.1 Add frontend client contract tests for index status and background refresh commands.
- [x] 4.2 Add component tests for index unavailable/building/refreshing feedback in launcher and settings views.
- [x] 4.3 Rework settings UI into a divided control-panel layout with sections for index, web search, history, command safety, and appearance/window.
- [x] 4.4 Implement web search engine add/edit/delete controls with a lightweight wizard modal and `{query}` validation.
- [x] 4.5 Implement index include/exclude rule editing and show background refresh status after saving.
- [x] 4.6 Polish responsive settings styles without introducing nested cards or layout-shifting controls.

## 5. Cross-Platform Runtime and Packaging

- [x] 5.1 Add tests or compile-time assertions around platform-specific background/window behavior where practical.
- [x] 5.2 Configure Windows release builds to avoid an extra cmd/console window while preserving debug diagnostics.
- [x] 5.3 Verify macOS and Linux code paths still keep tray, Shift+Shift, window hiding/showing, and terminal fallback behavior intact.

## 6. Documentation and Verification

- [x] 6.1 Update Windows, macOS, and Linux manual QA docs with background indexing, stale-index messaging, and no-console-window checks.
- [x] 6.2 Update architecture/development docs for persistent background indexing and configurable web search engines.
- [x] 6.3 Run frontend formatting/lint/tests/build and Rust fmt/clippy/tests.
- [x] 6.4 Run `openspec validate improve-indexing-settings-and-background` and fix any spec issues.
- [x] 6.5 Perform final verification-before-completion review and summarize remaining manual QA that requires real Windows/macOS/Linux machines.
