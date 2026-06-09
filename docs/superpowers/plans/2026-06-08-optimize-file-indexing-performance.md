# Optimize File Indexing Performance Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a faster, observable, incremental file index with structured file queries and opt-in `content:` text search.

**Architecture:** Split indexing out of the current monolithic `src-tauri/src/core/index.rs` into scanner, query, metadata, content, watcher, and status modules. SQLite remains the lightweight metadata snapshot store; Tantivy becomes a separate local content index; providers keep returning unified `SearchResult` values.

**Tech Stack:** Tauri 2, Rust, rusqlite, ignore, notify/debouncer, Tantivy, nucleo matcher, React, TypeScript, Vitest.

---

## File Structure

- Modify: `src-tauri/Cargo.toml` for Rust dependencies.
- Modify: `src-tauri/src/core/mod.rs` to expose new modules.
- Modify: `src-tauri/src/core/index.rs` to become the public facade and compatibility layer.
- Create: `src-tauri/src/core/index_entry.rs` for `IndexedEntry`, metadata, content state, stage/status types.
- Create: `src-tauri/src/core/index_scanner.rs` for `FileSystemScanner`, `IgnoreScanner`, stage plans, forced excludes.
- Create: `src-tauri/src/core/file_query.rs` for `FileQuery`, field parser, quoted values.
- Create: `src-tauri/src/core/file_matcher.rs` for ordinary name/path matching and nucleo integration boundary.
- Create: `src-tauri/src/core/content_index.rs` for Tantivy schema, text extraction, snippets, highlights.
- Create: `src-tauri/src/core/index_watcher.rs` for runtime notify/debounce updates.
- Modify: `src-tauri/src/core/storage.rs` for snapshot schema migration and metadata persistence.
- Modify: `src-tauri/src/core/config.rs` for index performance/content config.
- Modify: `src-tauri/src/core/search.rs` for content snippets and highlight payload on `SearchResult`.
- Modify: `src-tauri/src/core/providers.rs` to call structured file search.
- Modify: `src-tauri/src/lib.rs` for refresh pipeline, status events, watcher lifecycle.
- Modify: `src/tauriClient.ts` for config/status/result payload types.
- Modify: `src/App.tsx` for settings fields and content snippet rendering.
- Modify: `src/styles.css` for snippet/highlight/settings layout.
- Modify: `src/App.test.tsx` and `src/tauriClient.test.ts` for frontend behavior.
- Modify: docs/manual QA files for macOS and Windows validation.

## Task 1: Dependencies And Module Skeleton

**Files:**

- Modify: `src-tauri/Cargo.toml`
- Modify: `src-tauri/src/core/mod.rs`
- Modify: `src-tauri/src/core/index.rs`
- Create: `src-tauri/src/core/index_entry.rs`
- Create: `src-tauri/src/core/index_scanner.rs`
- Create: `src-tauri/src/core/file_query.rs`
- Create: `src-tauri/src/core/file_matcher.rs`
- Create: `src-tauri/src/core/content_index.rs`
- Create: `src-tauri/src/core/index_watcher.rs`

- [x] **Step 1: Add dependencies**

Run:

```bash
cargo add --manifest-path src-tauri/Cargo.toml ignore notify notify-debouncer-mini tantivy nucleo-matcher globset content_inspector tempfile
```

Expected: dependencies are added to `src-tauri/Cargo.toml` and `Cargo.lock`.

- [x] **Step 2: Create module skeletons**

Add modules in `src-tauri/src/core/mod.rs`:

```rust
pub mod actions;
pub mod config;
pub mod content_index;
pub mod file_matcher;
pub mod file_query;
pub mod index;
pub mod index_entry;
pub mod index_scanner;
pub mod index_watcher;
pub mod platform;
pub mod providers;
pub mod search;
pub mod storage;
```

- [x] **Step 3: Move entry/status models behind re-exports**

