//! File and directory indexing will live here.

use crate::core::actions::Action;
use crate::core::compact_index::CompactCandidateIndex;
use crate::core::content_index::{
    ContentIndex, ContentIndexOptions, ContentPathFilter, ContentSearchHit, PlainTextExtractor,
};
use crate::core::file_matcher::FileMatcher;
use crate::core::file_query::FileQuery;
pub use crate::core::index_entry::{
    ContentIndexState, IndexAvailability, IndexFailure, IndexLifecycle, IndexReport,
    IndexScanOptions, IndexStatus, IndexStatusKind, IndexedEntry, IndexedEntryKind,
};
use crate::core::index_scanner::{FileSystemScanner, IgnoreScanner, IndexScanPlan};
use crate::core::search::{QueryRequest, SearchMode, SearchResult, SearchResultKind};
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(test)]
static SEARCH_INDEX_CLONE_COUNT: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, Default)]
pub struct IndexScanner;

impl IndexScanner {
    pub fn scan(&self, options: IndexScanOptions) -> Result<IndexReport, std::io::Error> {
        self.scan_plan(IndexScanPlan {
            include_roots: options.include_dirs,
            exclude_dirs: options.exclude_dirs,
            exclude_patterns: options.exclude_patterns,
            respect_project_ignores: options.respect_project_ignores,
            stage: None,
        })
    }

    pub fn scan_plan(&self, plan: IndexScanPlan) -> Result<IndexReport, std::io::Error> {
        IgnoreScanner::default().scan(plan)
    }
}

#[derive(Debug, Default)]
pub struct SearchIndex {
    entries: Vec<IndexedEntry>,
    content_entry_index_by_path: Option<HashMap<String, usize>>,
    search_texts: Vec<String>,
    compact_candidates: CompactCandidateIndex,
    content_index: Option<ContentIndex>,
    #[cfg(test)]
    content_entry_lookup_probe_count: AtomicUsize,
    #[cfg(test)]
    content_visibility_entry_scan_count: AtomicUsize,
}

enum ContentSearchVisibility {
    AllVisible,
    EntryPredicate,
    PathFilter(ContentPathFilter),
}

pub trait FileSearchIndex: Send + Sync {
    fn search_files(&self, query: &QueryRequest, limit: usize) -> Vec<SearchResult>;
    fn indexed_entry_count(&self) -> usize;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchIndexMemoryEstimate {
    pub entry_count: usize,
    pub entry_struct_bytes: usize,
    pub entry_string_bytes: usize,
    pub cached_search_text_bytes: usize,
    pub compact_candidate_bytes: usize,
    pub path_lookup_entry_count: usize,
    pub path_lookup_bytes: usize,
}

impl Clone for SearchIndex {
    fn clone(&self) -> Self {
        #[cfg(test)]
        SEARCH_INDEX_CLONE_COUNT.fetch_add(1, Ordering::Relaxed);

        Self {
            entries: self.entries.clone(),
            content_entry_index_by_path: self.content_entry_index_by_path.clone(),
            search_texts: self.search_texts.clone(),
            compact_candidates: self.compact_candidates.clone(),
            content_index: self.content_index.clone(),
            #[cfg(test)]
            content_entry_lookup_probe_count: AtomicUsize::new(
                self.content_entry_lookup_probe_count
                    .load(Ordering::Relaxed),
            ),
            #[cfg(test)]
            content_visibility_entry_scan_count: AtomicUsize::new(
                self.content_visibility_entry_scan_count
                    .load(Ordering::Relaxed),
            ),
        }
    }
}

impl SearchIndex {
    pub fn from_entries(entries: Vec<IndexedEntry>) -> Self {
        let search_texts = entries.iter().map(searchable_text).collect();
        let compact_candidates = CompactCandidateIndex::from_entries(entries.clone());
        Self {
            entries,
            content_entry_index_by_path: None,
            search_texts,
            compact_candidates,
            content_index: None,
            #[cfg(test)]
            content_entry_lookup_probe_count: AtomicUsize::new(0),
            #[cfg(test)]
            content_visibility_entry_scan_count: AtomicUsize::new(0),
        }
    }

    pub fn from_entries_with_content_index(
        entries: Vec<IndexedEntry>,
        content_index: ContentIndex,
    ) -> Self {
        let mut index = Self::from_entries(entries);
        index.attach_content_index(content_index);
        index
    }

    pub fn entries(&self) -> &[IndexedEntry] {
        &self.entries
    }

    pub fn memory_estimate(&self) -> SearchIndexMemoryEstimate {
        let path_lookup_entry_count = self
            .content_entry_index_by_path
            .as_ref()
            .map(HashMap::len)
            .unwrap_or_default();
        let path_lookup_bytes = self
            .content_entry_index_by_path
            .as_ref()
            .map(|lookup| {
                std::mem::size_of::<HashMap<String, usize>>()
                    .saturating_add(lookup.keys().map(String::capacity).sum())
                    .saturating_add(
                        lookup
                            .capacity()
                            .saturating_mul(std::mem::size_of::<(String, usize)>()),
                    )
            })
            .unwrap_or_default();
        SearchIndexMemoryEstimate {
            entry_count: self.entries.len(),
            entry_struct_bytes: self
                .entries
                .capacity()
                .saturating_mul(std::mem::size_of::<IndexedEntry>()),
            entry_string_bytes: self.entries.iter().map(indexed_entry_string_bytes).sum(),
            cached_search_text_bytes: self.search_texts.iter().map(String::capacity).sum(),
            compact_candidate_bytes: self.compact_candidates.estimated_bytes(),
            path_lookup_entry_count,
            path_lookup_bytes,
        }
    }

    pub(crate) fn attach_content_index(&mut self, content_index: ContentIndex) {
        self.content_index = Some(content_index);
        self.rebuild_content_entry_lookup();
    }

    pub(crate) fn detach_content_index(&mut self) -> Option<ContentIndex> {
        self.content_entry_index_by_path = None;
        self.content_index.take()
    }

    fn rebuild_content_entry_lookup(&mut self) {
        self.content_entry_index_by_path = self
            .content_index
            .as_ref()
            .map(|_| build_content_entry_path_lookup(&self.entries));
    }

    #[cfg(test)]
    pub fn reset_clone_count() {
        SEARCH_INDEX_CLONE_COUNT.store(0, Ordering::Relaxed);
    }

    #[cfg(test)]
    pub fn clone_count() -> usize {
        SEARCH_INDEX_CLONE_COUNT.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    pub(crate) fn compact_build_id(&self) -> usize {
        self.compact_candidates.build_id()
    }

    #[cfg(test)]
    fn reset_content_query_probe_count(&self) {
        self.content_entry_lookup_probe_count
            .store(0, Ordering::Relaxed);
    }

    #[cfg(test)]
    fn content_entry_lookup_probe_count(&self) -> usize {
        self.content_entry_lookup_probe_count
            .load(Ordering::Relaxed)
    }

    #[cfg(test)]
    fn content_entry_lookup_count(&self) -> usize {
        self.content_entry_index_by_path
            .as_ref()
            .map(HashMap::len)
            .unwrap_or_default()
    }

    #[cfg(test)]
    fn content_entry_lookup_contains(&self, path: &str) -> bool {
        self.content_entry_index_by_path
            .as_ref()
            .is_some_and(|lookup| lookup.contains_key(path))
    }

    #[cfg(test)]
    fn reset_content_visibility_entry_scan_count(&self) {
        self.content_visibility_entry_scan_count
            .store(0, Ordering::Relaxed);
    }

    #[cfg(test)]
    fn content_visibility_entry_scan_count(&self) -> usize {
        self.content_visibility_entry_scan_count
            .load(Ordering::Relaxed)
    }

    #[cfg(test)]
    fn last_content_collector_limit(&self) -> usize {
        self.content_index
            .as_ref()
            .map(ContentIndex::last_collector_limit)
            .unwrap_or_default()
    }

    pub fn refresh_with_scanner(
        &mut self,
        scanner: &IndexScanner,
        options: IndexScanOptions,
    ) -> Result<IndexReport, std::io::Error> {
        let report = scanner.scan(options)?;
        self.replace_entries(report.entries.clone());
        Ok(report)
    }

    pub fn refresh_incremental_with_scanner(
        &mut self,
        scanner: &IndexScanner,
        options: IndexScanOptions,
    ) -> Result<IndexReport, std::io::Error> {
        let report = scanner.scan(options)?;
        let previous_by_path: std::collections::HashMap<_, _> = self
            .entries
            .iter()
            .cloned()
            .map(|entry| (entry.path.clone(), entry))
            .collect();

        let entries = report
            .entries
            .iter()
            .map(|entry| {
                previous_by_path
                    .get(&entry.path)
                    .filter(|previous| snapshot_metadata_matches(previous, entry))
                    .cloned()
                    .unwrap_or_else(|| entry.clone())
            })
            .collect();
        self.replace_entries(entries);
        Ok(report)
    }

    pub fn search(&self, query: &QueryRequest) -> Vec<SearchResult> {
        self.search_with_limit(query, usize::MAX)
    }

    pub fn apply_update_batch(
        &mut self,
        batch: &crate::core::index_watcher::IndexUpdateBatch,
        changed_entries: Vec<IndexedEntry>,
    ) {
        self.apply_update_batch_with_content_options(batch, changed_entries, None);
    }

    pub fn apply_update_batch_with_content_options(
        &mut self,
        batch: &crate::core::index_watcher::IndexUpdateBatch,
        mut changed_entries: Vec<IndexedEntry>,
        content_options: Option<&ContentIndexOptions>,
    ) {
        let removed_paths: HashSet<_> = batch
            .removed_paths
            .iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect();
        let changed_paths: HashSet<_> = changed_entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect();

        self.entries.retain(|entry| {
            !path_is_affected(&entry.path, &removed_paths) && !changed_paths.contains(&entry.path)
        });

        if let Some(content_index) = &mut self.content_index {
            for path in &batch.removed_paths {
                let _ = content_index.remove_path(path);
            }
            if let Some(options) = content_options {
                for entry in &mut changed_entries {
                    let _ = content_index.update_entry(entry, options, &PlainTextExtractor);
                }
            }
        }

        self.entries.extend(changed_entries);
        self.entries.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then_with(|| left.name.cmp(&right.name))
        });
        self.search_texts = self.entries.iter().map(searchable_text).collect();
        self.compact_candidates = CompactCandidateIndex::from_entries(self.entries.clone());
        self.rebuild_content_entry_lookup();
    }

