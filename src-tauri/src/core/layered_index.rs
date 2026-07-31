//! Baseline, delta overlay, and tombstone composition for runtime file search.

use crate::core::content_index::{ContentIndex, ContentPathFilter};
use crate::core::index::{FileSearchIndex, IndexedEntry, IndexedEntryKind, SearchIndex};
use crate::core::index_entry::{normalize_path_key, normalize_path_text_key};
use crate::core::search::{QueryRequest, SearchResult};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommittedIndexDelta {
    pub generation: u64,
    pub upserts: Vec<IndexedEntry>,
    pub removals: Vec<PathBuf>,
}

#[derive(Debug, Default)]
struct PathTombstones {
    exact: BTreeSet<String>,
    directories: BTreeSet<String>,
    #[cfg(test)]
    lookup_probe_count: AtomicUsize,
}

impl PathTombstones {
    fn insert_exact(&mut self, key: String) {
        if !self.directory_ancestor_contains(&key) {
            self.exact.insert(key);
        }
    }

    fn insert_directory(&mut self, key: String) {
        if self.directory_ancestor_contains(&key) {
            return;
        }
        remove_path_scope(&mut self.exact, &key);
        remove_path_scope(&mut self.directories, &key);
        self.directories.insert(key);
    }

    fn remove_exact(&mut self, key: &str) {
        self.exact.remove(key);
    }

    fn contains(&self, key: &str) -> bool {
        self.exact.contains(key) || self.directory_ancestor_contains(key)
    }

    fn contains_directory(&self, key: &str) -> bool {
        self.directories.contains(key)
    }

    fn len(&self) -> usize {
        self.exact.len().saturating_add(self.directories.len())
    }

    fn estimated_bytes(&self) -> usize {
        std::mem::size_of::<Self>().saturating_add(
            self.exact
                .iter()
                .chain(&self.directories)
                .map(|path| {
                    path.capacity()
                        .saturating_add(std::mem::size_of::<String>())
                        .saturating_add(std::mem::size_of::<usize>().saturating_mul(3))
                })
                .sum(),
        )
    }

    fn clear(&mut self) {
        self.exact.clear();
        self.directories.clear();
    }

    fn directory_ancestor_contains(&self, key: &str) -> bool {
        let mut candidate = Some(key);
        while let Some(path) = candidate {
            #[cfg(test)]
            self.lookup_probe_count.fetch_add(1, Ordering::Relaxed);
            if self.directories.contains(path) {
                return true;
            }
            candidate = parent_key(path);
        }
        false
    }

    #[cfg(test)]
    fn reset_lookup_probe_count(&self) {
        self.lookup_probe_count.store(0, Ordering::Relaxed);
    }

    #[cfg(test)]
    fn lookup_probe_count(&self) -> usize {
        self.lookup_probe_count.load(Ordering::Relaxed)
    }
}

#[derive(Debug, Default)]
struct ContentVisibilitySnapshot {
    overlay_paths: BTreeSet<String>,
    exact_tombstones: BTreeSet<String>,
    directory_tombstones: BTreeSet<String>,
}

impl ContentVisibilitySnapshot {
    fn from_delta(
        overlay_entries: &BTreeMap<String, IndexedEntry>,
        tombstones: &PathTombstones,
    ) -> Self {
        Self {
            overlay_paths: overlay_entries.keys().cloned().collect(),
            exact_tombstones: tombstones.exact.clone(),
            directory_tombstones: tombstones.directories.clone(),
        }
    }

    fn path_filter(snapshot: &Arc<Self>) -> ContentPathFilter {
        let snapshot = Arc::clone(snapshot);
        Arc::new(move |path: &str| snapshot.is_visible(path))
    }

    fn overlay_path_filter(snapshot: &Arc<Self>) -> ContentPathFilter {
        let snapshot = Arc::clone(snapshot);
        Arc::new(move |path: &str| {
            snapshot
                .overlay_paths
                .contains(&normalize_path_text_key(path))
        })
    }

    fn is_visible(&self, path: &str) -> bool {
        let key = normalize_path_text_key(path);
        if self.overlay_paths.contains(&key) || self.exact_tombstones.contains(&key) {
            return false;
        }

        let mut candidate = Some(key.as_str());
        while let Some(path) = candidate {
            if self.directory_tombstones.contains(path) {
                return false;
            }
            candidate = parent_key(path);
        }
        true
    }

    fn estimated_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            .saturating_add(string_set_estimated_bytes(&self.overlay_paths))
            .saturating_add(string_set_estimated_bytes(&self.exact_tombstones))
            .saturating_add(string_set_estimated_bytes(&self.directory_tombstones))
    }
}

#[derive(Debug)]
pub struct LayeredSearchIndex {
    baseline: SearchIndex,
    baseline_by_path: BTreeMap<String, IndexedEntryKind>,
    overlay_entries: BTreeMap<String, IndexedEntry>,
    overlay_generations: BTreeMap<String, u64>,
    overlay: SearchIndex,
    tombstones: PathTombstones,
    content_visibility: Arc<ContentVisibilitySnapshot>,
    generation: u64,
    visible_entry_count: usize,
    #[cfg(test)]
    baseline_probe_count: AtomicUsize,
    #[cfg(test)]
    overlay_compact_build_ids: Vec<usize>,
    #[cfg(test)]
    content_visibility_baseline_scan_count: AtomicUsize,
}

impl Default for LayeredSearchIndex {
    fn default() -> Self {
        Self::from_baseline(Vec::new())
    }
}

impl LayeredSearchIndex {
    pub fn from_baseline(entries: Vec<IndexedEntry>) -> Self {
        Self::from_search_index(SearchIndex::from_entries(entries))
    }

    pub fn from_search_index(baseline: SearchIndex) -> Self {
        let baseline_by_path: BTreeMap<_, _> = baseline
            .entries()
            .iter()
            .map(|entry| (normalize_path_text_key(&entry.path), entry.kind.clone()))
            .collect();
        let visible_entry_count = baseline_by_path.len();
        Self {
            baseline,
            baseline_by_path,
            overlay_entries: BTreeMap::new(),
            overlay_generations: BTreeMap::new(),
            overlay: SearchIndex::default(),
            tombstones: PathTombstones::default(),
            content_visibility: Arc::new(ContentVisibilitySnapshot::default()),
            generation: 0,
            visible_entry_count,
            #[cfg(test)]
            baseline_probe_count: AtomicUsize::new(0),
            #[cfg(test)]
            overlay_compact_build_ids: Vec::new(),
            #[cfg(test)]
            content_visibility_baseline_scan_count: AtomicUsize::new(0),
        }
    }