Move `IndexedEntryKind`, `IndexedEntry`, `IndexFailure`, `IndexReport`, `IndexStatusKind`, `IndexStatus`, `IndexLifecycle`, and `IndexScanOptions` into `index_entry.rs`; re-export them from `index.rs` with:

```rust
pub use crate::core::index_entry::{
    IndexedEntry, IndexedEntryKind, IndexFailure, IndexLifecycle, IndexReport, IndexScanOptions,
    IndexStatus, IndexStatusKind,
};
```

- [x] **Step 4: Verify skeleton compiles**

Run:

```bash
npm run rust:test
```

Expected: compile errors only point to missing moved imports if any; fix imports until existing tests pass.

## Task 2: Metadata Model And Storage Migration

**Files:**

- Modify: `src-tauri/src/core/index_entry.rs`
- Modify: `src-tauri/src/core/storage.rs`
- Modify: `src-tauri/src/core/index.rs`

- [x] **Step 1: Write failing metadata tests**

Add tests covering:

- `IndexedEntry::from_path_metadata` fills `parent`, `extension`, `depth`, `root`, `modified_ms`, `size_bytes`.
- old snapshots with only `path/name/kind` still load.
- new snapshots persist and restore metadata.

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml storage::tests::latest_snapshot_restores_index_metadata index_entry::tests::entry_metadata_from_path -- --nocapture
```

Expected: tests fail because metadata fields and migration do not exist.

- [x] **Step 2: Extend entry structs**

Add fields with serde defaults:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexedEntry {
    pub path: String,
    pub name: String,
    pub kind: IndexedEntryKind,
    #[serde(default)]
    pub parent: String,
    #[serde(default)]
    pub extension: Option<String>,
    #[serde(default)]
    pub depth: usize,
    #[serde(default)]
    pub root: String,
    #[serde(default)]
    pub modified_ms: Option<i64>,
    #[serde(default)]
    pub size_bytes: Option<u64>,
    #[serde(default)]
    pub content_index_state: ContentIndexState,
}
```

- [x] **Step 3: Migrate SQLite schema**

Add nullable columns to `index_entries`: `parent`, `extension`, `depth`, `root`, `modified_ms`, `size_bytes`, `content_index_state`. Use `ALTER TABLE` guarded by `PRAGMA table_info(index_entries)` so repeated migrations are safe.

- [x] **Step 4: Update snapshot save/load**

Save new fields for every entry. On load, default absent columns to empty/None values and mark loaded entries usable for search.

- [x] **Step 5: Verify**

Run:

```bash
npm run rust:test
```

Expected: Rust tests pass.

## Task 3: Ignore Scanner And Phased Indexing

**Files:**

- Modify: `src-tauri/src/core/index_scanner.rs`
- Modify: `src-tauri/src/core/index.rs`
- Modify: `src-tauri/src/lib.rs`

- [x] **OpenSpec 2.1:** Write `FileSystemScanner` trait, scan event, scan stats, and error degradation Rust unit tests
- [x] **OpenSpec 2.2:** Define `FileSystemScanner` trait and scan event/stat structures
- [x] **OpenSpec 2.3:** Preserve current scanner behavior as fallback/test comparison
- [x] **OpenSpec 2.4:** Implement `ignore::WalkBuilder` scanner with QuickFox forced excludes, user excludes, project ignores, and threaded traversal configuration
- [x] **OpenSpec 2.5:** Cover `.app`, `.exe`, `.lnk`, and `.desktop` application entry behavior
- [x] **OpenSpec 2.6:** Cover unreadable child/root degradation, duplicate roots, and unavailable roots

- [x] **Step 1: Write failing scanner tests**

Cover:

