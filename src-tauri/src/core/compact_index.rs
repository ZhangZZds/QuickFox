//! Compact search index data structures.

use std::collections::{BTreeMap, HashMap};

#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::core::file_query::FileQuery;
use crate::core::index_entry::{IndexedEntry, IndexedEntryKind};

#[cfg(test)]
static COMPACT_INDEX_BUILD_COUNT: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StringId(usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EntryId(usize);

impl EntryId {
    pub fn as_usize(self) -> usize {
        self.0
    }
}

#[derive(Debug, Clone, Default)]
pub struct EntryIdAllocator {
    next: usize,
}

impl EntryIdAllocator {
    pub fn next_id(&mut self) -> EntryId {
        let id = EntryId(self.next);
        self.next = self.next.saturating_add(1);
        id
    }

    pub fn len(&self) -> usize {
        self.next
    }

    pub fn is_empty(&self) -> bool {
        self.next == 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactEntry {
    pub id: EntryId,
    pub path: StringId,
    pub name: StringId,
    pub kind: IndexedEntryKind,
    pub parent: Option<StringId>,
    pub extension: Option<StringId>,
    pub depth: usize,
    pub root: Option<StringId>,
    pub modified_ms: Option<i64>,
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct EntryTable {
    entries: Vec<CompactEntry>,
    strings: StringPool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryTableMemoryEstimate {
    pub entry_count: usize,
    pub entry_struct_bytes: usize,
    pub string_pool_unique_values: usize,
    pub string_pool_bytes: usize,
}

impl EntryTable {
    pub fn from_entries(entries: Vec<IndexedEntry>) -> Self {
        let mut table = Self::default();
        let mut ids = EntryIdAllocator::default();

        for entry in entries {
            let id = ids.next_id();
            let path = table.strings.intern(entry.path);
            let name = table.strings.intern(entry.name);
            let parent = intern_non_empty(&mut table.strings, entry.parent);
            let extension = entry
                .extension
                .and_then(|extension| intern_non_empty(&mut table.strings, extension));
            let root = intern_non_empty(&mut table.strings, entry.root);
            table.entries.push(CompactEntry {
                id,
                path,
                name,
                kind: entry.kind,
                parent,
                extension,
                depth: entry.depth,
                root,
                modified_ms: entry.modified_ms,
                size_bytes: entry.size_bytes,
            });
        }

        table
    }

    pub fn get(&self, id: EntryId) -> Option<&CompactEntry> {
        self.entries.get(id.as_usize())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn path(&self, entry: &CompactEntry) -> Option<&str> {
        self.strings.get(entry.path)
    }

    pub fn name(&self, entry: &CompactEntry) -> Option<&str> {
        self.strings.get(entry.name)
    }

    pub fn parent(&self, entry: &CompactEntry) -> Option<&str> {
        entry.parent.and_then(|id| self.strings.get(id))
    }

    pub fn extension(&self, entry: &CompactEntry) -> Option<&str> {
        entry.extension.and_then(|id| self.strings.get(id))
    }

    pub fn root(&self, entry: &CompactEntry) -> Option<&str> {
        entry.root.and_then(|id| self.strings.get(id))
    }

    pub fn path_by_id(&self, id: EntryId) -> Option<&str> {
        self.get(id).and_then(|entry| self.path(entry))
    }

    pub fn string_pool_len(&self) -> usize {
        self.strings.len()
    }

    pub fn memory_estimate(&self) -> EntryTableMemoryEstimate {
        EntryTableMemoryEstimate {
            entry_count: self.entries.len(),
            entry_struct_bytes: self
                .entries
                .len()
                .saturating_mul(std::mem::size_of::<CompactEntry>()),
            string_pool_unique_values: self.strings.len(),
            string_pool_bytes: self.strings.total_bytes(),
        }
    }

    pub fn entries(&self) -> &[CompactEntry] {
        &self.entries
    }

    pub fn all_ids(&self) -> Vec<EntryId> {
        self.entries.iter().map(|entry| entry.id).collect()
    }
}

#[derive(Debug, Clone, Default)]
pub struct NameTokenIndex {
    ids_by_token: BTreeMap<String, Vec<EntryId>>,
}

impl NameTokenIndex {
    pub fn build(table: &EntryTable) -> Self {
        let mut index = Self::default();
        for entry in table.entries() {
            let Some(name) = table.name(entry) else {
                continue;
            };
            for token in name_tokens(name) {
                push_unique(&mut index.ids_by_token, token, entry.id);
            }
        }
        index
    }

    pub fn lookup(&self, token: &str) -> Vec<EntryId> {
        self.ids_by_token
            .get(&token.to_ascii_lowercase())
            .cloned()
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, Default)]
pub struct PrefixIndex {
    ids_by_prefix: BTreeMap<String, Vec<EntryId>>,
}

impl PrefixIndex {
    pub fn build(table: &EntryTable) -> Self {
        let mut index = Self::default();
        for entry in table.entries() {
            let Some(name) = table.name(entry) else {
                continue;
            };
            for prefix in name_prefixes(name) {
                push_unique(&mut index.ids_by_prefix, prefix, entry.id);
            }
        }
        index
    }

    pub fn lookup(&self, prefix: &str) -> Vec<EntryId> {
        self.ids_by_prefix
            .get(&prefix.to_ascii_lowercase())
            .cloned()
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, Default)]
pub struct NameTrigramIndex {
    ids_by_trigram: BTreeMap<String, Vec<EntryId>>,
}

impl NameTrigramIndex {
    pub fn build(table: &EntryTable) -> Self {
        let mut index = Self::default();
        for entry in table.entries() {
            let Some(name) = table.name(entry) else {
                continue;
            };
            for trigram in name_trigrams(name) {
                push_unique(&mut index.ids_by_trigram, trigram, entry.id);
            }
        }
        index
    }

    pub fn lookup(&self, term: &str) -> Vec<EntryId> {
        leading_trigram(term)
            .and_then(|trigram| self.ids_by_trigram.get(&trigram).cloned())
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, Default)]
pub struct ExtensionIndex {
    ids_by_extension: BTreeMap<String, Vec<EntryId>>,
}

impl ExtensionIndex {
    pub fn build(table: &EntryTable) -> Self {
        let mut index = Self::default();
        for entry in table.entries() {
            if let Some(extension) = table
                .extension(entry)
                .map(str::to_owned)
                .or_else(|| entry_name_extension(table.name(entry).unwrap_or_default()))
            {
                push_unique(
                    &mut index.ids_by_extension,
                    extension.to_ascii_lowercase(),
                    entry.id,
                );
            }
        }
        index
    }

    pub fn lookup(&self, extension: &str) -> Vec<EntryId> {
        self.ids_by_extension
            .get(
                extension
                    .trim_start_matches('.')
                    .to_ascii_lowercase()
                    .as_str(),
            )
            .cloned()
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, Default)]
pub struct PathSegmentIndex {
    ids_by_segment: BTreeMap<String, Vec<EntryId>>,
    ids_by_prefix: BTreeMap<String, Vec<EntryId>>,
    ids_by_fuzzy_key: BTreeMap<String, Vec<EntryId>>,
}

impl PathSegmentIndex {
    pub fn build(table: &EntryTable) -> Self {
        let mut index = Self::default();
        for entry in table.entries() {
            let path = table.path(entry).unwrap_or_default();
            let parent = table.parent(entry).unwrap_or_default();
            for segment in path_segments(path).chain(path_segments(parent)) {
                if let Some(key) = fuzzy_segment_key(&segment) {
                    push_unique(&mut index.ids_by_fuzzy_key, key, entry.id);
                }
                for prefix in segment_prefixes(&segment) {
                    push_unique(&mut index.ids_by_prefix, prefix, entry.id);
                }
                push_unique(&mut index.ids_by_segment, segment, entry.id);
            }
        }
        index
    }

    pub fn lookup(&self, segment: &str) -> Vec<EntryId> {
        let normalized = segment.to_ascii_lowercase();
        let exact = self
            .ids_by_segment
            .get(&normalized)
            .cloned()
            .unwrap_or_default();
        if !exact.is_empty() {
            return exact;
        }

        let prefix = self
            .ids_by_prefix
            .get(&normalized)
            .cloned()
            .unwrap_or_default();
        if !prefix.is_empty() {
            return prefix;
        }

        fuzzy_segment_key(&normalized)
            .and_then(|key| self.ids_by_fuzzy_key.get(&key).cloned())
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, Default)]
pub struct ExactPathIndex {
    id_by_path: BTreeMap<String, EntryId>,
}

impl ExactPathIndex {
    pub fn build(table: &EntryTable) -> Self {
        let mut index = Self::default();
        for entry in table.entries() {
            if let Some(path) = table.path(entry) {
                index.id_by_path.insert(path.to_ascii_lowercase(), entry.id);
            }
        }
        index
    }

    pub fn lookup(&self, path: &str) -> Vec<EntryId> {
        self.id_by_path
            .get(&path.to_ascii_lowercase())
            .copied()
            .into_iter()
            .collect()
    }
}

#[derive(Debug, Clone, Default)]
pub struct CompactCandidateIndex {
    table: EntryTable,
    name_tokens: NameTokenIndex,
    prefixes: PrefixIndex,
    name_trigrams: NameTrigramIndex,
    extensions: ExtensionIndex,
    path_segments: PathSegmentIndex,
    exact_paths: ExactPathIndex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CandidateRetrievalStats {
    pub indexed_entry_count: usize,
    pub candidate_count: usize,
    pub used_full_scan: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateRetrieval {
    pub candidates: Vec<EntryId>,
    pub stats: CandidateRetrievalStats,
}

impl CompactCandidateIndex {
    pub fn from_entries(entries: Vec<IndexedEntry>) -> Self {
        #[cfg(test)]
        COMPACT_INDEX_BUILD_COUNT.fetch_add(1, Ordering::Relaxed);

        let table = EntryTable::from_entries(entries);
        Self {
            name_tokens: NameTokenIndex::build(&table),
            prefixes: PrefixIndex::build(&table),
            name_trigrams: NameTrigramIndex::build(&table),
            extensions: ExtensionIndex::build(&table),
            path_segments: PathSegmentIndex::build(&table),
            exact_paths: ExactPathIndex::build(&table),
            table,
        }
    }

    #[cfg(test)]
    pub fn reset_build_count() {
        COMPACT_INDEX_BUILD_COUNT.store(0, Ordering::Relaxed);
    }

    #[cfg(test)]
    pub fn build_count() -> usize {
        COMPACT_INDEX_BUILD_COUNT.load(Ordering::Relaxed)
    }

    pub fn retrieve_ordinary_term(&self, term: &str) -> CandidateRetrieval {
        let normalized = term.to_ascii_lowercase();
        let (candidates, used_full_scan) = if normalized.contains(['/', '\\']) {
            let exact = self.exact_paths.lookup(&normalized);
            if exact.is_empty() {
                (self.table.all_ids(), true)
            } else {
                (exact, false)
            }
        } else {
            let precise = union_sorted_ids([
                self.name_tokens.lookup(&normalized),
                self.prefixes.lookup(&normalized),
                self.path_segments.lookup(&normalized),
            ]);
            if precise.is_empty() {
                (self.name_trigrams.lookup(&normalized), false)
            } else {
                (precise, false)
            }
        };
        CandidateRetrieval {
            stats: CandidateRetrievalStats {
                indexed_entry_count: self.table.len(),
                candidate_count: candidates.len(),
                used_full_scan,
            },
            candidates,
        }
    }

    pub fn retrieve_query(&self, query: &str) -> CandidateRetrieval {
        let query = FileQuery::parse(query);
        let mut candidates: Option<Vec<EntryId>> = None;
        let mut used_full_scan = false;

        for term in &query.ordinary_terms {
            let retrieval = self.retrieve_ordinary_term(term);
            used_full_scan |= retrieval.stats.used_full_scan;
            candidates = intersect_optional_candidates(candidates, retrieval.candidates);
        }

        for extension in &query.type_filters {
            candidates =
                intersect_optional_candidates(candidates, self.extensions.lookup(extension));
        }

        for name in &query.name_filters {
            let mut name_candidates =
                union_sorted_ids([self.name_tokens.lookup(name), self.prefixes.lookup(name)]);
            if name_candidates.is_empty() {
                name_candidates = self.name_trigrams.lookup(name);
            }
            if name_candidates.is_empty() {
                name_candidates = self.table.all_ids();
                used_full_scan = true;
            }
            candidates = intersect_optional_candidates(candidates, name_candidates);
        }

        for dir in &query.dir_filters {
            let dir_candidates = if dir.contains(['*', '?', '[']) {
                used_full_scan = true;
                self.table.all_ids()
            } else {
                self.path_segments.lookup(dir)
            };
            candidates = intersect_optional_candidates(candidates, dir_candidates);
        }

        let candidates = candidates.unwrap_or_default();
        CandidateRetrieval {
            stats: CandidateRetrievalStats {
                indexed_entry_count: self.table.len(),
                candidate_count: candidates.len(),
                used_full_scan,
            },
            candidates,
        }
    }

    pub fn retrieve_query_paths(&self, query: &str) -> Vec<String> {
        self.retrieve_query(query)
            .candidates
            .into_iter()
            .filter_map(|id| self.table.path_by_id(id).map(str::to_owned))
            .collect()
    }
}

#[derive(Debug, Clone, Default)]
pub struct StringPool {
    values: Vec<String>,
    ids_by_value: HashMap<String, StringId>,
    total_bytes: usize,
}

impl StringPool {
    pub fn intern(&mut self, value: impl AsRef<str>) -> StringId {
        let value = value.as_ref();
        if let Some(id) = self.ids_by_value.get(value) {
            return *id;
        }

        let id = StringId(self.values.len());
        self.total_bytes = self.total_bytes.saturating_add(value.len());
        self.values.push(value.to_owned());
        self.ids_by_value.insert(value.to_owned(), id);
        id
    }

    pub fn get(&self, id: StringId) -> Option<&str> {
        self.values.get(id.0).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn total_bytes(&self) -> usize {
        self.total_bytes
    }
}

fn intern_non_empty(pool: &mut StringPool, value: String) -> Option<StringId> {
    (!value.is_empty()).then(|| pool.intern(value))
}

fn entry_name_extension(name: &str) -> Option<String> {
    name.rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
}

fn name_tokens(name: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    for raw in name.split(|character: char| !character.is_alphanumeric()) {
        if raw.is_empty() {
            continue;
        }
        tokens.push(raw.to_ascii_lowercase());
        tokens.extend(camel_case_tokens(raw));
    }
    tokens.sort();
    tokens.dedup();
    tokens
}

fn name_prefixes(name: &str) -> Vec<String> {
    let lower = name.to_ascii_lowercase();
    let mut prefixes = Vec::new();
    for length in 1..=lower.len() {
        if lower.is_char_boundary(length) {
            prefixes.push(lower[..length].to_owned());
        }
    }
    for token in name_tokens(name) {
        for length in 1..=token.len() {
            if token.is_char_boundary(length) {
                prefixes.push(token[..length].to_owned());
            }
        }
    }
    prefixes.sort();
    prefixes.dedup();
    prefixes
}

fn name_trigrams(name: &str) -> Vec<String> {
    let lower = name.to_ascii_lowercase();
    let chars: Vec<_> = lower.chars().collect();
    if chars.len() < 3 {
        return Vec::new();
    }

    let mut trigrams = chars
        .windows(3)
        .map(|window| window.iter().collect::<String>())
        .collect::<Vec<_>>();
    trigrams.sort();
    trigrams.dedup();
    trigrams
}

fn segment_prefixes(segment: &str) -> Vec<String> {
    let mut prefixes = Vec::new();
    for length in 1..=segment.len() {
        if segment.is_char_boundary(length) {
            prefixes.push(segment[..length].to_owned());
        }
    }
    prefixes
}

fn leading_trigram(term: &str) -> Option<String> {
    let chars: Vec<_> = term.to_ascii_lowercase().chars().collect();
    (chars.len() >= 3).then(|| chars[..3].iter().collect())
}

fn path_segments(path: &str) -> impl Iterator<Item = String> + '_ {
    path.split(['/', '\\'])
        .map(|segment| segment.trim_end_matches(':'))
        .filter(|segment| !segment.is_empty())
        .map(str::to_ascii_lowercase)
}

fn camel_case_tokens(value: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut start = 0;
    let chars: Vec<_> = value.char_indices().collect();

    for window in chars.windows(2) {
        let (index, current) = window[0];
        let (_, next) = window[1];
        if current.is_lowercase() && next.is_uppercase() {
            if start < index + current.len_utf8() {
                tokens.push(value[start..index + current.len_utf8()].to_ascii_lowercase());
            }
            start = index + current.len_utf8();
        }
    }

    if start < value.len() {
        tokens.push(value[start..].to_ascii_lowercase());
    }

    tokens
        .into_iter()
        .filter(|token| !token.is_empty())
        .collect()
}

fn fuzzy_segment_key(segment: &str) -> Option<String> {
    let mut chars = segment.chars();
    let first = chars.next()?;
    let last = chars.last().unwrap_or(first);
    Some(format!("{first}{last}"))
}

fn push_unique(index: &mut BTreeMap<String, Vec<EntryId>>, key: String, id: EntryId) {
    let ids = index.entry(key).or_default();
    if ids.last().copied() != Some(id) {
        ids.push(id);
    }
}

pub fn intersect_sorted_ids(left: Vec<EntryId>, right: Vec<EntryId>) -> Vec<EntryId> {
    let mut intersection = Vec::new();
    let mut left_index = 0;
    let mut right_index = 0;

    while let (Some(left_id), Some(right_id)) = (left.get(left_index), right.get(right_index)) {
        match left_id.cmp(right_id) {
            std::cmp::Ordering::Less => left_index += 1,
            std::cmp::Ordering::Greater => right_index += 1,
            std::cmp::Ordering::Equal => {
                intersection.push(*left_id);
                left_index += 1;
                right_index += 1;
            }
        }
    }

    intersection
}

fn intersect_optional_candidates(
    current: Option<Vec<EntryId>>,
    next: Vec<EntryId>,
) -> Option<Vec<EntryId>> {
    Some(match current {
        Some(current) => intersect_sorted_ids(current, next),
        None => next,
    })
}

fn union_sorted_ids<const N: usize>(lists: [Vec<EntryId>; N]) -> Vec<EntryId> {
    let mut ids: Vec<_> = lists.into_iter().flatten().collect();
    ids.sort();
    ids.dedup();
    ids
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_pool_deduplicates_values_and_returns_stable_ids() {
        let mut pool = StringPool::default();

        let first = pool.intern("D:\\workspace\\QuickFox\\AGENTS.md");
        let duplicate = pool.intern("D:\\workspace\\QuickFox\\AGENTS.md");
        let second = pool.intern("AGENTS.md");

        assert_eq!(first, duplicate);
        assert_ne!(first, second);
        assert_eq!(pool.get(first), Some("D:\\workspace\\QuickFox\\AGENTS.md"));
        assert_eq!(pool.get(second), Some("AGENTS.md"));
        assert_eq!(pool.len(), 2);
    }

    #[test]
    fn string_pool_tracks_total_bytes_once_per_unique_value() {
        let mut pool = StringPool::default();

        pool.intern("workspace");
        pool.intern("workspace");
        pool.intern("AGENTS.md");

        assert_eq!(pool.total_bytes(), "workspace".len() + "AGENTS.md".len());
    }

    #[test]
    fn entry_id_allocator_returns_dense_stable_ids() {
        let mut allocator = EntryIdAllocator::default();

        let first = allocator.next_id();
        let second = allocator.next_id();

        assert_eq!(first.as_usize(), 0);
        assert_eq!(second.as_usize(), 1);
        assert_ne!(first, second);
        assert_eq!(allocator.len(), 2);
    }

    #[test]
    fn entry_table_preserves_metadata_and_reconstructs_strings() {
        let entries = vec![
            IndexedEntry {
                path: "D:\\workspace\\QuickFox\\AGENTS.md".to_owned(),
                name: "AGENTS.md".to_owned(),
                kind: IndexedEntryKind::File,
                parent: "D:\\workspace\\QuickFox".to_owned(),
                extension: Some("md".to_owned()),
                depth: 3,
                root: "D:\\".to_owned(),
                modified_ms: Some(123),
                size_bytes: Some(456),
                ..IndexedEntry::legacy("", "", IndexedEntryKind::File)
            },
            IndexedEntry {
                path: "D:\\workspace\\QuickFox\\README.md".to_owned(),
                name: "README.md".to_owned(),
                kind: IndexedEntryKind::File,
                parent: "D:\\workspace\\QuickFox".to_owned(),
                extension: Some("md".to_owned()),
                depth: 3,
                root: "D:\\".to_owned(),
                modified_ms: Some(789),
                size_bytes: Some(100),
                ..IndexedEntry::legacy("", "", IndexedEntryKind::File)
            },
        ];

        let table = EntryTable::from_entries(entries);
        let first = table.get(EntryId(0)).unwrap();
        let second = table.get(EntryId(1)).unwrap();

        assert_eq!(table.len(), 2);
        assert_eq!(
            table.path(first),
            Some("D:\\workspace\\QuickFox\\AGENTS.md")
        );
        assert_eq!(table.name(first), Some("AGENTS.md"));
        assert_eq!(table.parent(first), Some("D:\\workspace\\QuickFox"));
        assert_eq!(table.extension(first), Some("md"));
        assert_eq!(first.kind, IndexedEntryKind::File);
        assert_eq!(first.depth, 3);
        assert_eq!(first.modified_ms, Some(123));
        assert_eq!(first.size_bytes, Some(456));
        assert_eq!(table.extension(second), Some("md"));
        assert!(
            table.string_pool_len() < 10,
            "shared parent/root/extension strings should be deduplicated"
        );
    }

    #[test]
    fn entry_table_memory_estimate_reports_pool_and_entry_bytes() {
        let table = EntryTable::from_entries(vec![
            IndexedEntry {
                path: "D:\\workspace\\QuickFox\\AGENTS.md".to_owned(),
                name: "AGENTS.md".to_owned(),
                kind: IndexedEntryKind::File,
                parent: "D:\\workspace\\QuickFox".to_owned(),
                extension: Some("md".to_owned()),
                depth: 3,
                root: "D:\\".to_owned(),
                ..IndexedEntry::legacy("", "", IndexedEntryKind::File)
            },
            IndexedEntry {
                path: "D:\\workspace\\QuickFox\\README.md".to_owned(),
                name: "README.md".to_owned(),
                kind: IndexedEntryKind::File,
                parent: "D:\\workspace\\QuickFox".to_owned(),
                extension: Some("md".to_owned()),
                depth: 3,
                root: "D:\\".to_owned(),
                ..IndexedEntry::legacy("", "", IndexedEntryKind::File)
            },
        ]);

        let estimate = table.memory_estimate();

        assert_eq!(estimate.entry_count, 2);
        assert_eq!(estimate.string_pool_unique_values, table.string_pool_len());
        assert!(estimate.entry_struct_bytes >= std::mem::size_of::<CompactEntry>() * 2);
        assert!(estimate.string_pool_bytes > 0);
    }

    #[test]
    fn name_token_and_prefix_indexes_retrieve_agents_candidates() {
        let table = EntryTable::from_entries(vec![
            IndexedEntry::legacy(
                "D:\\workspace\\QuickFox\\docs\\AGENTS.md",
                "AGENTS.md",
                IndexedEntryKind::File,
            ),
            IndexedEntry::legacy(
                "D:\\workspace\\QuickFox\\docs\\README.md",
                "README.md",
                IndexedEntryKind::File,
            ),
        ]);

        let token_index = NameTokenIndex::build(&table);
        let prefix_index = PrefixIndex::build(&table);

        assert_eq!(token_index.lookup("agents"), vec![EntryId(0)]);
        assert_eq!(prefix_index.lookup("agents.m"), vec![EntryId(0)]);
        assert_eq!(prefix_index.lookup("read"), vec![EntryId(1)]);
    }

    #[test]
    fn name_trigram_index_retrieves_substring_candidates() {
        let table = EntryTable::from_entries(vec![
            IndexedEntry::legacy(
                "/workspace/reports/report.md",
                "report.md",
                IndexedEntryKind::File,
            ),
            IndexedEntry::legacy(
                "/workspace/reports/notes.md",
                "notes.md",
                IndexedEntryKind::File,
            ),
        ]);
        let index = NameTrigramIndex::build(&table);

        assert_eq!(index.lookup("port"), vec![EntryId(0)]);
        assert_eq!(index.lookup("ort"), vec![EntryId(0)]);
        assert!(index.lookup("zzzz").is_empty());
    }

    #[test]
    fn extension_index_filters_candidates_for_type_queries() {
        let table = EntryTable::from_entries(vec![
            IndexedEntry {
                path: "/workspace/QuickFox/AGENTS.md".to_owned(),
                name: "AGENTS.md".to_owned(),
                extension: Some("md".to_owned()),
                kind: IndexedEntryKind::File,
                ..IndexedEntry::legacy("", "", IndexedEntryKind::File)
            },
            IndexedEntry {
                path: "/workspace/QuickFox/agents.txt".to_owned(),
                name: "agents.txt".to_owned(),
                extension: Some("txt".to_owned()),
                kind: IndexedEntryKind::File,
                ..IndexedEntry::legacy("", "", IndexedEntryKind::File)
            },
            IndexedEntry {
                path: "/workspace/QuickFox/README.md".to_owned(),
                name: "README.md".to_owned(),
                extension: Some("md".to_owned()),
                kind: IndexedEntryKind::File,
                ..IndexedEntry::legacy("", "", IndexedEntryKind::File)
            },
        ]);

        let extension_index = ExtensionIndex::build(&table);
        let token_index = NameTokenIndex::build(&table);

        assert_eq!(extension_index.lookup("md"), vec![EntryId(0), EntryId(2)]);
        assert_eq!(
            intersect_sorted_ids(extension_index.lookup("md"), token_index.lookup("agents")),
            vec![EntryId(0)]
        );
    }

    #[test]
    fn path_segment_index_handles_windows_and_unix_paths() {
        let table = EntryTable::from_entries(vec![
            IndexedEntry {
                path: "D:\\workspace\\QuickFox\\docs\\AGENTS.md".to_owned(),
                name: "AGENTS.md".to_owned(),
                parent: "D:\\workspace\\QuickFox\\docs".to_owned(),
                kind: IndexedEntryKind::File,
                ..IndexedEntry::legacy("", "", IndexedEntryKind::File)
            },
            IndexedEntry {
                path: "/Users/frank/workspace/Other/notes.md".to_owned(),
                name: "notes.md".to_owned(),
                parent: "/Users/frank/workspace/Other".to_owned(),
                kind: IndexedEntryKind::File,
                ..IndexedEntry::legacy("", "", IndexedEntryKind::File)
            },
            IndexedEntry {
                path: "/Users/frank/Downloads/report.md".to_owned(),
                name: "report.md".to_owned(),
                parent: "/Users/frank/Downloads".to_owned(),
                kind: IndexedEntryKind::File,
                ..IndexedEntry::legacy("", "", IndexedEntryKind::File)
            },
        ]);

        let path_index = PathSegmentIndex::build(&table);

        assert_eq!(path_index.lookup("workspace"), vec![EntryId(0), EntryId(1)]);
        assert_eq!(path_index.lookup("quickfox"), vec![EntryId(0)]);
        assert_eq!(path_index.lookup("downloads"), vec![EntryId(2)]);
    }

    #[test]
    fn compact_candidate_index_reports_bounded_low_hit_retrieval() {
        let entries: Vec<_> = (0..1_000)
            .map(|index| {
                IndexedEntry::legacy(
                    format!("/workspace/project/file-{index:04}.md"),
                    format!("file-{index:04}.md"),
                    IndexedEntryKind::File,
                )
            })
            .collect();
        let index = CompactCandidateIndex::from_entries(entries);

        let low_hit = index.retrieve_ordinary_term("needle-not-present");
        let high_hit = index.retrieve_ordinary_term("file");

        assert_eq!(low_hit.candidates.len(), 0);
        assert_eq!(low_hit.stats.indexed_entry_count, 1_000);
        assert_eq!(low_hit.stats.candidate_count, 0);
        assert!(!low_hit.stats.used_full_scan);
        assert_eq!(high_hit.candidates.len(), 1_000);
        assert_eq!(high_hit.stats.candidate_count, 1_000);
        assert!(!high_hit.stats.used_full_scan);
    }

    #[test]
    fn compact_candidate_index_retrieves_structured_file_query_fields() {
        let index = CompactCandidateIndex::from_entries(vec![
            IndexedEntry {
                path: "/workspace/QuickFox/docs/AGENTS.md".to_owned(),
                name: "AGENTS.md".to_owned(),
                parent: "/workspace/QuickFox/docs".to_owned(),
                extension: Some("md".to_owned()),
                kind: IndexedEntryKind::File,
                ..IndexedEntry::legacy("", "", IndexedEntryKind::File)
            },
            IndexedEntry {
                path: "/workspace/QuickFox/docs/AGENTS.txt".to_owned(),
                name: "AGENTS.txt".to_owned(),
                parent: "/workspace/QuickFox/docs".to_owned(),
                extension: Some("txt".to_owned()),
                kind: IndexedEntryKind::File,
                ..IndexedEntry::legacy("", "", IndexedEntryKind::File)
            },
            IndexedEntry {
                path: "/downloads/AGENTS.md".to_owned(),
                name: "AGENTS.md".to_owned(),
                parent: "/downloads".to_owned(),
                extension: Some("md".to_owned()),
                kind: IndexedEntryKind::File,
                ..IndexedEntry::legacy("", "", IndexedEntryKind::File)
            },
        ]);

        let retrieval = index.retrieve_query("name:agents type:md dir:workspace");

        assert_eq!(retrieval.candidates, vec![EntryId(0)]);
        assert_eq!(retrieval.stats.candidate_count, 1);
        assert!(!retrieval.stats.used_full_scan);

        let mismatched_name = index.retrieve_query("name:agents.txt type:md dir:workspace");
        assert!(mismatched_name.candidates.is_empty());
        assert!(!mismatched_name.stats.used_full_scan);
    }

    #[test]
    fn compact_candidate_index_marks_full_scan_fallbacks_for_unsupported_narrowing() {
        let index = CompactCandidateIndex::from_entries(vec![
            IndexedEntry::legacy(
                "/Users/frank/workspace/QuickFox/budget.PDF",
                "budget.PDF",
                IndexedEntryKind::File,
            ),
            IndexedEntry::legacy(
                "/Users/frank/downloads/QuickFox/budget.PDF",
                "budget.PDF",
                IndexedEntryKind::File,
            ),
        ]);

        let glob = index.retrieve_query("dir:**/workspace/QuickFox budget");
        assert!(glob.stats.used_full_scan);
        assert_eq!(glob.stats.candidate_count, 2);

        let partial_path = index.retrieve_query("/workspace/QuickFox");
        assert!(partial_path.stats.used_full_scan);
        assert_eq!(partial_path.stats.candidate_count, 2);
    }
}