    pub fn apply_delta(&mut self, delta: CommittedIndexDelta) {
        if delta.generation <= self.generation {
            return;
        }

        let generation = delta.generation;
        for removal in delta.removals {
            let key = normalize_path_key(removal);
            let is_directory = self.path_is_directory(&key);
            let visible_before = self.count_visible_scope(&key, is_directory);
            if is_directory {
                self.overlay_entries
                    .retain(|overlay_key, _| !path_matches(&key, overlay_key));
                self.overlay_generations
                    .retain(|overlay_key, _| !path_matches(&key, overlay_key));
                self.tombstones.insert_directory(key.clone());
            } else {
                self.overlay_entries.remove(&key);
                self.overlay_generations.remove(&key);
                self.tombstones.insert_exact(key.clone());
            }
            let visible_after = self.count_visible_scope(&key, is_directory);
            self.adjust_visible_count(visible_before, visible_after);
        }

        for entry in delta.upserts {
            let key = normalize_path_text_key(&entry.path);
            let replaces_directory =
                entry.kind != IndexedEntryKind::Directory && self.path_is_directory(&key);
            let visible_before = self.count_visible_scope(&key, replaces_directory);
            if replaces_directory {
                self.overlay_entries
                    .retain(|overlay_key, _| !path_is_descendant(&key, overlay_key));
                self.overlay_generations
                    .retain(|overlay_key, _| !path_is_descendant(&key, overlay_key));
                self.tombstones.insert_directory(key.clone());
            }
            self.tombstones.remove_exact(&key);
            self.overlay_entries.insert(key.clone(), entry);
            self.overlay_generations.insert(key.clone(), generation);
            let visible_after = self.count_visible_scope(&key, replaces_directory);
            self.adjust_visible_count(visible_before, visible_after);
        }

        self.overlay = SearchIndex::from_entries(self.overlay_entries.values().cloned().collect());
        self.content_visibility = Arc::new(ContentVisibilitySnapshot::from_delta(
            &self.overlay_entries,
            &self.tombstones,
        ));
        #[cfg(test)]
        {
            self.overlay_compact_build_ids
                .push(self.overlay.compact_build_id());
        }
        self.generation = generation;
    }

    pub(crate) fn content_index_for_delta(&self) -> Option<ContentIndex> {
        self.baseline.content_index_clone()
    }

    pub(crate) fn publish_content_delta(
        &mut self,
        generation: u64,
        updated_entries: &[IndexedEntry],
        content_index: ContentIndex,
    ) -> bool {
        if generation > self.generation {
            return false;
        }
        let mut changed = false;
        for updated in updated_entries {
            let key = normalize_path_text_key(&updated.path);
            let Some(current) = self.overlay_entries.get_mut(&key) else {
                continue;
            };
            if self.overlay_generations.get(&key) != Some(&generation)
                || !same_content_target(current, updated)
                || current.content_index_state == updated.content_index_state
            {
                continue;
            }
            current.content_index_state = updated.content_index_state.clone();
            changed = true;
        }
        if !changed {
            return false;
        }
        self.overlay = SearchIndex::from_entries(self.overlay_entries.values().cloned().collect());
        self.overlay.attach_content_index(content_index);
        true
    }

    pub fn replace_baseline(&mut self, entries: Vec<IndexedEntry>, generation: u64) {
        self.replace_baseline_search_index(SearchIndex::from_entries(entries), generation);
    }

    pub fn replace_baseline_search_index(&mut self, baseline: SearchIndex, generation: u64) {
        if generation < self.generation {
            return;
        }
        self.install_baseline(baseline, generation);
    }

    pub fn replace_baseline_with_authoritative_tail(
        &mut self,
        baseline: SearchIndex,
        baseline_generation: u64,
        tail: &[CommittedIndexDelta],
    ) -> bool {
        let mut expected = baseline_generation.saturating_add(1);
        for delta in tail {
            if delta.generation != expected {
                return false;
            }
            expected = expected.saturating_add(1);
        }
        let authoritative_generation = tail
            .last()
            .map(|delta| delta.generation)
            .unwrap_or(baseline_generation);
        if authoritative_generation < self.generation {
            return false;
        }
        self.install_baseline(baseline, baseline_generation);
        for delta in tail {
            self.apply_delta(delta.clone());
        }
        true
    }

    fn install_baseline(&mut self, baseline: SearchIndex, generation: u64) {
        self.baseline_by_path = baseline
            .entries()
            .iter()
            .map(|entry| (normalize_path_text_key(&entry.path), entry.kind.clone()))
            .collect();
        self.visible_entry_count = self.baseline_by_path.len();
        self.baseline = baseline;
        self.overlay_entries.clear();
        self.overlay_generations.clear();
        self.overlay = SearchIndex::default();
        self.tombstones.clear();
        self.content_visibility = Arc::new(ContentVisibilitySnapshot::default());
        self.generation = generation;
    }

    pub fn watched_roots(&self) -> Vec<PathBuf> {
        self.baseline
            .entries()
            .iter()
            .chain(self.overlay_entries.values())
            .filter(|entry| !entry.root.is_empty())
            .map(|entry| PathBuf::from(&entry.root))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub fn materialized_entries(&self) -> Vec<IndexedEntry> {
        let mut entries: BTreeMap<String, IndexedEntry> = self
            .baseline
            .entries()
            .iter()
            .filter(|entry| self.baseline_entry_is_visible(entry))
            .cloned()
            .map(|entry| (normalize_path_text_key(&entry.path), entry))
            .collect();
        entries.extend(self.overlay_entries.clone());
        entries.into_values().collect()
    }

    #[cfg(test)]
    pub fn entries(&self) -> &[IndexedEntry] {
        self.baseline.entries()
    }

    pub fn search(&self, query: &QueryRequest, candidate_budget: usize) -> Vec<SearchResult> {
        if candidate_budget == 0 {
            return Vec::new();
        }

        let baseline_results = self.baseline.search_with_limit_visible_and_content_filter(
            query,
            candidate_budget,
            ContentVisibilitySnapshot::path_filter(&self.content_visibility),
            |entry| self.baseline_entry_is_visible(entry),
        );
        let baseline_has_content_feedback = baseline_results
            .iter()
            .any(|result| result.id == "feedback:content-index-unavailable");
        let overlay_results = self.overlay.search_with_limit_visible_and_content_filter(
            query,
            candidate_budget,
            ContentVisibilitySnapshot::overlay_path_filter(&self.content_visibility),
            |_| true,
        );
        let max_results = candidate_budget.saturating_mul(2);
        let mut seen_ids = HashSet::with_capacity(max_results.min(1024));
        let mut merged = Vec::with_capacity(max_results.min(1024));

        for result in baseline_results.into_iter().chain(overlay_results) {
            if result.id == "feedback:content-index-unavailable" && !baseline_has_content_feedback {
                continue;
            }
            if seen_ids.insert(result.id.clone()) {
                merged.push(result);
                if merged.len() >= max_results {
                    break;
                }
            }
        }
        merged
    }

    pub fn entry_count(&self) -> usize {
        self.visible_entry_count
    }

    pub fn baseline_entry_count(&self) -> usize {
        self.baseline.entries().len()
    }

    pub fn overlay_entry_count(&self) -> usize {
        self.overlay_entries.len()
    }

    pub fn tombstone_entry_count(&self) -> usize {
        self.tombstones.len()
    }

    pub fn delta_entry_count(&self) -> usize {
        self.overlay_entry_count()
            .saturating_add(self.tombstone_entry_count())
    }

    pub fn estimated_delta_bytes(&self) -> usize {
        let overlay_bytes: usize = self
            .overlay_entries
            .iter()
            .map(|(key, entry)| {
                key.capacity()
                    .saturating_add(std::mem::size_of::<String>())
                    .saturating_add(std::mem::size_of::<IndexedEntry>())
                    .saturating_add(std::mem::size_of::<usize>().saturating_mul(3))
                    .saturating_add(indexed_entry_string_bytes(entry))
            })
            .sum::<usize>()
            .saturating_add(std::mem::size_of::<BTreeMap<String, IndexedEntry>>());
        let overlay_index = self.overlay.memory_estimate();
        overlay_bytes
            .saturating_add(
                self.overlay_generations
                    .keys()
                    .map(|path| {
                        path.capacity()
                            .saturating_add(std::mem::size_of::<(String, u64)>())
                    })
                    .sum::<usize>()
                    .saturating_add(std::mem::size_of::<BTreeMap<String, u64>>()),
            )
            .saturating_add(overlay_index.total_resident_bytes())
            .saturating_add(self.tombstones.estimated_bytes())
            .saturating_add(self.content_visibility.estimated_bytes())
    }

    pub fn estimated_total_resident_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            .saturating_add(self.baseline.memory_estimate().total_resident_bytes())
            .saturating_add(self.estimated_baseline_path_metadata_bytes())
            .saturating_add(self.estimated_delta_bytes())
    }

