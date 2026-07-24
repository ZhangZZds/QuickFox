//! Baseline, delta overlay, and tombstone composition for runtime file search.

use crate::core::content_index::ContentPathFilter;
use crate::core::index::{FileSearchIndex, IndexedEntry, IndexedEntryKind, SearchIndex};
use crate::core::search::{QueryRequest, SearchResult};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
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

    fn is_visible(&self, path: &str) -> bool {
        let key = normalize_path_text(path);
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
            .map(|entry| (normalize_path_text(&entry.path), entry.kind.clone()))
            .collect();
        let visible_entry_count = baseline_by_path.len();
        Self {
            baseline,
            baseline_by_path,
            overlay_entries: BTreeMap::new(),
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

        for removal in delta.removals {
            let key = normalize_path(removal);
            let is_directory = self.path_is_directory(&key);
            let visible_before = self.count_visible_scope(&key, is_directory);
            if is_directory {
                self.overlay_entries
                    .retain(|overlay_key, _| !path_matches(&key, overlay_key));
                self.tombstones.insert_directory(key.clone());
            } else {
                self.overlay_entries.remove(&key);
                self.tombstones.insert_exact(key.clone());
            }
            let visible_after = self.count_visible_scope(&key, is_directory);
            self.adjust_visible_count(visible_before, visible_after);
        }

        for entry in delta.upserts {
            let key = normalize_path_text(&entry.path);
            let replaces_directory =
                entry.kind != IndexedEntryKind::Directory && self.path_is_directory(&key);
            let visible_before = self.count_visible_scope(&key, replaces_directory);
            if replaces_directory {
                self.overlay_entries
                    .retain(|overlay_key, _| !path_is_descendant(&key, overlay_key));
                self.tombstones.insert_directory(key.clone());
            }
            self.tombstones.remove_exact(&key);
            self.overlay_entries.insert(key.clone(), entry);
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
        self.generation = delta.generation;
    }

    pub fn replace_baseline(&mut self, entries: Vec<IndexedEntry>, generation: u64) {
        if generation < self.generation {
            return;
        }
        self.baseline_by_path = entries
            .iter()
            .map(|entry| (normalize_path_text(&entry.path), entry.kind.clone()))
            .collect();
        self.visible_entry_count = self.baseline_by_path.len();
        self.baseline = SearchIndex::from_entries(entries);
        self.overlay_entries.clear();
        self.overlay = SearchIndex::default();
        self.tombstones.clear();
        self.content_visibility = Arc::new(ContentVisibilitySnapshot::default());
        self.generation = generation;
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
        let overlay_results = self.overlay.search_with_limit(query, candidate_budget);
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

    pub fn delta_entry_count(&self) -> usize {
        self.overlay_entries
            .len()
            .saturating_add(self.tombstones.len())
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
            .saturating_add(overlay_index.entry_struct_bytes)
            .saturating_add(overlay_index.entry_string_bytes)
            .saturating_add(overlay_index.cached_search_text_bytes)
            .saturating_add(overlay_index.compact_candidate_bytes)
            .saturating_add(overlay_index.path_lookup_bytes)
            .saturating_add(self.tombstones.estimated_bytes())
            .saturating_add(self.content_visibility.estimated_bytes())
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
        let key = normalize_path_text(&entry.path);
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

impl FileSearchIndex for LayeredSearchIndex {
    fn search_files(&self, query: &QueryRequest, limit: usize) -> Vec<SearchResult> {
        self.search(query, limit)
    }

    fn indexed_entry_count(&self) -> usize {
        self.entry_count()
    }
}

fn normalize_path(path: impl AsRef<Path>) -> String {
    normalize_path_text(&path.as_ref().to_string_lossy())
}

fn normalize_path_text(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let normalized = if normalized.len() > 1 {
        normalized.trim_end_matches('/').to_owned()
    } else {
        normalized
    };

    #[cfg(target_os = "windows")]
    {
        normalized.to_lowercase()
    }
    #[cfg(not(target_os = "windows"))]
    {
        normalized
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
    use std::path::{Path, PathBuf};

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
    fn baseline_path_metadata_memory_is_reported_without_full_entry_copies() {
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
        let path_bytes: usize = baseline.iter().map(|entry| entry.path.len()).sum();
        let full_entry_copy_lower_bound = path_bytes.saturating_add(
            baseline
                .len()
                .saturating_mul(std::mem::size_of::<IndexedEntry>()),
        );
        let index = LayeredSearchIndex::from_baseline(baseline);

        let metadata_bytes = index.estimated_baseline_path_metadata_bytes();
        let projected_two_million_bytes = metadata_bytes.saturating_mul(100);

        assert!(metadata_bytes >= path_bytes);
        assert!(metadata_bytes < full_entry_copy_lower_bound);
        assert!(projected_two_million_bytes < 512 * 1024 * 1024);
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
}
