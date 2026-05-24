//! File and directory indexing will live here.

use crate::core::actions::Action;
use crate::core::search::{QueryRequest, SearchMode, SearchResult, SearchResultKind};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IndexedEntryKind {
    Application,
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
                &root,
                &exclude_dirs,
                &options.exclude_patterns,
                &mut report,
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
        root: &Path,
        dir: &Path,
        exclude_dirs: &[PathBuf],
        exclude_patterns: &[String],
        report: &mut IndexReport,
    ) -> Result<(), std::io::Error> {
        let read_dir = match fs::read_dir(dir) {
            Ok(read_dir) => read_dir,
            Err(error) => {
                report.failures.push(IndexFailure {
                    root: path_to_string(dir),
                    message: error.to_string(),
                });
                if dir == root {
                    return Err(error);
                }
                return Ok(());
            }
        };

        for entry in read_dir {
            let entry = entry?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();

            if is_excluded_dir(&path, exclude_dirs) || matches_any_pattern(&name, exclude_patterns)
            {
                continue;
            }

            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                if is_application_path(&path) {
                    report.entries.push(IndexedEntry {
                        path: path_to_string(&path),
                        name,
                        kind: IndexedEntryKind::Application,
                    });
                    continue;
                }
                report.entries.push(IndexedEntry {
                    path: path_to_string(&path),
                    name,
                    kind: IndexedEntryKind::Directory,
                });
                self.scan_dir(root, &path, exclude_dirs, exclude_patterns, report)?;
            } else if file_type.is_file() {
                let kind = if is_application_path(&path) {
                    IndexedEntryKind::Application
                } else {
                    IndexedEntryKind::File
                };
                report.entries.push(IndexedEntry {
                    path: path_to_string(&path),
                    name,
                    kind,
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

fn is_application_path(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let lower = name.to_lowercase();
    lower.ends_with(".app")
        || lower.ends_with(".exe")
        || lower.ends_with(".lnk")
        || lower.ends_with(".desktop")
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

    pub fn refresh_incremental_with_scanner(
        &mut self,
        scanner: &IndexScanner,
        options: IndexScanOptions,
    ) -> Result<IndexReport, std::io::Error> {
        let report = scanner.scan(options)?;
        let mut previous_by_path: std::collections::HashMap<_, _> = self
            .entries
            .iter()
            .cloned()
            .map(|entry| (entry.path.clone(), entry))
            .collect();

        self.entries = report
            .entries
            .iter()
            .map(|entry| {
                previous_by_path
                    .remove(&entry.path)
                    .filter(|previous| previous == entry)
                    .unwrap_or_else(|| entry.clone())
            })
            .collect();
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
                haystack.contains(&query) || fuzzy_matches_with_quality(&query, &haystack)
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
        IndexedEntryKind::Application => SearchResultKind::Application,
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

fn fuzzy_matches_with_quality(query: &str, haystack: &str) -> bool {
    if query.len() < 2 {
        return false;
    }

    let mut chars = query.chars();
    let Some(mut current) = chars.next() else {
        return true;
    };
    let mut first_match: Option<usize> = None;

    for (index, candidate) in haystack.chars().enumerate() {
        if candidate == current {
            first_match.get_or_insert(index);
            match chars.next() {
                Some(next) => current = next,
                None => {
                    let span = index.saturating_sub(first_match.unwrap_or(index)) + 1;
                    let max_span = query.chars().count().saturating_mul(4).max(16);
                    return span <= max_span;
                }
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
        }]);
        let parser = crate::core::search::QueryParser::new(Default::default());

        let results = index.search(&parser.parse("Openspec_123"));

        assert!(results.is_empty());
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