    pub fn estimated_baseline_path_metadata_bytes(&self) -> usize {
        let entries: usize = self
            .baseline_by_path
            .keys()
            .map(|path| {
                path.capacity()
                    .saturating_add(std::mem::size_of::<(String, IndexedEntryKind)>())
                    .saturating_add(std::mem::size_of::<usize>().saturating_mul(3))
            })
            .sum();
        std::mem::size_of::<BTreeMap<String, IndexedEntryKind>>().saturating_add(entries)
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    #[cfg(test)]
    pub fn baseline_build_count(&self) -> usize {
        self.baseline.compact_build_id()
    }

    #[cfg(test)]
    fn baseline_compact_build_id(&self) -> usize {
        self.baseline.compact_build_id()
    }

    #[cfg(test)]
    fn overlay_compact_build_id(&self) -> usize {
        self.overlay.compact_build_id()
    }

    #[cfg(test)]
    fn overlay_compact_build_ids(&self) -> &[usize] {
        &self.overlay_compact_build_ids
    }

    fn baseline_entry_is_visible(&self, entry: &IndexedEntry) -> bool {
        #[cfg(test)]
        self.content_visibility_baseline_scan_count
            .fetch_add(1, Ordering::Relaxed);
        let key = normalize_path_text_key(&entry.path);
        !self.overlay_entries.contains_key(&key) && !self.tombstones.contains(&key)
    }

    fn path_is_directory(&self, key: &str) -> bool {
        if self.tombstones.contains_directory(key) {
            return true;
        }
        if self
            .overlay_entries
            .get(key)
            .is_some_and(|entry| entry.kind == IndexedEntryKind::Directory)
        {
            return true;
        }
        self.record_baseline_probe();
        if self
            .baseline_by_path
            .get(key)
            .is_some_and(|kind| *kind == IndexedEntryKind::Directory)
        {
            return true;
        }

        if map_has_descendant(&self.overlay_entries, key) {
            return true;
        }
        self.record_baseline_probe();
        map_has_descendant(&self.baseline_by_path, key)
    }

    fn count_visible_scope(&self, key: &str, subtree: bool) -> usize {
        let baseline_count = self
            .baseline_scope_keys(key, subtree)
            .filter(|path| {
                !self.overlay_entries.contains_key(*path) && !self.tombstones.contains(path)
            })
            .count();
        let overlay_count = map_scope_keys(&self.overlay_entries, key, subtree).count();
        baseline_count.saturating_add(overlay_count)
    }

    fn baseline_scope_keys<'a>(
        &'a self,
        key: &'a str,
        subtree: bool,
    ) -> impl Iterator<Item = &'a String> {
        map_scope_keys(&self.baseline_by_path, key, subtree).inspect(|_| {
            self.record_baseline_probe();
        })
    }

    fn adjust_visible_count(&mut self, before: usize, after: usize) {
        self.visible_entry_count = self
            .visible_entry_count
            .saturating_sub(before)
            .saturating_add(after);
    }

    #[cfg(test)]
    fn reset_baseline_probe_count(&self) {
        self.baseline_probe_count.store(0, Ordering::Relaxed);
    }

    #[cfg(test)]
    fn baseline_probe_count(&self) -> usize {
        self.baseline_probe_count.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    fn reset_content_visibility_baseline_scan_count(&self) {
        self.content_visibility_baseline_scan_count
            .store(0, Ordering::Relaxed);
    }

    #[cfg(test)]
    fn content_visibility_baseline_scan_count(&self) -> usize {
        self.content_visibility_baseline_scan_count
            .load(Ordering::Relaxed)
    }

    #[cfg(test)]
    fn content_visibility_snapshot_id(&self) -> usize {
        Arc::as_ptr(&self.content_visibility) as usize
    }

    fn record_baseline_probe(&self) {
        #[cfg(test)]
        self.baseline_probe_count.fetch_add(1, Ordering::Relaxed);
    }
}

fn same_content_target(current: &IndexedEntry, updated: &IndexedEntry) -> bool {
    current.path == updated.path
        && current.kind == updated.kind
        && current.root == updated.root
        && current.modified_ms == updated.modified_ms
        && current.size_bytes == updated.size_bytes
}

impl FileSearchIndex for LayeredSearchIndex {
    fn search_files(&self, query: &QueryRequest, limit: usize) -> Vec<SearchResult> {
        self.search(query, limit)
    }

    fn indexed_entry_count(&self) -> usize {
        self.entry_count()
    }
}

fn path_matches(root: &str, candidate: &str) -> bool {
    root == candidate || path_is_descendant(root, candidate)
}

fn path_is_descendant(root: &str, candidate: &str) -> bool {
    if root.ends_with('/') {
        candidate.starts_with(root) && candidate.len() > root.len()
    } else {
        candidate
            .strip_prefix(root)
            .is_some_and(|rest| rest.starts_with('/'))
    }
}

fn parent_key(path: &str) -> Option<&str> {
    let separator = path.rfind('/')?;
    if separator == 0 {
        (path != "/").then_some("/")
    } else {
        Some(&path[..separator])
    }
}

fn descendant_prefix(key: &str) -> String {
    if key.ends_with('/') {
        key.to_owned()
    } else {
        format!("{key}/")
    }
}

fn map_has_descendant<V>(map: &BTreeMap<String, V>, key: &str) -> bool {
    let prefix = descendant_prefix(key);
    map.range(prefix.clone()..)
        .next()
        .is_some_and(|(path, _)| path.starts_with(&prefix))
}

fn map_scope_keys<'a, V>(
    map: &'a BTreeMap<String, V>,
    key: &'a str,
    subtree: bool,
) -> impl Iterator<Item = &'a String> {
    let exact = map.get_key_value(key).map(|(path, _)| path);
    let prefix = descendant_prefix(key);
    let descendants = subtree
        .then(|| {
            map.range(prefix.clone()..)
                .take_while(move |(path, _)| path.starts_with(&prefix))
                .map(|(path, _)| path)
        })
        .into_iter()
        .flatten();
    exact.into_iter().chain(descendants)
}

fn remove_path_scope(paths: &mut BTreeSet<String>, key: &str) {
    paths.remove(key);
    let prefix = descendant_prefix(key);
    let descendants: Vec<_> = paths
        .range(prefix.clone()..)
        .take_while(|path| path.starts_with(&prefix))
        .cloned()
        .collect();
    for path in descendants {
        paths.remove(&path);
    }
}

