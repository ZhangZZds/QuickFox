## 1. Baseline And Test Harness

- [x] 1.1 Add deterministic synthetic entry generator covering 100,000, 500,000, and 2,000,000 entry scales without requiring real disk files.
- [x] 1.2 Add benchmark queries for `agents.md`, `agents.m`, `agents`, `type:md agents`, `dir:workspace agents`, low-hit random terms, path segment fuzzy, and high-hit common terms.
- [x] 1.3 Add baseline tests or ignored benchmarks that show current linear search latency, candidate work, and memory estimate for large indexes.
- [x] 1.4 Add result-quality oracle fixtures that compare exact, prefix, field-filtered, and fuzzy results against the current expected semantics.
- [x] 1.5 Document how to run large-index benchmarks locally and which subset runs in normal CI.

## 2. Immediate Memory And Refresh Cleanup

- [x] 2.1 Add tests proving scan progress does not retain one full-path accepted event per indexed entry.
- [x] 2.2 Replace per-entry accepted scan events with bounded progress summaries and limited failure samples.
- [x] 2.3 Add tests proving runtime `last_report` and refresh progress do not retain duplicate full entries after index construction.
- [x] 2.4 Refactor refresh progress to avoid cloning aggregate reports and entries across worker/main-thread boundaries.
- [x] 2.5 Add tests or instrumentation showing duplicate `search_text` storage is removed or bounded.

## 3. Compact Index Data Model

- [x] 3.1 Add `StringPool` tests for deduplication, stable ids, and path/name/segment lookup.
- [x] 3.2 Implement `StringPool` and compact entry ids without changing user-visible search behavior.
- [x] 3.3 Add `EntryTable` tests for kind, extension, depth, root, mtime, size, and path reconstruction.
- [x] 3.4 Implement `EntryTable` construction from existing `IndexedEntry` and snapshot data.
- [x] 3.5 Add memory estimate output for string pool, entry table, and per-field indexes.

## 4. Candidate Retrieval Indexes

- [x] 4.1 Add tests for name token and prefix candidate retrieval, including `agents.md` and `agents.m`.
- [x] 4.2 Implement `NameTokenIndex` and `PrefixIndex` over compact entry ids.
- [x] 4.3 Add tests for extension and field-filter narrowing with `type:md agents`.
- [x] 4.4 Implement `ExtensionIndex` and field-filter candidate intersection.
- [x] 4.5 Add tests for path segment and `dir:` retrieval with Windows and Unix-style paths.
- [x] 4.6 Implement `PathSegmentIndex` and directory candidate narrowing.
- [x] 4.7 Add candidate count thresholds and tests proving low-hit queries do not invoke matcher on all entries.

## 5. Search Path Integration

- [x] 5.1 Add oracle tests comparing old linear search and compact candidate search on representative fixtures.
- [x] 5.2 Implement `CandidateRetriever` for ordinary terms and structured `FileQuery` fields.
- [x] 5.3 Route ordinary name/path search through compact candidate retrieval while preserving QuickFox ranker semantics.
- [x] 5.4 Add performance threshold tests for CI-scale indexes and ignored 2,000,000 entry benchmark thresholds.
- [x] 5.5 Add fallback or diagnostic path for unexpected candidate retrieval failures without silently returning empty results.

## 6. Query Scheduling And Frontend Responsiveness

- [x] 6.1 Add Rust tests for request generation or cancellation so stale searches cannot overwrite newer search state.
- [x] 6.2 Implement backend latest-query priority or cancellation checks at candidate retrieval and scoring boundaries.
- [x] 6.3 Add frontend tests for rapid typing, stale response ignoring, and index status revision during input.
- [x] 6.4 Make index status driven query refresh use the same debounce path as normal input searches.

## 7. Content Index Memory Boundary

- [x] 7.1 Add tests proving content index does not keep all indexed file contents and line vectors resident for snippet generation.
- [x] 7.2 Replace full in-memory content document cache with on-demand snippet generation or bounded snippet cache.
- [x] 7.3 Add tests for moved, unreadable, or changed content files returning degraded content hit feedback without breaking name/path search.
- [x] 7.4 Add memory estimate or instrumentation for content snippet cache size.

## 8. Storage And Migration

- [x] 8.1 Add snapshot migration tests for compact index metadata and backward compatibility with existing SQLite batches.
- [x] 8.2 Update SQLite snapshot loading to build compact indexes without holding unnecessary duplicate entries.
- [x] 8.3 Add tests for startup from old snapshot, refreshed snapshot, and content-index state compatibility.

## 9. Verification And QA

- [x] 9.1 Run Rust formatting, clippy, unit tests, and relevant ignored large-index benchmarks; record command output.
- [x] 9.2 Run frontend TypeScript, ESLint, Prettier, and tests covering search responsiveness.
- [x] 9.3 Run `openspec validate support-large-scale-local-index-performance --strict`.
- [x] 9.4 Update Windows manual QA docs with 200 万文件级 or C/D 多盘验收 steps and required metrics.
- [x] 9.5 Perform or explicitly schedule Windows release-build manual QA capturing entry count, disk scale, memory, `agents.md` input behavior, screenshots, and result quality.
- [x] 9.6 Update performance baseline notes with before/after latency, candidate counts, memory estimates, and known residual risks.
