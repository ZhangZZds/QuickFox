//! Compact search index data structures.

use std::collections::{BTreeMap, HashMap};

#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::core::file_query::FileQuery;
use crate::core::index_entry::{
    build_search_text, ContentIndexState, IndexedEntry, IndexedEntryKind,
};

#[cfg(test)]
static COMPACT_INDEX_BUILD_COUNT: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EntryId(u32);

impl EntryId {
    pub fn as_usize(self) -> usize {
        self.0 as usize
    }

    pub fn from_usize(value: usize) -> Option<Self> {
        u32::try_from(value).ok().map(Self)
    }
}

#[derive(Debug, Clone, Default)]
pub struct EntryIdAllocator {
    next: u32,
}

impl EntryIdAllocator {
    pub fn next_id(&mut self) -> EntryId {
        let id = EntryId(self.next);
        self.next = self.next.saturating_add(1);
        id
    }

    pub fn len(&self) -> usize {
        self.next as usize
    }

    pub fn is_empty(&self) -> bool {
        self.next == 0
    }
}

const MISSING_RANGE_START: u32 = u32::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PackedTextRange {
    start: u32,
    len: u32,
}

impl PackedTextRange {
    const MISSING: Self = Self {
        start: MISSING_RANGE_START,
        len: 0,
    };