    pub fn search_with_limit(&self, query: &QueryRequest, limit: usize) -> Vec<SearchResult> {
        self.search_with_limit_cancellable(query, limit, || false)
            .unwrap_or_default()
    }

    pub fn search_with_limit_visible(
        &self,
        query: &QueryRequest,
        limit: usize,
        is_visible: impl Fn(&IndexedEntry) -> bool,
    ) -> Vec<SearchResult> {
        self.search_with_limit_cancellable_visible(query, limit, || false, is_visible)
            .unwrap_or_default()
    }

    pub(crate) fn search_with_limit_visible_and_content_filter(
        &self,
        query: &QueryRequest,
        limit: usize,
        content_path_filter: ContentPathFilter,
        is_visible: impl Fn(&IndexedEntry) -> bool,
    ) -> Vec<SearchResult> {
        self.search_with_limit_cancellable_visibility(
            query,
            limit,
            || false,
            is_visible,
            ContentSearchVisibility::PathFilter(content_path_filter),
        )
        .unwrap_or_default()
    }

    pub fn search_with_limit_cancellable(
        &self,
        query: &QueryRequest,
        limit: usize,
        is_cancelled: impl Fn() -> bool,
    ) -> Option<Vec<SearchResult>> {
        self.search_with_limit_cancellable_visibility(
            query,
            limit,
            is_cancelled,
            |_| true,
            ContentSearchVisibility::AllVisible,
        )
    }

    pub fn search_with_limit_cancellable_visible(
        &self,
        query: &QueryRequest,
        limit: usize,
        is_cancelled: impl Fn() -> bool,
        is_visible: impl Fn(&IndexedEntry) -> bool,
    ) -> Option<Vec<SearchResult>> {
        self.search_with_limit_cancellable_visibility(
            query,
            limit,
            is_cancelled,
            is_visible,
            ContentSearchVisibility::EntryPredicate,
        )
    }

    fn search_with_limit_cancellable_visibility(
        &self,
        query: &QueryRequest,
        limit: usize,
        is_cancelled: impl Fn() -> bool,
        is_visible: impl Fn(&IndexedEntry) -> bool,
        content_visibility: ContentSearchVisibility,
    ) -> Option<Vec<SearchResult>> {
        if is_cancelled() {
            return None;
        }

        match &query.mode {
            SearchMode::Normal => self.search_normal(
                &query.text,
                limit,
                &is_cancelled,
                &is_visible,
                content_visibility,
            ),
            SearchMode::Regex => self.search_regex(&query.text, limit, &is_cancelled, &is_visible),
            SearchMode::WebSearch { .. } | SearchMode::Command => Some(Vec::new()),
        }
    }

    fn replace_entries(&mut self, entries: Vec<IndexedEntry>) {
        self.search_texts = entries.iter().map(searchable_text).collect();
        self.compact_candidates = CompactCandidateIndex::from_entries(entries.clone());
        self.entries = entries;
        self.detach_content_index();
    }

    fn search_normal(
        &self,
        query: &str,
        limit: usize,
        is_cancelled: &impl Fn() -> bool,
        is_visible: &impl Fn(&IndexedEntry) -> bool,
        content_visibility: ContentSearchVisibility,
    ) -> Option<Vec<SearchResult>> {
        let parsed_query = FileQuery::parse(query);
        if limit == 0
            || (!parsed_query.has_name_path_constraints() && !parsed_query.has_content_query())
        {
            return Some(Vec::new());
        }

        if parsed_query.has_content_query() && self.content_index.is_none() {
            return Some(vec![SearchResult::new(
                "feedback:content-index-unavailable",
                "内容索引仍在准备",
                SearchResultKind::Feedback,
                Action::CopyText {
                    text: "内容索引仍在准备，普通文件名和路径搜索可先使用。".to_owned(),
                },
            )
            .with_detail("普通文件名和路径搜索已可用；content: 查询需要等待内容索引。".to_owned())
            .with_score(100)]);
        }

        if parsed_query.has_content_query() && !parsed_query.has_name_path_constraints() {
            return Some(self.search_content_only(
                &parsed_query,
                limit,
                is_visible,
                content_visibility,
            ));
        }

        let matcher = FileMatcher::default();
        let candidate_ids = self.compact_candidates.retrieve_query(query).candidates;
        if is_cancelled() {
            return None;
        }

        let mut candidates = Vec::new();
        for (candidate_index, id) in candidate_ids.into_iter().enumerate() {
            if candidate_index % 1024 == 0 && is_cancelled() {
                return None;
            }
            let index = id.as_usize();
            let Some(entry) = self.entries.get(index) else {
                continue;
            };
            if !is_visible(entry) {
                continue;
            }
            let Some(search_text) = self.search_texts.get(index) else {
                continue;
            };
            if matcher.matches_with_search_text(&parsed_query, entry, search_text) {
                candidates.push(entry);
                if !parsed_query.has_content_query() && candidates.len() >= limit {
                    break;
                }
            }
        }

        if !parsed_query.has_content_query() {
            return Some(candidates.into_iter().map(entry_to_result).collect());
        }

        let candidate_paths = Arc::new(candidates.iter().map(|entry| entry.path.clone()).collect());
        if is_cancelled() {
            return None;
        }
        let content_hits = self.search_content_hits(&parsed_query, Some(candidate_paths), limit);
        let hit_by_path: HashMap<_, _> = content_hits
            .into_iter()
            .map(|hit| (hit.path.clone(), hit))
            .collect();
        let mut results: Vec<_> = candidates
            .into_iter()
            .filter_map(|entry| {
                hit_by_path.get(&entry.path).map(|hit| {
                    entry_to_result(entry)
                        .with_snippet(hit.snippet.clone())
                        .with_score(content_score(hit).saturating_add(10_000))
                })
            })
            .collect();

        results.sort_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then_with(|| left.title.cmp(&right.title))
                .then_with(|| left.id.cmp(&right.id))
        });
        results.truncate(limit);
        Some(results)
    }

    fn search_content_only(
        &self,
        query: &FileQuery,
        limit: usize,
        is_visible: &impl Fn(&IndexedEntry) -> bool,
        content_visibility: ContentSearchVisibility,
    ) -> Vec<SearchResult> {
        let path_filter = match content_visibility {
            ContentSearchVisibility::AllVisible => None,
            ContentSearchVisibility::PathFilter(path_filter) => Some(path_filter),
            ContentSearchVisibility::EntryPredicate => {
                let hidden_paths: HashSet<_> = self
                    .entries
                    .iter()
                    .filter(|entry| {
                        #[cfg(test)]
                        self.content_visibility_entry_scan_count
                            .fetch_add(1, Ordering::Relaxed);
                        !is_visible(entry)
                    })
                    .map(|entry| entry.path.clone())
                    .collect();
                if hidden_paths.is_empty() {
                    None
                } else {
                    let hidden_paths = Arc::new(hidden_paths);
                    Some(Arc::new(move |path: &str| !hidden_paths.contains(path))
                        as ContentPathFilter)
                }
            }
        };
        let hits = self.search_content_hits_with_filter(query, path_filter, limit);
        let mut results: Vec<_> = hits
            .into_iter()
            .filter_map(|hit| {
                let score = content_score(&hit);
                #[cfg(test)]
                self.content_entry_lookup_probe_count
                    .fetch_add(1, Ordering::Relaxed);
                self.content_entry_index_by_path
                    .as_ref()
                    .and_then(|lookup| lookup.get(&hit.path))
                    .and_then(|index| self.entries.get(*index))
                    .map(|entry| {
                        entry_to_result(entry)
                            .with_snippet(hit.snippet)
                            .with_score(score)
                    })
            })
            .collect();
        results.sort_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then_with(|| left.title.cmp(&right.title))
                .then_with(|| left.id.cmp(&right.id))
        });
        results.truncate(limit);
        results
    }

    fn search_content_hits(
        &self,
        query: &FileQuery,
        candidate_paths: Option<Arc<HashSet<String>>>,
        limit: usize,
    ) -> Vec<ContentSearchHit> {
        let Some(content_index) = &self.content_index else {
            return Vec::new();
        };
        let content_query = query.content_queries.join(" ");
        content_index
            .search_with_candidate_paths(&content_query, candidate_paths, limit)
            .unwrap_or_default()
    }

    fn search_content_hits_with_filter(
        &self,
        query: &FileQuery,
        path_filter: Option<ContentPathFilter>,
        limit: usize,
    ) -> Vec<ContentSearchHit> {
        let Some(content_index) = &self.content_index else {
            return Vec::new();
        };
        let content_query = query.content_queries.join(" ");
        content_index
            .search_with_path_filter(&content_query, path_filter, limit)
            .unwrap_or_default()
    }

    fn search_regex(
        &self,
        pattern: &str,
        limit: usize,
        is_cancelled: &impl Fn() -> bool,
        is_visible: &impl Fn(&IndexedEntry) -> bool,
    ) -> Option<Vec<SearchResult>> {
        let regex = match Regex::new(pattern) {
            Ok(regex) => regex,
            Err(error) => {
                return Some(vec![SearchResult::new(
                    "feedback:invalid-regex",
                    format!("无效正则: {error}"),
                    SearchResultKind::Feedback,
                    Action::CopyText {
                        text: error.to_string(),
                    },
                )]);
            }
        };

        let mut results = Vec::new();
        for (index, entry) in self.entries.iter().enumerate() {
            if index % 1024 == 0 && is_cancelled() {
                return None;
            }
            if !is_visible(entry) {
                continue;
            }
            if regex.is_match(&entry.name) || regex.is_match(&entry.path) {
                results.push(entry_to_result(entry));
                if results.len() >= limit {
                    break;
                }
            }
        }
        Some(results)
    }
}

