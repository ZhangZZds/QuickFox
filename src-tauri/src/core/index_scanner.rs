//! Scanner boundary for the next file indexing pipeline.

use crate::core::index_entry::{
    IndexFailure, IndexReport, IndexScanStats, IndexedEntry, IndexedEntryKind, ScanEvent,
};
use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::{DirEntry, WalkBuilder};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

pub trait FileSystemScanner {
    fn scan(&self, plan: IndexScanPlan) -> Result<IndexReport, std::io::Error>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexScanStage {
    pub name: String,
    pub root_priority: u32,
}

impl IndexScanStage {
    pub fn new(name: impl Into<String>, root_priority: u32) -> Self {
        Self {
            name: name.into(),
            root_priority,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexScanPlan {
    pub include_roots: Vec<PathBuf>,
    pub exclude_dirs: Vec<PathBuf>,
    pub exclude_patterns: Vec<String>,
    pub respect_project_ignores: bool,
    pub stage: Option<IndexScanStage>,
}

#[derive(Debug, Clone)]
pub struct IndexPathRules {
    pub roots: Vec<PathBuf>,
    exclude_dirs: HashSet<PathBuf>,
    exclude_patterns: GlobSet,
    pub respect_project_ignores: bool,
}

impl IndexPathRules {
    pub fn from_plan(plan: &IndexScanPlan) -> Result<Self, std::io::Error> {
        Ok(Self {
            roots: unique_paths(plan.include_roots.clone()),
            exclude_dirs: canonicalize_existing_paths(&plan.exclude_dirs),
            exclude_patterns: compile_exclude_patterns(&plan.exclude_patterns)?,
            respect_project_ignores: plan.respect_project_ignores,
        })
    }

    pub fn configured_root_for(&self, path: &Path) -> Option<&Path> {
        self.roots
            .iter()
            .filter(|root| path.starts_with(root))
            .max_by_key(|root| root.components().count())
            .map(PathBuf::as_path)
    }

    pub fn is_forced_or_user_excluded(&self, path: &Path) -> bool {
        let configured_root = self.configured_root_for(path);
        path.ancestors()
            .take_while(|candidate| Some(*candidate) != configured_root)
            .any(|candidate| {
                is_forced_excluded(candidate)
                    || is_user_excluded(candidate, &self.exclude_dirs)
                    || matches_exclude_patterns(candidate, &self.exclude_patterns)
            })
    }

    pub fn is_excluded(&self, path: &Path) -> bool {
        self.is_forced_or_user_excluded(path) || is_inside_app_bundle(path)
    }
}

impl Default for IndexScanPlan {
    fn default() -> Self {
        Self {
            include_roots: Vec::new(),
            exclude_dirs: Vec::new(),
            exclude_patterns: Vec::new(),
            respect_project_ignores: true,
            stage: None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct IgnoreScanner {
    threads: usize,
}

impl IgnoreScanner {
    pub fn with_threads(threads: usize) -> Self {
        Self { threads }
    }

    pub fn scan_subtree(
        &self,
        target: &Path,
        configured_root: &Path,
        rules: &IndexPathRules,
    ) -> Result<IndexReport, std::io::Error> {
        self.scan_subtree_cancellable(target, configured_root, rules, || false)
    }

    pub fn scan_subtree_cancellable(
        &self,
        target: &Path,
        configured_root: &Path,
        rules: &IndexPathRules,
        is_cancelled: impl Fn() -> bool,
    ) -> Result<IndexReport, std::io::Error> {
        if !target.starts_with(configured_root) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "target is outside its configured index root",
            ));
        }
        if rules.is_excluded(target) {
            return Ok(IndexReport::default());
        }
        if !target.exists() {
            let failure = IndexFailure {
                root: path_to_string(target),
                message: "target path is not accessible".to_owned(),
            };
            return Ok(IndexReport {
                failures: vec![failure.clone()],
                scan_stats: IndexScanStats {
                    failures: 1,
                    ..IndexScanStats::default()
                },
                scan_events: vec![ScanEvent::Failure(failure)],
                ..IndexReport::default()
            });
        }
        if !self.path_is_included(target, configured_root, rules)? {
            return Ok(IndexReport::default());
        }

        let mut report = IndexReport::default();
        let filtered_skips = Arc::new(AtomicUsize::new(0));
        let filter_rules = rules.clone();
        let filter_skips = Arc::clone(&filtered_skips);
        let mut builder = WalkBuilder::new(target);
        builder
            .standard_filters(rules.respect_project_ignores)
            .hidden(false)
            .require_git(false)
            .threads(self.threads.max(1))
            .filter_entry(move |entry| {
                let keep = !filter_rules.is_excluded(entry.path());
                if !keep {
                    filter_skips.fetch_add(1, Ordering::Relaxed);
                }
                keep
            });

        for entry in builder.build() {
            if is_cancelled() {
                break;
            }
            match entry {
                Ok(entry) => {
                    report.scan_stats.scanned += 1;
                    if let Some(indexed_entry) =
                        indexed_entry_from_dir_entry(&entry, configured_root)
                    {
                        report.entries.push(indexed_entry);
                        report.scan_stats.accepted += 1;
                    } else {
                        report.scan_stats.skipped += 1;
                    }
                }
                Err(error) => push_failure(
                    &mut report,
                    IndexFailure {
                        root: error_path(&error)
                            .map(path_to_string)
                            .unwrap_or_else(|| path_to_string(target)),
                        message: error.to_string(),
                    },
                ),
            }
        }
        report.scan_stats.skipped += filtered_skips.load(Ordering::Relaxed);
        sort_report_entries(&mut report);
        Ok(report)
    }

    pub fn path_is_included(
        &self,
        target: &Path,
        configured_root: &Path,
        rules: &IndexPathRules,
    ) -> Result<bool, std::io::Error> {
        if !target.starts_with(configured_root) || rules.is_excluded(target) {
            return Ok(false);
        }
        if !rules.respect_project_ignores || target == configured_root {
            return Ok(true);
        }

        let target_path = target.to_path_buf();
        let filter_target = target_path.clone();
        let filter_rules = rules.clone();
        let mut builder = WalkBuilder::new(configured_root);
        builder
            .standard_filters(true)
            .hidden(false)
            .require_git(false)
            .threads(1)
            .filter_entry(move |entry| {
                entry.depth() == 0
                    || (filter_target.starts_with(entry.path())
                        && !filter_rules.is_excluded(entry.path()))
            });

        for entry in builder.build() {
            match entry {
                Ok(entry) if entry.path() == target_path => return Ok(true),
                Ok(_) => {}
                Err(error) => return Err(glob_error_to_io(error)),
            }
        }
        Ok(false)
    }
}

impl FileSystemScanner for IgnoreScanner {
    fn scan(&self, plan: IndexScanPlan) -> Result<IndexReport, std::io::Error> {
        let rules = IndexPathRules::from_plan(&plan)?;
        let mut report = IndexReport::default();

        for root in rules.roots.clone() {
            let root_label = path_to_string(&root);
            let stage_name = plan.stage.as_ref().map(|stage| stage.name.clone());
            report.scan_events.push(ScanEvent::RootStarted {
                root: root_label.clone(),
                stage: stage_name.clone(),
            });
            let root_start = report.scan_stats.clone();

            if !root.is_dir() {
                push_failure(
                    &mut report,
                    IndexFailure {
                        root: root_label.clone(),
                        message: "index root is not a readable directory".to_owned(),
                    },
                );
                report.scan_events.push(ScanEvent::RootFinished {
                    root: root_label,
                    stage: stage_name,
                    stats: delta_stats(&root_start, &report.scan_stats),
                });
                continue;
            }

            let filtered_skips = Arc::new(AtomicUsize::new(0));
            let filter_rules = rules.clone();
            let filter_skips = Arc::clone(&filtered_skips);
            let mut builder = WalkBuilder::new(&root);
            builder
                .standard_filters(plan.respect_project_ignores)
                .hidden(false)
                .require_git(false)
                .threads(self.threads.max(1))
                .filter_entry(move |entry| {
                    let path = entry.path();
                    let keep = entry.depth() == 0 || !filter_rules.is_excluded(path);
                    if !keep {
                        filter_skips.fetch_add(1, Ordering::Relaxed);
                    }
                    keep
                });

            for entry in builder.build() {
                match entry {
                    Ok(entry) => {
                        if entry.depth() == 0 {
                            continue;
                        }
                        report.scan_stats.scanned += 1;
                        if let Some(indexed_entry) = indexed_entry_from_dir_entry(&entry, &root) {
                            report.entries.push(indexed_entry);
                            report.scan_stats.accepted += 1;
                        } else {
                            report.scan_stats.skipped += 1;
                        }
                    }
                    Err(error) => {
                        let failure = IndexFailure {
                            root: error_path(&error)
                                .map(path_to_string)
                                .unwrap_or_else(|| root_label.clone()),
                            message: error.to_string(),
                        };
                        push_failure(&mut report, failure);
                    }
                }
            }

            report.scan_stats.skipped += filtered_skips.load(Ordering::Relaxed);
            report.scan_events.push(ScanEvent::RootFinished {
                root: root_label,
                stage: stage_name,
                stats: delta_stats(&root_start, &report.scan_stats),
            });
        }

        sort_report_entries(&mut report);
        Ok(report)
    }
}

#[derive(Debug, Clone, Default)]
pub struct StdFsScanner;

impl FileSystemScanner for StdFsScanner {
    fn scan(&self, plan: IndexScanPlan) -> Result<IndexReport, std::io::Error> {
        let rules = IndexPathRules::from_plan(&plan)?;
        let mut report = IndexReport::default();

        for root in rules.roots.clone() {
            if !root.is_dir() {
                push_failure(
                    &mut report,
                    IndexFailure {
                        root: path_to_string(&root),
                        message: "index root is not a readable directory".to_owned(),
                    },
                );
                continue;
            }
            scan_dir_std(&root, &root, &rules, &mut report)?;
        }

        sort_report_entries(&mut report);
        Ok(report)
    }
}

fn scan_dir_std(
    root: &Path,
    dir: &Path,
    rules: &IndexPathRules,
    report: &mut IndexReport,
) -> Result<(), std::io::Error> {
    let read_dir = match fs::read_dir(dir) {
        Ok(read_dir) => read_dir,
        Err(error) => {
            push_failure(
                report,
                IndexFailure {
                    root: path_to_string(dir),
                    message: error.to_string(),
                },
            );
            if dir == root {
                return Err(error);
            }
            return Ok(());
        }
    };

    for entry in read_dir {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                push_failure(
                    report,
                    IndexFailure {
                        root: path_to_string(dir),
                        message: error.to_string(),
                    },
                );
                continue;
            }
        };
        let path = entry.path();
        if rules.is_excluded(&path) {
            report.scan_stats.skipped += 1;
            continue;
        }
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) => {
                push_failure(
                    report,
                    IndexFailure {
                        root: path_to_string(&path),
                        message: error.to_string(),
                    },
                );
                continue;
            }
        };
        report.scan_stats.scanned += 1;
        let kind = if is_application_path(&path) {
            IndexedEntryKind::Application
        } else if file_type.is_dir() {
            IndexedEntryKind::Directory
        } else if file_type.is_file() {
            IndexedEntryKind::File
        } else {
            report.scan_stats.skipped += 1;
            continue;
        };
        report
            .entries
            .push(IndexedEntry::from_path_metadata(&path, root, kind));
        report.scan_stats.accepted += 1;
        if file_type.is_dir() && !is_application_path(&path) {
            scan_dir_std(root, &path, rules, report)?;
        }
    }
    Ok(())
}

