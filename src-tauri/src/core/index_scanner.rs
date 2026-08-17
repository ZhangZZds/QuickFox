//! Scanner boundary for the next file indexing pipeline.

use crate::core::index_entry::{
    normalize_path_key, normalize_path_key_for_mode, path_is_same_or_descendant_for_mode,
    IndexFailure, IndexReport, IndexScanStats, IndexedEntry, IndexedEntryKind, PathComparisonMode,
    ScanEvent,
};
use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::gitignore::{gitconfig_excludes_path, Gitignore, GitignoreBuilder};
use ignore::{DirEntry, Match, WalkBuilder};
use std::collections::VecDeque;
use std::collections::{HashMap, HashSet};
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexDirectoryScanCheckpoint {
    pub root: PathBuf,
    pub directory: PathBuf,
    pub discovered_directories: Vec<PathBuf>,
    pub stats: IndexScanStats,
    pub failure: Option<IndexFailure>,
}

#[derive(Debug, Clone)]
pub struct IndexPathRules {
    pub roots: Vec<PathBuf>,
    root_candidates: Vec<ConfiguredRootCandidate>,
    exclude_dirs: HashSet<PathBuf>,
    exclude_patterns: GlobSet,
    pub respect_project_ignores: bool,
}

#[derive(Debug, Clone)]
struct ConfiguredRootCandidate {
    path: PathBuf,
    root_index: usize,
}

impl IndexPathRules {
    pub fn from_plan(plan: &IndexScanPlan) -> Result<Self, std::io::Error> {
        let roots = unique_paths(plan.include_roots.clone());
        let root_candidates = canonicalize_with_originals(&roots);
        Ok(Self {
            roots,
            root_candidates,
            exclude_dirs: canonicalize_existing_paths(&plan.exclude_dirs),
            exclude_patterns: compile_exclude_patterns(&plan.exclude_patterns)?,
            respect_project_ignores: plan.respect_project_ignores,
        })
    }

    pub fn configured_root_for(&self, path: &Path) -> Option<&Path> {
        self.configured_root_for_mode(path, PathComparisonMode::native())
    }

    fn configured_root_for_mode(&self, path: &Path, mode: PathComparisonMode) -> Option<&Path> {
        self.configured_root_match_for_mode(path, mode)
            .map(|candidate| self.roots[candidate.root_index].as_path())
    }

    fn configured_root_boundary_for_mode(
        &self,
        path: &Path,
        mode: PathComparisonMode,
    ) -> Option<&Path> {
        self.configured_root_match_for_mode(path, mode)
            .map(|candidate| candidate.path.as_path())
    }

    fn configured_root_match_for_mode(
        &self,
        path: &Path,
        mode: PathComparisonMode,
    ) -> Option<&ConfiguredRootCandidate> {
        self.root_candidates
            .iter()
            .filter(|candidate| path_is_same_or_descendant_for_mode(&candidate.path, path, mode))
            .max_by_key(|candidate| {
                normalize_path_key_for_mode(&candidate.path, mode)
                    .split('/')
                    .filter(|component| !component.is_empty())
                    .count()
            })
    }