impl FileSearchIndex for SearchIndex {
    fn search_files(&self, query: &QueryRequest, limit: usize) -> Vec<SearchResult> {
        self.search_with_limit(query, limit)
    }

    fn indexed_entry_count(&self) -> usize {
        self.entries.len()
    }
}

fn content_score(hit: &ContentSearchHit) -> i64 {
    (hit.score * 1_000.0).round() as i64
}

fn searchable_text(entry: &IndexedEntry) -> String {
    if entry.search_text.is_empty() {
        crate::core::index_entry::build_search_text(&entry.name, &entry.path)
    } else {
        entry.search_text.clone()
    }
}

fn build_content_entry_path_lookup(entries: &[IndexedEntry]) -> HashMap<String, usize> {
    entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.content_index_state == ContentIndexState::Indexed)
        .map(|(index, entry)| (entry.path.clone(), index))
        .collect()
}

fn indexed_entry_string_bytes(entry: &IndexedEntry) -> usize {
    entry.path.capacity()
        + entry.name.capacity()
        + entry.parent.capacity()
        + entry
            .extension
            .as_ref()
            .map(String::capacity)
            .unwrap_or_default()
        + entry.root.capacity()
        + entry.search_text.capacity()
}

fn snapshot_metadata_matches(previous: &IndexedEntry, scanned: &IndexedEntry) -> bool {
    previous.path == scanned.path
        && previous.kind == scanned.kind
        && previous.root == scanned.root
        && previous.modified_ms == scanned.modified_ms
        && previous.size_bytes == scanned.size_bytes
}

fn path_is_affected(path: &str, roots: &HashSet<String>) -> bool {
    roots.iter().any(|root| {
        path == root
            || path
                .strip_prefix(root)
                .is_some_and(|rest| rest.starts_with(std::path::MAIN_SEPARATOR))
    })
}

fn entry_to_result(entry: &IndexedEntry) -> SearchResult {
    let kind = match entry.kind {
        IndexedEntryKind::Application => SearchResultKind::Application,
        IndexedEntryKind::File => SearchResultKind::File,
        IndexedEntryKind::Directory => SearchResultKind::Directory,
    };

    let mut result = SearchResult::new(
        format!("path:{}", entry.path),
        entry.name.clone(),
        kind,
        Action::OpenPath {
            path: entry.path.clone(),
        },
    )
    .with_detail(entry.path.clone());

    match entry.kind {
        IndexedEntryKind::Application => {
            result = result.with_secondary_action(Action::OpenPath {
                path: entry.path.clone(),
            });
        }
        IndexedEntryKind::File => {
            result = result.with_secondary_action(Action::OpenContainingFolder {
                path: entry.path.clone(),
            });
            if let Some(parent) = parent_path_text(&entry.path) {
                result = result
                    .with_secondary_action(Action::CopyText {
                        text: parent.clone(),
                    })
                    .with_secondary_action(Action::OpenPath { path: parent });
            }
        }
        IndexedEntryKind::Directory => {
            result = result.with_secondary_action(Action::OpenPath {
                path: entry.path.clone(),
            });
            if let Some(parent) = parent_path_text(&entry.path) {
                result = result
                    .with_secondary_action(Action::CopyText {
                        text: parent.clone(),
                    })
                    .with_secondary_action(Action::OpenPath { path: parent });
            }
        }
    }

    result.with_secondary_action(Action::CopyText {
        text: entry.path.clone(),
    })
}