fn indexed_entry_from_dir_entry(entry: &DirEntry, root: &Path) -> Option<IndexedEntry> {
    let path = entry.path();
    let file_type = entry.file_type()?;
    let kind = if is_application_path(path) {
        IndexedEntryKind::Application
    } else if file_type.is_dir() {
        IndexedEntryKind::Directory
    } else if file_type.is_file() {
        IndexedEntryKind::File
    } else {
        return None;
    };

    Some(IndexedEntry::from_path_metadata(path, root, kind))
}

fn push_failure(report: &mut IndexReport, failure: IndexFailure) {
    report.scan_stats.failures += 1;
    report.scan_events.push(ScanEvent::Failure(failure.clone()));
    report.failures.push(failure);
}

fn delta_stats(before: &IndexScanStats, after: &IndexScanStats) -> IndexScanStats {
    IndexScanStats {
        scanned: after.scanned.saturating_sub(before.scanned),
        accepted: after.accepted.saturating_sub(before.accepted),
        skipped: after.skipped.saturating_sub(before.skipped),
        failures: after.failures.saturating_sub(before.failures),
    }
}

fn sort_report_entries(report: &mut IndexReport) {
    report.entries.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.name.cmp(&right.name))
    });
}

fn compile_exclude_patterns(patterns: &[String]) -> Result<GlobSet, std::io::Error> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let pattern = pattern.to_ascii_lowercase();
        builder.add(Glob::new(&pattern).map_err(glob_error_to_io)?);
        if !pattern.contains('/') && !pattern.contains('\\') {
            builder.add(Glob::new(&format!("**/{pattern}")).map_err(glob_error_to_io)?);
        }
    }
    builder.build().map_err(glob_error_to_io)
}

