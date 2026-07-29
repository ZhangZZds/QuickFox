//! Text content index boundary.

use crate::core::index_entry::{ContentIndexState, IndexedEntry, IndexedEntryKind};
use crate::core::search::SearchSnippet;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

#[cfg(test)]
use std::sync::atomic::AtomicUsize;
use tantivy::collector::{BytesFilterCollector, TopDocs};
use tantivy::query::QueryParser;
use tantivy::schema::{Field, Schema, Value, FAST, STORED, STRING, TEXT};
use tantivy::{doc, Index, IndexWriter, TantivyDocument, Term};

pub const DEFAULT_MAX_CONTENT_BYTES: u64 = 2 * 1024 * 1024;
pub const CONTENT_INDEX_DIR_VERSION: &str = "content-v1";
static CONTENT_BUILD_NONCE: AtomicU64 = AtomicU64::new(0);

type ContentResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
pub(crate) type ContentPathFilter = Arc<dyn Fn(&str) -> bool + Send + Sync>;

const PATH_FILTER_FIELD_NAME: &str = "path_filter";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentIndexOptions {
    pub index_dir: PathBuf,
    pub max_file_bytes: u64,
}

impl Default for ContentIndexOptions {
    fn default() -> Self {
        Self {
            index_dir: std::env::temp_dir()
                .join("quickfox")
                .join(CONTENT_INDEX_DIR_VERSION),
            max_file_bytes: DEFAULT_MAX_CONTENT_BYTES,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ContentIndex {
    index: Index,
    path_field: Field,
    path_filter_field: Field,
    content_field: Field,
    // Field order keeps the directory alive until every Tantivy handle above has dropped.
    _directory_lease: Arc<ContentIndexDirectoryLease>,
    #[cfg(test)]
    last_collector_limit: Arc<AtomicUsize>,
}

#[derive(Debug)]
struct ContentIndexDirectoryLease {
    path: PathBuf,
}

impl Drop for ContentIndexDirectoryLease {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                eprintln!(
                    "QuickFox failed to reclaim content index version {}: {error}",
                    self.path.display()
                );
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContentSearchHit {
    pub path: String,
    pub score: f32,
    pub snippet: SearchSnippet,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentIndexMemoryEstimate {
    pub resident_document_count: usize,
    pub resident_cached_content_bytes: usize,
}

#[derive(Debug, Clone)]
struct ExtractedDocument {
    content: String,
    lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContentExtractionResult {
    Text(String),
    TooLarge,
    Binary,
    UnsupportedType,
    ReadFailed,
}

pub trait TextExtractor {
    fn extract(&self, path: &Path, max_bytes: u64) -> ContentExtractionResult;
}

#[derive(Debug, Clone, Default)]
pub struct PlainTextExtractor;

impl TextExtractor for PlainTextExtractor {
    fn extract(&self, path: &Path, max_bytes: u64) -> ContentExtractionResult {
        let metadata = match fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(_) => return ContentExtractionResult::ReadFailed,
        };
        if !metadata.is_file() {
            return ContentExtractionResult::UnsupportedType;
        }
        if metadata.len() > max_bytes {
            return ContentExtractionResult::TooLarge;
        }
        if is_unsupported_rich_document(path) {
            return ContentExtractionResult::UnsupportedType;
        }

        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(_) => return ContentExtractionResult::ReadFailed,
        };
        if content_inspector::inspect(&bytes).is_binary() {
            return ContentExtractionResult::Binary;
        }

        match String::from_utf8(bytes) {
            Ok(content) => ContentExtractionResult::Text(content),
            Err(_) => ContentExtractionResult::Binary,
        }
    }
}

impl ContentIndex {
    pub fn build(
        entries: &mut [IndexedEntry],
        options: ContentIndexOptions,
    ) -> ContentResult<Self> {
        Self::build_with_extractor(entries, options, &PlainTextExtractor)
    }

    pub fn build_with_extractor(
        entries: &mut [IndexedEntry],
        options: ContentIndexOptions,
        extractor: &dyn TextExtractor,
    ) -> ContentResult<Self> {
        let version_root = options.index_dir.join(CONTENT_INDEX_DIR_VERSION);
        fs::create_dir_all(&version_root)?;
        let nonce = CONTENT_BUILD_NONCE.fetch_add(1, Ordering::Relaxed);
        let index_dir = version_root.join(format!("build-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&index_dir)?;
        let directory_lease = Arc::new(ContentIndexDirectoryLease {
            path: index_dir.clone(),
        });

        let schema = content_schema();
        let path_field = schema.get_field("path")?;
        let path_filter_field = schema.get_field(PATH_FILTER_FIELD_NAME)?;
        let content_field = schema.get_field("content")?;
        let index = Index::create_in_dir(&index_dir, schema)?;
        let mut writer: IndexWriter = index.writer(50_000_000)?;
        for entry in entries {
            if entry.kind != IndexedEntryKind::File {
                entry.content_index_state = ContentIndexState::NotIndexed;
                continue;
            }

            match extractor.extract(Path::new(&entry.path), options.max_file_bytes) {
                ContentExtractionResult::Text(content) => {
                    writer.add_document(doc!(
                        path_field => entry.path.clone(),
                        path_filter_field => entry.path.as_bytes(),
                        content_field => content.clone(),
                    ))?;
                    entry.content_index_state = ContentIndexState::Indexed;
                }
                ContentExtractionResult::TooLarge => {
                    entry.content_index_state = ContentIndexState::SkippedTooLarge;
                }
                ContentExtractionResult::Binary | ContentExtractionResult::UnsupportedType => {
                    entry.content_index_state = ContentIndexState::SkippedBinary;
                }
                ContentExtractionResult::ReadFailed => {
                    entry.content_index_state = ContentIndexState::ReadFailed;
                }
            }
        }

        writer.commit()?;

        Ok(Self {
            index,
            path_field,
            path_filter_field,
            content_field,
            _directory_lease: directory_lease,
            #[cfg(test)]
            last_collector_limit: Arc::new(AtomicUsize::new(0)),
        })
    }

    pub fn search(
        &self,
        content_query: &str,
        candidate_paths: Option<&HashSet<String>>,
        limit: usize,
    ) -> ContentResult<Vec<ContentSearchHit>> {
        self.search_with_candidate_paths(
            content_query,
            candidate_paths.map(|paths| Arc::new(paths.clone())),
            limit,
        )
    }

    pub(crate) fn search_with_candidate_paths(
        &self,
        content_query: &str,
        candidate_paths: Option<Arc<HashSet<String>>>,
        limit: usize,
    ) -> ContentResult<Vec<ContentSearchHit>> {
        let path_filter = candidate_paths
            .map(|paths| Arc::new(move |path: &str| paths.contains(path)) as ContentPathFilter);
        self.search_with_path_filter(content_query, path_filter, limit)
    }

    pub(crate) fn search_with_path_filter(
        &self,
        content_query: &str,
        path_filter: Option<ContentPathFilter>,
        limit: usize,
    ) -> ContentResult<Vec<ContentSearchHit>> {
        if limit == 0 || content_query.trim().is_empty() {
            return Ok(Vec::new());
        }

        let reader = self.index.reader()?;
        let searcher = reader.searcher();
        let parser = QueryParser::for_index(&self.index, vec![self.content_field]);
        let query = parser.parse_query(content_query)?;
        let tantivy_limit = limit.saturating_mul(4).clamp(10, 10_000);
        #[cfg(test)]
        self.last_collector_limit
            .store(tantivy_limit, Ordering::Relaxed);
        let top_docs = if let Some(path_filter) = path_filter {
            let collector = BytesFilterCollector::new(
                PATH_FILTER_FIELD_NAME.to_owned(),
                move |path_bytes: &[u8]| {
                    std::str::from_utf8(path_bytes).is_ok_and(|path| path_filter(path))
                },
                TopDocs::with_limit(tantivy_limit).order_by_score(),
            );
            searcher.search(&query, &collector)?
        } else {
            searcher.search(&query, &TopDocs::with_limit(tantivy_limit).order_by_score())?
        };
        let mut hits = Vec::new();

        for (score, address) in top_docs {
            let doc = searcher.doc::<TantivyDocument>(address)?;
            let Some(path) = doc
                .get_first(self.path_field)
                .and_then(|value| value.as_str())
                .map(str::to_owned)
            else {
                continue;
            };

            let Some(content) = doc
                .get_first(self.content_field)
                .and_then(|value| value.as_str())
            else {
                continue;
            };
            let document = ExtractedDocument::from_content(content.to_owned());
            let Some(snippet) = document.snippet(content_query) else {
                continue;
            };

            hits.push(ContentSearchHit {
                path,
                score,
                snippet,
            });

            if hits.len() >= limit {
                break;
            }
        }

        Ok(hits)
    }

    pub fn remove_path(&mut self, path: impl AsRef<Path>) -> ContentResult<()> {
        let path = path.as_ref().to_string_lossy().to_string();
        let mut writer: IndexWriter = self.index.writer(50_000_000)?;
        writer.delete_term(Term::from_field_text(self.path_field, &path));
        writer.commit()?;
        Ok(())
    }

    pub fn update_entry(
        &mut self,
        entry: &mut IndexedEntry,
        options: &ContentIndexOptions,
        extractor: &dyn TextExtractor,
    ) -> ContentResult<()> {
        self.remove_path(&entry.path)?;
        if entry.kind != IndexedEntryKind::File {
            entry.content_index_state = ContentIndexState::NotIndexed;
            return Ok(());
        }

        match extractor.extract(Path::new(&entry.path), options.max_file_bytes) {
            ContentExtractionResult::Text(content) => {
                let mut writer: IndexWriter = self.index.writer(50_000_000)?;
                writer.add_document(doc!(
                        self.path_field => entry.path.clone(),
                        self.path_filter_field => entry.path.as_bytes(),
                        self.content_field => content.clone(),
                ))?;
                writer.commit()?;
                entry.content_index_state = ContentIndexState::Indexed;
            }
            ContentExtractionResult::TooLarge => {
                entry.content_index_state = ContentIndexState::SkippedTooLarge;
            }
            ContentExtractionResult::Binary | ContentExtractionResult::UnsupportedType => {
                entry.content_index_state = ContentIndexState::SkippedBinary;
            }
            ContentExtractionResult::ReadFailed => {
                entry.content_index_state = ContentIndexState::ReadFailed;
            }
        }

        Ok(())
    }

    pub fn memory_estimate(&self) -> ContentIndexMemoryEstimate {
        ContentIndexMemoryEstimate {
            resident_document_count: 0,
            resident_cached_content_bytes: 0,
        }
    }

    #[cfg(test)]
    pub(crate) fn last_collector_limit(&self) -> usize {
        self.last_collector_limit.load(Ordering::Relaxed)
    }
}

impl ExtractedDocument {
    fn from_content(content: String) -> Self {
        let lines = content.lines().map(str::to_owned).collect();
        Self { content, lines }
    }

    fn snippet(&self, query: &str) -> Option<SearchSnippet> {
        let terms = query_highlight_terms(query);
        let content_lower = self.content.to_ascii_lowercase();
        let first_term = terms.iter().find(|term| content_lower.contains(&***term))?;

        let hit_line_index =
            self.lines.iter().enumerate().find_map(|(index, line)| {
                line.to_ascii_lowercase().find(first_term).map(|_| index)
            })?;
        let start_index = hit_line_index.saturating_sub(5);
        let end_index = (hit_line_index + 6).min(self.lines.len());
        let start_line = start_index + 1;

        let mut snippet = SearchSnippet {
            start_line,
            lines: self.lines[start_index..end_index].to_vec(),
            highlights: Vec::new(),
        };

        for term in terms {
            for (line_offset, line) in snippet.lines.iter().enumerate() {
                let lower = line.to_ascii_lowercase();
                let mut search_start = 0;
                while let Some(relative) = lower[search_start..].find(&term) {
                    let start = search_start + relative;
                    let end = start + term.len();
                    snippet
                        .highlights
                        .push(crate::core::search::SearchHighlight {
                            line: snippet.start_line + line_offset,
                            start_column: start + 1,
                            end_column: end + 1,
                            matched_text: line[start..end].to_owned(),
                        });
                    search_start = end;
                }
            }
        }

        Some(snippet)
    }
}

fn content_schema() -> Schema {
    let mut builder = Schema::builder();
    builder.add_text_field("path", STRING | STORED);
    builder.add_bytes_field(PATH_FILTER_FIELD_NAME, FAST);
    builder.add_text_field("content", TEXT | STORED);
    builder.build()
}

fn is_unsupported_rich_document(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension.to_ascii_lowercase())
            .as_deref(),
        Some("pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx")
    )
}

fn query_highlight_terms(query: &str) -> Vec<String> {
    query
        .split(|character: char| {
            character.is_whitespace()
                || matches!(character, '"' | '\'' | ':' | '(' | ')' | '+' | '-')
        })
        .map(|term| {
            term.trim_matches(|character: char| !character.is_alphanumeric() && character != '_')
                .to_ascii_lowercase()
        })
        .filter(|term| !term.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::index::{IndexedEntry, IndexedEntryKind, SearchIndex};
    use crate::core::index_entry::ContentIndexState;
    use crate::core::search::QueryParser;
    use std::fs;
    use std::sync::{mpsc, Mutex};

    #[test]
    fn concurrent_build_in_same_base_keeps_live_reader_until_atomic_publish() {
        struct BlockingExtractor {
            started: mpsc::SyncSender<()>,
            release: Mutex<mpsc::Receiver<()>>,
        }

        impl TextExtractor for BlockingExtractor {
            fn extract(&self, path: &Path, _max_bytes: u64) -> ContentExtractionResult {
                self.started.send(()).unwrap();
                self.release.lock().unwrap().recv().unwrap();
                ContentExtractionResult::Text(fs::read_to_string(path).unwrap())
            }
        }

        let workspace = tempfile::tempdir().unwrap();
        let base = workspace.path().join("production-content-base");
        let old_file = workspace.path().join("old.txt");
        let new_file = workspace.path().join("new.txt");
        fs::write(&old_file, "old durable needle").unwrap();
        fs::write(&new_file, "new replacement haystack").unwrap();
        let options = ContentIndexOptions {
            index_dir: base.clone(),
            max_file_bytes: DEFAULT_MAX_CONTENT_BYTES,
        };
        let mut old_entries = vec![entry(&old_file)];
        let old = ContentIndex::build(&mut old_entries, options.clone()).unwrap();
        let old_version = fs::read_dir(base.join(CONTENT_INDEX_DIR_VERSION))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let (started_tx, started_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let extractor = BlockingExtractor {
            started: started_tx,
            release: Mutex::new(release_rx),
        };

        let new = std::thread::scope(|scope| {
            let options = options.clone();
            let builder = scope.spawn(|| {
                let mut entries = vec![entry(&new_file)];
                let index =
                    ContentIndex::build_with_extractor(&mut entries, options, &extractor).unwrap();
                (index, entries)
            });
            started_rx.recv().unwrap();
            assert!(old_version.is_dir());
            assert_eq!(
                fs::read_dir(base.join(CONTENT_INDEX_DIR_VERSION))
                    .unwrap()
                    .count(),
                2
            );
            for _ in 0..100 {
                assert_eq!(old.search("needle", None, 10).unwrap().len(), 1);
            }
            release_tx.send(()).unwrap();
            builder.join().unwrap()
        });

        assert_eq!(old.search("needle", None, 10).unwrap().len(), 1);
        assert_eq!(new.0.search("haystack", None, 10).unwrap().len(), 1);
        drop(old);
        assert!(!old_version.exists());
        assert_eq!(
            fs::read_dir(base.join(CONTENT_INDEX_DIR_VERSION))
                .unwrap()
                .count(),
            1
        );
        drop(new);
        assert_eq!(
            fs::read_dir(base.join(CONTENT_INDEX_DIR_VERSION))
                .unwrap()
                .count(),
            0
        );
    }

    #[test]
    fn cloned_reader_keeps_version_directory_leased_until_last_drop() {
        let workspace = tempfile::tempdir().unwrap();
        let base = workspace.path().join("shared-content-base");
        let file = workspace.path().join("leased.txt");
        fs::write(&file, "leased reader needle").unwrap();
        let mut entries = vec![entry(&file)];
        let index = ContentIndex::build(
            &mut entries,
            ContentIndexOptions {
                index_dir: base.clone(),
                max_file_bytes: DEFAULT_MAX_CONTENT_BYTES,
            },
        )
        .unwrap();
        let version = fs::read_dir(base.join(CONTENT_INDEX_DIR_VERSION))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let reader = index.clone();

        drop(index);
        assert!(version.is_dir());
        assert_eq!(reader.search("needle", None, 10).unwrap().len(), 1);

        drop(reader);
        assert!(!version.exists());
    }

    #[test]
    fn indexes_text_files_and_searches_content_with_snippets() {
        let workspace = tempfile::tempdir().unwrap();
        let file = workspace.path().join("notes.txt");
        let lines: Vec<_> = (1..=13)
            .map(|line| {
                if line == 7 {
                    "line 07 alpha needle beta".to_owned()
                } else {
                    format!("line {line:02}")
                }
            })
            .collect();
        fs::write(&file, lines.join("\n")).unwrap();

        let mut entries = vec![entry(&file)];
        let options = ContentIndexOptions {
            index_dir: workspace.path().join("tantivy-content"),
            max_file_bytes: DEFAULT_MAX_CONTENT_BYTES,
        };

        let content_index = ContentIndex::build(&mut entries, options).unwrap();

        assert_eq!(entries[0].content_index_state, ContentIndexState::Indexed);

        let hits = content_index.search("needle", None, 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, file.to_string_lossy());
        assert_eq!(hits[0].snippet.start_line, 2);
        assert_eq!(hits[0].snippet.lines.len(), 11);
        assert_eq!(hits[0].snippet.lines[5], "line 07 alpha needle beta");
        assert_eq!(hits[0].snippet.highlights.len(), 1);
        assert_eq!(hits[0].snippet.highlights[0].line, 7);
        assert_eq!(hits[0].snippet.highlights[0].start_column, 15);
        assert_eq!(hits[0].snippet.highlights[0].end_column, 21);

        let index = SearchIndex::from_entries_with_content_index(entries, content_index);
        let parser = QueryParser::new(Default::default());
        let results = index.search(&parser.parse("content:needle"));

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "notes.txt");
        assert_eq!(results[0].snippet.as_ref().unwrap().start_line, 2);
        assert_eq!(
            results[0].snippet.as_ref().unwrap().highlights[0].matched_text,
            "needle"
        );
    }

    #[test]
    fn skips_too_large_and_binary_files_but_keeps_name_path_searchable() {
        let workspace = tempfile::tempdir().unwrap();
        let big = workspace.path().join("big-report.txt");
        let binary = workspace.path().join("binary-report.bin");
        fs::write(&big, "x".repeat(128)).unwrap();
        fs::write(&binary, [0, 159, 146, 150, 0, 1]).unwrap();

        let mut entries = vec![entry(&big), entry(&binary)];
        let options = ContentIndexOptions {
            index_dir: workspace.path().join("tantivy-content"),
            max_file_bytes: 32,
        };

        let content_index = ContentIndex::build(&mut entries, options).unwrap();

        assert_eq!(
            entries[0].content_index_state,
            ContentIndexState::SkippedTooLarge
        );
        assert_eq!(
            entries[1].content_index_state,
            ContentIndexState::SkippedBinary
        );
        assert!(content_index.search("report", None, 10).unwrap().is_empty());

        let index = SearchIndex::from_entries_with_content_index(entries, content_index);
        let parser = QueryParser::new(Default::default());
        let name_results = index.search(&parser.parse("report"));
        let content_results = index.search(&parser.parse("content:report"));

        assert_eq!(name_results.len(), 2);
        assert!(content_results.is_empty());
    }

    #[test]
    fn content_index_does_not_keep_full_documents_resident_for_snippets() {
        let workspace = tempfile::tempdir().unwrap();
        let first = workspace.path().join("first.txt");
        let second = workspace.path().join("second.txt");
        fs::write(&first, "alpha\nneedle\nomega").unwrap();
        fs::write(&second, "beta\nneedle\nomega").unwrap();

        let mut entries = vec![entry(&first), entry(&second)];
        let content_index = ContentIndex::build(
            &mut entries,
            ContentIndexOptions {
                index_dir: workspace.path().join("tantivy-content"),
                max_file_bytes: DEFAULT_MAX_CONTENT_BYTES,
            },
        )
        .unwrap();

        let estimate = content_index.memory_estimate();

        assert_eq!(estimate.resident_document_count, 0);
        assert_eq!(estimate.resident_cached_content_bytes, 0);
        assert_eq!(content_index.search("needle", None, 10).unwrap().len(), 2);
    }

    #[test]
    fn applies_candidate_constraints_before_content_results() {
        let workspace = tempfile::tempdir().unwrap();
        let docs = workspace.path().join("docs");
        let archive = workspace.path().join("archive");
        fs::create_dir_all(&docs).unwrap();
        fs::create_dir_all(&archive).unwrap();
        let visible = docs.join("visible.md");
        let hidden = archive.join("hidden.md");
        fs::write(&visible, "shared body needle").unwrap();
        fs::write(&hidden, "shared body needle").unwrap();

        let mut entries = vec![entry(&visible), entry(&hidden)];
        let content_index = ContentIndex::build(
            &mut entries,
            ContentIndexOptions {
                index_dir: workspace.path().join("tantivy-content"),
                max_file_bytes: DEFAULT_MAX_CONTENT_BYTES,
            },
        )
        .unwrap();
        let index = SearchIndex::from_entries_with_content_index(entries, content_index);
        let parser = QueryParser::new(Default::default());

        let titles: Vec<_> = index
            .search(&parser.parse("dir:docs content:needle"))
            .into_iter()
            .map(|result| result.title)
            .collect();

        assert_eq!(titles, vec!["visible.md"]);
    }

    #[test]
    fn mixed_sort_keeps_only_candidate_content_hits() {
        let workspace = tempfile::tempdir().unwrap();
        let name_match = workspace.path().join("needle-title.md");
        let boosted = workspace.path().join("needle-body.md");
        let content_only = workspace.path().join("body-only.md");
        fs::write(&name_match, "ordinary text").unwrap();
        fs::write(&boosted, "contains needle in the body").unwrap();
        fs::write(&content_only, "contains needle in the body").unwrap();

        let mut entries = vec![entry(&name_match), entry(&boosted), entry(&content_only)];
        let content_index = ContentIndex::build(
            &mut entries,
            ContentIndexOptions {
                index_dir: workspace.path().join("tantivy-content"),
                max_file_bytes: DEFAULT_MAX_CONTENT_BYTES,
            },
        )
        .unwrap();
        let index = SearchIndex::from_entries_with_content_index(entries, content_index);
        let parser = QueryParser::new(Default::default());

        let pure_content_titles: Vec<_> = index
            .search(&parser.parse("content:needle"))
            .into_iter()
            .map(|result| result.title)
            .collect();
        let mixed_titles: Vec<_> = index
            .search(&parser.parse("needle content:needle"))
            .into_iter()
            .map(|result| result.title)
            .collect();

        assert_eq!(pure_content_titles, vec!["body-only.md", "needle-body.md"]);
        assert_eq!(mixed_titles, vec!["needle-body.md"]);
    }

    #[test]
    fn updates_and_removes_individual_content_documents() {
        let workspace = tempfile::tempdir().unwrap();
        let first = workspace.path().join("first.txt");
        let second = workspace.path().join("second.txt");
        fs::write(&first, "alpha needle").unwrap();
        fs::write(&second, "beta haystack").unwrap();

        let mut entries = vec![entry(&first)];
        let options = ContentIndexOptions {
            index_dir: workspace.path().join("tantivy-content"),
            max_file_bytes: DEFAULT_MAX_CONTENT_BYTES,
        };
        let mut content_index = ContentIndex::build(&mut entries, options.clone()).unwrap();

        let mut second_entry = entry(&second);
        content_index
            .update_entry(&mut second_entry, &options, &PlainTextExtractor)
            .unwrap();
        assert_eq!(second_entry.content_index_state, ContentIndexState::Indexed);
        assert_eq!(content_index.search("haystack", None, 10).unwrap().len(), 1);

        content_index.remove_path(&first).unwrap();
        assert!(content_index.search("needle", None, 10).unwrap().is_empty());
    }

    #[test]
    fn changed_or_unreadable_content_degrades_without_breaking_name_search() {
        let workspace = tempfile::tempdir().unwrap();
        let file = workspace.path().join("report.txt");
        fs::write(&file, "old needle").unwrap();

        let mut entries = vec![entry(&file)];
        let options = ContentIndexOptions {
            index_dir: workspace.path().join("tantivy-content"),
            max_file_bytes: DEFAULT_MAX_CONTENT_BYTES,
        };
        let mut content_index = ContentIndex::build(&mut entries, options.clone()).unwrap();
        assert_eq!(content_index.search("needle", None, 10).unwrap().len(), 1);

        fs::write(&file, "fresh haystack").unwrap();
        content_index
            .update_entry(&mut entries[0], &options, &PlainTextExtractor)
            .unwrap();
        assert_eq!(entries[0].content_index_state, ContentIndexState::Indexed);
        assert!(content_index.search("needle", None, 10).unwrap().is_empty());
        assert_eq!(content_index.search("haystack", None, 10).unwrap().len(), 1);

        fs::remove_file(&file).unwrap();
        content_index
            .update_entry(&mut entries[0], &options, &PlainTextExtractor)
            .unwrap();
        assert_eq!(
            entries[0].content_index_state,
            ContentIndexState::ReadFailed
        );
        assert!(content_index
            .search("haystack", None, 10)
            .unwrap()
            .is_empty());

        let index = SearchIndex::from_entries_with_content_index(entries, content_index);
        let parser = QueryParser::new(Default::default());
        let name_results = index.search(&parser.parse("report"));

        assert_eq!(name_results.len(), 1);
        assert_eq!(name_results[0].title, "report.txt");
    }

    fn entry(path: &std::path::Path) -> IndexedEntry {
        IndexedEntry::from_path_metadata(path, path.parent().unwrap(), IndexedEntryKind::File)
    }
}