- respects forced excludes such as `.git`, `node_modules`, `target`, Windows system directories.
- default project ignore support skips `.gitignore` entries.
- config can disable project ignore.
- `.app` directory is indexed as one application and not traversed inside.
- unreadable child path records failure but does not abort the whole root.

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml index_scanner::tests -- --nocapture
```

Expected: tests fail because `IgnoreScanner` does not exist.

- [x] **Step 2: Implement scanner trait**

Define:

```rust
pub trait FileSystemScanner {
    fn scan(&self, plan: IndexScanPlan) -> Result<IndexReport, std::io::Error>;
}
```

`IndexScanPlan` contains include roots, exclude dirs, exclude patterns, `respect_project_ignores`, stage name, and root priority.

- [x] **Step 3: Implement `IgnoreScanner`**

Use `ignore::WalkBuilder` for each root. Enable hidden/project ignore based on config, apply QuickFox forced excludes before accepting entries, and build metadata-rich entries.

- [x] **Step 4: Implement staged refresh**

Add stages: `applications`, `user-hot-paths`, `configured-roots`, `remaining-drives`. Each stage emits `IndexStatus` with stage, current root, scanned, accepted, skipped, failures.

- [x] **Step 5: Verify**

Run:

```bash
npm run rust:test
openspec validate optimize-file-indexing-performance --strict
```

Expected: tests and OpenSpec validation pass.

## Task 4: Structured File Query Parser

**Files:**

- Modify: `src-tauri/src/core/file_query.rs`
- Modify: `src-tauri/src/core/index.rs`
- Modify: `src-tauri/src/core/providers.rs`

- [x] **Step 1: Write failing parser tests**

Test exact parse outputs for:

```text
report budget
type:pdf
name:test
dir:workspace
dir:**/workspace
name:"project report" dir:"D:\My Projects"
workspace type:md content:"hello world"
```

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml file_query::tests -- --nocapture
```

Expected: tests fail because `FileQuery` does not exist.

- [x] **Step 2: Implement parser**

Create:

```rust
pub struct FileQuery {
    pub ordinary_terms: Vec<String>,
    pub type_filters: Vec<String>,
    pub name_filters: Vec<String>,
    pub dir_filters: Vec<DirFilter>,
    pub content_query: Option<String>,
}
```

Fields are AND filters. Quoted values preserve spaces and Windows backslashes.

- [x] **Step 3: Connect to file provider**

`FileProvider` continues receiving `QueryRequest`, but `SearchIndex::search_with_limit` parses `query.text` into `FileQuery` for normal mode.

- [x] **Step 4: Verify**

Run:

```bash
npm run rust:test
```

Expected: parser tests pass and existing provider tests keep passing.

## Task 5: Ordinary Matching, Field Filters, And Ranking

**Files:**

- Modify: `src-tauri/src/core/file_matcher.rs`
- Modify: `src-tauri/src/core/index.rs`
- Modify: `src-tauri/src/core/search.rs`

- [x] **Step 1: Write failing matcher tests**

Cover:

- `type:pdf` returns `.pdf` and `.PDF`.
- `name:test` matches filename only, not parent-only path.
- `dir:workspace` contains parent path.
- `dir:**/workspace` uses glob semantics.
- ordinary terms use precomputed search text.
- candidate limit stops result construction.

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml file_matcher::tests index::tests::field_query_filters -- --nocapture
```

Expected: tests fail until matcher exists.

- [x] **Step 2: Implement field filters**

Use extension metadata for `type`, `entry.name.to_lowercase().contains()` for `name`, and `globset` only when the dir value contains glob metacharacters.

- [x] **Step 3: Integrate nucleo boundary**

Create a `NamePathMatcher` trait. First implementation can wrap existing fuzzy behavior; add `NucleoNamePathMatcher` behind the same trait once dependency compiles cleanly. The final result order still goes through `Ranker`.

- [ ] **Step 4: Update ranking for mixed content-ready signals**

Add optional result metadata fields needed later, but keep existing behavior for non-content queries.

- [x] **Step 5: Verify**

Run:

```bash
npm run rust:test
```

Expected: field-filter and existing search tests pass.

## Task 6: Tantivy Content Index And Snippets

> Completed as OpenSpec task 5.x. `SearchIndex::from_entries` remains name/path only; tests and future config paths use explicit `SearchIndex::from_entries_with_content_index`.

**Files:**

- Modify: `src-tauri/src/core/content_index.rs`
- Modify: `src-tauri/src/core/index_entry.rs`
- Modify: `src-tauri/src/core/index.rs`
- Modify: `src-tauri/src/core/search.rs`

- [x] **Step 1: Write failing content tests**

Cover:

- text file under size limit is indexed.
- file over configured limit is skipped with `SizeSkipped`.
- binary/PDF placeholder is skipped unless extractor supports it.
- `content:invoice` returns matching file.
- snippet includes 5 lines before and 5 lines after the hit.
- structured highlight ranges point to the hit inside snippet lines.

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml content_index::tests -- --nocapture
```