fn glob_error_to_io(error: impl ToString) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, error.to_string())
}

fn matches_exclude_patterns(path: &Path, patterns: &GlobSet) -> bool {
    let path_lower = path_to_string(path).to_ascii_lowercase();
    let name_matches = path.file_name().is_some_and(|name| {
        let name_lower = name.to_string_lossy().to_ascii_lowercase();
        patterns.is_match(Path::new(&name_lower))
    });
    name_matches || patterns.is_match(Path::new(&path_lower))
}

fn unique_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut unique = Vec::new();
    for path in paths {
        let key = path.canonicalize().unwrap_or_else(|_| path.clone());
        if seen.insert(key) {
            unique.push(path);
        }
    }
    unique
}

fn canonicalize_existing_paths(paths: &[PathBuf]) -> HashSet<PathBuf> {
    paths
        .iter()
        .flat_map(|path| {
            let canonical = path.canonicalize().ok();
            std::iter::once(path.clone()).chain(canonical)
        })
        .collect()
}

fn is_user_excluded(path: &Path, exclude_dirs: &HashSet<PathBuf>) -> bool {
    exclude_dirs.contains(path)
}

fn is_forced_excluded(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    matches!(
        name.to_ascii_lowercase().as_str(),
        ".git"
            | "node_modules"
            | "target"
            | "$recycle.bin"
            | "system volume information"
            | "windows"
            | "recovery"
            | "appdata"
            | ".cache"
            | "__pycache__"
            | ".venv"
            | "venv"
    )
}

