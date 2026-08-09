//! Compact search index data structures.

use std::cmp::Ordering as CmpOrdering;
use std::collections::HashMap;

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
    TooManySegmentPostings { count: usize },
    TooManyPathSegmentBuckets { count: usize },
    TooManyNameNgramBytes { count: usize },
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
        Self::try_from_entries_with_arena_limit(entries, MISSING_RANGE_START as usize - 1)
    }

    fn try_from_entries_with_arena_limit(
        entries: Vec<IndexedEntry>,
        arena_limit: usize,
    ) -> Result<Self, CompactIndexBuildError> {
        if entries.len() > u32::MAX as usize {
            return Err(CompactIndexBuildError::TooManyEntries {
                count: entries.len(),
            });
        }

        let mut table = Self::default();
        let mut arena = PackedArenaBuilder::with_limit(arena_limit);
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

        table.entries.shrink_to_fit();
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

#[derive(Debug)]
struct PackedArenaBuilder {
    arena: Vec<u8>,
    ranges_by_value: HashMap<String, PackedTextRange>,
    arena_limit: usize,
}

impl PackedArenaBuilder {
    fn with_limit(arena_limit: usize) -> Self {
        Self {
            arena: Vec::new(),
            ranges_by_value: HashMap::new(),
            arena_limit: arena_limit.min(MISSING_RANGE_START as usize - 1),
        }
    }

    fn store(&mut self, value: &str) -> Result<PackedTextRange, CompactIndexBuildError> {
        if let Some(range) = self.ranges_by_value.get(value) {
            return Ok(*range);
        }
        let requested_bytes = self.arena.len().saturating_add(value.len());
        if requested_bytes > self.arena_limit {
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

    fn finish(mut self) -> Vec<u8> {
        self.arena.shrink_to_fit();
        self.arena
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct FingerprintPosting {
    fingerprint: u32,
    id: EntryId,
}

#[derive(Debug, Clone, Default)]
struct FingerprintIndex {
    postings: Vec<FingerprintPosting>,
}

impl FingerprintIndex {
    fn from_pairs(pairs: impl IntoIterator<Item = (u32, EntryId)>) -> Self {
        let mut postings: Vec<_> = pairs
            .into_iter()
            .map(|(fingerprint, id)| FingerprintPosting { fingerprint, id })
            .collect();
        postings.sort_unstable();
        postings.dedup();
        postings.shrink_to_fit();
        Self { postings }
    }

    fn matching_ids(&self, fingerprint: u32) -> impl Iterator<Item = EntryId> + '_ {
        let start = self
            .postings
            .partition_point(|posting| posting.fingerprint < fingerprint);
        let end = self
            .postings
            .partition_point(|posting| posting.fingerprint <= fingerprint);
        self.postings[start..end].iter().map(|posting| posting.id)
    }

    fn heap_bytes(&self) -> usize {
        self.postings
            .capacity()
            .saturating_mul(std::mem::size_of::<FingerprintPosting>())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NameNgramPosting {
    width: u8,
    fingerprint: u32,
    start: u32,
    len: u32,
}

#[derive(Debug, Clone, Default)]
pub struct NameNgramIndex {
    postings: Vec<NameNgramPosting>,
    encoded_ids: Vec<u8>,
}

impl NameNgramIndex {
    pub fn build(table: &EntryTable) -> Result<Self, CompactIndexBuildError> {
        let mut ids_by_ngram: HashMap<(u8, u32), Vec<EntryId>> = HashMap::new();
        for entry in table.entries() {
            for (width, fingerprint) in name_ngram_keys(table.name(entry).unwrap_or_default()) {
                ids_by_ngram
                    .entry((width, fingerprint))
                    .or_default()
                    .push(entry.id);
            }
        }

        let mut groups: Vec<_> = ids_by_ngram.into_iter().collect();
        groups.sort_unstable_by_key(|((width, fingerprint), _)| (*width, *fingerprint));

        let mut postings = Vec::with_capacity(groups.len());
        let mut encoded_ids = Vec::new();
        for ((width, fingerprint), ids) in groups {
            let start = encoded_ids.len();
            let mut previous = 0_u32;
            for id in ids {
                let value = id.0;
                encode_varint(value.saturating_sub(previous), &mut encoded_ids);
                previous = value;
            }
            let len = encoded_ids.len().saturating_sub(start);
            postings.push(NameNgramPosting {
                width,
                fingerprint,
                start: u32::try_from(start).map_err(|_| {
                    CompactIndexBuildError::TooManyNameNgramBytes {
                        count: encoded_ids.len(),
                    }
                })?,
                len: u32::try_from(len).map_err(|_| {
                    CompactIndexBuildError::TooManyNameNgramBytes {
                        count: encoded_ids.len(),
                    }
                })?,
            });
        }
        postings.shrink_to_fit();
        encoded_ids.shrink_to_fit();
        Ok(Self {
            postings,
            encoded_ids,
        })
    }

    pub fn lookup(&self, table: &EntryTable, term: &str) -> Vec<EntryId> {
        let Some((width, ngram)) = query_ngram(term) else {
            return Vec::new();
        };
        let fingerprint = text_fingerprint(&ngram);
        let start = self
            .postings
            .partition_point(|posting| (posting.width, posting.fingerprint) < (width, fingerprint));
        let candidates = self.postings[start..]
            .iter()
            .take_while(|posting| (posting.width, posting.fingerprint) == (width, fingerprint))
            .flat_map(|posting| self.decode_ids(*posting));
        verified_ids(candidates, |id| {
            table
                .get(id)
                .and_then(|entry| table.name(entry))
                .is_some_and(|name| contains_ascii_case_insensitive(name, &ngram))
        })
    }

    pub fn lookup_full(&self, table: &EntryTable, term: &str) -> Vec<EntryId> {
        let normalized = term.to_ascii_lowercase();
        verified_ids(self.lookup(table, &normalized), |id| {
            table
                .get(id)
                .and_then(|entry| table.name(entry))
                .is_some_and(|name| contains_ascii_case_insensitive(name, &normalized))
        })
    }

    fn decode_ids(&self, posting: NameNgramPosting) -> Vec<EntryId> {
        let start = posting.start as usize;
        let end = start.saturating_add(posting.len as usize);
        let Some(bytes) = self.encoded_ids.get(start..end) else {
            return Vec::new();
        };
        let mut ids = Vec::new();
        let mut offset = 0;
        let mut previous = 0_u32;
        while offset < bytes.len() {
            let Some((delta, consumed)) = decode_varint(&bytes[offset..]) else {
                return Vec::new();
            };
            let Some(value) = previous.checked_add(delta) else {
                return Vec::new();
            };
            ids.push(EntryId(value));
            previous = value;
            offset = offset.saturating_add(consumed);
        }
        ids
    }

    fn heap_bytes(&self) -> usize {
        self.postings
            .capacity()
            .saturating_mul(std::mem::size_of::<NameNgramPosting>())
            .saturating_add(self.encoded_ids.capacity())
    }
}

#[derive(Debug, Clone, Default)]
pub struct ExtensionIndex {
    extensions: FingerprintIndex,
}

impl ExtensionIndex {
    pub fn build(table: &EntryTable) -> Self {
        let pairs = table.entries().iter().filter_map(|entry| {
            table
                .extension(entry)
                .map(str::to_owned)
                .or_else(|| entry_name_extension(table.name(entry).unwrap_or_default()))
                .map(|extension| (text_fingerprint(&extension), entry.id))
        });
        Self {
            extensions: FingerprintIndex::from_pairs(pairs),
        }
    }

    pub fn lookup(&self, table: &EntryTable, extension: &str) -> Vec<EntryId> {
        let normalized = extension.trim_start_matches('.').to_ascii_lowercase();
        verified_ids(
            self.extensions.matching_ids(text_fingerprint(&normalized)),
            |id| {
                table.get(id).is_some_and(|entry| {
                    table
                        .extension(entry)
                        .map(str::to_owned)
                        .or_else(|| entry_name_extension(table.name(entry).unwrap_or_default()))
                        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(&normalized))
                })
            },
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SegmentBucket {
    text: PackedTextRange,
    start: u32,
    len: u32,
}

#[derive(Debug, Clone, Default)]
pub struct PathSegmentIndex {
    buckets: Vec<SegmentBucket>,
    ids: Vec<EntryId>,
    fuzzy_buckets: FingerprintIndex,
}

impl PathSegmentIndex {
    pub fn build(table: &EntryTable) -> Result<Self, CompactIndexBuildError> {
        Self::build_with_limits(table, u32::MAX as usize, u32::MAX as usize)
    }

    fn build_with_limits(
        table: &EntryTable,
        max_postings: usize,
        max_buckets: usize,
    ) -> Result<Self, CompactIndexBuildError> {
        Self::build_with_limits_impl(table, max_postings, max_buckets)
    }

    fn build_with_limits_impl(
        table: &EntryTable,
        max_postings: usize,
        max_buckets: usize,
    ) -> Result<Self, CompactIndexBuildError> {
        let mut grouped: HashMap<String, (PackedTextRange, Vec<EntryId>)> = HashMap::new();
        for entry in table.entries() {
            let Some(path) = table.path(entry) else {
                continue;
            };
            let mut ranges = path_segment_ranges(path, entry.path.start);
            if entry.kind != IndexedEntryKind::Directory {
                ranges.pop();
            }
            for text in ranges {
                let segment = table.text(text).unwrap_or_default();
                let key = segment.to_ascii_lowercase();
                let (_, ids) = grouped.entry(key).or_insert_with(|| (text, Vec::new()));
                ids.push(entry.id);
            }
        }

        let mut grouped: Vec<_> = grouped.into_iter().collect();
        grouped.sort_unstable_by(|(_, left), (_, right)| {
            compare_ascii_case_insensitive(
                table.text(left.0).unwrap_or_default(),
                table.text(right.0).unwrap_or_default(),
            )
        });

        let total_ids = grouped.iter().map(|(_, (_, ids))| ids.len()).sum();
        let mut buckets = Vec::with_capacity(grouped.len());
        let mut ids = Vec::with_capacity(total_ids);
        let mut fuzzy_pairs = Vec::with_capacity(grouped.len());
        for (bucket_index, (_, (text, mut bucket_ids))) in grouped.into_iter().enumerate() {
            bucket_ids.sort_unstable();
            bucket_ids.dedup();
            let start = ids.len();
            let posting_count = start.saturating_add(bucket_ids.len());
            if posting_count > max_postings || posting_count > u32::MAX as usize {
                return Err(CompactIndexBuildError::TooManySegmentPostings {
                    count: posting_count,
                });
            }
            ids.extend(bucket_ids);
            let bucket = SegmentBucket {
                text,
                start: u32::try_from(start)
                    .map_err(|_| CompactIndexBuildError::TooManySegmentPostings { count: start })?,
                len: u32::try_from(ids.len().saturating_sub(start)).map_err(|_| {
                    CompactIndexBuildError::TooManySegmentPostings {
                        count: ids.len().saturating_sub(start),
                    }
                })?,
            };
            if let Some(key) = fuzzy_segment_key(table.text(text).unwrap_or_default()) {
                if bucket_index > max_buckets || bucket_index > u32::MAX as usize {
                    return Err(CompactIndexBuildError::TooManyPathSegmentBuckets {
                        count: bucket_index.saturating_add(1),
                    });
                }
                fuzzy_pairs.push((
                    text_fingerprint(&key),
                    EntryId::from_usize(bucket_index).ok_or(
                        CompactIndexBuildError::TooManyPathSegmentBuckets {
                            count: bucket_index.saturating_add(1),
                        },
                    )?,
                ));
            }
            buckets.push(bucket);
        }
        buckets.shrink_to_fit();
        ids.shrink_to_fit();
        Ok(Self {
            buckets,
            ids,
            fuzzy_buckets: FingerprintIndex::from_pairs(fuzzy_pairs),
        })
    }

    pub fn lookup(&self, table: &EntryTable, segment: &str) -> Vec<EntryId> {
        let normalized = segment.to_ascii_lowercase();
        let precise = self.lookup_precise(table, &normalized);
        if !precise.is_empty() {
            return precise;
        }
        self.lookup_fuzzy(table, &normalized)
    }

    fn lookup_precise(&self, table: &EntryTable, segment: &str) -> Vec<EntryId> {
        let matching = self.matching_buckets(table, segment);
        if let Some(exact) = matching.iter().find(|bucket| {
            table
                .text(bucket.text)
                .is_some_and(|text| text.eq_ignore_ascii_case(segment))
        }) {
            return self.bucket_ids(*exact);
        }
        if !matching.is_empty() {
            return self.collect_bucket_ids(matching);
        }

        Vec::new()
    }

    fn lookup_fuzzy(&self, table: &EntryTable, segment: &str) -> Vec<EntryId> {
        let Some(key) = fuzzy_segment_key(segment) else {
            return Vec::new();
        };
        let query_mask = ascii_alphabetic_mask(segment);
        let matching: Vec<_> = self
            .fuzzy_buckets
            .matching_ids(text_fingerprint(&key))
            .filter_map(|id| self.buckets.get(id.as_usize()).copied())
            .filter(|bucket| {
                table.text(bucket.text).is_some_and(|text| {
                    fuzzy_segment_key(text).as_deref() == Some(key.as_str())
                        && query_mask.is_none_or(|mask| {
                            ascii_alphabetic_mask(text)
                                .is_none_or(|text_mask| text_mask & mask == mask)
                        })
                })
            })
            .collect();
        self.collect_bucket_ids(matching)
    }

    fn matching_buckets(&self, table: &EntryTable, prefix: &str) -> Vec<SegmentBucket> {
        let start = self.buckets.partition_point(|bucket| {
            compare_ascii_case_insensitive(table.text(bucket.text).unwrap_or_default(), prefix)
                == CmpOrdering::Less
        });
        self.buckets[start..]
            .iter()
            .copied()
            .take_while(|bucket| {
                starts_with_ascii_case_insensitive(
                    table.text(bucket.text).unwrap_or_default(),
                    prefix,
                )
            })
            .collect()
    }

    fn bucket_ids(&self, bucket: SegmentBucket) -> Vec<EntryId> {
        let start = bucket.start as usize;
        let end = start.saturating_add(bucket.len as usize);
        self.ids.get(start..end).unwrap_or_default().to_vec()
    }

    fn collect_bucket_ids(&self, buckets: Vec<SegmentBucket>) -> Vec<EntryId> {
        let mut result = Vec::new();
        for bucket in buckets {
            let start = bucket.start as usize;
            let end = start.saturating_add(bucket.len as usize);
            if let Some(ids) = self.ids.get(start..end) {
                result.extend_from_slice(ids);
            }
        }
        result.sort_unstable();
        result.dedup();
        result
    }
}

#[derive(Debug, Clone, Default)]
pub struct ExactPathIndex {
    paths: FingerprintIndex,
}

impl ExactPathIndex {
    pub fn build(table: &EntryTable) -> Self {
        let pairs = table.entries().iter().filter_map(|entry| {
            table
                .path(entry)
                .map(|path| (text_fingerprint(path), entry.id))
        });
        Self {
            paths: FingerprintIndex::from_pairs(pairs),
        }
    }

    pub fn lookup(&self, table: &EntryTable, path: &str) -> Vec<EntryId> {
        verified_ids(self.paths.matching_ids(text_fingerprint(path)), |id| {
            table
                .path_by_id(id)
                .is_some_and(|candidate| candidate.eq_ignore_ascii_case(path))
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct CompactCandidateIndex {
    table: EntryTable,
    name_ngrams: NameNgramIndex,
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
    pub entry_table_heap_bytes: usize,
    pub name_ngram_bytes: usize,
    pub extension_bytes: usize,
    pub path_segment_bytes: usize,
    pub path_fuzzy_bytes: usize,
    pub exact_path_bytes: usize,
    pub heap_bytes: usize,
    pub total_resident_bytes: usize,
}

impl CompactCandidateIndex {
    /// Panicking convenience for tests and already-bounded in-memory overlay batches.
    /// Production baseline construction must use [`Self::try_from_entries`].
    pub fn from_entries(entries: Vec<IndexedEntry>) -> Self {
        Self::try_from_entries(entries)
            .expect("known-size compact candidate build exceeded u32 limits")
    }

    pub fn try_from_entries(entries: Vec<IndexedEntry>) -> Result<Self, CompactIndexBuildError> {
        Self::try_from_entries_with_arena_limit(entries, MISSING_RANGE_START as usize - 1)
    }

    pub(crate) fn try_from_entries_with_arena_limit(
        entries: Vec<IndexedEntry>,
        arena_limit: usize,
    ) -> Result<Self, CompactIndexBuildError> {
        #[cfg(test)]
        let build_id = COMPACT_INDEX_BUILD_COUNT
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);

        let table = EntryTable::try_from_entries_with_arena_limit(entries, arena_limit)?;
        Ok(Self {
            name_ngrams: NameNgramIndex::build(&table)?,
            extensions: ExtensionIndex::build(&table),
            path_segments: PathSegmentIndex::build(&table)?,
            exact_paths: ExactPathIndex::build(&table),
            table,
            #[cfg(test)]
            build_id,
        })
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
            let exact = self.exact_paths.lookup(&self.table, &normalized);
            if exact.is_empty() {
                (self.table.all_ids(), true)
            } else {
                (exact, false)
            }
        } else {
            let name_candidates = self.name_ngrams.lookup(&self.table, &normalized);
            let path_candidates = self.path_segments.lookup_precise(&self.table, &normalized);
            let precise = union_sorted_ids([name_candidates, path_candidates]);
            if !precise.is_empty() {
                (
                    union_sorted_ids([
                        precise,
                        self.path_segments.lookup_fuzzy(&self.table, &normalized),
                    ]),
                    false,
                )
            } else {
                (
                    self.path_segments.lookup_fuzzy(&self.table, &normalized),
                    false,
                )
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
            candidates = intersect_optional_candidates(
                candidates,
                self.extensions.lookup(&self.table, extension),
            );
        }

        for name in &query.name_filters {
            let name_candidates = self.name_ngrams.lookup_full(&self.table, name);
            candidates = intersect_optional_candidates(candidates, name_candidates);
        }

        for dir in &query.dir_filters {
            let dir_candidates = if dir.contains(['*', '?', '[']) {
                used_full_scan = true;
                self.table.all_ids()
            } else {
                self.path_segments.lookup(&self.table, dir)
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

    pub fn retrieve_query_name_priority(&self, query: &str) -> Option<CandidateRetrieval> {
        let parsed = FileQuery::parse(query);
        if parsed.ordinary_terms.len() != 1 {
            return None;
        }

        let term = &parsed.ordinary_terms[0];
        let mut candidates = self.name_ngrams.lookup(&self.table, term);
        if candidates.is_empty() {
            return None;
        }

        for extension in &parsed.type_filters {
            candidates =
                intersect_sorted_ids(candidates, self.extensions.lookup(&self.table, extension));
        }
        for name in &parsed.name_filters {
            let name_candidates = self.name_ngrams.lookup_full(&self.table, name);
            candidates = intersect_sorted_ids(candidates, name_candidates);
        }
        for dir in &parsed.dir_filters {
            if dir.contains(['*', '?', '[']) {
                return None;
            }
            candidates =
                intersect_sorted_ids(candidates, self.path_segments.lookup(&self.table, dir));
        }

        Some(CandidateRetrieval {
            stats: CandidateRetrievalStats {
                indexed_entry_count: self.table.len(),
                candidate_count: candidates.len(),
                used_full_scan: false,
            },
            candidates,
        })
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

    pub fn entry_ids(&self) -> impl Iterator<Item = EntryId> + '_ {
        self.table.entries().iter().map(|entry| entry.id)
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
            .saturating_add(self.name_ngrams.heap_bytes())
            .saturating_add(self.extensions.extensions.heap_bytes())
            .saturating_add(
                self.path_segments
                    .buckets
                    .capacity()
                    .saturating_mul(std::mem::size_of::<SegmentBucket>()),
            )
            .saturating_add(
                self.path_segments
                    .ids
                    .capacity()
                    .saturating_mul(std::mem::size_of::<EntryId>()),
            )
            .saturating_add(self.path_segments.fuzzy_buckets.heap_bytes())
            .saturating_add(self.exact_paths.paths.heap_bytes())
    }

    pub fn memory_stats(&self) -> CompactCandidateMemoryStats {
        let entry_table_heap_bytes = self.table.estimated_heap_bytes();
        let name_ngram_bytes = self.name_ngrams.heap_bytes();
        let extension_bytes = self.extensions.extensions.heap_bytes();
        let path_segment_bytes = self
            .path_segments
            .buckets
            .capacity()
            .saturating_mul(std::mem::size_of::<SegmentBucket>())
            .saturating_add(
                self.path_segments
                    .ids
                    .capacity()
                    .saturating_mul(std::mem::size_of::<EntryId>()),
            );
        let path_fuzzy_bytes = self.path_segments.fuzzy_buckets.heap_bytes();
        let exact_path_bytes = self.exact_paths.paths.heap_bytes();
        let heap_bytes = entry_table_heap_bytes
            .saturating_add(name_ngram_bytes)
            .saturating_add(extension_bytes)
            .saturating_add(path_segment_bytes)
            .saturating_add(path_fuzzy_bytes)
            .saturating_add(exact_path_bytes);
        CompactCandidateMemoryStats {
            entry_count: self.table.len(),
            prefix_key_count: 0,
            retained_build_interner_bytes: 0,
            entry_table_heap_bytes,
            name_ngram_bytes,
            extension_bytes,
            path_segment_bytes,
            path_fuzzy_bytes,
            exact_path_bytes,
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

fn text_fingerprint(text: &str) -> u32 {
    const FNV_OFFSET: u32 = 0x811c_9dc5;
    const FNV_PRIME: u32 = 0x0100_0193;

    text.bytes().fold(FNV_OFFSET, |hash, byte| {
        hash.wrapping_mul(FNV_PRIME) ^ byte.to_ascii_lowercase() as u32
    })
}

fn verified_ids(
    ids: impl IntoIterator<Item = EntryId>,
    matches: impl Fn(EntryId) -> bool,
) -> Vec<EntryId> {
    let mut verified: Vec<_> = ids.into_iter().filter(|id| matches(*id)).collect();
    verified.sort_unstable();
    verified.dedup();
    verified
}

fn compare_ascii_case_insensitive(left: &str, right: &str) -> CmpOrdering {
    let mut left_bytes = left.bytes();
    let mut right_bytes = right.bytes();
    loop {
        match (left_bytes.next(), right_bytes.next()) {
            (Some(left), Some(right)) => {
                match left.to_ascii_lowercase().cmp(&right.to_ascii_lowercase()) {
                    CmpOrdering::Equal => continue,
                    ordering => return ordering,
                }
            }
            (None, Some(_)) => return CmpOrdering::Less,
            (Some(_), None) => return CmpOrdering::Greater,
            (None, None) => return CmpOrdering::Equal,
        }
    }
}

fn starts_with_ascii_case_insensitive(text: &str, prefix: &str) -> bool {
    text.len() >= prefix.len()
        && text
            .bytes()
            .zip(prefix.bytes())
            .all(|(text, prefix)| text.eq_ignore_ascii_case(&prefix))
}

fn contains_ascii_case_insensitive(text: &str, needle: &str) -> bool {
    let needle = needle.as_bytes();
    needle.is_empty()
        || text.as_bytes().windows(needle.len()).any(|window| {
            window
                .iter()
                .zip(needle)
                .all(|(text, needle)| text.eq_ignore_ascii_case(needle))
        })
}

fn path_segment_ranges(path: &str, path_start: u32) -> Vec<PackedTextRange> {
    let mut ranges = Vec::new();
    let mut segment_start = 0;
    for (index, byte) in path
        .bytes()
        .enumerate()
        .chain(std::iter::once((path.len(), b'/')))
    {
        if !matches!(byte, b'/' | b'\\') {
            continue;
        }
        let segment = path[segment_start..index].trim_end_matches(':');
        if !segment.is_empty() {
            ranges.push(PackedTextRange {
                start: path_start.saturating_add(segment_start as u32),
                len: segment.len() as u32,
            });
        }
        segment_start = index.saturating_add(1);
    }
    ranges
}

fn entry_name_extension(name: &str) -> Option<String> {
    name.rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
}

fn name_ngram_keys(name: &str) -> Vec<(u8, u32)> {
    let chars: Vec<_> = name.to_ascii_lowercase().chars().collect();
    let mut keys = Vec::new();
    for width in 1..=3 {
        for window in chars.windows(width) {
            keys.push((
                width as u8,
                text_fingerprint(&window.iter().collect::<String>()),
            ));
        }
    }
    keys.sort_unstable();
    keys.dedup();
    keys
}

fn query_ngram(term: &str) -> Option<(u8, String)> {
    let ngram: String = term.to_ascii_lowercase().chars().take(3).collect();
    (!ngram.is_empty()).then_some((ngram.chars().count() as u8, ngram))
}

fn encode_varint(mut value: u32, output: &mut Vec<u8>) {
    while value >= 0x80 {
        output.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    output.push(value as u8);
}

fn decode_varint(input: &[u8]) -> Option<(u32, usize)> {
    let mut value = 0_u32;
    for (index, byte) in input.iter().copied().enumerate().take(5) {
        if index == 4 && byte & 0x70 != 0 {
            return None;
        }
        value |= ((byte & 0x7f) as u32).checked_shl((index * 7) as u32)?;
        if byte & 0x80 == 0 {
            return Some((value, index + 1));
        }
    }
    None
}

fn fuzzy_segment_key(segment: &str) -> Option<String> {
    segment
        .chars()
        .next()
        .map(|character| character.to_string())
}

fn ascii_alphabetic_mask(text: &str) -> Option<u32> {
    let mut mask = 0_u32;
    for byte in text.bytes() {
        if !byte.is_ascii_alphabetic() {
            continue;
        }
        mask |= 1 << (byte.to_ascii_lowercase() - b'a');
    }
    (mask != 0).then_some(mask)
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
    fn fallible_candidate_build_reports_an_arena_limit_without_panicking() {
        let error = CompactCandidateIndex::try_from_entries_with_arena_limit(
            vec![IndexedEntry::legacy(
                "/workspace/QuickFox/AGENTS.md",
                "AGENTS.md",
                IndexedEntryKind::File,
            )],
            8,
        )
        .unwrap_err();

        assert!(matches!(
            error,
            CompactIndexBuildError::ArenaTooLarge { .. }
        ));
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
    fn long_names_and_path_segments_do_not_create_per_character_prefix_keys() {
        let long_name_stem = "n".repeat(4 * 1024);
        let long_segment = "p".repeat(4 * 1024);
        let long_name = format!("{long_name_stem}.md");
        let long_path = format!("/workspace/{long_segment}/{long_name}");
        let index = CompactCandidateIndex::from_entries(vec![IndexedEntry::legacy(
            long_path,
            long_name,
            IndexedEntryKind::File,
        )]);

        let name_prefix = index.retrieve_ordinary_term(&long_name_stem[..2048]);
        let segment_prefix = index.retrieve_ordinary_term(&long_segment[..2048]);
        let stats = index.memory_stats();

        assert_eq!(name_prefix.candidates, vec![EntryId(0)]);
        assert!(!name_prefix.stats.used_full_scan);
        assert_eq!(segment_prefix.candidates, vec![EntryId(0)]);
        assert!(!segment_prefix.stats.used_full_scan);
        assert!(
            stats.prefix_key_count <= 4,
            "4 KiB fields must not create O(length) resident prefix keys: {stats:#?}"
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
    fn name_ngram_index_retrieves_name_and_substring_candidates() {
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

        let ngram_index = NameNgramIndex::build(&table).unwrap();

        assert_eq!(ngram_index.lookup(&table, "agents"), vec![EntryId(0)]);
        assert_eq!(ngram_index.lookup(&table, "agents.m"), vec![EntryId(0)]);
        assert_eq!(ngram_index.lookup(&table, "ead"), vec![EntryId(1)]);
    }

    #[test]
    fn name_ngram_index_recalls_delimited_and_camel_case_substrings() {
        let table = EntryTable::from_entries(vec![
            IndexedEntry::legacy(
                "/workspace/my-report.md",
                "my-report.md",
                IndexedEntryKind::File,
            ),
            IndexedEntry::legacy(
                "/workspace/QuickFoxGuide.md",
                "QuickFoxGuide.md",
                IndexedEntryKind::File,
            ),
        ]);
        let ngram_index = NameNgramIndex::build(&table).unwrap();

        assert_eq!(ngram_index.lookup(&table, "re"), vec![EntryId(0)]);
        assert_eq!(ngram_index.lookup(&table, "Fox"), vec![EntryId(1)]);
    }

    #[test]
    fn numeric_name_substrings_use_bounded_ngram_candidates() {
        let index = CompactCandidateIndex::from_entries(vec![
            IndexedEntry::legacy(
                "/workspace/file-1234.md",
                "file-1234.md",
                IndexedEntryKind::File,
            ),
            IndexedEntry::legacy("/workspace/notes.md", "notes.md", IndexedEntryKind::File),
        ]);

        let retrieval = index.retrieve_ordinary_term("123");

        assert_eq!(retrieval.candidates, vec![EntryId(0)]);
        assert!(!retrieval.stats.used_full_scan);
    }

    #[test]
    fn short_name_substrings_use_bounded_ngram_candidates() {
        let index = CompactCandidateIndex::from_entries(vec![
            IndexedEntry::legacy("/workspace/cmd.txt", "cmd.txt", IndexedEntryKind::File),
            IndexedEntry::legacy("/workspace/notes.md", "notes.md", IndexedEntryKind::File),
        ]);

        let retrieval = index.retrieve_ordinary_term("md");

        assert_eq!(retrieval.candidates, vec![EntryId(0), EntryId(1)]);
        assert!(!retrieval.stats.used_full_scan);
    }

    #[test]
    fn name_ngram_index_retrieves_substring_candidates() {
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
        let index = NameNgramIndex::build(&table).unwrap();

        assert_eq!(index.lookup(&table, "port"), vec![EntryId(0)]);
        assert_eq!(index.lookup(&table, "ort"), vec![EntryId(0)]);
        assert!(index.lookup(&table, "zzzz").is_empty());
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
        let ngram_index = NameNgramIndex::build(&table).unwrap();

        assert_eq!(
            extension_index.lookup(&table, "md"),
            vec![EntryId(0), EntryId(2)]
        );
        assert_eq!(
            intersect_sorted_ids(
                extension_index.lookup(&table, "md"),
                ngram_index.lookup(&table, "agents"),
            ),
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

        let path_index = PathSegmentIndex::build(&table).unwrap();

        assert_eq!(
            path_index.lookup(&table, "workspace"),
            vec![EntryId(0), EntryId(1)]
        );
        assert_eq!(path_index.lookup(&table, "quickfox"), vec![EntryId(0)]);
        assert_eq!(path_index.lookup(&table, "downloads"), vec![EntryId(2)]);
    }

    #[test]
    fn path_segment_fuzzy_lookup_accepts_subsequences_with_different_end_characters() {
        let table = EntryTable::from_entries(vec![IndexedEntry::legacy(
            "/workspace/QuickFox/AGENTS.md",
            "AGENTS.md",
            IndexedEntryKind::File,
        )]);
        let path_index = PathSegmentIndex::build(&table).unwrap();

        assert_eq!(path_index.lookup(&table, "wrkspc"), vec![EntryId(0)]);
    }

    #[test]
    fn path_segment_index_reports_posting_limit_overflow() {
        let table = EntryTable::from_entries(vec![IndexedEntry::legacy(
            "/workspace/QuickFox/docs/AGENTS.md",
            "AGENTS.md",
            IndexedEntryKind::File,
        )]);

        let error = PathSegmentIndex::build_with_limits(&table, 0, usize::MAX).unwrap_err();

        assert!(matches!(
            error,
            CompactIndexBuildError::TooManySegmentPostings { .. }
        ));
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
    fn limited_search_can_prioritize_direct_name_candidates_over_fuzzy_path_recall() {
        let index = CompactCandidateIndex::from_entries(vec![
            IndexedEntry::legacy(
                "/workspace/agent-sources/unrelated.md",
                "unrelated.md",
                IndexedEntryKind::File,
            ),
            IndexedEntry::legacy(
                "/workspace/docs/AGENTS.md",
                "AGENTS.md",
                IndexedEntryKind::File,
            ),
        ]);

        let all_candidates = index.retrieve_query("agents");
        let direct_names = index.retrieve_query_name_priority("agents").unwrap();

        assert_eq!(all_candidates.candidates, vec![EntryId(0), EntryId(1)]);
        assert_eq!(direct_names.candidates, vec![EntryId(1)]);
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
