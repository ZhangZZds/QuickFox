//! File and directory indexing will live here.

use crate::core::actions::Action;
use crate::core::search::{QueryRequest, SearchMode, SearchResult, SearchResultKind};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IndexedEntryKind {
    File,
    Directory,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexedEntry {
    pub path: String,
    pub name: String,
    pub kind: IndexedEntryKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexFailure {
    pub root: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct IndexReport {
    pub entries: Vec<IndexedEntry>,
    pub failures: Vec<IndexFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IndexScanOptions {
    pub include_dirs: Vec<PathBuf>,
    pub exclude_dirs: Vec<PathBuf>,
    pub exclude_patterns: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct IndexScanner;

impl IndexScanner {
    pub fn scan(&self, options: IndexScanOptions) -> Result<IndexReport, std::io::Error> {
        let exclude_dirs = canonicalize_existing_paths(&options.exclude_dirs);
        let mut report = IndexReport::default();

        for root in options.include_dirs {
            if !root.is_dir() {
                report.failures.push(IndexFailure {
                    root: path_to_string(&root),
                    message: "index root is not a readable directory".to_owned(),
                });
                continue;
            }

            self.scan_dir(
                &root,
                &exclude_dirs,
                &options.exclude_patterns,
                &mut report.entries,
            )?;
        }

        report.entries.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then_with(|| left.name.cmp(&right.name))
        });
        Ok(report)
    }

    fn scan_dir(
        &self,
        dir: &Path,
        exclude_dirs: &[PathBuf],
        exclude_patterns: &[String],
        entries: &mut Vec<IndexedEntry>,
    ) -> Result<(), std::io::Error> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();

            if is_excluded_dir(&path, exclude_dirs) || matches_any_pattern(&name, exclude_patterns)
            {
                continue;
            }

            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                entries.push(IndexedEntry {
                    path: path_to_string(&path),
                    name,
                    kind: IndexedEntryKind::Directory,
                });
                self.scan_dir(&path, exclude_dirs, exclude_patterns, entries)?;
            } else if file_type.is_file() {
                entries.push(IndexedEntry {
                    path: path_to_string(&path),
                    name,
                    kind: IndexedEntryKind::File,
                });
            }
        }

        Ok(())
    }
}

fn canonicalize_existing_paths(paths: &[PathBuf]) -> Vec<PathBuf> {
    paths
        .iter()
        .filter_map(|path| path.canonicalize().ok())
        .collect()
}

fn is_excluded_dir(path: &Path, exclude_dirs: &[PathBuf]) -> bool {
    let Ok(canonical) = path.canonicalize() else {
        return false;
    };
    exclude_dirs.contains(&canonical)
}

fn matches_any_pattern(name: &str, patterns: &[String]) -> bool {
    patterns
        .iter()
        .any(|pattern| wildcard_matches(pattern, name))
}

fn wildcard_matches(pattern: &str, text: &str) -> bool {
    if pattern == "*" {
        return true;
    }

    if let Some((prefix, suffix)) = pattern.split_once('*') {
        return text.starts_with(prefix) && text.ends_with(suffix);
    }

    pattern == text
}

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

#[derive(Debug, Clone, Default)]
pub struct SearchIndex {
    entries: Vec<IndexedEntry>,
}

impl SearchIndex {
    pub fn from_entries(entries: Vec<IndexedEntry>) -> Self {
        Self { entries }
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
        self.entries = report.entries.clone();
        Ok(report)
    }

    pub fn search(&self, query: &QueryRequest) -> Vec<SearchResult> {
        match &query.mode {
            SearchMode::Normal => self.search_normal(&query.text),
            SearchMode::Regex => self.search_regex(&query.text),
            SearchMode::WebSearch { .. } | SearchMode::Command => Vec::new(),
        }
    }

    fn search_normal(&self, query: &str) -> Vec<SearchResult> {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return Vec::new();
        }

        self.entries
            .iter()
            .filter(|entry| {
                let haystack = format!("{} {}", entry.name, entry.path).to_lowercase();
                haystack.contains(&query) || fuzzy_matches(&query, &haystack)
            })
            .map(entry_to_result)
            .collect()
    }

    fn search_regex(&self, pattern: &str) -> Vec<SearchResult> {
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
            .map(entry_to_result)
            .collect()
    }
}

fn entry_to_result(entry: &IndexedEntry) -> SearchResult {
    let kind = match entry.kind {
        IndexedEntryKind::File => SearchResultKind::File,
        IndexedEntryKind::Directory => SearchResultKind::Directory,
    };

    SearchResult::new(
        format!("path:{}", entry.path),
        entry.name.clone(),
        kind,
        Action::OpenPath {
            path: entry.path.clone(),
        },
    )
    .with_detail(entry.path.clone())
    .with_secondary_action(Action::OpenContainingFolder {
        path: entry.path.clone(),
    })
    .with_secondary_action(Action::CopyText {
        text: entry.path.clone(),
    })
}

fn fuzzy_matches(query: &str, haystack: &str) -> bool {
    let mut chars = query.chars();
    let Some(mut current) = chars.next() else {
        return true;
    };

    for candidate in haystack.chars() {
        if candidate == current {
            match chars.next() {
                Some(next) => current = next,
                None => return true,
            }
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
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

        let report = IndexScanner
            .scan(IndexScanOptions {
                include_dirs: vec![root.clone()],
                exclude_dirs: vec![excluded_dir.clone()],
                exclude_patterns: vec!["*.log".to_owned()],
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

    fn file_entry(path: &str) -> IndexedEntry {
        IndexedEntry {
            path: path.to_owned(),
            name: path.rsplit('/').next().unwrap().to_owned(),
            kind: IndexedEntryKind::File,
        }
    }

    fn temp_dir(label: &str) -> std::path::PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("quickfox-{label}-{suffix}"))
    }
}