fn is_inside_app_bundle(path: &Path) -> bool {
    let mut ancestors = path.ancestors();
    let _self = ancestors.next();
    ancestors.any(is_application_path)
}

fn is_application_path(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".app")
        || lower.ends_with(".exe")
        || lower.ends_with(".lnk")
        || lower.ends_with(".desktop")
}

fn error_path(error: &ignore::Error) -> Option<&Path> {
    match error {
        ignore::Error::Partial(errors) => errors.iter().find_map(error_path),
        ignore::Error::WithLineNumber { err, .. } => error_path(err),
        ignore::Error::WithPath { path, .. } => Some(path),
        ignore::Error::WithDepth { err, .. } => error_path(err),
        ignore::Error::Loop { child, .. } => Some(child),
        ignore::Error::Io(_)
        | ignore::Error::Glob { .. }
        | ignore::Error::UnrecognizedFileType(_)
        | ignore::Error::InvalidDefinition => None,
    }
}

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::index::{IndexFailure, IndexedEntryKind};
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn ignore_scanner_reports_events_and_stats_for_basic_scan() {
        let root = temp_dir("ignore-basic");
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::write(root.join("docs").join("readme.md"), "").unwrap();

        let report = IgnoreScanner::default()
            .scan(IndexScanPlan {
                include_roots: vec![root.clone()],
                stage: Some(IndexScanStage::new("configured-roots", 10)),
                ..IndexScanPlan::default()
            })
            .unwrap();

        assert!(report.failures.is_empty());
        assert!(report
            .entries
            .iter()
            .any(|entry| { entry.name == "docs" && entry.kind == IndexedEntryKind::Directory }));
        assert!(report
            .entries
            .iter()
            .any(|entry| { entry.name == "readme.md" && entry.kind == IndexedEntryKind::File }));
        assert!(report.scan_stats.scanned >= 2);
        assert_eq!(report.scan_stats.accepted, report.entries.len());
        assert_eq!(report.scan_stats.failures, 0);
        assert!(report.scan_events.iter().any(|event| matches!(
            event,
            ScanEvent::RootStarted { root, stage }
                if root.contains("ignore-basic") && stage.as_deref() == Some("configured-roots")
        )));
        assert!(report.scan_events.iter().any(|event| matches!(
            event,
            ScanEvent::RootFinished { root, stage, .. }
                if root.contains("ignore-basic") && stage.as_deref() == Some("configured-roots")
        )));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ignore_scanner_forced_excludes_cannot_be_reincluded_by_gitignore() {
        let root = temp_dir("ignore-forced-excludes");
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::create_dir_all(root.join("node_modules")).unwrap();
        fs::create_dir_all(root.join("target")).unwrap();
        fs::create_dir_all(root.join("Windows")).unwrap();
        fs::write(root.join(".git").join("config"), "").unwrap();
        fs::write(root.join("node_modules").join("package.js"), "").unwrap();
        fs::write(root.join("target").join("artifact"), "").unwrap();
        fs::write(root.join("Windows").join("system.ini"), "").unwrap();
        fs::write(
            root.join(".gitignore"),
            "!node_modules/**\n!target/**\n!.git/**\n",
        )
        .unwrap();
        fs::write(root.join("keep.md"), "").unwrap();

        let report = IgnoreScanner::default()
            .scan(IndexScanPlan {
                include_roots: vec![root.clone()],
                respect_project_ignores: true,
                ..IndexScanPlan::default()
            })
            .unwrap();
        let names: Vec<_> = report
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();

        assert!(names.contains(&"keep.md"));
        assert!(!names.contains(&".git"));
        assert!(!names.contains(&"config"));
        assert!(!names.contains(&"node_modules"));
        assert!(!names.contains(&"package.js"));
        assert!(!names.contains(&"target"));
        assert!(!names.contains(&"artifact"));
        assert!(!names.contains(&"Windows"));
        assert!(!names.contains(&"system.ini"));
        assert!(report.scan_stats.skipped >= 4);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ignore_scanner_respects_and_can_disable_project_ignores() {
        let root = temp_dir("ignore-project-ignore");
        fs::write(root.join(".gitignore"), "ignored.txt\n").unwrap();
        fs::write(root.join("ignored.txt"), "").unwrap();
        fs::write(root.join("keep.txt"), "").unwrap();

        let honoring = IgnoreScanner::default()
            .scan(IndexScanPlan {
                include_roots: vec![root.clone()],
                respect_project_ignores: true,
                ..IndexScanPlan::default()
            })
            .unwrap();
        assert!(!honoring
            .entries
            .iter()
            .any(|entry| entry.name == "ignored.txt"));

        let disabled = IgnoreScanner::default()
            .scan(IndexScanPlan {
                include_roots: vec![root.clone()],
                respect_project_ignores: false,
                ..IndexScanPlan::default()
            })
            .unwrap();
        assert!(disabled
            .entries
            .iter()
            .any(|entry| entry.name == "ignored.txt"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ignore_scanner_does_not_retain_per_entry_accepted_events() {
        let root = temp_dir("ignore-bounded-events");
        for index in 0..64 {
            fs::write(root.join(format!("file-{index}.md")), "").unwrap();
        }

        let report = IgnoreScanner::default()
            .scan(IndexScanPlan {
                include_roots: vec![root.clone()],
                stage: Some(IndexScanStage::new("large-root", 10)),
                ..IndexScanPlan::default()
            })
            .unwrap();

        assert_eq!(report.entries.len(), 64);
        assert_eq!(report.scan_stats.accepted, 64);
        assert!(!report
            .scan_events
            .iter()
            .any(|event| matches!(event, ScanEvent::EntryAccepted { .. })));
        assert!(report.scan_events.iter().any(|event| matches!(
            event,
            ScanEvent::RootStarted { stage, .. } if stage.as_deref() == Some("large-root")
        )));
        assert!(report.scan_events.iter().any(|event| matches!(
            event,
            ScanEvent::RootFinished { stage, stats, .. }
                if stage.as_deref() == Some("large-root") && stats.accepted == 64
        )));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ignore_scanner_indexes_application_entry_points_without_app_internals() {
        let root = temp_dir("ignore-apps");
        let app_helper = root.join("Tool.app").join("Contents").join("MacOS");
        fs::create_dir_all(&app_helper).unwrap();
        fs::write(app_helper.join("internal-helper"), "").unwrap();
        fs::write(root.join("tool.exe"), "").unwrap();
        fs::write(root.join("shortcut.lnk"), "").unwrap();
        fs::write(root.join("QuickFox.desktop"), "").unwrap();

        let report = IgnoreScanner::default()
            .scan(IndexScanPlan {
                include_roots: vec![root.clone()],
                ..IndexScanPlan::default()
            })
            .unwrap();

        for app_name in ["Tool.app", "tool.exe", "shortcut.lnk", "QuickFox.desktop"] {
            assert!(
                report.entries.iter().any(|entry| {
                    entry.name == app_name && entry.kind == IndexedEntryKind::Application
                }),
                "{app_name} should be indexed as an application"
            );
        }
        assert!(!report
            .entries
            .iter()
            .any(|entry| entry.name == "internal-helper"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ignore_scanner_deduplicates_roots_and_reports_unavailable_roots() {
        let root = temp_dir("ignore-dedupe");
        fs::write(root.join("keep.md"), "").unwrap();
        let missing = root.join("missing");

        let report = IgnoreScanner::default()
            .scan(IndexScanPlan {
                include_roots: vec![root.clone(), root.clone(), missing.clone()],
                ..IndexScanPlan::default()
            })
            .unwrap();

        assert_eq!(
            report
                .entries
                .iter()
                .filter(|entry| entry.name == "keep.md")
                .count(),
            1
        );
        assert!(report
            .failures
            .iter()
            .any(|failure| failure.root == missing.to_string_lossy()));
        assert_eq!(report.scan_stats.failures, report.failures.len());

        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn ignore_scanner_degrades_on_unreadable_child_without_aborting_root() {
        use std::os::unix::fs::PermissionsExt;

        let root = temp_dir("ignore-unreadable-child");
        fs::write(root.join("keep.md"), "").unwrap();
        let locked = root.join("locked");
        fs::create_dir_all(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

        let report = IgnoreScanner::default()
            .scan(IndexScanPlan {
                include_roots: vec![root.clone()],
                ..IndexScanPlan::default()
            })
            .unwrap();

        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();

        assert!(report.entries.iter().any(|entry| entry.name == "keep.md"));
        assert!(report
            .failures
            .iter()
            .any(|failure| failure.root.ends_with("locked")));
        assert!(report.scan_events.iter().any(|event| matches!(
            event,
            ScanEvent::Failure(IndexFailure { root, .. }) if root.ends_with("locked")
        )));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn index_path_rules_choose_longest_segment_boundary_root() {
        let root = temp_dir("rules-root");
        let nested = root.join("work");
        let sibling_prefix = root.join("workspace");
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir_all(&sibling_prefix).unwrap();
        let rules = IndexPathRules::from_plan(&IndexScanPlan {
            include_roots: vec![root.clone(), nested.clone()],
            ..IndexScanPlan::default()
        })
        .unwrap();

        assert_eq!(
            rules.configured_root_for(&nested.join("file.md")),
            Some(nested.as_path())
        );
        assert_eq!(
            rules.configured_root_for(&sibling_prefix.join("file.md")),
            Some(root.as_path())
        );
        assert_eq!(
            rules.configured_root_for(&root.with_extension("other")),
            None
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn index_path_rules_apply_forced_user_and_glob_exclusions_to_descendants() {
        let root = temp_dir("rules-excludes");
        let user_excluded = root.join("private");
        fs::create_dir_all(user_excluded.join("nested")).unwrap();
        let rules = IndexPathRules::from_plan(&IndexScanPlan {
            include_roots: vec![root.clone()],
            exclude_dirs: vec![user_excluded.clone()],
            exclude_patterns: vec!["*.tmp".to_owned()],
            ..IndexScanPlan::default()
        })
        .unwrap();

        assert!(rules.is_forced_or_user_excluded(&root.join("node_modules/pkg/a.js")));
        assert!(rules.is_forced_or_user_excluded(&user_excluded.join("nested/file.md")));
        assert!(rules.is_forced_or_user_excluded(&root.join("nested/cache.tmp")));
        assert!(!rules.is_forced_or_user_excluded(&root.join("nested/keep.md")));

        let _ = fs::remove_dir_all(root);
    }

    fn temp_dir(label: &str) -> std::path::PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("quickfox-{label}-{suffix}"));
        fs::create_dir_all(&root).unwrap();
        root
    }
}