fn parent_path_text(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches(['/', '\\']);
    let separator_index = trimmed.rfind(['/', '\\'])?;
    if separator_index == 0 {
        return Some(trimmed[..=separator_index].to_owned());
    }
    let parent = &trimmed[..separator_index];
    if parent.ends_with(':') {
        return Some(format!("{parent}\\"));
    }
    (!parent.is_empty()).then(|| parent.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::content_index::{
        ContentExtractionResult, TextExtractor, DEFAULT_MAX_CONTENT_BYTES,
    };
    use std::fs;
    use std::path::PathBuf;
    use std::time::Instant;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn scanner_indexes_file_and_directory_names_and_paths() {
        let root = temp_dir("scan-basic");
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::write(root.join("docs").join("readme.md"), "secret body text").unwrap();

        let report = IndexScanner
            .scan(IndexScanOptions {
                include_dirs: vec![root.clone()],
                exclude_dirs: Vec::new(),
                exclude_patterns: Vec::new(),
                respect_project_ignores: true,
            })
            .unwrap();

        assert!(report.failures.is_empty());
        assert!(report.entries.iter().any(|entry| {
            entry.kind == IndexedEntryKind::Directory
                && entry.name == "docs"
                && entry.path.ends_with("docs")
        }));
        assert!(report.entries.iter().any(|entry| {
            entry.kind == IndexedEntryKind::File
                && entry.name == "readme.md"
                && entry.path.ends_with("readme.md")
        }));
        assert!(!report
            .entries
            .iter()
            .any(|entry| entry.name.contains("secret") || entry.path.contains("secret")));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn scanner_honors_excluded_directories_and_patterns() {
        let root = temp_dir("scan-exclude");
        let excluded_dir = root.join("vendor");
        fs::create_dir_all(&excluded_dir).unwrap();
        fs::write(excluded_dir.join("hidden.md"), "").unwrap();
        fs::write(root.join("keep.md"), "").unwrap();
        fs::write(root.join("debug.log"), "").unwrap();
        fs::write(root.join("ERROR.LOG"), "").unwrap();

        let report = IndexScanner
            .scan(IndexScanOptions {
                include_dirs: vec![root.clone()],
                exclude_dirs: vec![excluded_dir.clone()],
                exclude_patterns: vec!["*.log".to_owned()],
                respect_project_ignores: true,
            })
            .unwrap();

        let names: Vec<_> = report
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        assert!(names.contains(&"keep.md"));
        assert!(!names.contains(&"vendor"));
        assert!(!names.contains(&"hidden.md"));
        assert!(!names.contains(&"debug.log"));
        assert!(!names.contains(&"ERROR.LOG"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn scanner_deduplicates_include_roots_before_scanning() {
        let root = temp_dir("scan-deduplicate");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("keep.md"), "").unwrap();

        let report = IndexScanner
            .scan(IndexScanOptions {
                include_dirs: vec![root.clone(), root.clone()],
                exclude_dirs: Vec::new(),
                exclude_patterns: Vec::new(),
                respect_project_ignores: true,
            })
            .unwrap();

        let keep_count = report
            .entries
            .iter()
            .filter(|entry| entry.name == "keep.md")
            .count();
        assert_eq!(keep_count, 1);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn scanner_indexes_applications_without_app_bundle_internals() {
        let root = temp_dir("scan-app-bundle");
        let app_contents = root.join("PyCharm.app").join("Contents").join("Helpers");
        fs::create_dir_all(&app_contents).unwrap();
        fs::write(
            app_contents.join("pydevd_cython_win32_312_64.cp312-win_amd64.pyd"),
            "",
        )
        .unwrap();
        fs::write(root.join("tool.exe"), "").unwrap();
        fs::write(root.join("QuickFox.desktop"), "").unwrap();
        fs::write(root.join("notes.md"), "").unwrap();

        let report = IndexScanner
            .scan(IndexScanOptions {
                include_dirs: vec![root.clone()],
                exclude_dirs: Vec::new(),
                exclude_patterns: Vec::new(),
                respect_project_ignores: true,
            })
            .unwrap();

        assert!(report.entries.iter().any(|entry| {
            entry.name == "PyCharm.app" && entry.kind == IndexedEntryKind::Application
        }));
        assert!(report
            .entries
            .iter()
            .any(|entry| entry.name == "tool.exe" && entry.kind == IndexedEntryKind::Application));
        assert!(report.entries.iter().any(|entry| {
            entry.name == "QuickFox.desktop" && entry.kind == IndexedEntryKind::Application
        }));
        assert!(!report
            .entries
            .iter()
            .any(|entry| entry.name.contains("pydevd_cython")));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn manual_refresh_replaces_entries_and_reports_failed_roots() {
        let root = temp_dir("refresh");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("old.md"), "").unwrap();
        let missing = root.join("missing");

        let mut index = SearchIndex::default();
        let scanner = IndexScanner;
        let first_report = index
            .refresh_with_scanner(
                &scanner,
                IndexScanOptions {
                    include_dirs: vec![root.clone()],
                    exclude_dirs: Vec::new(),
                    exclude_patterns: Vec::new(),
                    respect_project_ignores: true,
                },
            )
            .unwrap();
        assert!(first_report.failures.is_empty());
        assert!(index.entries().iter().any(|entry| entry.name == "old.md"));

        fs::remove_file(root.join("old.md")).unwrap();
        fs::write(root.join("new.md"), "").unwrap();
        let second_report = index
            .refresh_with_scanner(
                &scanner,
                IndexScanOptions {
                    include_dirs: vec![root.clone(), missing.clone()],
                    exclude_dirs: Vec::new(),
                    exclude_patterns: Vec::new(),
                    respect_project_ignores: true,
                },
            )
            .unwrap();

        assert_eq!(second_report.failures.len(), 1);
        assert_eq!(second_report.failures[0].root, missing.to_string_lossy());
        assert!(index.entries().iter().any(|entry| entry.name == "new.md"));
        assert!(!index.entries().iter().any(|entry| entry.name == "old.md"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn incremental_refresh_updates_changed_paths_without_dropping_unchanged_entries() {
        let root = temp_dir("incremental-refresh");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("keep.md"), "").unwrap();
        fs::write(root.join("old.md"), "").unwrap();

        let mut index = SearchIndex::default();
        let scanner = IndexScanner;
        index
            .refresh_with_scanner(
                &scanner,
                IndexScanOptions {
                    include_dirs: vec![root.clone()],
                    exclude_dirs: Vec::new(),
                    exclude_patterns: Vec::new(),
                    respect_project_ignores: true,
                },
            )
            .unwrap();

        fs::remove_file(root.join("old.md")).unwrap();
        fs::write(root.join("new.md"), "").unwrap();
        index
            .refresh_incremental_with_scanner(
                &scanner,
                IndexScanOptions {
                    include_dirs: vec![root.clone()],
                    exclude_dirs: Vec::new(),
                    exclude_patterns: Vec::new(),
                    respect_project_ignores: true,
                },
            )
            .unwrap();

        let names: Vec<_> = index
            .entries()
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        assert!(names.contains(&"keep.md"));
        assert!(names.contains(&"new.md"));
        assert!(!names.contains(&"old.md"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn incremental_refresh_reuses_snapshot_entry_when_metadata_is_unchanged() {
        let root = temp_dir("incremental-refresh-snapshot-assisted");
        fs::create_dir_all(&root).unwrap();
        let keep = root.join("keep.md");
        fs::write(&keep, "same bytes").unwrap();

        let mut index = SearchIndex::default();
        let scanner = IndexScanner;
        index
            .refresh_with_scanner(
                &scanner,
                IndexScanOptions {
                    include_dirs: vec![root.clone()],
                    exclude_dirs: Vec::new(),
                    exclude_patterns: Vec::new(),
                    respect_project_ignores: true,
                },
            )
            .unwrap();
        let entry = index
            .entries
            .iter_mut()
            .find(|entry| entry.name == "keep.md")
            .unwrap();
        entry.search_text = "snapshot-only-search-text".to_owned();
        entry.content_index_state = ContentIndexState::Indexed;

        index
            .refresh_incremental_with_scanner(
                &scanner,
                IndexScanOptions {
                    include_dirs: vec![root.clone()],
                    exclude_dirs: Vec::new(),
                    exclude_patterns: Vec::new(),
                    respect_project_ignores: true,
                },
            )
            .unwrap();

        let refreshed = index
            .entries()
            .iter()
            .find(|entry| entry.name == "keep.md")
            .unwrap();
        assert_eq!(refreshed.search_text, "snapshot-only-search-text");
        assert_eq!(refreshed.content_index_state, ContentIndexState::Indexed);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn update_batch_removes_replaces_and_adds_name_path_entries() {
        let root = temp_dir("update-batch");
        fs::create_dir_all(&root).unwrap();
        let removed = root.join("removed.md");
        let changed = root.join("changed.md");
        let added = root.join("added.md");
        fs::write(&removed, "removed").unwrap();
        fs::write(&changed, "old").unwrap();
        fs::write(&added, "new").unwrap();

        let mut index = SearchIndex::from_entries(vec![
            IndexedEntry::from_path_metadata(&removed, &root, IndexedEntryKind::File),
            IndexedEntry::from_path_metadata(&changed, &root, IndexedEntryKind::File),
        ]);
        let mut changed_entry =
            IndexedEntry::from_path_metadata(&changed, &root, IndexedEntryKind::File);
        changed_entry.name = "renamed-changed.md".to_owned();
        changed_entry.search_text =
            crate::core::index_entry::build_search_text(&changed_entry.name, &changed_entry.path);
        let added_entry = IndexedEntry::from_path_metadata(&added, &root, IndexedEntryKind::File);

        let batch = crate::core::index_watcher::IndexUpdateBatch {
            changed_paths: vec![changed.clone(), added.clone()],
            removed_paths: vec![removed.clone()],
        };
        index.apply_update_batch(&batch, vec![changed_entry, added_entry]);

        let names: Vec<_> = index
            .entries()
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        assert_eq!(names, vec!["added.md", "renamed-changed.md"]);

        let parser = crate::core::search::QueryParser::new(Default::default());
        let results = index.search(&parser.parse("renamed-changed"));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "renamed-changed.md");

        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn scanner_reports_unreadable_nested_directories_without_dropping_other_results() {
        use std::os::unix::fs::PermissionsExt;

        let root = temp_dir("scan-unreadable");
        let locked = root.join("locked");
        fs::create_dir_all(&locked).unwrap();
        fs::write(root.join("keep.md"), "").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

        let report = IndexScanner
            .scan(IndexScanOptions {
                include_dirs: vec![root.clone()],
                exclude_dirs: Vec::new(),
                exclude_patterns: Vec::new(),
                respect_project_ignores: true,
            })
            .unwrap();

        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();

        assert!(report.entries.iter().any(|entry| entry.name == "keep.md"));
        assert!(report
            .failures
            .iter()
            .any(|failure| failure.root.ends_with("locked")));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn search_returns_fuzzy_name_and_path_matches_but_not_file_content() {
        let index = SearchIndex::from_entries(vec![
            file_entry("/home/frank/projects/quickfox/readme.md"),
            file_entry("/home/frank/docs/plain.txt"),
        ]);
        let parser = crate::core::search::QueryParser::new(Default::default());

        let results = index.search(&parser.parse("qfx"));
        assert_eq!(results[0].title, "readme.md");

        let content_results = index.search(&parser.parse("body-only-secret"));
        assert!(content_results.is_empty());
    }

    #[test]
    fn search_supports_explicit_regex_queries() {
        let index = SearchIndex::from_entries(vec![
            file_entry("/home/frank/report.pdf"),
            file_entry("/home/frank/report.txt"),
        ]);
        let parser = crate::core::search::QueryParser::new(Default::default());

        let results = index.search(&parser.parse(r"re:.*\.pdf$"));

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "report.pdf");
    }

    #[test]
    fn search_returns_feedback_for_invalid_regex_without_panicking() {
        let index = SearchIndex::from_entries(vec![file_entry("/home/frank/report.pdf")]);
        let parser = crate::core::search::QueryParser::new(Default::default());

        let results = index.search(&parser.parse("re:*bad"));

        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].kind,
            crate::core::search::SearchResultKind::Feedback
        );
        assert!(results[0].title.contains("无效正则"));
    }

    #[test]
    fn search_does_not_return_low_quality_fuzzy_noise_for_missing_query() {
        let index = SearchIndex::from_entries(vec![IndexedEntry {
            path: "/Users/frank/Applications/PyCharm.app/Contents/plugins/python-ce/helpers/pydev/_pydevd_bundle/pydevd_cython_win32_312_64.cp312-win_amd64.pyd".to_owned(),
            name: "pydevd_cython_win32_312_64.cp312-win_amd64.pyd".to_owned(),
            kind: IndexedEntryKind::File,
            ..IndexedEntry::legacy("", "", IndexedEntryKind::File)
        }]);
        let parser = crate::core::search::QueryParser::new(Default::default());

        let results = index.search(&parser.parse("Openspec_123"));

        assert!(results.is_empty());
    }

    #[test]
    fn search_does_not_return_codex_fuzzy_noise() {
        let index = SearchIndex::from_entries(vec![
            IndexedEntry {
                path: "/Users/frankzhang/Desktop/Codex.app".to_owned(),
                name: "Codex.app".to_owned(),
                kind: IndexedEntryKind::Application,
                ..IndexedEntry::legacy("", "", IndexedEntryKind::Application)
            },
            IndexedEntry {
                path: "/Users/frankzhang/Documents/Codex".to_owned(),
                name: "Codex".to_owned(),
                kind: IndexedEntryKind::Directory,
                ..IndexedEntry::legacy("", "", IndexedEntryKind::Directory)
            },
            file_entry("/Users/frankzhang/workspace/QuickFox/index.html"),
            file_entry("/Users/frankzhang/Library/Metadata/CoreSpotlight/index.spotlightV3/0.ivf-vector-indexes"),
            file_entry("/Users/frankzhang/Library/Mobile Documents/com~apple~CloudDocs/cpl/cloudsync.noindex"),
        ]);
        let parser = crate::core::search::QueryParser::new(Default::default());

        let titles: Vec<_> = index
            .search(&parser.parse("Codex"))
            .into_iter()
            .map(|result| result.title)
            .collect();

        assert_eq!(titles, vec!["Codex.app", "Codex"]);
    }

    #[test]
    fn search_does_not_return_children_only_matching_parent_mid_word() {
        let index = SearchIndex::from_entries(vec![
            file_entry("/Users/frankzhang/workspace/QuickFox/src/setupTests.ts"),
            file_entry("/Users/frankzhang/Pictures/Photos Library.photoslibrary/resources/cpl/cloudsync.noindex/outgoingRecordComputeStates/filecache"),
            file_entry("/Users/frankzhang/Pictures/Photos Library.photoslibrary/resources/cpl/cloudsync.noindex/outgoingRecordComputeStates/outgoingRecordCompute"),
        ]);
        let parser = crate::core::search::QueryParser::new(Default::default());

        let titles: Vec<_> = index
            .search(&parser.parse("Test"))
            .into_iter()
            .map(|result| result.title)
            .collect();

        assert_eq!(titles, vec!["setupTests.ts"]);
    }

    #[test]
    fn search_matches_full_path_queries_exactly() {
        let index = SearchIndex::from_entries(vec![
            file_entry("/Users/frankzhang/workspace/codeforge/README.md"),
            file_entry("/Users/frankzhang/workspace/QuickFox/README.md"),
        ]);
        let parser = crate::core::search::QueryParser::new(Default::default());

        let results =
            index.search(&parser.parse("/Users/frankzhang/workspace/codeforge/README.md"));

        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].detail.as_deref(),
            Some("/Users/frankzhang/workspace/codeforge/README.md")
        );
    }

    #[test]
    fn search_uses_cached_case_insensitive_text_for_name_and_path_matches() {
        let index =
            SearchIndex::from_entries(vec![file_entry("/home/frank/Projects/QuickFox/README.md")]);
        let parser = crate::core::search::QueryParser::new(Default::default());

        let name_results = index.search(&parser.parse("readme"));
        let path_results = index.search(&parser.parse("quickfox"));

        assert_eq!(name_results[0].title, "README.md");
        assert_eq!(path_results[0].title, "README.md");
    }

    #[test]
    fn search_preserves_name_substring_matches_when_candidates_miss() {
        let index = SearchIndex::from_entries(vec![
            file_entry("/Users/frank/workspace/reports/report.md"),
            file_entry("/Users/frank/workspace/reports/notes.md"),
        ]);
        let parser = crate::core::search::QueryParser::new(Default::default());

        let titles: Vec<_> = index
            .search(&parser.parse("port"))
            .into_iter()
            .map(|result| result.title)
            .collect();

        assert_eq!(titles, vec!["report.md"]);
    }

    #[test]
    fn search_with_limit_bounds_constructed_results() {
        let index = SearchIndex::from_entries(vec![
            file_entry("/tmp/project-alpha.md"),
            file_entry("/tmp/project-beta.md"),
            file_entry("/tmp/project-gamma.md"),
        ]);
        let parser = crate::core::search::QueryParser::new(Default::default());

        let results = index.search_with_limit(&parser.parse("project"), 2);

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "project-alpha.md");
        assert_eq!(results[1].title, "project-beta.md");
    }

    #[test]
    fn structured_file_query_filters_by_type_name_and_dir() {
        let index = SearchIndex::from_entries(vec![
            file_entry("/Users/frank/workspace/reports/budget.PDF"),
            file_entry("/Users/frank/workspace/reports/budget.md"),
            file_entry("/Users/frank/downloads/reports/budget.PDF"),
            file_entry("/Users/frank/workspace/reports/notes.PDF"),
        ]);
        let parser = crate::core::search::QueryParser::new(Default::default());

        let titles: Vec<_> = index
            .search(&parser.parse("budget type:pdf name:budget dir:workspace"))
            .into_iter()
            .map(|result| result.title)
            .collect();

        assert_eq!(titles, vec!["budget.PDF"]);
    }

    #[test]
    fn structured_file_query_preserves_name_substring_matches() {
        let index = SearchIndex::from_entries(vec![
            file_entry("/Users/frank/workspace/reports/report.md"),
            file_entry("/Users/frank/workspace/reports/notes.md"),
        ]);
        let parser = crate::core::search::QueryParser::new(Default::default());

        let titles: Vec<_> = index
            .search(&parser.parse("name:port"))
            .into_iter()
            .map(|result| result.title)
            .collect();

        assert_eq!(titles, vec!["report.md"]);
    }

    #[test]
    fn structured_file_query_filters_over_recalled_name_candidates() {
        let index = SearchIndex::from_entries(vec![
            file_entry("/Users/frank/workspace/QuickFox/AGENTS.md"),
            file_entry("/Users/frank/workspace/QuickFox/AGENTS.txt"),
        ]);
        let parser = crate::core::search::QueryParser::new(Default::default());

        let results = index.search(&parser.parse("name:agents.txt type:md"));

        assert!(results.is_empty());
    }

    #[test]
    fn structured_file_query_preserves_dir_glob_matches() {
        let index = SearchIndex::from_entries(vec![
            file_entry("/Users/frank/workspace/QuickFox/budget.PDF"),
            file_entry("/Users/frank/workspace/Other/budget.PDF"),
            file_entry("/Users/frank/downloads/QuickFox/budget.PDF"),
        ]);
        let parser = crate::core::search::QueryParser::new(Default::default());

        let titles: Vec<_> = index
            .search(&parser.parse("budget type:pdf dir:**/workspace/QuickFox"))
            .into_iter()
            .map(|result| result.detail.unwrap_or_default())
            .collect();

        assert_eq!(titles, vec!["/Users/frank/workspace/QuickFox/budget.PDF"]);
    }

    #[test]
    fn structured_file_query_allows_space_after_field_colon() {
        let index = SearchIndex::from_entries(vec![
            file_entry("/Users/frank/workspace/QuickFox/AGENTS.md"),
            file_entry("/Users/frank/workspace/QuickFox/README.txt"),
        ]);
        let parser = crate::core::search::QueryParser::new(Default::default());

        let titles: Vec<_> = index
            .search(&parser.parse("Agent type: md"))
            .into_iter()
            .map(|result| result.title)
            .collect();

        assert_eq!(titles, vec!["AGENTS.md"]);
    }

    #[test]
    fn content_only_query_returns_feedback_before_content_indexing() {
        let index = SearchIndex::from_entries(vec![file_entry("/Users/frank/workspace/hello.md")]);
        let parser = crate::core::search::QueryParser::new(Default::default());

        let results = index.search(&parser.parse(r#"content:"hello world""#));

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].kind, SearchResultKind::Feedback);
        assert!(results[0].title.contains("内容索引"));
    }

    #[test]
    fn content_field_filters_name_candidates_to_real_content_hits() {
        let root = temp_dir("content-field-filter");
        let quickfox = root.join("QuickFox");
        let cc = root.join("cc");
        let src = root.join("src");
        fs::create_dir_all(&quickfox).unwrap();
        fs::create_dir_all(&cc).unwrap();
        fs::create_dir_all(&src).unwrap();

        let matching = quickfox.join("AGENTS.md");
        let name_only = cc.join("AGENTS.md");
        let other = src.join("agent");
        fs::write(&matching, "specific openspec guidance").unwrap();
        fs::write(&name_only, "agent notes without the content term").unwrap();
        fs::write(&other, "agent helper without the content term").unwrap();

        let mut entries = vec![
            file_entry(&matching.to_string_lossy()),
            file_entry(&name_only.to_string_lossy()),
            file_entry(&other.to_string_lossy()),
        ];
        let content_index = ContentIndex::build(
            &mut entries,
            ContentIndexOptions {
                index_dir: root.join("tantivy-content"),
                max_file_bytes: DEFAULT_MAX_CONTENT_BYTES,
            },
        )
        .unwrap();
        let index = SearchIndex::from_entries_with_content_index(entries, content_index);
        let parser = crate::core::search::QueryParser::new(Default::default());

        let results = index.search(&parser.parse("name:Agent content:”openspec”"));

        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].detail.as_deref(),
            Some(matching.to_str().unwrap())
        );
        assert!(results[0].snippet.is_some());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn large_index_search_stops_at_candidate_limit() {
        let entries: Vec<_> = (0..10_000)
            .map(|index| file_entry(&format!("/tmp/project-{index}.md")))
            .collect();
        let index = SearchIndex::from_entries(entries);
        let parser = crate::core::search::QueryParser::new(Default::default());

        let started = Instant::now();
        let results = index.search_with_limit(&parser.parse("project"), 25);
        let elapsed = started.elapsed();

        assert_eq!(results.len(), 25);
        assert!(
            elapsed.as_millis() < 500,
            "large index query took {elapsed:?}, expected bounded candidate work"
        );
    }

    #[test]
    fn synthetic_large_index_fixture_places_agents_target_late_and_defines_queries() {
        let fixture = SyntheticLargeIndexFixture::new(10_000);
        let queries = large_index_benchmark_queries();

        assert_eq!(fixture.entries.len(), 10_000);
        assert_eq!(
            fixture.agents_path,
            "D:\\workspace\\QuickFox\\docs\\AGENTS.md"
        );
        assert!(fixture
            .entries
            .iter()
            .position(|entry| entry.path == fixture.agents_path)
            .is_some_and(|position| position > 9_000));
        assert!(queries.iter().any(|query| query.name == "agents-exact"));
        assert!(queries.iter().any(|query| query.query == "agents.m"));
        assert!(queries.iter().any(|query| query.query == "type:md agents"));
        assert!(queries
            .iter()
            .any(|query| query.query == "dir:workspace agents"));
        assert!(queries
            .iter()
            .any(|query| query.kind == BenchmarkQueryKind::LowHit));
        assert!(queries
            .iter()
            .any(|query| query.kind == BenchmarkQueryKind::HighHit));
    }

    #[test]
    fn synthetic_large_index_oracle_keeps_agents_queries_in_top_five() {
        let fixture = SyntheticLargeIndexFixture::new(10_000);
        let parser = crate::core::search::QueryParser::new(Default::default());
        let search_index = SearchIndex::from_entries(fixture.entries.clone());

        for query in large_index_benchmark_queries()
            .into_iter()
            .filter(|query| query.expected_agents_top_five)
        {
            let results = search_index.search_with_limit(&parser.parse(query.query), 20);
            let target_position = results
                .iter()
                .position(|result| result.detail.as_deref() == Some(fixture.agents_path.as_str()));
            assert!(
                target_position.is_some_and(|position| position < 5),
                "query {} did not keep {} in top five; got {:?}",
                query.name,
                fixture.agents_path,
                results
                    .iter()
                    .map(|result| result.detail.as_deref().unwrap_or_default())
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn search_index_memory_estimate_reports_duplicate_search_text_storage() {
        let fixture = SyntheticLargeIndexFixture::new(1_000);
        let search_index = SearchIndex::from_entries(fixture.entries);
        let estimate = search_index.memory_estimate();

        assert_eq!(estimate.entry_count, 1_000);
        assert!(estimate.entry_string_bytes > 0);
        assert!(estimate.cached_search_text_bytes > 0);
        assert!(
            estimate.cached_search_text_bytes >= estimate.entry_string_bytes / 4,
            "cached search text should be visible in memory estimates: {estimate:?}"
        );
    }

    #[test]
    fn content_path_lookup_is_absent_without_content_index() {
        let entries: Vec<_> = (0..20_000)
            .map(|index| file_entry(&format!("/tmp/content-memory-{index:05}.md")))
            .collect();
        let search_index = SearchIndex::from_entries(entries);
        let estimate = search_index.memory_estimate();

        assert_eq!(estimate.path_lookup_entry_count, 0);
        assert_eq!(estimate.path_lookup_bytes, 0);
        assert_eq!(search_index.content_entry_lookup_count(), 0);
    }

    #[test]
    fn content_path_lookup_tracks_attach_update_detach_and_replace() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        let first = root.join("first.md");
        let skipped = root.join("skipped.bin");
        let second = root.join("second.md");
        fs::write(&first, "first needle").unwrap();
        fs::write(&skipped, b"binary\0payload").unwrap();
        fs::write(&second, "second needle").unwrap();
        let options = ContentIndexOptions {
            index_dir: root.join("tantivy-lifecycle"),
            max_file_bytes: DEFAULT_MAX_CONTENT_BYTES,
        };
        let mut entries = vec![
            file_entry(&first.to_string_lossy()),
            file_entry(&skipped.to_string_lossy()),
        ];
        let content_index = ContentIndex::build(&mut entries, options.clone()).unwrap();
        assert_eq!(entries[0].content_index_state, ContentIndexState::Indexed);
        assert_eq!(
            entries[1].content_index_state,
            ContentIndexState::SkippedBinary
        );
        let mut search_index = SearchIndex::from_entries(entries);

        assert_eq!(search_index.content_entry_lookup_count(), 0);
        search_index.attach_content_index(content_index);
        assert_eq!(search_index.content_entry_lookup_count(), 1);
        assert!(search_index.content_entry_lookup_contains(&first.to_string_lossy()));
        assert!(!search_index.content_entry_lookup_contains(&skipped.to_string_lossy()));

        search_index.apply_update_batch_with_content_options(
            &crate::core::index_watcher::IndexUpdateBatch {
                changed_paths: vec![second.clone()],
                removed_paths: vec![first.clone()],
            },
            vec![file_entry(&second.to_string_lossy())],
            Some(&options),
        );

        assert_eq!(search_index.content_entry_lookup_count(), 1);
        assert!(!search_index.content_entry_lookup_contains(&first.to_string_lossy()));
        assert!(search_index.content_entry_lookup_contains(&second.to_string_lossy()));

        let content_index = search_index.detach_content_index().unwrap();
        assert_eq!(search_index.content_entry_lookup_count(), 0);
        assert_eq!(search_index.memory_estimate().path_lookup_bytes, 0);

        search_index.attach_content_index(content_index);
        assert_eq!(search_index.content_entry_lookup_count(), 1);
        search_index.replace_entries(vec![file_entry("/tmp/replaced.md")]);
        assert_eq!(search_index.content_entry_lookup_count(), 0);
        assert!(search_index.content_index.is_none());
    }

    #[test]
    fn visibility_filter_precedes_normal_query_limit() {
        let index = SearchIndex::from_entries(vec![
            file_entry("/tmp/hidden-item.md"),
            file_entry("/tmp/visible-item.md"),
        ]);
        let parser = crate::core::search::QueryParser::new(Default::default());

        let results = index.search_with_limit_visible(&parser.parse("item"), 1, |entry| {
            entry.name.starts_with("visible")
        });

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "visible-item.md");
    }

    #[test]
    fn visibility_filter_precedes_regex_query_limit() {
        let index = SearchIndex::from_entries(vec![
            file_entry("/tmp/hidden-item.md"),
            file_entry("/tmp/visible-item.md"),
        ]);
        let parser = crate::core::search::QueryParser::new(Default::default());

        let results = index.search_with_limit_visible(&parser.parse("re:item"), 1, |entry| {
            entry.name.starts_with("visible")
        });

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "visible-item.md");
    }

    #[test]
    fn visibility_filter_precedes_content_only_query_limit() {
        let root = temp_dir("content-visibility");
        fs::create_dir_all(&root).unwrap();
        let hidden = root.join("hidden.md");
        let visible = root.join("visible.md");
        fs::write(&hidden, "needle needle needle needle").unwrap();
        fs::write(&visible, "needle").unwrap();
        let mut entries = vec![
            file_entry(&hidden.to_string_lossy()),
            file_entry(&visible.to_string_lossy()),
        ];
        let content_index = ContentIndex::build(
            &mut entries,
            ContentIndexOptions {
                index_dir: root.join("tantivy-content"),
                max_file_bytes: DEFAULT_MAX_CONTENT_BYTES,
            },
        )
        .unwrap();
        let index = SearchIndex::from_entries_with_content_index(entries, content_index);
        let parser = crate::core::search::QueryParser::new(Default::default());

        let results =
            index.search_with_limit_visible(&parser.parse("content:needle"), 1, |entry| {
                entry.name == "visible.md"
            });

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "visible.md");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn content_only_visibility_filter_precedes_tantivy_top_docs_cutoff() {
        let (_workspace, index) = large_visibility_content_fixture("content-only-top-docs");
        let parser = crate::core::search::QueryParser::new(Default::default());
        index.reset_content_query_probe_count();

        let results =
            index.search_with_limit_visible(&parser.parse("content:needle"), 1, |entry| {
                entry.name == "shared-visible.md"
            });

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "shared-visible.md");
        assert_eq!(index.last_content_collector_limit(), 10);
        assert_eq!(index.content_entry_lookup_probe_count(), 1);
    }

    #[test]
    fn mixed_content_visibility_filter_precedes_tantivy_top_docs_cutoff() {
        let (_workspace, index) = large_visibility_content_fixture("mixed-top-docs");
        let parser = crate::core::search::QueryParser::new(Default::default());
        index.reset_content_query_probe_count();

        let results =
            index.search_with_limit_visible(&parser.parse("shared content:needle"), 1, |entry| {
                entry.name == "shared-visible.md"
            });

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "shared-visible.md");
        assert_eq!(index.last_content_collector_limit(), 10);
        assert_eq!(index.content_entry_lookup_probe_count(), 0);
    }

    #[test]
    fn content_only_default_visibility_does_not_scan_entries() {
        let (_workspace, index) = large_visibility_content_fixture("default-content-visibility");
        let parser = crate::core::search::QueryParser::new(Default::default());
        index.reset_content_visibility_entry_scan_count();

        let results = index.search_with_limit(&parser.parse("content:needle"), 1);

        assert_eq!(results.len(), 1);
        assert_eq!(index.content_visibility_entry_scan_count(), 0);
    }

    #[test]
    fn update_batch_rebuilds_full_compact_candidate_index() {
        let mut index = SearchIndex::from_entries(vec![file_entry("/tmp/base.md")]);
        let builds_before_update = CompactCandidateIndex::build_count();
        let added = file_entry("/tmp/added.md");

        index.apply_update_batch(
            &crate::core::index_watcher::IndexUpdateBatch {
                changed_paths: vec![PathBuf::from(&added.path)],
                removed_paths: Vec::new(),
            },
            vec![added],
        );

        assert!(CompactCandidateIndex::build_count() > builds_before_update);
    }

    #[test]
    fn compact_candidate_retriever_covers_linear_search_oracle_results() {
        let fixture = SyntheticLargeIndexFixture::new(10_000);
        let parser = crate::core::search::QueryParser::new(Default::default());
        let linear_index = SearchIndex::from_entries(fixture.entries.clone());
        let compact_index = crate::core::compact_index::CompactCandidateIndex::from_entries(
            fixture.entries.clone(),
        );

        for query in [
            "agents",
            "agents.m",
            "type:md agents",
            "dir:workspace agents",
        ] {
            let linear_paths: Vec<_> = linear_index
                .search_with_limit(&parser.parse(query), 5)
                .into_iter()
                .filter_map(|result| result.detail)
                .collect();
            let candidate_paths = compact_index.retrieve_query_paths(query);

            for path in linear_paths {
                assert!(
                    candidate_paths.contains(&path),
                    "compact candidates for {query:?} should include linear oracle path {path:?}"
                );
            }
        }
    }

    #[test]
    fn low_hit_large_index_search_uses_bounded_candidate_retrieval() {
        let fixture = SyntheticLargeIndexFixture::new(100_000);
        let parser = crate::core::search::QueryParser::new(Default::default());
        let search_index = SearchIndex::from_entries(fixture.entries);

        let started = Instant::now();
        let results =
            search_index.search_with_limit(&parser.parse("needle-not-present-987654321"), 20);
        let elapsed = started.elapsed();

        assert!(results.is_empty());
        assert!(
            elapsed.as_millis() < 100,
            "low-hit query should use bounded candidate retrieval, took {elapsed:?}"
        );
    }

    #[test]
    fn cancellable_search_stops_before_returning_stale_results() {
        let fixture = SyntheticLargeIndexFixture::new(10_000);
        let parser = crate::core::search::QueryParser::new(Default::default());
        let search_index = SearchIndex::from_entries(fixture.entries);
        let checks = AtomicUsize::new(0);

        let results =
            search_index.search_with_limit_cancellable(&parser.parse("project"), 20, || {
                checks.fetch_add(1, Ordering::Relaxed) >= 2
            });

        assert!(results.is_none());
        assert!(checks.load(Ordering::Relaxed) >= 3);
    }

    #[test]
    fn ci_scale_large_index_search_stays_within_latency_budget() {
        let fixture = SyntheticLargeIndexFixture::new(100_000);
        let parser = crate::core::search::QueryParser::new(Default::default());
        let search_index = SearchIndex::from_entries(fixture.entries);

        for query in large_index_benchmark_queries().into_iter().filter(|query| {
            matches!(
                query.kind,
                BenchmarkQueryKind::Exact
                    | BenchmarkQueryKind::Prefix
                    | BenchmarkQueryKind::FieldFiltered
                    | BenchmarkQueryKind::LowHit
            )
        }) {
            let started = Instant::now();
            let results = search_index.search_with_limit(&parser.parse(query.query), 20);
            let elapsed = started.elapsed();

            assert!(
                elapsed.as_millis() < 150,
                "CI-scale query {} took {elapsed:?}, expected compact candidate budget",
                query.name
            );
            if query.expected_agents_top_five {
                assert!(
                    results.iter().take(5).any(|result| {
                        result.detail.as_deref() == Some(fixture.agents_path.as_str())
                    }),
                    "query {} should keep AGENTS.md in top five",
                    query.name
                );
            }
        }
    }

    #[test]
    #[ignore = "2,000,000 entry performance threshold for local release-build and Windows validation"]
    fn two_million_entry_search_stays_within_latency_budget() {
        let fixture = SyntheticLargeIndexFixture::new(2_000_000);
        let parser = crate::core::search::QueryParser::new(Default::default());
        let search_index = SearchIndex::from_entries(fixture.entries);

        for query in large_index_benchmark_queries().into_iter().filter(|query| {
            matches!(
                query.kind,
                BenchmarkQueryKind::Exact
                    | BenchmarkQueryKind::Prefix
                    | BenchmarkQueryKind::FieldFiltered
                    | BenchmarkQueryKind::LowHit
            )
        }) {
            let started = Instant::now();
            let results = search_index.search_with_limit(&parser.parse(query.query), 20);
            let elapsed = started.elapsed();

            println!(
                "QUICKFOX_LARGE_INDEX_THRESHOLD scale=2000000 query={} kind={:?} elapsed_us={} results={}",
                query.name,
                query.kind,
                elapsed.as_micros(),
                results.len()
            );
            assert!(
                elapsed.as_millis() < 250,
                "2,000,000-entry query {} took {elapsed:?}, expected compact candidate budget",
                query.name
            );
        }
    }

    #[test]
    #[ignore = "prints machine-dependent large-index baseline timings; set QUICKFOX_LARGE_INDEX_SCALE=2000000 for the full target scale"]
    fn synthetic_large_index_baseline_reports_current_linear_search_characteristics() {
        let scale = std::env::var("QUICKFOX_LARGE_INDEX_SCALE")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(100_000);
        let fixture = SyntheticLargeIndexFixture::new(scale);
        let parser = crate::core::search::QueryParser::new(Default::default());
        let search_index = SearchIndex::from_entries(fixture.entries);
        let memory_estimate = search_index.memory_estimate();

        for query in large_index_benchmark_queries() {
            let started = Instant::now();
            let results = search_index.search_with_limit(&parser.parse(query.query), 20);
            let elapsed = started.elapsed();
            let target_position = results
                .iter()
                .position(|result| result.detail.as_deref() == Some(fixture.agents_path.as_str()))
                .map(|position| position as i64)
                .unwrap_or(-1);

            println!(
                "QUICKFOX_LARGE_INDEX_BASELINE scale={} query={} kind={:?} elapsed_us={} results={} target_position={} linear_entries={} entry_struct_bytes={} entry_string_bytes={} cached_search_text_bytes={}",
                scale,
                query.name,
                query.kind,
                elapsed.as_micros(),
                results.len(),
                target_position,
                search_index.entries().len(),
                memory_estimate.entry_struct_bytes,
                memory_estimate.entry_string_bytes,
                memory_estimate.cached_search_text_bytes
            );
        }
    }

    #[test]
    fn index_lifecycle_tracks_unbuilt_building_ready_refreshing_and_failed_states() {
        let mut lifecycle = IndexLifecycle::default();

        assert_eq!(lifecycle.status().kind, IndexStatusKind::Unbuilt);

        let first_generation = lifecycle.start_refresh(false);
        assert_eq!(lifecycle.status().kind, IndexStatusKind::Building);

        lifecycle.complete_refresh(first_generation, 42, 100);
        assert_eq!(lifecycle.status().kind, IndexStatusKind::Ready);
        assert_eq!(lifecycle.status().entry_count, 42);
        assert_eq!(lifecycle.status().completed_at_ms, Some(100));

        let second_generation = lifecycle.start_refresh(true);
        assert_eq!(lifecycle.status().kind, IndexStatusKind::Refreshing);
        assert_eq!(lifecycle.status().entry_count, 42);

        lifecycle.fail_refresh(second_generation, "permission denied".to_owned());
        assert_eq!(lifecycle.status().kind, IndexStatusKind::Failed);
        assert_eq!(lifecycle.status().entry_count, 42);
        assert_eq!(
            lifecycle.status().message.as_deref(),
            Some("permission denied")
        );
    }

    #[test]
    fn index_lifecycle_ignores_stale_refresh_completion() {
        let mut lifecycle = IndexLifecycle::default();

        let stale_generation = lifecycle.start_refresh(false);
        let current_generation = lifecycle.start_refresh(false);
        lifecycle.complete_refresh(stale_generation, 10, 100);

        assert_eq!(lifecycle.status().kind, IndexStatusKind::Building);
        assert_eq!(lifecycle.status().entry_count, 0);

        lifecycle.complete_refresh(current_generation, 20, 200);
        assert_eq!(lifecycle.status().kind, IndexStatusKind::Ready);
        assert_eq!(lifecycle.status().entry_count, 20);
    }

    #[test]
    fn entry_and_status_types_are_available_from_entry_module_and_index_facade() {
        let entry_from_entry_module = crate::core::index_entry::IndexedEntry {
            path: "/tmp/report.md".to_owned(),
            name: "report.md".to_owned(),
            kind: crate::core::index_entry::IndexedEntryKind::File,
            ..crate::core::index_entry::IndexedEntry::legacy(
                "",
                "",
                crate::core::index_entry::IndexedEntryKind::File,
            )
        };
        let entry_from_index_facade: IndexedEntry = entry_from_entry_module.clone();

        assert_eq!(entry_from_index_facade, entry_from_entry_module);
        assert_eq!(
            IndexStatusKind::Ready,
            crate::core::index_entry::IndexStatusKind::Ready
        );
    }

    #[test]
    #[ignore = "baseline fixture prints machine-dependent scanner/query timings"]
    fn indexing_baseline_fixture_reports_current_scanner_characteristics() {
        let root = temp_dir("index-baseline");
        let small = root.join("small");
        let deep = root.join("deep");
        let windows_c = root.join("windows-drives").join("C");
        let windows_d = root.join("windows-drives").join("D");
        let excluded = root.join("excluded");
        let text = root.join("content-text");

        fs::create_dir_all(&small).unwrap();
        fs::write(small.join("report-budget.md"), "ordinary name/path fixture").unwrap();
        fs::write(small.join("notes.txt"), "ordinary notes").unwrap();

        let mut current = deep.clone();
        for level in 0..12 {
            current = current.join(format!("level-{level}"));
            fs::create_dir_all(&current).unwrap();
            fs::write(current.join(format!("deep-file-{level}.txt")), "").unwrap();
        }

        fs::create_dir_all(windows_c.join("Users").join("Frank").join("Desktop")).unwrap();
        fs::create_dir_all(windows_d.join("Projects").join("QuickFox")).unwrap();
        fs::write(
            windows_c
                .join("Users")
                .join("Frank")
                .join("Desktop")
                .join("desktop-report.txt"),
            "",
        )
        .unwrap();
        fs::write(
            windows_d
                .join("Projects")
                .join("QuickFox")
                .join("workspace-readme.md"),
            "",
        )
        .unwrap();

        let mut exclude_dirs = Vec::new();
        for index in 0..32 {
            let dir = excluded.join(format!("node_modules_{index}"));
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("ignored.js"), "").unwrap();
            exclude_dirs.push(dir);
        }

        fs::create_dir_all(&text).unwrap();
        fs::write(
            text.join("body.txt"),
            "alpha\nbeta\nneedle-from-file-content-only\ngamma\n",
        )
        .unwrap();

        let scan_started = Instant::now();
        let report = IndexScanner
            .scan(IndexScanOptions {
                include_dirs: vec![root.clone()],
                exclude_dirs,
                exclude_patterns: vec!["*.log".to_owned()],
                respect_project_ignores: true,
            })
            .unwrap();
        let scan_duration = scan_started.elapsed();

        let index = SearchIndex::from_entries(report.entries.clone());
        let parser = crate::core::search::QueryParser::new(Default::default());

        SearchIndex::reset_clone_count();
        let query_started = Instant::now();
        let ordinary_results = index.search(&parser.parse("report"));
        let query_duration = query_started.elapsed();
        let content_results = index.search(&parser.parse("content:needle-from-file-content-only"));

        println!(
            "QUICKFOX_INDEX_BASELINE scan_ms={} entries={} failures={} ordinary_query_us={} ordinary_results={} search_index_clones={} content_query_results={}",
            scan_duration.as_millis(),
            report.entries.len(),
            report.failures.len(),
            query_duration.as_micros(),
            ordinary_results.len(),
            SearchIndex::clone_count(),
            content_results.len()
        );

        assert!(report.failures.is_empty());
        assert!(report
            .entries
            .iter()
            .any(|entry| entry.name == "report-budget.md"));
        assert!(report
            .entries
            .iter()
            .any(|entry| entry.name == "deep-file-11.txt"));
        assert!(report
            .entries
            .iter()
            .any(|entry| entry.path.contains("windows-drives")));
        assert!(!report
            .entries
            .iter()
            .any(|entry| entry.name == "ignored.js"));
        assert!(!ordinary_results.is_empty());
        assert_eq!(content_results[0].kind, SearchResultKind::Feedback);

        let _ = fs::remove_dir_all(root);
    }

    fn file_entry(path: &str) -> IndexedEntry {
        IndexedEntry::legacy(
            path,
            path.rsplit('/').next().unwrap().to_owned(),
            IndexedEntryKind::File,
        )
    }

    #[derive(Debug, Clone)]
    struct SyntheticLargeIndexFixture {
        entries: Vec<IndexedEntry>,
        agents_path: String,
    }

    impl SyntheticLargeIndexFixture {
        fn new(entry_count: usize) -> Self {
            assert!(entry_count >= 128);
            let agents_path = "D:\\workspace\\QuickFox\\docs\\AGENTS.md".to_owned();
            let agents_index = (entry_count.saturating_mul(95) / 100).min(entry_count - 1);
            let mut entries = Vec::with_capacity(entry_count);

            for index in 0..entry_count {
                if index == agents_index {
                    entries.push(synthetic_file_entry(&agents_path));
                    continue;
                }

                let drive = if index % 2 == 0 { "C:" } else { "D:" };
                let area = match index % 8 {
                    0 => "Users\\Frank\\Documents",
                    1 => "Users\\Frank\\Downloads",
                    2 => "workspace\\ProjectAlpha",
                    3 => "workspace\\ProjectBeta",
                    4 => "src\\packages",
                    5 => "logs\\archive",
                    6 => "media\\assets",
                    _ => "tools\\cache",
                };
                let extension = match index % 7 {
                    0 => "md",
                    1 => "txt",
                    2 => "rs",
                    3 => "tsx",
                    4 => "json",
                    5 => "pdf",
                    _ => "log",
                };
                let stem = match index % 11 {
                    0 => "project",
                    1 => "report",
                    2 => "notes",
                    3 => "readme",
                    4 => "index",
                    5 => "cache",
                    6 => "module",
                    7 => "config",
                    8 => "asset",
                    9 => "summary",
                    _ => "record",
                };
                entries.push(synthetic_file_entry(&format!(
                    "{drive}\\{area}\\folder-{:04}\\{stem}-{:07}.{extension}",
                    index % 10_000,
                    index
                )));
            }

            Self {
                entries,
                agents_path,
            }
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum BenchmarkQueryKind {
        Exact,
        Prefix,
        FieldFiltered,
        LowHit,
        HighHit,
        PathSegmentFuzzy,
    }

    #[derive(Debug, Clone)]
    struct BenchmarkQuery {
        name: &'static str,
        query: &'static str,
        kind: BenchmarkQueryKind,
        expected_agents_top_five: bool,
    }

    fn large_index_benchmark_queries() -> Vec<BenchmarkQuery> {
        vec![
            BenchmarkQuery {
                name: "agents-exact",
                query: "agents.md",
                kind: BenchmarkQueryKind::Exact,
                expected_agents_top_five: true,
            },
            BenchmarkQuery {
                name: "agents-prefix-extension",
                query: "agents.m",
                kind: BenchmarkQueryKind::Prefix,
                expected_agents_top_five: true,
            },
            BenchmarkQuery {
                name: "agents-prefix",
                query: "agents",
                kind: BenchmarkQueryKind::Prefix,
                expected_agents_top_five: true,
            },
            BenchmarkQuery {
                name: "agents-type-md",
                query: "type:md agents",
                kind: BenchmarkQueryKind::FieldFiltered,
                expected_agents_top_five: true,
            },
            BenchmarkQuery {
                name: "agents-dir-workspace",
                query: "dir:workspace agents",
                kind: BenchmarkQueryKind::FieldFiltered,
                expected_agents_top_five: true,
            },
            BenchmarkQuery {
                name: "low-hit-random",
                query: "needle-not-present-987654321",
                kind: BenchmarkQueryKind::LowHit,
                expected_agents_top_five: false,
            },
            BenchmarkQuery {
                name: "path-segment-fuzzy",
                query: "wrkspc",
                kind: BenchmarkQueryKind::PathSegmentFuzzy,
                expected_agents_top_five: false,
            },
            BenchmarkQuery {
                name: "high-hit-project",
                query: "project",
                kind: BenchmarkQueryKind::HighHit,
                expected_agents_top_five: false,
            },
        ]
    }

    fn synthetic_file_entry(path: &str) -> IndexedEntry {
        let name = path.rsplit(['/', '\\']).next().unwrap_or(path).to_owned();
        let mut entry = IndexedEntry::legacy(path, name, IndexedEntryKind::File);
        entry.parent = path
            .rsplit_once(['/', '\\'])
            .map(|(parent, _)| parent.to_owned())
            .unwrap_or_default();
        entry.extension = entry
            .name
            .rsplit_once('.')
            .map(|(_, extension)| extension.to_ascii_lowercase());
        entry.root = path
            .split(['/', '\\'])
            .next()
            .unwrap_or_default()
            .to_owned();
        entry.depth = path.split(['/', '\\']).count().saturating_sub(1);
        entry
    }

    struct RankedContentExtractor;

    impl TextExtractor for RankedContentExtractor {
        fn extract(&self, path: &std::path::Path, _max_bytes: u64) -> ContentExtractionResult {
            if path
                .file_name()
                .is_some_and(|name| name == "shared-visible.md")
            {
                ContentExtractionResult::Text("needle".to_owned())
            } else {
                ContentExtractionResult::Text("needle ".repeat(32))
            }
        }
    }

    fn large_visibility_content_fixture(label: &str) -> (tempfile::TempDir, SearchIndex) {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        let mut entries: Vec<_> = (0..10_001)
            .map(|index| {
                file_entry(
                    &root
                        .join(format!("shared-hidden-{index:05}.md"))
                        .to_string_lossy(),
                )
            })
            .collect();
        entries.push(file_entry(
            &root.join("shared-visible.md").to_string_lossy(),
        ));
        let content_index = ContentIndex::build_with_extractor(
            &mut entries,
            ContentIndexOptions {
                index_dir: root.join(format!("tantivy-{label}")),
                max_file_bytes: DEFAULT_MAX_CONTENT_BYTES,
            },
            &RankedContentExtractor,
        )
        .unwrap();

        (
            workspace,
            SearchIndex::from_entries_with_content_index(entries, content_index),
        )
    }

    fn temp_dir(label: &str) -> std::path::PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("quickfox-{label}-{suffix}"))
    }
}