Expected: tests fail because content index does not exist.

- [x] **Step 2: Implement text extractor boundary**

Create:

```rust
pub trait TextExtractor {
    fn extract(&self, path: &std::path::Path, max_bytes: u64) -> ContentExtractionResult;
}
```

First implementation reads UTF-8 or text-like files detected by extension/content inspection. PDF and Office return `UnsupportedType`.

- [x] **Step 3: Implement Tantivy schema**

Fields: stable path, lowercase extension, root, content body, stored line offsets or stored text for snippet reconstruction. Store index under app data, separate from SQLite.

- [x] **Step 4: Implement content search**

Use Tantivy default query parser for `content_query`. If candidate paths are provided, filter results to candidates before returning.

- [x] **Step 5: Add snippets to `SearchResult`**

Extend `SearchResult` with optional:

```rust
pub snippet: Option<SearchSnippet>
```

`SearchSnippet` contains `lines`, `start_line`, and `highlights`.

- [x] **Step 6: Verify**

Run:

```bash
npm run rust:test
```

Expected: content tests pass.

## Task 7: Content Scope And Config Defaults

**Files:**

- Modify: `src-tauri/src/core/config.rs`
- Modify: `src-tauri/src/lib.rs`
- Modify: `src/tauriClient.ts`
- Modify: `src/App.tsx`
- Modify: `src/App.test.tsx`

- [x] **Step 1: Write failing config tests**

Cover:

- default `respect_project_ignores` is true.
- default content size limit is 2MB.
- Windows default content roots only include Desktop.
- macOS default content roots include Desktop, Documents, Downloads, workspace if present.

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml config::tests::index_content_defaults -- --nocapture
```

Expected: tests fail until config exists.

- [x] **Step 2: Extend config schema**

Add:

```rust
pub struct IndexConfig {
    pub include_dirs: Vec<String>,
    pub exclude_dirs: Vec<String>,
    pub exclude_patterns: Vec<String>,
    #[serde(default = "default_true")]
    pub respect_project_ignores: bool,
    #[serde(default)]
    pub content_include_dirs: Vec<String>,
    #[serde(default = "default_content_max_bytes")]
    pub content_max_bytes: u64,
    #[serde(default = "default_true")]
    pub runtime_watcher_enabled: bool,
}
```

- [x] **Step 3: Update frontend types and fallback config**

Mirror the schema in `src/tauriClient.ts` and `fallbackConfig` in `src/App.tsx`.

- [x] **Step 4: Add settings fields**

Add content index directories, max file size, respect ignore checkbox, and watcher checkbox. Help text must explain one path per line and content privacy implications.

- [x] **Step 5: Verify**

Run:

```bash
npm run test -- App.test.tsx
npm run rust:test
```

Expected: config and settings tests pass.

## Task 8: Runtime Watcher And Incremental Updates

**Files:**

- Modify: `src-tauri/src/core/index_watcher.rs`
- Modify: `src-tauri/src/core/index.rs`
- Modify: `src-tauri/src/lib.rs`
- Modify: `src-tauri/src/core/storage.rs`

- [x] **Step 1: Write failing watcher tests**

Use temp directories to cover create, modify, delete, rename batching and watcher failure fallback where possible. Keep OS-specific file event tests minimal and isolate pure debounce logic for deterministic tests.

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml index_watcher::tests -- --nocapture
```

