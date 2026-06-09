//! File and directory indexing will live here.

use crate::core::actions::Action;
use crate::core::content_index::{
    ContentIndex, ContentIndexOptions, ContentSearchHit, PlainTextExtractor,
};
use crate::core::file_matcher::FileMatcher;
use crate::core::file_query::FileQuery;
pub use crate::core::index_entry::{
    ContentIndexState, IndexFailure, IndexLifecycle, IndexReport, IndexScanOptions, IndexStatus,
    IndexStatusKind, IndexedEntry, IndexedEntryKind,
};
use crate::core::index_scanner::{FileSystemScanner, IgnoreScanner, IndexScanPlan};
use crate::core::search::{QueryRequest, SearchMode, SearchResult, SearchResultKind};
use regex::Regex;
use std::collections::{HashMap, HashSet};

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

#[derive(Debug, Clone, Default)]
pub struct SearchIndex {
    entries: Vec<IndexedEntry>,
    search_texts: Vec<String>,
    content_index: Option<ContentIndex>,
}

impl SearchIndex {
    pub fn from_entries(entries: Vec<IndexedEntry>) -> Self {
        let search_texts = entries.iter().map(searchable_text).collect();
        Self {
            entries,
            search_texts,
            content_index: None,
        }
    }

    pub fn from_entries_with_content_index(
        entries: Vec<IndexedEntry>,
        content_index: ContentIndex,
    ) -> Self {
        let search_texts = entries.iter().map(searchable_text).collect();
        Self {
            entries,
            search_texts,
            content_index: Some(content_index),
        }
    }

    pub fn entries(&self) -> &[IndexedEntry] {
        &self.entries
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
    }

    pub fn search_with_limit(&self, query: &QueryRequest, limit: usize) -> Vec<SearchResult> {
        match &query.mode {
            SearchMode::Normal => self.search_normal(&query.text, limit),
            SearchMode::Regex => self.search_regex(&query.text, limit),
            SearchMode::WebSearch { .. } | SearchMode::Command => Vec::new(),
        }
    }

    fn replace_entries(&mut self, entries: Vec<IndexedEntry>) {
        self.search_texts = entries.iter().map(searchable_text).collect();
        self.entries = entries;
        self.content_index = None;
    }

    fn search_normal(&self, query: &str, limit: usize) -> Vec<SearchResult> {
        let query = FileQuery::parse(query);
        if limit == 0 || (!query.has_name_path_constraints() && !query.has_content_query()) {
            return Vec::new();
        }

        if query.has_content_query() && !query.has_name_path_constraints() {
            return self.search_content_only(&query, limit);
        }

        let matcher = FileMatcher::default();
        let candidates: Vec<_> = self
            .entries
            .iter()
            .zip(self.search_texts.iter())
            .filter(|(entry, search_text)| {
                matcher.matches_with_search_text(&query, entry, search_text)
            })
            .take(limit)
            .map(|(entry, _)| entry)
            .collect();

        if !query.has_content_query() {
            return candidates.into_iter().map(entry_to_result).collect();
        }

        let candidate_paths: HashSet<_> =
            candidates.iter().map(|entry| entry.path.clone()).collect();
        let content_hits = self.search_content_hits(&query, Some(&candidate_paths), limit);
        let hit_by_path: HashMap<_, _> = content_hits
            .into_iter()
            .map(|hit| (hit.path.clone(), hit))
            .collect();
        let mut results: Vec<_> = candidates
            .into_iter()
            .map(|entry| {
                let mut result = entry_to_result(entry);
                if let Some(hit) = hit_by_path.get(&entry.path) {
                    result = result
                        .with_snippet(hit.snippet.clone())
                        .with_score(content_score(hit).saturating_add(10_000));
                }
                result
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

    fn search_content_only(&self, query: &FileQuery, limit: usize) -> Vec<SearchResult> {
        let hits = self.search_content_hits(query, None, limit);
        let entry_by_path: HashMap<_, _> = self
            .entries
            .iter()
            .map(|entry| (entry.path.as_str(), entry))
            .collect();
        let mut results: Vec<_> = hits
            .into_iter()
            .filter_map(|hit| {
                let score = content_score(&hit);
                entry_by_path.get(hit.path.as_str()).map(|entry| {
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
        candidate_paths: Option<&HashSet<String>>,
        limit: usize,
    ) -> Vec<ContentSearchHit> {
        let Some(content_index) = &self.content_index else {
            return Vec::new();
        };
        let content_query = query.content_queries.join(" ");
        content_index
            .search(&content_query, candidate_paths, limit)
            .unwrap_or_default()
    }

    fn search_regex(&self, pattern: &str, limit: usize) -> Vec<SearchResult> {
        let regex = match Regex::new(pattern) {
            Ok(regex) => regex,
            Err(error) => {
                return vec![SearchResult::new(
                    "feedback:invalid-regex",
                    format!("无效正则: {error}"),
                    SearchResultKind::Feedback,
                    Action::CopyText {
                        text: error.to_string(),
                    },
                )];
            }
        };

        self.entries
            .iter()
            .filter(|entry| regex.is_match(&entry.name) || regex.is_match(&entry.path))
            .take(limit)
            .map(entry_to_result)
            .collect()
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
        }
        IndexedEntryKind::Directory => {
            result = result.with_secondary_action(Action::OpenPath {
                path: entry.path.clone(),
            });
        }
    }

    result.with_secondary_action(Action::CopyText {
        text: entry.path.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
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
    fn content_only_query_returns_no_name_path_results_before_content_indexing() {
        let index = SearchIndex::from_entries(vec![file_entry("/Users/frank/workspace/hello.md")]);
        let parser = crate::core::search::QueryParser::new(Default::default());

        let results = index.search(&parser.parse(r#"content:"hello world""#));

        assert!(results.is_empty());
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

        let query_started = Instant::now();
        let ordinary_results = index.search(&parser.parse("report"));
        let query_duration = query_started.elapsed();
        let content_results = index.search(&parser.parse("content:needle-from-file-content-only"));

        println!(
            "QUICKFOX_INDEX_BASELINE scan_ms={} entries={} failures={} ordinary_query_us={} ordinary_results={} content_query_results={}",
            scan_duration.as_millis(),
            report.entries.len(),
            report.failures.len(),
            query_duration.as_micros(),
            ordinary_results.len(),
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
        assert!(content_results.is_empty());

        let _ = fs::remove_dir_all(root);
    }

    fn file_entry(path: &str) -> IndexedEntry {
        IndexedEntry::legacy(
            path,
            path.rsplit('/').next().unwrap().to_owned(),
            IndexedEntryKind::File,
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