fn string_set_estimated_bytes(paths: &BTreeSet<String>) -> usize {
    paths
        .iter()
        .map(|path| {
            path.capacity()
                .saturating_add(std::mem::size_of::<String>())
                .saturating_add(std::mem::size_of::<usize>().saturating_mul(3))
        })
        .sum()
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

#[cfg(test)]
mod tests {
    use super::{CommittedIndexDelta, LayeredSearchIndex, PathTombstones};
    use crate::core::content_index::{
        ContentExtractionResult, ContentIndex, ContentIndexOptions, TextExtractor,
        DEFAULT_MAX_CONTENT_BYTES,
    };
    use crate::core::index::{IndexedEntry, IndexedEntryKind, SearchIndex};
    use crate::core::index_watcher::IndexUpdateBatch;
    use crate::core::search::{HistoryScores, QueryRequest, Ranker, SearchMode};
    use crate::core::storage::SqliteStorage;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    #[test]
    fn overlay_replaces_baseline_by_normalized_path_without_duplicates() {
        let root = PathBuf::from("/tmp/root");
        let old = named_entry(root.join("docs/readme.md"), &root, "old-readme.md");
        let replacement = named_entry(root.join("docs/readme.md"), &root, "new-readme.md");
        let mut index = LayeredSearchIndex::from_baseline(vec![old]);

        index.apply_delta(CommittedIndexDelta {
            generation: 1,
            upserts: vec![replacement],
            removals: Vec::new(),
        });

        assert!(index.search(&request("old-readme"), 20).is_empty());
        let results = index.search(&request("new-readme"), 20);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "new-readme.md");
        assert_eq!(index.entry_count(), 1);
    }

    #[test]
    fn incremental_content_publish_makes_overlay_searchable_and_clears_renamed_document() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        let old_path = root.join("old-name.md");
        let new_path = root.join("new-name.md");
        std::fs::write(&old_path, "stale needle").unwrap();
        let options = ContentIndexOptions {
            index_dir: root.join("content-index"),
            max_file_bytes: DEFAULT_MAX_CONTENT_BYTES,
        };
        let mut baseline_entries = vec![named_entry(old_path.clone(), root, "old-name.md")];
        let content_index = ContentIndex::build(&mut baseline_entries, options.clone()).unwrap();
        let mut index = LayeredSearchIndex::from_search_index(
            SearchIndex::from_entries_with_content_index(baseline_entries, content_index),
        );

        std::fs::rename(&old_path, &new_path).unwrap();
        std::fs::write(&new_path, "fresh haystack").unwrap();
        let upsert = named_entry(new_path.clone(), root, "new-name.md");
        index.apply_delta(CommittedIndexDelta {
            generation: 1,
            upserts: vec![upsert.clone()],
            removals: vec![old_path.clone()],
        });
        assert_eq!(
            index.search(&request("new-name"), 20).len(),
            1,
            "name/path delta is visible before content indexing"
        );
        assert!(index.search(&request("content:haystack"), 20).is_empty());

        let mut content_index = index
            .content_index_for_delta()
            .expect("baseline content index remains available");
        let mut content_upserts = vec![upsert];
        let outcome = content_index.apply_content_delta(
            &mut content_upserts,
            std::slice::from_ref(&old_path),
            &options,
        );
        assert!(outcome.failures.is_empty());
        assert!(index.publish_content_delta(1, &content_upserts, content_index));

        let results = index.search(&request("content:haystack"), 20);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "new-name.md");
        assert!(index.search(&request("content:needle"), 20).is_empty());
        assert_eq!(
            index
                .materialized_entries()
                .into_iter()
                .find(|entry| entry.path == new_path.to_string_lossy())
                .unwrap()
                .content_index_state,
            crate::core::index_entry::ContentIndexState::Indexed
        );
    }

    #[test]
    fn overlay_content_filter_runs_before_tantivy_top_docs_cutoff() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        let hidden_root = root.join("hidden");
        std::fs::create_dir_all(&hidden_root).unwrap();
        let mut baseline_entries = Vec::new();
        for ordinal in 0..60 {
            let path = hidden_root.join(format!("high-{ordinal}.md"));
            std::fs::write(&path, "needle needle needle needle").unwrap();
            baseline_entries.push(named_entry(path, root, &format!("high-{ordinal}.md")));
        }
        let visible = root.join("visible.md");
        std::fs::write(&visible, "needle").unwrap();
        let options = ContentIndexOptions {
            index_dir: root.join("content-index"),
            max_file_bytes: DEFAULT_MAX_CONTENT_BYTES,
        };
        let content_index = ContentIndex::build(&mut baseline_entries, options.clone()).unwrap();
        let mut index = LayeredSearchIndex::from_search_index(
            SearchIndex::from_entries_with_content_index(baseline_entries, content_index),
        );
        let visible_entry = named_entry(visible, root, "visible.md");
        index.apply_delta(CommittedIndexDelta {
            generation: 1,
            upserts: vec![visible_entry.clone()],
            removals: vec![hidden_root],
        });
        let mut delta_index = index.content_index_for_delta().unwrap();
        let mut upserts = vec![visible_entry];
        let outcome = delta_index.apply_content_delta(&mut upserts, &[], &options);
        assert!(outcome.failures.is_empty());
        assert!(index.publish_content_delta(1, &upserts, delta_index));

        let results = index.search(&request("content:needle"), 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "visible.md");
    }

    #[test]
    fn overlay_path_keys_normalize_separator_styles() {
        let root = PathBuf::from("/tmp/root");
        let old = named_entry(root.join("docs/readme.md"), &root, "old-readme.md");
        let replacement = named_entry(
            PathBuf::from(r"\tmp\root\docs\readme.md"),
            &root,
            "new-readme.md",
        );
        let mut index = LayeredSearchIndex::from_baseline(vec![old]);

        index.apply_delta(CommittedIndexDelta {
            generation: 1,
            upserts: vec![replacement],
            removals: Vec::new(),
        });

        assert!(index.search(&request("old-readme"), 20).is_empty());
        assert_eq!(index.search(&request("new-readme"), 20).len(), 1);
        assert_eq!(index.entry_count(), 1);
    }

    #[test]
    fn exact_file_tombstone_does_not_hide_similar_paths() {
        let root = PathBuf::from("/tmp/root");
        let mut index = LayeredSearchIndex::from_baseline(vec![
            entry(root.join("docs/readme.md"), &root, IndexedEntryKind::File),
            entry(
                root.join("docs/readme.md.bak"),
                &root,
                IndexedEntryKind::File,
            ),
        ]);

        index.apply_delta(CommittedIndexDelta {
            generation: 1,
            upserts: Vec::new(),
            removals: vec![root.join("docs/readme.md")],
        });

        let results = index.search(&request("readme"), 20);
        assert_eq!(result_paths(&results), vec!["/tmp/root/docs/readme.md.bak"]);
    }

    #[test]
    fn directory_tombstone_hides_descendants_on_segment_boundaries() {
        let root = PathBuf::from("/tmp/root");
        let mut index = LayeredSearchIndex::from_baseline(vec![
            entry(root.join("docs/readme.md"), &root, IndexedEntryKind::File),
            entry(
                root.join("docs/guides/start.md"),
                &root,
                IndexedEntryKind::File,
            ),
            entry(
                root.join("docs-old/legacy.md"),
                &root,
                IndexedEntryKind::File,
            ),
            entry(root.join("keep.md"), &root, IndexedEntryKind::File),
        ]);

        index.apply_delta(CommittedIndexDelta {
            generation: 1,
            upserts: Vec::new(),
            removals: vec![root.join("docs")],
        });

        assert!(index.search(&request("readme"), 20).is_empty());
        assert!(index.search(&request("start"), 20).is_empty());
        assert_eq!(index.search(&request("legacy"), 20).len(), 1);
        assert_eq!(index.search(&request("keep"), 20).len(), 1);
    }

    #[test]
    fn rename_is_old_path_removal_plus_new_path_upsert() {
        let root = PathBuf::from("/tmp/root");
        let old_path = root.join("old-name.md");
        let new_path = root.join("new-name.md");
        let mut index = LayeredSearchIndex::from_baseline(vec![entry(
            old_path.clone(),
            &root,
            IndexedEntryKind::File,
        )]);

        index.apply_delta(CommittedIndexDelta {
            generation: 1,
            upserts: vec![entry(new_path, &root, IndexedEntryKind::File)],
            removals: vec![old_path],
        });

        assert!(index.search(&request("old-name"), 20).is_empty());
        assert_eq!(index.search(&request("new-name"), 20).len(), 1);
    }

    #[test]
    fn type_replacements_keep_only_the_new_path_shape_visible() {
        let root = PathBuf::from("/tmp/root");
        let node = root.join("node");
        let mut directory_to_file = LayeredSearchIndex::from_baseline(vec![
            entry(node.clone(), &root, IndexedEntryKind::Directory),
            entry(node.join("old-child.md"), &root, IndexedEntryKind::File),
        ]);

        directory_to_file.apply_delta(CommittedIndexDelta {
            generation: 1,
            upserts: vec![entry(node.clone(), &root, IndexedEntryKind::File)],
            removals: Vec::new(),
        });

        assert!(directory_to_file
            .search(&request("old-child"), 20)
            .is_empty());
        assert_eq!(directory_to_file.entry_count(), 1);

        let mut file_to_directory = LayeredSearchIndex::from_baseline(vec![entry(
            node.clone(),
            &root,
            IndexedEntryKind::File,
        )]);
        file_to_directory.apply_delta(CommittedIndexDelta {
            generation: 1,
            upserts: vec![entry(node.clone(), &root, IndexedEntryKind::Directory)],
            removals: Vec::new(),
        });
        file_to_directory.apply_delta(CommittedIndexDelta {
            generation: 2,
            upserts: vec![entry(
                node.join("new-child.md"),
                &root,
                IndexedEntryKind::File,
            )],
            removals: Vec::new(),
        });

        assert_eq!(
            file_to_directory
                .search(&request("node"), 20)
                .into_iter()
                .filter(|result| result.title == "node")
                .count(),
            1
        );
        assert_eq!(file_to_directory.search(&request("new-child"), 20).len(), 1);
        assert_eq!(file_to_directory.entry_count(), 2);
    }

    #[test]
    fn committed_generation_replay_is_idempotent() {
        let root = PathBuf::from("/tmp/root");
        let delta = CommittedIndexDelta {
            generation: 7,
            upserts: vec![entry(
                root.join("journal.md"),
                &root,
                IndexedEntryKind::File,
            )],
            removals: Vec::new(),
        };
        let mut index = LayeredSearchIndex::from_baseline(Vec::new());

        index.apply_delta(delta.clone());
        let snapshot = (
            index.generation(),
            index.entry_count(),
            index.delta_entry_count(),
            index.estimated_delta_bytes(),
        );
        index.apply_delta(delta);

        assert_eq!(
            (
                index.generation(),
                index.entry_count(),
                index.delta_entry_count(),
                index.estimated_delta_bytes(),
            ),
            snapshot
        );
        assert_eq!(index.search(&request("journal"), 20).len(), 1);
    }

    #[test]
    fn replace_baseline_clears_delta_state_at_new_generation() {
        let root = PathBuf::from("/tmp/root");
        let mut index = LayeredSearchIndex::from_baseline(vec![entry(
            root.join("old.md"),
            &root,
            IndexedEntryKind::File,
        )]);
        index.apply_delta(CommittedIndexDelta {
            generation: 2,
            upserts: vec![entry(root.join("delta.md"), &root, IndexedEntryKind::File)],
            removals: vec![root.join("old.md")],
        });

        index.replace_baseline(
            vec![entry(
                root.join("compacted.md"),
                &root,
                IndexedEntryKind::File,
            )],
            2,
        );

        assert_eq!(index.generation(), 2);
        assert_eq!(index.delta_entry_count(), 0);
        assert_eq!(index.entry_count(), 1);
        assert_eq!(index.search(&request("compacted"), 20).len(), 1);
    }

    #[test]
    fn layered_counts_report_baseline_overlay_and_tombstones_separately() {
        let root = PathBuf::from("/tmp/root");
        let mut index = LayeredSearchIndex::from_baseline(vec![
            entry(root.join("kept.md"), &root, IndexedEntryKind::File),
            entry(root.join("removed.md"), &root, IndexedEntryKind::File),
        ]);
        index.apply_delta(CommittedIndexDelta {
            generation: 1,
            upserts: vec![entry(
                root.join("created.md"),
                &root,
                IndexedEntryKind::File,
            )],
            removals: vec![root.join("removed.md")],
        });

        assert_eq!(index.baseline_entry_count(), 2);
        assert_eq!(index.overlay_entry_count(), 1);
        assert_eq!(index.tombstone_entry_count(), 1);
    }

    #[test]
    fn p95_duration_uses_the_nearest_rank_without_averaging_tail_samples() {
        let samples: Vec<_> = (1..=20).map(Duration::from_millis).collect();

        assert_eq!(p95_duration(&samples), Duration::from_millis(19));
    }

    #[test]
    fn ci_scale_layered_benchmark_fixture_covers_create_overwrite_and_subtree_delete() {
        let mut index =
            LayeredSearchIndex::from_baseline(synthetic_layered_benchmark_entries(20_000));
        let baseline_build_id = index.baseline_compact_build_id();

        index.apply_delta(synthetic_layered_benchmark_delta(20_000, 1_000));

        assert_eq!(index.baseline_compact_build_id(), baseline_build_id);
        assert_eq!(index.overlay_entry_count(), 500);
        assert_eq!(index.tombstone_entry_count(), 500);
        for (_, query, expected) in layered_benchmark_queries() {
            assert_benchmark_results(expected, &index.search(&query, 20));
        }
    }

    #[test]
    #[ignore = "2,000,000 baseline plus 10,000 runtime delta release threshold"]
    fn two_million_baseline_with_runtime_delta_stays_within_latency_budget() {
        const BASELINE_ENTRIES: usize = 2_000_000;
        const DELTA_ENTRIES: usize = 10_000;
        const QUERY_ROUNDS: usize = 20;

        let baseline = synthetic_layered_benchmark_entries(BASELINE_ENTRIES);
        let mut index = LayeredSearchIndex::from_baseline(baseline);
        let baseline_build_id = index.baseline_compact_build_id();
        index.apply_delta(synthetic_layered_benchmark_delta(
            BASELINE_ENTRIES,
            DELTA_ENTRIES,
        ));

        assert_eq!(index.baseline_compact_build_id(), baseline_build_id);
        assert_eq!(index.baseline_entry_count(), BASELINE_ENTRIES);
        assert_eq!(index.overlay_entry_count(), DELTA_ENTRIES / 2);
        assert_eq!(index.tombstone_entry_count(), DELTA_ENTRIES / 2);
        assert_eq!(index.delta_entry_count(), DELTA_ENTRIES);

        let queries = layered_benchmark_queries();
        for (_, query, _) in &queries {
            for _ in 0..3 {
                let _ = index.search(query, 20);
            }
        }

        for (name, query, expected) in queries {
            let mut samples = Vec::with_capacity(QUERY_ROUNDS);
            let mut result_count = 0;
            for _ in 0..QUERY_ROUNDS {
                let started = Instant::now();
                let results = index.search(&query, 20);
                samples.push(started.elapsed());
                result_count = results.len();
                assert_benchmark_results(expected, &results);
            }
            let p95 = p95_duration(&samples);
            println!(
                "QUICKFOX_LAYERED_INDEX scale={} delta={} query={} rounds={} p95_us={} results={} baseline={} overlay={} tombstones={} delta_bytes={}",
                BASELINE_ENTRIES,
                DELTA_ENTRIES,
                name,
                QUERY_ROUNDS,
                p95.as_micros(),
                result_count,
                index.baseline_entry_count(),
                index.overlay_entry_count(),
                index.tombstone_entry_count(),
                index.estimated_delta_bytes(),
            );
            assert!(
                p95 <= Duration::from_millis(50),
                "layered query {name} P95 {p95:?} exceeded 50ms"
            );
        }
    }

    #[test]
    #[ignore = "release benchmark for a durable 10,000-entry incremental batch"]
    fn incremental_batch_benchmark_reports_commit_layers_query_p95_and_memory() {
        const BASELINE_ENTRIES: usize = 100_000;
        const DELTA_ENTRIES: usize = 10_000;
        const QUERY_ROUNDS: usize = 20;

        let mut index = LayeredSearchIndex::from_baseline(synthetic_layered_benchmark_entries(
            BASELINE_ENTRIES,
        ));
        let delta = synthetic_layered_benchmark_delta(BASELINE_ENTRIES, DELTA_ENTRIES);
        let database = tempfile::tempdir().unwrap();
        let storage =
            SqliteStorage::open(database.path().join("incremental-batch.sqlite")).unwrap();
        let baseline_id = storage.save_completed_index_batch(1, &[]).unwrap();
        storage
            .activate_baseline_and_clear_incremental_state(baseline_id, 0)
            .unwrap();

        let commit_started = Instant::now();
        storage.commit_incremental_batch(&delta, &[], &[]).unwrap();
        let journal_commit = commit_started.elapsed();
        let apply_started = Instant::now();
        index.apply_delta(delta);
        let layer_apply = apply_started.elapsed();
        let batch_commit = commit_started.elapsed();

        let queries = layered_benchmark_queries();
        for (_, query, _) in &queries {
            for _ in 0..3 {
                let _ = index.search(query, 20);
            }
        }
        let mut query_samples = Vec::with_capacity(queries.len() * QUERY_ROUNDS);
        let mut total_results = 0_usize;
        for (_, query, expected) in &queries {
            for _ in 0..QUERY_ROUNDS {
                let started = Instant::now();
                let results = index.search(query, 20);
                query_samples.push(started.elapsed());
                total_results = total_results.saturating_add(results.len());
                assert_benchmark_results(*expected, &results);
            }
        }
        let query_p95 = p95_duration(&query_samples);

        println!(
            "QUICKFOX_INCREMENTAL_BATCH entries={} commit_us={} journal_commit_us={} layer_apply_us={} baseline={} overlay={} tombstones={} query_rounds={} query_latency_p95_us={} results={} estimated_delta_bytes={}",
            DELTA_ENTRIES,
            batch_commit.as_micros(),
            journal_commit.as_micros(),
            layer_apply.as_micros(),
            index.baseline_entry_count(),
            index.overlay_entry_count(),
            index.tombstone_entry_count(),
            QUERY_ROUNDS,
            query_p95.as_micros(),
            total_results,
            index.estimated_delta_bytes(),
        );

        assert_eq!(index.delta_entry_count(), DELTA_ENTRIES);
        assert_eq!(storage.committed_index_deltas_after(0).unwrap().len(), 1);
        assert!(query_p95 <= Duration::from_millis(50));
    }

    #[test]
    fn authoritative_tail_can_replace_a_baseline_older_than_current_overlay() {
        let root = PathBuf::from("/tmp/root");
        let make_delta = |generation, upserts| CommittedIndexDelta {
            generation,
            upserts,
            removals: vec![],
        };
        let mut index = LayeredSearchIndex::from_baseline(vec![entry(
            root.join("old.md"),
            &root,
            IndexedEntryKind::File,
        )]);
        index.apply_delta(make_delta(
            4,
            vec![entry(
                root.join("old-overlay.md"),
                &root,
                IndexedEntryKind::File,
            )],
        ));
        index.apply_delta(make_delta(5, vec![]));

        let installed = index.replace_baseline_with_authoritative_tail(
            SearchIndex::from_entries(vec![entry(
                root.join("new.md"),
                &root,
                IndexedEntryKind::File,
            )]),
            3,
            &[
                make_delta(
                    4,
                    vec![entry(root.join("tail.md"), &root, IndexedEntryKind::File)],
                ),
                make_delta(5, vec![]),
            ],
        );

        assert!(installed);
        assert_eq!(index.generation(), 5);
        assert_eq!(index.entry_count(), 2);
        assert_eq!(index.search(&request("old"), 10).len(), 0);
        assert_eq!(index.search(&request("new"), 10).len(), 1);
        assert_eq!(index.search(&request("tail"), 10).len(), 1);
    }

    #[test]
    fn visibility_filter_runs_before_each_layer_limit() {
        let root = PathBuf::from("/tmp/root");
        let mut index = LayeredSearchIndex::from_baseline(vec![
            entry(root.join("docs/item-0.md"), &root, IndexedEntryKind::File),
            entry(root.join("docs/item-1.md"), &root, IndexedEntryKind::File),
            entry(root.join("item-visible.md"), &root, IndexedEntryKind::File),
        ]);
        index.apply_delta(CommittedIndexDelta {
            generation: 1,
            upserts: vec![entry(
                root.join("item-overlay.md"),
                &root,
                IndexedEntryKind::File,
            )],
            removals: vec![root.join("docs")],
        });

        let results = index.search(&request("item"), 1);

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "item-visible.md");
        assert_eq!(results[1].title, "item-overlay.md");
    }

    #[test]
    fn small_delta_rebuilds_only_overlay_compact_index() {
        let root = PathBuf::from("/tmp/root");
        let mut index = LayeredSearchIndex::from_baseline(vec![entry(
            root.join("base.md"),
            &root,
            IndexedEntryKind::File,
        )]);
        let baseline_build_id = index.baseline_compact_build_id();
        let overlay_build_id = index.overlay_compact_build_id();
        let overlay_build_events = index.overlay_compact_build_ids().len();

        index.apply_delta(CommittedIndexDelta {
            generation: 1,
            upserts: vec![entry(root.join("new.md"), &root, IndexedEntryKind::File)],
            removals: Vec::new(),
        });

        assert_eq!(index.baseline_compact_build_id(), baseline_build_id);
        assert_ne!(index.overlay_compact_build_id(), overlay_build_id);
        assert_eq!(
            index.overlay_compact_build_ids().len(),
            overlay_build_events + 1
        );
        assert_eq!(
            index.overlay_compact_build_ids().last().copied(),
            Some(index.overlay_compact_build_id())
        );
        assert_ne!(baseline_build_id, 0);
    }

    #[test]
    fn tombstone_lookup_cost_depends_on_path_depth_not_tombstone_count() {
        let mut tombstones = PathTombstones::default();
        for index in 0..5_000 {
            tombstones.insert_directory(format!("/archive-{index}"));
        }
        tombstones.reset_lookup_probe_count();

        assert!(!tombstones.contains("/active/docs/readme.md"));

        assert!(tombstones.lookup_probe_count() <= 4);
    }

    #[test]
    fn batch_file_removals_do_not_scan_the_full_baseline_and_count_is_cached() {
        let root = PathBuf::from("/tmp/root");
        let baseline: Vec<_> = (0..20_000)
            .map(|index| {
                entry(
                    root.join(format!("file-{index:05}.md")),
                    &root,
                    IndexedEntryKind::File,
                )
            })
            .collect();
        let mut index = LayeredSearchIndex::from_baseline(baseline);
        index.reset_baseline_probe_count();

        index.apply_delta(CommittedIndexDelta {
            generation: 1,
            upserts: Vec::new(),
            removals: (0..50)
                .map(|index| root.join(format!("file-{:05}.md", index * 200)))
                .collect(),
        });

        let probes_after_delta = index.baseline_probe_count();
        assert!(
            probes_after_delta < 500,
            "baseline probes: {probes_after_delta}"
        );
        assert_eq!(index.entry_count(), 19_950);
        assert_eq!(index.entry_count(), 19_950);
        assert_eq!(index.baseline_probe_count(), probes_after_delta);
    }

    #[test]
    fn baseline_path_metadata_reuses_the_authoritative_compact_index() {
        let root = PathBuf::from("/tmp/root");
        let baseline: Vec<_> = (0..20_000)
            .map(|index| {
                entry(
                    root.join(format!("memory-file-{index:05}.md")),
                    &root,
                    IndexedEntryKind::File,
                )
            })
            .collect();
        let index = LayeredSearchIndex::from_baseline(baseline);

        assert_eq!(index.estimated_baseline_path_metadata_bytes(), 0);
    }

    #[test]
    #[ignore = "2,000,000 baseline plus 10,000-entry delta resident memory budget"]
    fn two_million_baseline_with_delta_stays_within_total_resident_memory_budget() {
        const BASELINE_ENTRIES: usize = 2_000_000;
        const DELTA_ENTRIES: usize = 10_000;
        let mut index = LayeredSearchIndex::from_baseline(synthetic_layered_benchmark_entries(
            BASELINE_ENTRIES,
        ));
        index.apply_delta(synthetic_layered_benchmark_delta(
            BASELINE_ENTRIES,
            DELTA_ENTRIES,
        ));

        let total_bytes = index.estimated_total_resident_bytes();
        assert!(
            total_bytes < 500 * 1024 * 1024,
            "baseline + delta resident estimate is {total_bytes} bytes"
        );
    }

    #[test]
    fn delta_memory_estimate_includes_all_overlay_search_copies_and_compact_storage() {
        let root = PathBuf::from("/tmp/root");
        let path = format!("/tmp/root/{}.md", "p".repeat(4_096));
        let name = format!("{}.md", "n".repeat(2_048));
        let search_text = "search".repeat(1_024);
        let mut long_entry = entry(PathBuf::from(&path), &root, IndexedEntryKind::File);
        long_entry.name = name.clone();
        long_entry.search_text = search_text.clone();
        let mut index = LayeredSearchIndex::from_baseline(Vec::new());

        index.apply_delta(CommittedIndexDelta {
            generation: 1,
            upserts: vec![long_entry],
            removals: Vec::new(),
        });

        let obvious_duplicate_lower_bound = path
            .len()
            .saturating_mul(3)
            .saturating_add(name.len().saturating_mul(2))
            .saturating_add(search_text.len().saturating_mul(3))
            .saturating_add(std::mem::size_of::<IndexedEntry>().saturating_mul(2));
        assert!(
            index.estimated_delta_bytes() >= obvious_duplicate_lower_bound,
            "estimate={} lower_bound={obvious_duplicate_lower_bound}",
            index.estimated_delta_bytes()
        );
    }

    #[test]
    fn content_visibility_snapshot_build_is_independent_of_baseline_size() {
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path();
        let hidden_root = root.join("hidden");
        let mut entries: Vec<_> = (0..10_001)
            .map(|index| {
                entry(
                    hidden_root.join(format!("shared-hidden-{index:05}.md")),
                    root,
                    IndexedEntryKind::File,
                )
            })
            .collect();
        entries.push(entry(
            root.join("shared-visible.md"),
            root,
            IndexedEntryKind::File,
        ));
        let content_index = ContentIndex::build_with_extractor(
            &mut entries,
            ContentIndexOptions {
                index_dir: root.join("tantivy-layered-visibility"),
                max_file_bytes: DEFAULT_MAX_CONTENT_BYTES,
            },
            &LayeredRankedContentExtractor,
        )
        .unwrap();
        let baseline = SearchIndex::from_entries_with_content_index(entries, content_index);
        let mut index = LayeredSearchIndex::from_search_index(baseline);

        index.apply_delta(CommittedIndexDelta {
            generation: 1,
            upserts: Vec::new(),
            removals: vec![hidden_root],
        });

        assert_eq!(index.overlay.memory_estimate().path_lookup_entry_count, 0);
        assert_eq!(index.overlay.memory_estimate().path_lookup_bytes, 0);
        let snapshot_id = index.content_visibility_snapshot_id();
        index.reset_content_visibility_baseline_scan_count();

        let content_only = index.search(&request("content:needle"), 1);

        assert_eq!(content_only[0].title, "shared-visible.md");
        assert_eq!(index.content_visibility_baseline_scan_count(), 0);

        let mixed = index.search(&request("shared content:needle"), 1);

        assert_eq!(mixed[0].title, "shared-visible.md");
        assert_eq!(index.content_visibility_snapshot_id(), snapshot_id);
    }

    #[test]
    fn layered_results_match_full_index_oracle_after_mixed_updates_and_ranking() {
        let root = PathBuf::from("/tmp/root");
        let baseline_entries = vec![
            entry(root.join("alpha.md"), &root, IndexedEntryKind::File),
            entry(root.join("remove.md"), &root, IndexedEntryKind::File),
            entry(root.join("tree/child.md"), &root, IndexedEntryKind::File),
            entry(root.join("rename-old.md"), &root, IndexedEntryKind::File),
            entry(root.join("Tools.app"), &root, IndexedEntryKind::Application),
        ];
        let mut layered = LayeredSearchIndex::from_baseline(baseline_entries.clone());
        let mut oracle = SearchIndex::from_entries(baseline_entries);

        let replacement = named_entry(root.join("alpha.md"), &root, "alpha-updated.md");
        let added = entry(root.join("added.md"), &root, IndexedEntryKind::File);
        apply_to_both(
            &mut layered,
            &mut oracle,
            1,
            vec![replacement, added],
            Vec::new(),
        );
        apply_to_both(
            &mut layered,
            &mut oracle,
            2,
            Vec::new(),
            vec![root.join("remove.md"), root.join("tree")],
        );
        apply_to_both(
            &mut layered,
            &mut oracle,
            3,
            vec![entry(
                root.join("rename-new.md"),
                &root,
                IndexedEntryKind::File,
            )],
            vec![root.join("rename-old.md")],
        );

        let history = HistoryScores::from_pairs([("path:/tmp/root/rename-new.md", 50)]);
        for query in ["md", "alpha", "rename", "Tools"] {
            let request = request(query);
            let layered_results =
                Ranker::default().rank(query, layered.search(&request, 20), &history);
            let oracle_results =
                Ranker::default().rank(query, oracle.search_with_limit(&request, 20), &history);
            assert_eq!(
                result_ids(&layered_results),
                result_ids(&oracle_results),
                "{query}"
            );
        }
    }

    fn apply_to_both(
        layered: &mut LayeredSearchIndex,
        oracle: &mut SearchIndex,
        generation: u64,
        upserts: Vec<IndexedEntry>,
        removals: Vec<PathBuf>,
    ) {
        layered.apply_delta(CommittedIndexDelta {
            generation,
            upserts: upserts.clone(),
            removals: removals.clone(),
        });
        oracle.apply_update_batch(
            &IndexUpdateBatch {
                changed_paths: upserts
                    .iter()
                    .map(|entry| PathBuf::from(&entry.path))
                    .collect(),
                removed_paths: removals,
            },
            upserts,
        );
    }

    fn entry(path: PathBuf, root: &Path, kind: IndexedEntryKind) -> IndexedEntry {
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let mut entry = IndexedEntry::legacy(path.to_string_lossy(), name, kind);
        entry.root = root.to_string_lossy().into_owned();
        entry
    }

    struct LayeredRankedContentExtractor;

    impl TextExtractor for LayeredRankedContentExtractor {
        fn extract(&self, path: &Path, _max_bytes: u64) -> ContentExtractionResult {
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

    fn named_entry(path: PathBuf, root: &Path, name: &str) -> IndexedEntry {
        let mut entry = entry(path, root, IndexedEntryKind::File);
        entry.name = name.to_owned();
        entry.search_text = crate::core::index_entry::build_search_text(&entry.name, &entry.path);
        entry
    }

    fn request(text: &str) -> QueryRequest {
        QueryRequest::new(text, SearchMode::Normal)
    }

    fn result_paths(results: &[crate::core::search::SearchResult]) -> Vec<&str> {
        results
            .iter()
            .filter_map(|result| result.detail.as_deref())
            .collect()
    }

    fn result_ids(results: &[crate::core::search::SearchResult]) -> Vec<&str> {
        results.iter().map(|result| result.id.as_str()).collect()
    }

    #[derive(Clone, Copy)]
    enum BenchmarkExpectation {
        ContainsTitle(&'static str),
        Empty,
    }

    fn synthetic_layered_benchmark_entries(entry_count: usize) -> Vec<IndexedEntry> {
        (0..entry_count)
            .map(synthetic_layered_benchmark_entry)
            .collect()
    }

    fn synthetic_layered_benchmark_entry(index: usize) -> IndexedEntry {
        if index == 0 {
            let mut entry = IndexedEntry::legacy(
                "/benchmark/group-0000",
                "group-0000",
                IndexedEntryKind::Directory,
            );
            entry.root = "/benchmark".to_owned();
            entry.depth = 1;
            return entry;
        }
        let group = index / 1_000;
        let name = if index == 42 {
            "baseline-deleted-target.md".to_owned()
        } else {
            format!("entry-{index:07}.md")
        };
        let path = format!("/benchmark/group-{group:04}/{name}");
        let mut entry = IndexedEntry::legacy(path, name, IndexedEntryKind::File);
        entry.extension = Some("md".to_owned());
        entry.root = "/benchmark".to_owned();
        entry.depth = 2;
        entry
    }

    fn synthetic_layered_benchmark_delta(
        baseline_entries: usize,
        delta_entries: usize,
    ) -> CommittedIndexDelta {
        assert!(baseline_entries >= 10_000);
        assert_eq!(delta_entries % 2, 0);
        let overlay_count = delta_entries / 2;
        let tombstone_count = delta_entries / 2;
        let mut upserts = Vec::with_capacity(overlay_count);
        for index in 0..overlay_count {
            if index == 0 {
                upserts.push(benchmark_named_entry(
                    "/benchmark/runtime/runtime-delta-target.md",
                    "runtime-delta-target.md",
                ));
            } else if index == 1 {
                let mut replacement = synthetic_layered_benchmark_entry(baseline_entries / 2);
                replacement.name = "runtime-overwrite-target.md".to_owned();
                replacement.search_text = crate::core::index_entry::build_search_text(
                    &replacement.name,
                    &replacement.path,
                );
                upserts.push(replacement);
            } else {
                upserts.push(benchmark_named_entry(
                    &format!("/benchmark/runtime/runtime-new-{index:05}.md"),
                    &format!("runtime-new-{index:05}.md"),
                ));
            }
        }

        let mut removals = Vec::with_capacity(tombstone_count);
        removals.push(PathBuf::from("/benchmark/group-0000"));
        removals.extend(
            (1..tombstone_count)
                .map(|index| PathBuf::from(synthetic_layered_benchmark_entry(index + 1_000).path)),
        );
        CommittedIndexDelta {
            generation: 1,
            upserts,
            removals,
        }
    }

    fn benchmark_named_entry(path: &str, name: &str) -> IndexedEntry {
        let mut entry = IndexedEntry::legacy(path, name, IndexedEntryKind::File);
        entry.extension = Some("md".to_owned());
        entry.root = "/benchmark".to_owned();
        entry.depth = 2;
        entry
    }

    fn layered_benchmark_queries() -> Vec<(&'static str, QueryRequest, BenchmarkExpectation)> {
        vec![
            (
                "overlay-exact",
                request("runtime-delta-target.md"),
                BenchmarkExpectation::ContainsTitle("runtime-delta-target.md"),
            ),
            (
                "overlay-prefix",
                request("runtime-delta-tar"),
                BenchmarkExpectation::ContainsTitle("runtime-delta-target.md"),
            ),
            (
                "overlay-field-filtered",
                request("type:md runtime-delta-target"),
                BenchmarkExpectation::ContainsTitle("runtime-delta-target.md"),
            ),
            (
                "baseline-overwrite",
                request("runtime-overwrite-target"),
                BenchmarkExpectation::ContainsTitle("runtime-overwrite-target.md"),
            ),
            (
                "subtree-tombstone",
                request("baseline-deleted-target"),
                BenchmarkExpectation::Empty,
            ),
            (
                "low-hit",
                request("needle-not-present-987654321"),
                BenchmarkExpectation::Empty,
            ),
        ]
    }

    fn assert_benchmark_results(
        expected: BenchmarkExpectation,
        results: &[crate::core::search::SearchResult],
    ) {
        match expected {
            BenchmarkExpectation::ContainsTitle(title) => {
                assert!(results.iter().any(|result| result.title == title));
            }
            BenchmarkExpectation::Empty => assert!(results.is_empty()),
        }
    }

    fn p95_duration(samples: &[Duration]) -> Duration {
        assert!(!samples.is_empty());
        let mut sorted = samples.to_vec();
        sorted.sort_unstable();
        let rank = sorted.len().saturating_mul(95).div_ceil(100);
        sorted[rank.saturating_sub(1)]
    }
}