    fn is_missing(self) -> bool {
        self.start == MISSING_RANGE_START
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompactIndexBuildError {
    TooManyEntries { count: usize },
    ArenaTooLarge { requested_bytes: usize },
    DepthTooLarge { depth: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactEntry {
    pub id: EntryId,
    path: PackedTextRange,
    name: PackedTextRange,
    pub kind: IndexedEntryKind,
    parent: PackedTextRange,
    extension: PackedTextRange,
    pub depth: u32,
    root: PackedTextRange,
    pub modified_ms: Option<i64>,
    pub size_bytes: Option<u64>,
    search_text: PackedTextRange,
    pub content_index_state: ContentIndexState,
}

#[derive(Debug, Clone, Default)]
pub struct EntryTable {
    entries: Vec<CompactEntry>,
    arena: Vec<u8>,
    unique_value_count: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct CompactEntryRef<'a> {
    table: &'a EntryTable,
    entry: &'a CompactEntry,
}

impl CompactEntryRef<'_> {
    pub fn path(&self) -> &str {
        self.table.path(self.entry).unwrap_or_default()
    }

    pub fn name(&self) -> &str {
        self.table.name(self.entry).unwrap_or_default()
    }

    pub fn parent(&self) -> &str {
        self.table.parent(self.entry).unwrap_or_default()
    }

    pub fn extension(&self) -> Option<&str> {
        self.table.extension(self.entry)
    }

    pub fn search_text(&self) -> std::borrow::Cow<'_, str> {
        self.table.search_text(self.entry)
    }

    pub fn match_search_text(&self) -> &str {
        self.table
            .text(self.entry.search_text)
            .unwrap_or_else(|| self.path())
    }

    pub fn has_custom_search_text(&self) -> bool {
        !self.entry.search_text.is_missing()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryTableMemoryEstimate {
    pub entry_count: usize,
    pub entry_struct_bytes: usize,
    pub string_pool_unique_values: usize,
    pub string_pool_bytes: usize,
    pub string_pool_heap_bytes: usize,
    pub retained_build_interner_bytes: usize,
}

impl EntryTableMemoryEstimate {
    pub fn total_resident_bytes(&self) -> usize {
        std::mem::size_of::<EntryTable>()
            .saturating_add(self.entry_struct_bytes)
            .saturating_add(self.string_pool_heap_bytes)
    }
}

impl EntryTable {
    pub fn from_entries(entries: Vec<IndexedEntry>) -> Self {
        Self::try_from_entries(entries).expect("compact entry table exceeds packed u32 limits")
    }

    pub fn try_from_entries(entries: Vec<IndexedEntry>) -> Result<Self, CompactIndexBuildError> {
        if entries.len() > u32::MAX as usize {
            return Err(CompactIndexBuildError::TooManyEntries {
                count: entries.len(),
            });
        }

        let mut table = Self::default();
        let mut arena = PackedArenaBuilder::default();
        let mut ids = EntryIdAllocator::default();

        for entry in entries {
            let id = ids.next_id();
            let depth = u32::try_from(entry.depth)
                .map_err(|_| CompactIndexBuildError::DepthTooLarge { depth: entry.depth })?;
            let path = arena.store(&entry.path)?;
            let name = arena.store_path_slice_or_value(path, &entry.path, &entry.name)?;
            let parent = arena.store_optional_path_slice(path, &entry.path, &entry.parent)?;
            let extension = match entry.extension.as_deref() {
                Some(extension) if !extension.is_empty() => {
                    arena.store_path_slice_or_value(path, &entry.path, extension)?
                }
                _ => PackedTextRange::MISSING,
            };
            let root = arena.store_optional_path_slice(path, &entry.path, &entry.root)?;
            let standard_search_text = build_search_text(&entry.name, &entry.path);
            let search_text =
                if entry.search_text.is_empty() || entry.search_text == standard_search_text {
                    PackedTextRange::MISSING
                } else {
                    arena.store(&entry.search_text)?
                };
            table.entries.push(CompactEntry {
                id,
                path,
                name,
                kind: entry.kind,
                parent,
                extension,
                depth,
                root,
                modified_ms: entry.modified_ms,
                size_bytes: entry.size_bytes,
                search_text,
                content_index_state: entry.content_index_state,
            });
        }

        table.unique_value_count = arena.unique_value_count();
        table.arena = arena.finish();
        Ok(table)
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
        self.text(entry.path)
    }

    pub fn name(&self, entry: &CompactEntry) -> Option<&str> {
        self.text(entry.name)
    }

    pub fn parent(&self, entry: &CompactEntry) -> Option<&str> {
        self.text(entry.parent)
    }

    pub fn extension(&self, entry: &CompactEntry) -> Option<&str> {
        self.text(entry.extension)
    }

    pub fn root(&self, entry: &CompactEntry) -> Option<&str> {
        self.text(entry.root)
    }

    pub fn search_text<'a>(&'a self, entry: &'a CompactEntry) -> std::borrow::Cow<'a, str> {
        self.text(entry.search_text)
            .map(std::borrow::Cow::Borrowed)
            .unwrap_or_else(|| {
                std::borrow::Cow::Owned(build_search_text(
                    self.name(entry).unwrap_or_default(),
                    self.path(entry).unwrap_or_default(),
                ))
            })
    }

    pub fn materialize(&self, id: EntryId) -> Option<IndexedEntry> {
        let entry = self.get(id)?;
        Some(IndexedEntry {
            path: self.path(entry)?.to_owned(),
            name: self.name(entry)?.to_owned(),
            kind: entry.kind.clone(),
            parent: self.parent(entry).unwrap_or_default().to_owned(),
            extension: self.extension(entry).map(str::to_owned),
            depth: entry.depth as usize,
            root: self.root(entry).unwrap_or_default().to_owned(),
            modified_ms: entry.modified_ms,
            size_bytes: entry.size_bytes,
            search_text: self.search_text(entry).into_owned(),
            content_index_state: entry.content_index_state.clone(),
        })
    }

    fn text(&self, range: PackedTextRange) -> Option<&str> {
        if range.is_missing() {
            return None;
        }
        let start = range.start as usize;
        let end = start.checked_add(range.len as usize)?;
        std::str::from_utf8(self.arena.get(start..end)?).ok()
    }

    pub fn path_by_id(&self, id: EntryId) -> Option<&str> {
        self.get(id).and_then(|entry| self.path(entry))
    }

    pub fn string_pool_len(&self) -> usize {
        self.unique_value_count
    }

    pub fn memory_estimate(&self) -> EntryTableMemoryEstimate {
        EntryTableMemoryEstimate {
            entry_count: self.entries.len(),
            entry_struct_bytes: self
                .entries
                .capacity()
                .saturating_mul(std::mem::size_of::<CompactEntry>()),
            string_pool_unique_values: self.unique_value_count,
            string_pool_bytes: self.arena.len(),
            string_pool_heap_bytes: self.arena.capacity(),
            retained_build_interner_bytes: 0,
        }
    }

    pub fn entries(&self) -> &[CompactEntry] {
        &self.entries
    }

    pub fn all_ids(&self) -> Vec<EntryId> {
        self.entries.iter().map(|entry| entry.id).collect()
    }
}

#[derive(Debug, Default)]
struct PackedArenaBuilder {
    arena: Vec<u8>,
    ranges_by_value: HashMap<String, PackedTextRange>,
}

impl PackedArenaBuilder {
    fn store(&mut self, value: &str) -> Result<PackedTextRange, CompactIndexBuildError> {
        if let Some(range) = self.ranges_by_value.get(value) {
            return Ok(*range);
        }
        let requested_bytes = self.arena.len().saturating_add(value.len());
        if requested_bytes >= MISSING_RANGE_START as usize {
            return Err(CompactIndexBuildError::ArenaTooLarge { requested_bytes });
        }
        let range = PackedTextRange {
            start: self.arena.len() as u32,
            len: value.len() as u32,
        };
        self.arena.extend_from_slice(value.as_bytes());
        self.ranges_by_value.insert(value.to_owned(), range);
        Ok(range)
    }

    fn store_path_slice_or_value(
        &mut self,
        path_range: PackedTextRange,
        path: &str,
        value: &str,
    ) -> Result<PackedTextRange, CompactIndexBuildError> {
        if let Some(offset) = path.rfind(value) {
            return Ok(PackedTextRange {
                start: path_range.start.saturating_add(offset as u32),
                len: value.len() as u32,
            });
        }
        self.store(value)
    }

    fn store_optional_path_slice(
        &mut self,
        path_range: PackedTextRange,
        path: &str,
        value: &str,
    ) -> Result<PackedTextRange, CompactIndexBuildError> {
        if value.is_empty() {
            Ok(PackedTextRange::MISSING)
        } else {
            self.store_path_slice_or_value(path_range, path, value)
        }
    }

    fn unique_value_count(&self) -> usize {
        self.ranges_by_value.len()
    }

    fn finish(self) -> Vec<u8> {
        self.arena
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
    #[cfg(test)]
    build_id: usize,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactCandidateMemoryStats {
    pub entry_count: usize,
    pub prefix_key_count: usize,
    pub retained_build_interner_bytes: usize,
    pub heap_bytes: usize,
    pub total_resident_bytes: usize,
}

impl CompactCandidateIndex {
    pub fn from_entries(entries: Vec<IndexedEntry>) -> Self {
        #[cfg(test)]
        let build_id = COMPACT_INDEX_BUILD_COUNT
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);

        let table = EntryTable::from_entries(entries);
        Self {
            name_tokens: NameTokenIndex::build(&table),
            prefixes: PrefixIndex::build(&table),
            name_trigrams: NameTrigramIndex::build(&table),
            extensions: ExtensionIndex::build(&table),
            path_segments: PathSegmentIndex::build(&table),
            exact_paths: ExactPathIndex::build(&table),
            table,
            #[cfg(test)]
            build_id,
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

    #[cfg(test)]
    pub fn build_id(&self) -> usize {
        self.build_id
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

    pub fn entry_count(&self) -> usize {
        self.table.len()
    }

    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }

    pub fn materialize(&self, id: EntryId) -> Option<IndexedEntry> {
        self.table.materialize(id)
    }

    pub fn entry_ref(&self, id: EntryId) -> Option<CompactEntryRef<'_>> {
        self.table.get(id).map(|entry| CompactEntryRef {
            table: &self.table,
            entry,
        })
    }

    pub fn materialized_entries(&self) -> Vec<IndexedEntry> {
        self.table
            .entries()
            .iter()
            .filter_map(|entry| self.table.materialize(entry.id))
            .collect()
    }

    pub fn estimated_bytes(&self) -> usize {
        std::mem::size_of::<Self>().saturating_add(self.estimated_heap_bytes())
    }

    fn estimated_heap_bytes(&self) -> usize {
        self.table
            .estimated_heap_bytes()
            .saturating_add(string_ids_map_heap_bytes(&self.name_tokens.ids_by_token))
            .saturating_add(string_ids_map_heap_bytes(&self.prefixes.ids_by_prefix))
            .saturating_add(string_ids_map_heap_bytes(
                &self.name_trigrams.ids_by_trigram,
            ))
            .saturating_add(string_ids_map_heap_bytes(&self.extensions.ids_by_extension))
            .saturating_add(string_ids_map_heap_bytes(
                &self.path_segments.ids_by_segment,
            ))
            .saturating_add(string_ids_map_heap_bytes(&self.path_segments.ids_by_prefix))
            .saturating_add(string_ids_map_heap_bytes(
                &self.path_segments.ids_by_fuzzy_key,
            ))
            .saturating_add(exact_path_map_heap_bytes(&self.exact_paths.id_by_path))
    }

    pub fn memory_stats(&self) -> CompactCandidateMemoryStats {
        let heap_bytes = self.estimated_heap_bytes();
        CompactCandidateMemoryStats {
            entry_count: self.table.len(),
            prefix_key_count: self
                .prefixes
                .ids_by_prefix
                .len()
                .saturating_add(self.path_segments.ids_by_prefix.len()),
            retained_build_interner_bytes: 0,
            heap_bytes,
            total_resident_bytes: std::mem::size_of::<Self>().saturating_add(heap_bytes),
        }
    }
}

impl EntryTable {
    fn estimated_heap_bytes(&self) -> usize {
        self.entries
            .capacity()
            .saturating_mul(std::mem::size_of::<CompactEntry>())
            .saturating_add(self.arena.capacity())
    }
}

fn string_ids_map_heap_bytes(map: &BTreeMap<String, Vec<EntryId>>) -> usize {
    let payload: usize = map
        .iter()
        .map(|(key, ids)| {
            key.capacity()
                .saturating_add(
                    ids.capacity()
                        .saturating_mul(std::mem::size_of::<EntryId>()),
                )
                .saturating_add(std::mem::size_of::<(String, Vec<EntryId>)>())
                .saturating_add(std::mem::size_of::<usize>().saturating_mul(3))
        })
        .sum();
    payload
}

fn exact_path_map_heap_bytes(map: &BTreeMap<String, EntryId>) -> usize {
    let payload: usize = map
        .keys()
        .map(|key| {
            key.capacity()
                .saturating_add(std::mem::size_of::<(String, EntryId)>())
                .saturating_add(std::mem::size_of::<usize>().saturating_mul(3))
        })
        .sum();
    payload
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
    fn packed_entry_table_round_trips_all_authoritative_entry_fields() {
        let expected = IndexedEntry {
            path: "D:\\workspace\\QuickFox\\AGENTS.md".to_owned(),
            name: "AGENTS.md".to_owned(),
            kind: IndexedEntryKind::File,
            parent: "D:\\workspace\\QuickFox".to_owned(),
            extension: Some("md".to_owned()),
            depth: 3,
            root: "D:\\".to_owned(),
            modified_ms: Some(123),
            size_bytes: Some(456),
            search_text: "custom searchable alias".to_owned(),
            content_index_state: crate::core::index_entry::ContentIndexState::Indexed,
        };

        let table = EntryTable::try_from_entries(vec![expected.clone()]).unwrap();

        assert_eq!(table.materialize(EntryId(0)), Some(expected));
    }

    #[test]
    fn packed_entry_table_reuses_path_slices_for_standard_fields() {
        let path = "D:\\workspace\\QuickFox\\AGENTS.md";
        let entry = IndexedEntry {
            path: path.to_owned(),
            name: "AGENTS.md".to_owned(),
            kind: IndexedEntryKind::File,
            parent: "D:\\workspace\\QuickFox".to_owned(),
            extension: Some("md".to_owned()),
            depth: 3,
            root: "D:\\".to_owned(),
            search_text: crate::core::index_entry::build_search_text("AGENTS.md", path),
            ..IndexedEntry::legacy("", "", IndexedEntryKind::File)
        };

        let table = EntryTable::try_from_entries(vec![entry]).unwrap();

        assert_eq!(table.memory_estimate().string_pool_bytes, path.len());
    }

    #[test]
    fn packed_entry_table_stores_only_custom_search_text_as_an_exception() {
        let path = "/workspace/QuickFox/AGENTS.md";
        let custom_search_text = "agents documentation shortcut";
        let entry = IndexedEntry {
            path: path.to_owned(),
            name: "AGENTS.md".to_owned(),
            kind: IndexedEntryKind::File,
            parent: "/workspace/QuickFox".to_owned(),
            extension: Some("md".to_owned()),
            depth: 3,
            root: "/workspace".to_owned(),
            search_text: custom_search_text.to_owned(),
            ..IndexedEntry::legacy("", "", IndexedEntryKind::File)
        };

        let table = EntryTable::try_from_entries(vec![entry]).unwrap();

        assert_eq!(
            table.memory_estimate().string_pool_bytes,
            path.len() + custom_search_text.len()
        );
        assert_eq!(
            table.search_text(table.get(EntryId(0)).unwrap()),
            custom_search_text
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
    fn entry_table_does_not_retain_its_build_interner() {
        let table = EntryTable::from_entries(vec![IndexedEntry::legacy(
            "D:\\workspace\\QuickFox\\AGENTS.md",
            "AGENTS.md",
            IndexedEntryKind::File,
        )]);

        assert_eq!(table.memory_estimate().retained_build_interner_bytes, 0);
    }

    #[test]
    fn candidate_prefix_key_growth_is_bounded_per_entry() {
        let index = CompactCandidateIndex::from_entries(vec![
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
        let stats = index.memory_stats();

        assert!(
            stats.prefix_key_count <= stats.entry_count.saturating_mul(2),
            "unbounded prefix storage: {stats:#?}"
        );
    }

    #[test]
    fn compact_memory_total_counts_the_inline_struct_once() {
        let index = CompactCandidateIndex::from_entries(vec![IndexedEntry::legacy(
            "D:\\workspace\\QuickFox\\AGENTS.md",
            "AGENTS.md",
            IndexedEntryKind::File,
        )]);
        let stats = index.memory_stats();

        assert_eq!(
            stats.total_resident_bytes,
            std::mem::size_of::<CompactCandidateIndex>().saturating_add(stats.heap_bytes)
        );
    }

    #[test]
    fn entry_table_memory_total_counts_the_inline_string_pool_once() {
        let table = EntryTable::from_entries(vec![IndexedEntry::legacy(
            "D:\\workspace\\QuickFox\\AGENTS.md",
            "AGENTS.md",
            IndexedEntryKind::File,
        )]);
        let estimate = table.memory_estimate();

        assert_eq!(
            estimate.total_resident_bytes(),
            std::mem::size_of::<EntryTable>()
                .saturating_add(estimate.entry_struct_bytes)
                .saturating_add(estimate.string_pool_heap_bytes)
        );
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
