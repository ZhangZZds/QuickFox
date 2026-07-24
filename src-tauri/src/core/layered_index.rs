//! Baseline, delta overlay, and tombstone composition for runtime file search.

use crate::core::index::{FileSearchIndex, IndexedEntry, IndexedEntryKind, SearchIndex};
use crate::core::search::{QueryRequest, SearchResult};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

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
}

impl PathTombstones {
    fn insert_exact(&mut self, key: String) {
        if !self.directories.iter().any(|root| path_matches(root, &key)) {
            self.exact.insert(key);
        }
    }

    fn insert_directory(&mut self, key: String) {
        if self.directories.iter().any(|root| path_matches(root, &key)) {
            return;
        }
        self.exact.retain(|path| !path_matches(&key, path));
        self.directories
            .retain(|directory| !path_matches(&key, directory));
        self.directories.insert(key);
    }

    fn remove_exact(&mut self, key: &str) {
        self.exact.remove(key);
    }

    fn contains(&self, key: &str) -> bool {
        self.exact.contains(key)
            || self
                .directories
                .iter()
                .any(|directory| path_matches(directory, key))
    }

    fn contains_directory(&self, key: &str) -> bool {
        self.directories.contains(key)
    }

    fn len(&self) -> usize {
        self.exact.len().saturating_add(self.directories.len())
    }

    fn estimated_bytes(&self) -> usize {
        self.exact
            .iter()
            .chain(&self.directories)
            .map(|path| path.len().saturating_add(std::mem::size_of::<String>()))
            .sum()
    }

    fn clear(&mut self) {
        self.exact.clear();
        self.directories.clear();
    }
}

#[derive(Debug)]
pub struct LayeredSearchIndex {
    baseline: SearchIndex,
    overlay_entries: BTreeMap<String, IndexedEntry>,
    overlay: SearchIndex,
    tombstones: PathTombstones,
    generation: u64,
    #[cfg(test)]
    baseline_build_count: usize,
}

impl Default for LayeredSearchIndex {
    fn default() -> Self {
        Self::from_baseline(Vec::new())
    }
}

impl LayeredSearchIndex {
    pub fn from_baseline(entries: Vec<IndexedEntry>) -> Self {
        let baseline = SearchIndex::from_entries(entries);
        Self {
            baseline,
            overlay_entries: BTreeMap::new(),
            overlay: SearchIndex::default(),
            tombstones: PathTombstones::default(),
            generation: 0,
            #[cfg(test)]
            baseline_build_count: 1,
        }
    }

    pub fn apply_delta(&mut self, delta: CommittedIndexDelta) {
        if delta.generation <= self.generation {
            return;
        }

        for removal in delta.removals {
            let key = normalize_path(removal);
            if self.path_is_directory(&key) {
                self.overlay_entries
                    .retain(|overlay_key, _| !path_matches(&key, overlay_key));
                self.tombstones.insert_directory(key);
            } else {
                self.overlay_entries.remove(&key);
                self.tombstones.insert_exact(key);
            }
        }

        for entry in delta.upserts {
            let key = normalize_path_text(&entry.path);
            self.tombstones.remove_exact(&key);
            self.overlay_entries.insert(key, entry);
        }

        self.overlay = SearchIndex::from_entries(self.overlay_entries.values().cloned().collect());
        self.generation = delta.generation;
    }

    pub fn replace_baseline(&mut self, entries: Vec<IndexedEntry>, generation: u64) {
        if generation < self.generation {
            return;
        }
        self.baseline = SearchIndex::from_entries(entries);
        self.overlay_entries.clear();
        self.overlay = SearchIndex::default();
        self.tombstones.clear();
        self.generation = generation;
        #[cfg(test)]
        {
            self.baseline_build_count = self.baseline_build_count.saturating_add(1);
        }
    }

    pub fn search(&self, query: &QueryRequest, candidate_budget: usize) -> Vec<SearchResult> {
        if candidate_budget == 0 {
            return Vec::new();
        }

        let baseline_results =
            self.baseline
                .search_with_limit_visible(query, candidate_budget, |entry| {
                    self.baseline_entry_is_visible(entry)
                });
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
        let baseline_count = self
            .baseline
            .entries()
            .iter()
            .filter(|entry| self.baseline_entry_is_visible(entry))
            .count();
        baseline_count.saturating_add(self.overlay_entries.len())
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
                key.len()
                    .saturating_add(std::mem::size_of::<String>())
                    .saturating_add(std::mem::size_of::<IndexedEntry>())
                    .saturating_add(indexed_entry_string_bytes(entry))
            })
            .sum();
        overlay_bytes.saturating_add(self.tombstones.estimated_bytes())
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    #[cfg(test)]
    pub fn baseline_build_count(&self) -> usize {
        self.baseline_build_count
    }

    fn baseline_entry_is_visible(&self, entry: &IndexedEntry) -> bool {
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
        if self.baseline.entries().iter().any(|entry| {
            normalize_path_text(&entry.path) == key && entry.kind == IndexedEntryKind::Directory
        }) {
            return true;
        }

        self.overlay_entries
            .keys()
            .any(|candidate| path_is_descendant(key, candidate))
            || self
                .baseline
                .entries()
                .iter()
                .map(|entry| normalize_path_text(&entry.path))
                .any(|candidate| path_is_descendant(key, &candidate))
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

fn indexed_entry_string_bytes(entry: &IndexedEntry) -> usize {
    entry.path.len()
        + entry.name.len()
        + entry.parent.len()
        + entry
            .extension
            .as_ref()
            .map(String::len)
            .unwrap_or_default()
        + entry.root.len()
        + entry.search_text.len()
}

#[cfg(test)]
mod tests {
    use super::{CommittedIndexDelta, LayeredSearchIndex};
    use crate::core::compact_index::CompactCandidateIndex;
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
        let compact_builds = CompactCandidateIndex::build_count();
        let baseline_builds = index.baseline_build_count();

        index.apply_delta(CommittedIndexDelta {
            generation: 1,
            upserts: vec![entry(root.join("new.md"), &root, IndexedEntryKind::File)],
            removals: Vec::new(),
        });

        assert!(CompactCandidateIndex::build_count() > compact_builds);
        assert_eq!(index.baseline_build_count(), baseline_builds);
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