    pub fn is_forced_or_user_excluded(&self, path: &Path) -> bool {
        let configured_root =
            self.configured_root_boundary_for_mode(path, PathComparisonMode::native());
        let configured_root_key = configured_root.map(normalize_path_key);
        path.ancestors()
            .take_while(|candidate| {
                configured_root_key
                    .as_ref()
                    .is_none_or(|root| normalize_path_key(candidate) != *root)
            })
            .any(|candidate| {
                let pattern_candidate = configured_root
                    .and_then(|root| candidate.strip_prefix(root).ok())
                    .filter(|relative| !relative.as_os_str().is_empty())
                    .unwrap_or(candidate);
                is_forced_excluded(candidate)
                    || is_user_excluded(candidate, &self.exclude_dirs)
                    || matches_exclude_patterns(pattern_candidate, &self.exclude_patterns)
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

pub trait IgnorePathProbe: std::fmt::Debug + Send + Sync {
    fn read_file(&self, path: &Path) -> Result<Option<String>, std::io::Error>;
    fn is_directory(&self, path: &Path) -> Result<bool, std::io::Error>;
    fn global_ignore_path(&self) -> Option<PathBuf>;
    fn read_dir(&self, path: &Path) -> Result<Vec<PathBuf>, std::io::Error>;
}

#[derive(Debug, Default)]
struct StdIgnorePathProbe;

impl IgnorePathProbe for StdIgnorePathProbe {
    fn read_file(&self, path: &Path) -> Result<Option<String>, std::io::Error> {
        match fs::read_to_string(path) {
            Ok(contents) => Ok(Some(contents)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn is_directory(&self, path: &Path) -> Result<bool, std::io::Error> {
        match fs::metadata(path) {
            Ok(metadata) => Ok(metadata.is_dir()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    fn global_ignore_path(&self) -> Option<PathBuf> {
        gitconfig_excludes_path()
    }

    fn read_dir(&self, path: &Path) -> Result<Vec<PathBuf>, std::io::Error> {
        fs::read_dir(path)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect()
    }
}

#[derive(Debug, Clone)]
pub struct IgnoreScanner {
    threads: usize,
    path_probe: Arc<dyn IgnorePathProbe>,
}

#[derive(Debug, Default)]
pub(crate) struct IgnoreBatchCache {
    directory_kinds: HashMap<PathBuf, bool>,
    matchers: HashMap<(PathBuf, PathBuf), Option<Gitignore>>,
    global_ignore_path: Option<Option<PathBuf>>,
}

impl Default for IgnoreScanner {
    fn default() -> Self {
        Self {
            threads: 0,
            path_probe: Arc::new(StdIgnorePathProbe),
        }
    }
}

impl IgnoreScanner {
    pub fn with_threads(threads: usize) -> Self {
        Self {
            threads,
            ..Self::default()
        }
    }

    pub fn with_path_probe(path_probe: Arc<dyn IgnorePathProbe>) -> Self {
        Self {
            path_probe,
            ..Self::default()
        }
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
        let mut cache = IgnoreBatchCache::default();
        self.scan_subtree_cancellable_cached(
            target,
            configured_root,
            rules,
            &mut cache,
            is_cancelled,
        )
    }

    pub(crate) fn batch_cache(&self) -> IgnoreBatchCache {
        IgnoreBatchCache::default()
    }

    pub(crate) fn scan_subtree_cancellable_cached(
        &self,
        target: &Path,
        configured_root: &Path,
        rules: &IndexPathRules,
        cache: &mut IgnoreBatchCache,
        is_cancelled: impl Fn() -> bool,
    ) -> Result<IndexReport, std::io::Error> {
        if rules.configured_root_for(target) != Some(configured_root) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "target is outside its configured index root",
            ));
        }
        if rules.is_excluded(target) {
            return Ok(IndexReport::default());
        }
        let target_metadata = match fs::metadata(target) {
            Ok(metadata) => metadata,
            Err(error) => {
                let failure = IndexFailure {
                    root: path_to_string(target),
                    message: if error.kind() == std::io::ErrorKind::NotFound {
                        "target path is not accessible".to_owned()
                    } else {
                        error.to_string()
                    },
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
        };
        match self.path_is_included_cancellable_cached(
            target,
            configured_root,
            rules,
            target_metadata.is_dir(),
            cache,
            &is_cancelled,
        )? {
            Some(true) => {}
            Some(false) | None => return Ok(IndexReport::default()),
        }

        if !target_metadata.is_dir() {
            let mut report = IndexReport {
                scan_stats: IndexScanStats {
                    scanned: 1,
                    ..IndexScanStats::default()
                },
                ..IndexReport::default()
            };
            if let Some(entry) =
                indexed_entry_from_metadata(target, configured_root, &target_metadata)
            {
                report.entries.push(entry);
                report.scan_stats.accepted = 1;
            } else {
                report.scan_stats.skipped = 1;
            }
            return Ok(report);
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
        Ok(self
            .path_is_included_cancellable(target, configured_root, rules, &|| false)?
            .unwrap_or(false))
    }

    pub fn path_is_included_cancellable(
        &self,
        target: &Path,
        configured_root: &Path,
        rules: &IndexPathRules,
        is_cancelled: &impl Fn() -> bool,
    ) -> Result<Option<bool>, std::io::Error> {
        let mut cache = IgnoreBatchCache::default();
        self.path_is_included_cancellable_cached(
            target,
            configured_root,
            rules,
            target.is_dir(),
            &mut cache,
            is_cancelled,
        )
    }

    fn path_is_included_cancellable_cached(
        &self,
        target: &Path,
        configured_root: &Path,
        rules: &IndexPathRules,
        target_is_dir: bool,
        cache: &mut IgnoreBatchCache,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<Option<bool>, std::io::Error> {
        if !path_is_same_or_descendant_for_mode(
            configured_root,
            target,
            PathComparisonMode::native(),
        ) || rules.is_excluded(target)
        {
            return Ok(Some(false));
        }
        if !rules.respect_project_ignores
            || normalize_path_key(target) == normalize_path_key(configured_root)
        {
            return Ok(Some(true));
        }
        AncestorIgnoreEvaluator::new(self.path_probe.as_ref(), is_cancelled, cache)
            .evaluate_inclusion(configured_root, target, target_is_dir)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IgnoreDecision {
    None,
    Ignore,
    Include,
}

struct AncestorIgnoreEvaluator<'a> {
    probe: &'a dyn IgnorePathProbe,
    is_cancelled: &'a dyn Fn() -> bool,
    cache: &'a mut IgnoreBatchCache,
    dot_ignore: Vec<Gitignore>,
    git_ignore: Vec<Gitignore>,
    git_exclude: Vec<Gitignore>,
    global: Option<Gitignore>,
}

impl<'a> AncestorIgnoreEvaluator<'a> {
    fn new(
        probe: &'a dyn IgnorePathProbe,
        is_cancelled: &'a dyn Fn() -> bool,
        cache: &'a mut IgnoreBatchCache,
    ) -> Self {
        Self {
            probe,
            is_cancelled,
            cache,
            dot_ignore: Vec::new(),
            git_ignore: Vec::new(),
            git_exclude: Vec::new(),
            global: None,
        }
    }

    fn evaluate_inclusion(
        mut self,
        configured_root: &Path,
        target: &Path,
        target_is_dir: bool,
    ) -> Result<Option<bool>, std::io::Error> {
        if self.cancelled() {
            return Ok(None);
        }
        self.load_global()?;
        self.load_ancestor_directories(configured_root)?;
        if self.cancelled() {
            return Ok(None);
        }
        self.load_directory(configured_root)?;

        let relative = target
            .strip_prefix(configured_root)
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                target
                    .components()
                    .skip(configured_root.components().count())
                    .collect()
            });
        let components: Vec<_> = relative.components().collect();
        let mut current = configured_root.to_path_buf();
        for (index, component) in components.iter().enumerate() {
            if self.cancelled() {
                return Ok(None);
            }
            current.push(component);
            let is_last = index + 1 == components.len();
            let is_directory = !is_last || target_is_dir;
            if self.decision(&current, is_directory) == IgnoreDecision::Ignore {
                return Ok(Some(false));
            }
            if is_directory && !is_last {
                self.load_directory(&current)?;
            }
        }
        Ok(Some(true))
    }

    fn load_global(&mut self) -> Result<(), std::io::Error> {
        if self.cancelled() {
            return Ok(());
        }
        let global_ignore_path = match self.cache.global_ignore_path.clone() {
            Some(path) => path,
            None => {
                let path = self.probe.global_ignore_path();
                self.cache.global_ignore_path = Some(path.clone());
                path
            }
        };
        let Some(path) = global_ignore_path else {
            return Ok(());
        };
        let root = std::env::current_dir()?;
        self.global = self.load_matcher(&root, &path)?;
        Ok(())
    }

    fn load_directory(&mut self, directory: &Path) -> Result<(), std::io::Error> {
        if self.cancelled() {
            return Ok(());
        }
        let git_dir = directory.join(".git");
        let is_git_directory = if let Some(is_directory) = self.cache.directory_kinds.get(&git_dir)
        {
            *is_directory
        } else {
            let is_directory = self.probe.is_directory(&git_dir)?;
            self.cache
                .directory_kinds
                .insert(git_dir.clone(), is_directory);
            is_directory
        };
        if is_git_directory {
            if let Some(matcher) = self.load_matcher(directory, &git_dir.join("info/exclude"))? {
                self.git_exclude.push(matcher);
            }
        }
        if let Some(matcher) = self.load_matcher(directory, &directory.join(".gitignore"))? {
            self.git_ignore.push(matcher);
        }
        if let Some(matcher) = self.load_matcher(directory, &directory.join(".ignore"))? {
            self.dot_ignore.push(matcher);
        }
        Ok(())
    }

    fn load_ancestor_directories(&mut self, configured_root: &Path) -> Result<(), std::io::Error> {
        let mut ancestors: Vec<_> = configured_root.ancestors().skip(1).collect();
        ancestors.reverse();
        for directory in ancestors {
            if self.cancelled() {
                return Ok(());
            }
            self.load_directory(directory)?;
        }
        Ok(())
    }

    fn load_matcher(
        &mut self,
        root: &Path,
        path: &Path,
    ) -> Result<Option<Gitignore>, std::io::Error> {
        if self.cancelled() {
            return Ok(None);
        }
        let cache_key = (root.to_path_buf(), path.to_path_buf());
        if let Some(matcher) = self.cache.matchers.get(&cache_key) {
            return Ok(matcher.clone());
        }
        let Some(contents) = self.probe.read_file(path)? else {
            self.cache.matchers.insert(cache_key, None);
            return Ok(None);
        };
        let mut builder = GitignoreBuilder::new(root);
        for line in contents.lines() {
            if self.cancelled() {
                return Ok(None);
            }
            builder
                .add_line(Some(path.to_path_buf()), line)
                .map_err(glob_error_to_io)?;
        }
        let matcher = builder.build().map(Some).map_err(glob_error_to_io)?;
        self.cache.matchers.insert(cache_key, matcher.clone());
        Ok(matcher)
    }

    fn decision(&self, path: &Path, is_dir: bool) -> IgnoreDecision {
        first_match(&self.dot_ignore, path, is_dir)
            .or_else(|| first_match(&self.git_ignore, path, is_dir))
            .or_else(|| first_match(&self.git_exclude, path, is_dir))
            .or_else(|| {
                self.global
                    .as_ref()
                    .map(|matcher| matcher_decision(matcher, path, is_dir))
                    .filter(|decision| *decision != IgnoreDecision::None)
            })
            .unwrap_or(IgnoreDecision::None)
    }

    fn cancelled(&self) -> bool {
        (self.is_cancelled)()
    }
}

fn first_match(matchers: &[Gitignore], path: &Path, is_dir: bool) -> Option<IgnoreDecision> {
    matchers
        .iter()
        .rev()
        .map(|matcher| matcher_decision(matcher, path, is_dir))
        .find(|decision| *decision != IgnoreDecision::None)
}

fn matcher_decision(matcher: &Gitignore, path: &Path, is_dir: bool) -> IgnoreDecision {
    match matcher.matched(path, is_dir) {
        Match::None => IgnoreDecision::None,
        Match::Ignore(_) => IgnoreDecision::Ignore,
        Match::Whitelist(_) => IgnoreDecision::Include,
    }
}

impl IgnoreScanner {
    pub fn scan_cancellable(
        &self,
        plan: IndexScanPlan,
        is_cancelled: impl Fn() -> bool,
    ) -> Result<IndexReport, std::io::Error> {
        self.scan_cancellable_inner(plan, is_cancelled, true, |_, _| Ok(()))
    }

    pub fn scan_cancellable_streaming(
        &self,
        plan: IndexScanPlan,
        is_cancelled: impl Fn() -> bool,
        on_batch: impl FnMut(&[IndexedEntry], &IndexScanStats) -> Result<(), std::io::Error>,
    ) -> Result<IndexReport, std::io::Error> {
        self.scan_cancellable_inner(plan, is_cancelled, false, on_batch)
    }

    pub fn scan_resumable_cancellable_streaming(
        &self,
        plan: IndexScanPlan,
        pending_directories: Vec<PathBuf>,
        completed_stats: IndexScanStats,
        is_cancelled: impl Fn() -> bool,
        mut on_batch: impl FnMut(&[IndexedEntry], &IndexScanStats) -> Result<(), std::io::Error>,
        mut on_directory: impl FnMut(&IndexDirectoryScanCheckpoint) -> Result<(), std::io::Error>,
    ) -> Result<IndexReport, std::io::Error> {
        const STREAM_BATCH_SIZE: usize = 2_048;
        if plan.respect_project_ignores {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "resumable scanning requires project ignore handling to be disabled",
            ));
        }
        let rules = IndexPathRules::from_plan(&plan)?;
        let Some(root) = rules.roots.first().cloned() else {
            return Ok(IndexReport::default());
        };
        if rules.roots.len() != 1 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "resumable scanning accepts exactly one configured root",
            ));
        }

        let root_label = path_to_string(&root);
        let stage_name = plan.stage.as_ref().map(|stage| stage.name.clone());
        let mut report = IndexReport {
            scan_stats: completed_stats,
            ..IndexReport::default()
        };
        report.scan_events.push(ScanEvent::RootStarted {
            root: root_label.clone(),
            stage: stage_name.clone(),
        });
        if !root.is_dir() {
            let failure = IndexFailure {
                root: root_label.clone(),
                message: "index root is not a readable directory".to_owned(),
            };
            push_failure(&mut report, failure.clone());
            on_directory(&IndexDirectoryScanCheckpoint {
                root: root.clone(),
                directory: root.clone(),
                discovered_directories: Vec::new(),
                stats: IndexScanStats {
                    failures: 1,
                    ..IndexScanStats::default()
                },
                failure: Some(failure),
            })?;
            report.scan_events.push(ScanEvent::RootFinished {
                root: root_label,
                stage: stage_name,
                stats: report.scan_stats.clone(),
            });
            return Ok(report);
        }

        let mut queue = VecDeque::from(pending_directories);
        let mut queued_keys: HashSet<String> = queue.iter().map(normalize_path_key).collect();
        let mut pending_entries = Vec::with_capacity(STREAM_BATCH_SIZE);
        while let Some(directory) = queue.pop_front() {
            if is_cancelled() {
                break;
            }
            queued_keys.remove(&normalize_path_key(&directory));
            let mut directory_stats = IndexScanStats::default();
            let mut discovered_directories = Vec::new();
            let mut directory_failure = None;
            let read_dir = match fs::read_dir(&directory) {
                Ok(read_dir) => read_dir,
                Err(error) => {
                    directory_failure = Some(IndexFailure {
                        root: path_to_string(&directory),
                        message: error.to_string(),
                    });
                    report.scan_stats.failures = report.scan_stats.failures.saturating_add(1);
                    directory_stats.failures = 1;
                    let failure = directory_failure.clone().expect("directory failure set");
                    report.scan_events.push(ScanEvent::Failure(failure.clone()));
                    report.failures.push(failure);
                    on_directory(&IndexDirectoryScanCheckpoint {
                        root: root.clone(),
                        directory,
                        discovered_directories,
                        stats: directory_stats,
                        failure: directory_failure,
                    })?;
                    continue;
                }
            };

            for entry in read_dir {
                if is_cancelled() {
                    break;
                }
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(error) => {
                        directory_failure = Some(IndexFailure {
                            root: path_to_string(&directory),
                            message: error.to_string(),
                        });
                        break;
                    }
                };
                let path = entry.path();
                if rules.is_excluded(&path) {
                    report.scan_stats.skipped = report.scan_stats.skipped.saturating_add(1);
                    directory_stats.skipped = directory_stats.skipped.saturating_add(1);
                    continue;
                }
                let file_type = match entry.file_type() {
                    Ok(file_type) => file_type,
                    Err(error) => {
                        directory_failure = Some(IndexFailure {
                            root: path_to_string(&path),
                            message: error.to_string(),
                        });
                        break;
                    }
                };
                report.scan_stats.scanned = report.scan_stats.scanned.saturating_add(1);
                directory_stats.scanned = directory_stats.scanned.saturating_add(1);
                let kind = if is_application_path(&path) {
                    IndexedEntryKind::Application
                } else if file_type.is_dir() {
                    IndexedEntryKind::Directory
                } else if file_type.is_file() {
                    IndexedEntryKind::File
                } else {
                    report.scan_stats.skipped = report.scan_stats.skipped.saturating_add(1);
                    directory_stats.skipped = directory_stats.skipped.saturating_add(1);
                    continue;
                };
                pending_entries.push(IndexedEntry::from_path_metadata(&path, &root, kind));
                report.scan_stats.accepted = report.scan_stats.accepted.saturating_add(1);
                directory_stats.accepted = directory_stats.accepted.saturating_add(1);
                if file_type.is_dir() && !is_application_path(&path) {
                    discovered_directories.push(path);
                }
                if pending_entries.len() >= STREAM_BATCH_SIZE {
                    on_batch(&pending_entries, &report.scan_stats)?;
                    pending_entries.clear();
                }
            }

            if is_cancelled() {
                break;
            }
            if let Some(failure) = directory_failure.clone() {
                pending_entries.clear();
                directory_stats.failures = directory_stats.failures.saturating_add(1);
                report.scan_stats.failures = report.scan_stats.failures.saturating_add(1);
                report.scan_events.push(ScanEvent::Failure(failure.clone()));
                report.failures.push(failure);
                discovered_directories.clear();
            } else if !pending_entries.is_empty() {
                on_batch(&pending_entries, &report.scan_stats)?;
                pending_entries.clear();
            }

            discovered_directories.sort_by_key(|directory| normalize_path_key(directory));
            discovered_directories
                .dedup_by(|left, right| normalize_path_key(left) == normalize_path_key(right));
            on_directory(&IndexDirectoryScanCheckpoint {
                root: root.clone(),
                directory: directory.clone(),
                discovered_directories: discovered_directories.clone(),
                stats: directory_stats,
                failure: directory_failure,
            })?;
            for discovered in discovered_directories {
                if queued_keys.insert(normalize_path_key(&discovered)) {
                    queue.push_back(discovered);
                }
            }
        }

        report.scan_events.push(ScanEvent::RootFinished {
            root: root_label,
            stage: stage_name,
            stats: report.scan_stats.clone(),
        });
        Ok(report)
    }

    fn scan_cancellable_inner(
        &self,
        plan: IndexScanPlan,
        is_cancelled: impl Fn() -> bool,
        retain_entries: bool,
        mut on_batch: impl FnMut(&[IndexedEntry], &IndexScanStats) -> Result<(), std::io::Error>,
    ) -> Result<IndexReport, std::io::Error> {
        const STREAM_BATCH_SIZE: usize = 2_048;
        let rules = IndexPathRules::from_plan(&plan)?;
        let mut report = IndexReport::default();
        let mut pending_entries = Vec::with_capacity(STREAM_BATCH_SIZE);

        for root in rules.roots.clone() {
            if is_cancelled() {
                break;
            }
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
                if is_cancelled() {
                    break;
                }
                match entry {
                    Ok(entry) => {
                        if entry.depth() == 0 {
                            continue;
                        }
                        report.scan_stats.scanned += 1;
                        if let Some(indexed_entry) = indexed_entry_from_dir_entry(&entry, &root) {
                            pending_entries.push(indexed_entry);
                            report.scan_stats.accepted += 1;
                            if pending_entries.len() >= STREAM_BATCH_SIZE {
                                on_batch(&pending_entries, &report.scan_stats)?;
                                if retain_entries {
                                    report.entries.append(&mut pending_entries);
                                } else {
                                    pending_entries.clear();
                                }
                            }
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

            if !pending_entries.is_empty() {
                on_batch(&pending_entries, &report.scan_stats)?;
                if retain_entries {
                    report.entries.append(&mut pending_entries);
                } else {
                    pending_entries.clear();
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

impl FileSystemScanner for IgnoreScanner {
    fn scan(&self, plan: IndexScanPlan) -> Result<IndexReport, std::io::Error> {
        self.scan_cancellable(plan, || false)
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

fn indexed_entry_from_metadata(
    path: &Path,
    root: &Path,
    metadata: &fs::Metadata,
) -> Option<IndexedEntry> {
    let kind = if is_application_path(path) {
        IndexedEntryKind::Application
    } else if metadata.is_dir() {
        IndexedEntryKind::Directory
    } else if metadata.is_file() {
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

fn canonicalize_with_originals(paths: &[PathBuf]) -> Vec<ConfiguredRootCandidate> {
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    for (root_index, path) in paths.iter().enumerate() {
        for candidate in std::iter::once(path.clone()).chain(path.canonicalize().ok()) {
            if seen.insert(candidate.clone()) {
                candidates.push(ConfiguredRootCandidate {
                    path: candidate,
                    root_index,
                });
            }
        }
    }
    candidates
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
    #[cfg(unix)]
    use crate::core::index::IndexFailure;
    use crate::core::index::IndexedEntryKind;
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

    #[test]
    fn ignore_scanner_cancels_during_root_walk() {
        let root = temp_dir("ignore-cancel-root");
        for index in 0..128 {
            fs::write(root.join(format!("file-{index}.md")), "test").unwrap();
        }
        let checks = AtomicUsize::new(0);

        let report = IgnoreScanner::default()
            .scan_cancellable(
                IndexScanPlan {
                    include_roots: vec![root.clone()],
                    ..IndexScanPlan::default()
                },
                || checks.fetch_add(1, Ordering::Relaxed) >= 8,
            )
            .unwrap();

        assert!(report.scan_stats.scanned < 128);
        assert!(checks.load(Ordering::Relaxed) >= 9);

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
    fn index_path_rules_accept_native_events_reported_through_a_canonical_root_alias() {
        let temp = tempfile::tempdir().unwrap();
        let configured_root = temp.path().to_path_buf();
        let canonical_root = configured_root.canonicalize().unwrap();
        let rules = IndexPathRules::from_plan(&IndexScanPlan {
            include_roots: vec![configured_root.clone()],
            ..IndexScanPlan::default()
        })
        .unwrap();

        assert_eq!(
            rules.configured_root_for(&canonical_root.join("native-event.txt")),
            Some(configured_root.as_path())
        );
        assert!(!rules.is_excluded(&canonical_root.join("native-event.txt")));
    }

    #[test]
    fn configured_root_matching_accepts_filesystem_root_descendants() {
        let posix_root = PathBuf::from("/");
        let posix_rules = IndexPathRules::from_plan(&IndexScanPlan {
            include_roots: vec![posix_root.clone()],
            ..IndexScanPlan::default()
        })
        .unwrap();

        assert_eq!(
            posix_rules.configured_root_for_mode(Path::new("/tmp/a"), PathComparisonMode::Native,),
            Some(posix_root.as_path())
        );
        assert_eq!(
            posix_rules.configured_root_for_mode(Path::new("tmp/a"), PathComparisonMode::Native),
            None
        );

        let windows_root = PathBuf::from("C:/");
        let windows_rules = IndexPathRules::from_plan(&IndexScanPlan {
            include_roots: vec![windows_root.clone()],
            ..IndexScanPlan::default()
        })
        .unwrap();
        assert_eq!(
            windows_rules
                .configured_root_for_mode(Path::new(r"c:\tmp\a"), PathComparisonMode::Windows,),
            Some(windows_root.as_path())
        );
        assert_eq!(
            windows_rules
                .configured_root_for_mode(Path::new("D:/tmp/a"), PathComparisonMode::Windows,),
            None
        );
    }

    #[test]
    fn configured_root_matching_supports_explicit_windows_mode() {
        let windows_root = PathBuf::from(r"C:\Users\Frank");
        let nested_root = PathBuf::from(r"C:\Users\Frank\Projects");
        let rules = IndexPathRules::from_plan(&IndexScanPlan {
            include_roots: vec![windows_root.clone(), nested_root.clone()],
            ..IndexScanPlan::default()
        })
        .unwrap();

        assert_eq!(
            rules.configured_root_for_mode(
                Path::new("c:/users/frank/projects/quickfox/file.md"),
                crate::core::index_entry::PathComparisonMode::Windows,
            ),
            Some(nested_root.as_path())
        );
        assert_eq!(
            rules.configured_root_for_mode(
                Path::new("C:/Users/Frankish/file.md"),
                crate::core::index_entry::PathComparisonMode::Windows,
            ),
            None
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn configured_root_matching_preserves_native_posix_case() {
        let root = PathBuf::from("/Data");
        let rules = IndexPathRules::from_plan(&IndexScanPlan {
            include_roots: vec![root.clone()],
            ..IndexScanPlan::default()
        })
        .unwrap();

        assert_eq!(
            rules.configured_root_for(Path::new("/Data/file.md")),
            Some(root.as_path())
        );
        assert_eq!(rules.configured_root_for(Path::new("/data/file.md")), None);
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

    #[test]
    fn streaming_scan_flushes_entries_without_retaining_a_second_snapshot() {
        let root = temp_dir("streaming-scan");
        fs::write(root.join("a.md"), "a").unwrap();
        fs::write(root.join("b.md"), "b").unwrap();
        let mut streamed_paths = Vec::new();

        let report = IgnoreScanner::default()
            .scan_cancellable_streaming(
                IndexScanPlan {
                    include_roots: vec![root.clone()],
                    respect_project_ignores: false,
                    ..IndexScanPlan::default()
                },
                || false,
                |entries, _| {
                    streamed_paths.extend(entries.iter().map(|entry| entry.path.clone()));
                    Ok(())
                },
            )
            .unwrap();

        assert!(report.entries.is_empty());
        assert_eq!(report.scan_stats.accepted, 2);
        assert_eq!(streamed_paths.len(), 2);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resumable_scan_continues_from_the_persisted_directory_frontier() {
        let root = temp_dir("resumable-directory-frontier");
        let child = root.join("large-folder");
        fs::create_dir_all(&child).unwrap();
        fs::write(root.join("top.md"), "top").unwrap();
        fs::write(child.join("nested.md"), "nested").unwrap();
        let cancelled = std::cell::Cell::new(false);
        let mut first_paths = Vec::new();
        let mut child_frontier = Vec::new();
        let mut completed_stats = IndexScanStats::default();

        IgnoreScanner::default()
            .scan_resumable_cancellable_streaming(
                IndexScanPlan {
                    include_roots: vec![root.clone()],
                    respect_project_ignores: false,
                    ..IndexScanPlan::default()
                },
                vec![root.clone()],
                IndexScanStats::default(),
                || cancelled.get(),
                |entries, _| {
                    first_paths.extend(entries.iter().map(|entry| entry.path.clone()));
                    Ok(())
                },
                |checkpoint| {
                    if checkpoint.directory == root {
                        child_frontier = checkpoint.discovered_directories.clone();
                        completed_stats = checkpoint.stats.clone();
                        cancelled.set(true);
                    }
                    Ok(())
                },
            )
            .unwrap();

        assert!(first_paths.iter().any(|path| path.ends_with("top.md")));
        assert_eq!(child_frontier, vec![child.clone()]);
        let mut resumed_paths = Vec::new();
        let report = IgnoreScanner::default()
            .scan_resumable_cancellable_streaming(
                IndexScanPlan {
                    include_roots: vec![root.clone()],
                    respect_project_ignores: false,
                    ..IndexScanPlan::default()
                },
                child_frontier,
                completed_stats,
                || false,
                |entries, _| {
                    resumed_paths.extend(entries.iter().map(|entry| entry.path.clone()));
                    Ok(())
                },
                |_| Ok(()),
            )
            .unwrap();

        assert_eq!(
            resumed_paths,
            vec![child.join("nested.md").to_string_lossy()]
        );
        assert_eq!(report.scan_stats.accepted, 3);
        assert!(report.entries.is_empty());

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