Expected: tests fail until watcher queue exists.

- [x] **Step 2: Implement debounce queue**

Create pure `IndexEventBatcher` that accepts path events and produces affected roots/subtrees after debounce.

- [x] **Step 3: Connect notify watcher**

Create runtime watcher owned by `QuickFoxRuntime` or app state. Start watcher after successful staged refresh and when loading a snapshot with configured roots.

- [x] **Step 4: Apply incremental batch**

For changed paths, update metadata index, SQLite rows, and Tantivy content docs. Deletes remove all associated records.

- [x] **Step 5: Fallback on watcher failure**

Emit `IndexStatus` with watcher warning and schedule background refresh.

- [x] **Step 6: Verify**

Run:

```bash
npm run rust:test
```

Expected: watcher tests pass.

## Task 9: Frontend Result Snippets And Status

**Files:**

- Modify: `src/tauriClient.ts`
- Modify: `src/App.tsx`
- Modify: `src/styles.css`
- Modify: `src/App.test.tsx`

- [x] **Step 1: Write failing frontend tests**

Cover:

- content results render snippet lines.
- highlighted ranges are wrapped in a highlight element.
- index status shows phase/current root/counts when present.
- settings help includes examples for multiple directories and `content:`.

Run:

```bash
npm run test -- App.test.tsx
```

Expected: tests fail until UI reads new payload.

- [x] **Step 2: Extend frontend result types**

Add `snippet?: { startLine: number; lines: string[]; highlights: Array<{ line: number; start: number; end: number }> }`.

- [x] **Step 3: Render snippet**

In result list, render snippet below detail for file results with snippets. Highlight ranges with spans and preserve line wrapping.

- [x] **Step 4: Update responsive styles**

Ensure snippets wrap, do not resize icon buttons, and settings fields use fluid grid widths.

- [x] **Step 5: Verify**

Run:

```bash
npm run test -- App.test.tsx
npm run build
```

Expected: frontend tests and build pass.

## Task 10: Docs, Benchmarks, And Final Verification

**Files:**

- Modify: `docs/macos-manual-qa.md`
- Modify: `docs/windows-manual-qa.md`
- Modify: `docs/linux-manual-qa.md`
- Modify: `openspec/changes/optimize-file-indexing-performance/tasks.md`

- [x] **Step 1: Update docs**

Document field query examples:

```text
type:pdf
name:test
dir:**/workspace
content:"hello world"
workspace type:md content:invoice
```

Document that Windows content search defaults to Desktop only, while other disks default to name/path unless configured.

- [x] **Step 2: Run full verification**

Run:

```bash
npm run check
openspec validate optimize-file-indexing-performance --strict
```

Expected: both commands pass.

- [x] **Step 3: Record benchmark result**

Run the benchmark command introduced in Task 1 and paste before/after numbers into the change notes or manual QA doc.

- [ ] **Step 4: Mark OpenSpec tasks**

Check off completed items in `openspec/changes/optimize-file-indexing-performance/tasks.md` only after corresponding tests and verification pass.

2026-06-09: 自动化验证项、基准项、`.gitignore` 行为项和 OpenSpec validate 项已按实际命令结果勾选；macOS/Windows 手工验收项仍保留未勾，等待对应真实机器验收。

## Self-Review

- Spec coverage: The plan covers phased indexing, ignore scanning, metadata, progress, structured query syntax, `content:`, text extraction, Tantivy snippets, watcher, config defaults, UI, docs, and verification.
- Placeholder scan: No placeholder markers or deferred-work wording remain. PDF/Office is explicitly out of scope for implementation and handled by extractor boundary.
- Type consistency: New core types are `IndexedEntry`, `ContentIndexState`, `FileQuery`, `SearchSnippet`, and `IndexStatus`; frontend mirrors camelCase serde payloads.
- Scope check: This is large but cohesive because all tasks are one search-index capability. Each task is independently testable and can be delegated.
