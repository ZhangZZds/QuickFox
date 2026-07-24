//! Targeted path scanning and directory-manifest calibration.

use crate::core::index_entry::{
    normalize_path_key, normalize_path_text_key, IndexFailure, IndexedEntry, IndexedEntryKind,
};
use crate::core::index_scanner::{IgnoreScanner, IndexPathRules};
use crate::core::index_update_coordinator::CoordinatorBatch;
use serde::{Deserialize, Serialize};
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectoryFingerprint {
    pub path: String,
    pub parent: Option<String>,
    pub root: String,
    pub modified_ms: Option<i64>,
}

#[derive(Debug, Default)]
pub struct TargetedScanResult {
    pub upserts: Vec<IndexedEntry>,
    pub removals: Vec<PathBuf>,
    pub manifest_upserts: Vec<DirectoryFingerprint>,
    pub manifest_removals: Vec<PathBuf>,
    pub failures: Vec<IndexFailure>,
}

#[derive(Debug)]
pub enum TargetedScanError {
    Cancelled,
    Io(io::Error),
}

impl std::fmt::Display for TargetedScanError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => formatter.write_str("targeted index scan was cancelled"),
            Self::Io(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for TargetedScanError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Cancelled => None,
            Self::Io(error) => Some(error),
        }
    }
}

impl From<io::Error> for TargetedScanError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub type TargetedScanResultValue<T> = Result<T, TargetedScanError>;

pub trait DirectoryManifestReader {
    fn directories_for_root(&self, root: &Path) -> Result<Vec<DirectoryFingerprint>, String>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnownIndexedChild {
    pub path: String,
    pub kind: IndexedEntryKind,
    pub filesystem_kind: FileSystemEntryKind,
    pub modified_ms: Option<i64>,
    pub size_bytes: Option<u64>,
}

pub trait KnownDirectoryEntriesReader {
    fn entries_for_directory(
        &self,
        root: &Path,
        directory: &Path,
    ) -> Result<Vec<KnownIndexedChild>, String>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileSystemEntryKind {
    File,
    Directory,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileSystemMetadata {
    pub kind: FileSystemEntryKind,
    pub modified_ms: Option<i64>,
    pub size_bytes: Option<u64>,
}

impl FileSystemMetadata {
    pub const fn directory(modified_ms: Option<i64>) -> Self {
        Self {
            kind: FileSystemEntryKind::Directory,
            modified_ms,
            size_bytes: None,
        }
    }

    pub const fn file(modified_ms: Option<i64>, size_bytes: Option<u64>) -> Self {
        Self {
            kind: FileSystemEntryKind::File,
            modified_ms,
            size_bytes,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSystemEntry {
    pub path: PathBuf,
    pub metadata: FileSystemMetadata,
}

impl FileSystemEntry {
    pub fn directory(path: impl Into<PathBuf>, modified_ms: Option<i64>) -> Self {
        Self {
            path: path.into(),
            metadata: FileSystemMetadata::directory(modified_ms),
        }
    }

    pub fn file(
        path: impl Into<PathBuf>,
        modified_ms: Option<i64>,
        size_bytes: Option<u64>,
    ) -> Self {
        Self {
            path: path.into(),
            metadata: FileSystemMetadata::file(modified_ms, size_bytes),
        }
    }
}

pub trait FileSystemProbe {
    fn metadata(&self, path: &Path) -> io::Result<Option<FileSystemMetadata>>;
    fn read_dir(&self, path: &Path) -> io::Result<Vec<FileSystemEntry>>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct StdFileSystemProbe;

impl FileSystemProbe for StdFileSystemProbe {
    fn metadata(&self, path: &Path) -> io::Result<Option<FileSystemMetadata>> {
        match fs::metadata(path) {
            Ok(metadata) => Ok(Some(metadata_from_std(&metadata))),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<FileSystemEntry>> {
        let mut entries = Vec::new();
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            entries.push(FileSystemEntry {
                path: entry.path(),
                metadata: metadata_from_std(&entry.metadata()?),
            });
        }
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(entries)
    }
}

#[derive(Debug, Default)]
pub struct ManifestCalibrationResult {
    pub changed_directories: Vec<PathBuf>,
    pub new_directories: Vec<PathBuf>,
    pub missing_directories: Vec<PathBuf>,
    pub failures: Vec<IndexFailure>,
    pub entry_upserts: Vec<FileSystemEntry>,
    pub entry_removals: Vec<PathBuf>,
    current_metadata: BTreeMap<PathBuf, FileSystemMetadata>,
}

#[derive(Debug, Clone)]
pub struct TargetedIndexScanner {
    rules: IndexPathRules,
    scanner: IgnoreScanner,
}

impl TargetedIndexScanner {
    pub fn new(rules: IndexPathRules) -> Self {
        Self {
            rules,
            scanner: IgnoreScanner::default(),
        }
    }

    pub fn with_scanner(rules: IndexPathRules, scanner: IgnoreScanner) -> Self {
        Self { rules, scanner }
    }

    pub fn rules(&self) -> &IndexPathRules {
        &self.rules
    }

    pub fn scan_changed_paths(
        &self,
        paths: &[PathBuf],
    ) -> TargetedScanResultValue<TargetedScanResult> {
        self.scan_changed_paths_cancellable(paths, || false)
    }

    pub fn scan_changed_paths_cancellable(
        &self,
        paths: &[PathBuf],
        is_cancelled: impl Fn() -> bool,
    ) -> TargetedScanResultValue<TargetedScanResult> {
        let mut result = TargetedScanResult::default();
        for path in sorted_unique_paths(paths) {
            if is_cancelled() {
                return Err(TargetedScanError::Cancelled);
            }
            let Some(root) = self.rules.configured_root_for(&path) else {
                continue;
            };
            if self.rules.is_excluded(&path) {
                continue;
            }
            let cancelled_during_walk = Cell::new(false);
            let report = self
                .scanner
                .scan_subtree_cancellable(&path, root, &self.rules, || {
                    let cancelled = is_cancelled();
                    cancelled_during_walk.set(cancelled_during_walk.get() || cancelled);
                    cancelled
                })?;
            if cancelled_during_walk.get() {
                return Err(TargetedScanError::Cancelled);
            }
            result.failures.extend(report.failures);
            for entry in report.entries {
                if entry.kind == IndexedEntryKind::Directory || Path::new(&entry.path).is_dir() {
                    result.manifest_upserts.push(directory_fingerprint(&entry));
                }
                result.upserts.push(entry);
            }
        }
        if is_cancelled() {
            return Err(TargetedScanError::Cancelled);
        }
        sort_result(&mut result);
        Ok(result)
    }

    pub fn scan_removed_paths(&self, paths: &[PathBuf]) -> TargetedScanResult {
        let removals: Vec<_> = sorted_unique_paths(paths)
            .into_iter()
            .filter(|path| self.rules.configured_root_for(path).is_some())
            .filter(|path| !self.rules.is_forced_or_user_excluded(path))
            .collect();
        TargetedScanResult {
            manifest_removals: removals.clone(),
            removals,
            ..TargetedScanResult::default()
        }
    }

    pub fn scan_rename(
        &self,
        old_path: &Path,
        new_path: &Path,
    ) -> TargetedScanResultValue<TargetedScanResult> {
        let mut result = self.scan_removed_paths(&[old_path.to_path_buf()]);
        merge_result(
            &mut result,
            self.scan_changed_paths(&[new_path.to_path_buf()])?,
        );
        sort_result(&mut result);
        Ok(result)
    }

    pub fn scan_batch(
        &self,
        batch: CoordinatorBatch,
    ) -> TargetedScanResultValue<TargetedScanResult> {
        self.scan_batch_cancellable(batch, || false)
    }

    pub fn scan_batch_cancellable(
        &self,
        batch: CoordinatorBatch,
        is_cancelled: impl Fn() -> bool,
    ) -> TargetedScanResultValue<TargetedScanResult> {
        if is_cancelled() {
            return Err(TargetedScanError::Cancelled);
        }
        let mut result = self.scan_removed_paths(&batch.removed_paths);
        merge_result(
            &mut result,
            self.scan_changed_paths_cancellable(&batch.changed_paths, &is_cancelled)?,
        );
        if is_cancelled() {
            return Err(TargetedScanError::Cancelled);
        }
        sort_result(&mut result);
        Ok(result)
    }

    pub fn calibrate_root(
        &self,
        probe: &impl FileSystemProbe,
        manifest: &impl DirectoryManifestReader,
        known_entries: &impl KnownDirectoryEntriesReader,
        root: &Path,
    ) -> TargetedScanResultValue<TargetedScanResult> {
        self.calibrate_root_cancellable(probe, manifest, known_entries, root, || false)
    }

    pub fn calibrate_root_cancellable(
        &self,
        probe: &impl FileSystemProbe,
        manifest: &impl DirectoryManifestReader,
        known_entries: &impl KnownDirectoryEntriesReader,
        root: &Path,
        is_cancelled: impl Fn() -> bool,
    ) -> TargetedScanResultValue<TargetedScanResult> {
        let calibration =
            calibrate_manifest_cancellable(probe, manifest, known_entries, root, &is_cancelled)?;
        let mut result = TargetedScanResult {
            failures: calibration.failures.clone(),
            removals: calibration
                .missing_directories
                .iter()
                .cloned()
                .chain(calibration.entry_removals.iter().cloned())
                .collect(),
            manifest_removals: calibration.missing_directories.clone(),
            ..TargetedScanResult::default()
        };

        for directory in &calibration.changed_directories {
            if is_cancelled() {
                return Err(TargetedScanError::Cancelled);
            }
            if let Some(metadata) = calibration.current_metadata.get(directory) {
                result.manifest_upserts.push(DirectoryFingerprint {
                    path: path_to_string(directory),
                    parent: (directory != root)
                        .then(|| directory.parent().map(path_to_string))
                        .flatten(),
                    root: path_to_string(root),
                    modified_ms: metadata.modified_ms,
                });
            }
        }

        let changed_files: Vec<_> = calibration
            .entry_upserts
            .iter()
            .map(|entry| entry.path.clone())
            .collect();
        merge_result(
            &mut result,
            self.scan_changed_paths_cancellable(&changed_files, &is_cancelled)?,
        );
        merge_result(
            &mut result,
            self.scan_changed_paths_cancellable(&calibration.new_directories, &is_cancelled)?,
        );
        sort_result(&mut result);
        Ok(result)
    }
}

pub fn calibrate_manifest(
    probe: &impl FileSystemProbe,
    manifest: &impl DirectoryManifestReader,
    known_entries: &impl KnownDirectoryEntriesReader,
    root: &Path,
) -> TargetedScanResultValue<ManifestCalibrationResult> {
    calibrate_manifest_cancellable(probe, manifest, known_entries, root, &|| false)
}

pub fn calibrate_manifest_cancellable(
    probe: &impl FileSystemProbe,
    manifest: &impl DirectoryManifestReader,
    known_entries: &impl KnownDirectoryEntriesReader,
    root: &Path,
    is_cancelled: &impl Fn() -> bool,
) -> TargetedScanResultValue<ManifestCalibrationResult> {
    let mut known = manifest
        .directories_for_root(root)
        .map_err(io::Error::other)?;
    let root_key = normalize_path_key(root);
    known.retain(|fingerprint| normalize_path_text_key(&fingerprint.root) == root_key);
    known.sort_by(|left, right| {
        normalize_path_text_key(&left.path)
            .cmp(&normalize_path_text_key(&right.path))
            .then_with(|| left.path.cmp(&right.path))
    });
    known.dedup_by(|left, right| {
        normalize_path_text_key(&left.path) == normalize_path_text_key(&right.path)
    });
    let known_paths: BTreeSet<_> = known
        .iter()
        .map(|fingerprint| normalize_path_text_key(&fingerprint.path))
        .collect();
    let mut result = ManifestCalibrationResult::default();

    for fingerprint in &known {
        if is_cancelled() {
            return Err(TargetedScanError::Cancelled);
        }
        let path = PathBuf::from(&fingerprint.path);
        match probe.metadata(&path) {
            Ok(None) => result.missing_directories.push(path),
            Ok(Some(metadata)) if metadata.kind != FileSystemEntryKind::Directory => {
                result.missing_directories.push(path);
            }
            Ok(Some(metadata)) => {
                result.current_metadata.insert(path.clone(), metadata);
                if metadata.modified_ms == fingerprint.modified_ms {
                    continue;
                }
                match probe.read_dir(&path) {
                    Ok(mut entries) => {
                        entries.sort_by(|left, right| {
                            normalize_path_key(&left.path)
                                .cmp(&normalize_path_key(&right.path))
                                .then_with(|| left.path.cmp(&right.path))
                        });
                        entries.dedup_by(|left, right| {
                            normalize_path_key(&left.path) == normalize_path_key(&right.path)
                        });
                        let known_children = match known_entries.entries_for_directory(root, &path)
                        {
                            Ok(entries) => entries,
                            Err(error) => {
                                result.failures.push(IndexFailure {
                                    root: path_to_string(&path),
                                    message: error,
                                });
                                continue;
                            }
                        };
                        diff_direct_entries(
                            &entries,
                            &known_children,
                            &mut result.entry_upserts,
                            &mut result.entry_removals,
                        );
                        for entry in &entries {
                            if entry.metadata.kind == FileSystemEntryKind::Directory
                                && !known_paths.contains(&normalize_path_key(&entry.path))
                            {
                                result.new_directories.push(entry.path.clone());
                            }
                        }
                        result.changed_directories.push(path);
                    }
                    Err(error) => result.failures.push(IndexFailure {
                        root: path_to_string(&path),
                        message: error.to_string(),
                    }),
                }
            }
            Err(error) => result.failures.push(IndexFailure {
                root: path_to_string(&path),
                message: error.to_string(),
            }),
        }
    }

    if is_cancelled() {
        return Err(TargetedScanError::Cancelled);
    }
    sort_calibration_result(&mut result);
    Ok(result)
}

fn metadata_from_std(metadata: &fs::Metadata) -> FileSystemMetadata {
    let kind = if metadata.is_dir() {
        FileSystemEntryKind::Directory
    } else if metadata.is_file() {
        FileSystemEntryKind::File
    } else {
        FileSystemEntryKind::Other
    };
    let modified_ms = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i64);
    FileSystemMetadata {
        kind,
        modified_ms,
        size_bytes: metadata.is_file().then_some(metadata.len()),
    }
}

fn diff_direct_entries(
    current: &[FileSystemEntry],
    known: &[KnownIndexedChild],
    upserts: &mut Vec<FileSystemEntry>,
    removals: &mut Vec<PathBuf>,
) {
    let known_by_path: BTreeMap<_, _> = known
        .iter()
        .map(|entry| (normalize_path_text_key(&entry.path), entry))
        .collect();
    let current_by_path: BTreeMap<_, _> = current
        .iter()
        .map(|entry| (normalize_path_key(&entry.path), entry))
        .collect();

    for entry in current {
        if entry.metadata.kind != FileSystemEntryKind::File {
            continue;
        }
        let unchanged = known_by_path
            .get(&normalize_path_key(&entry.path))
            .is_some_and(|known| {
                known.filesystem_kind == FileSystemEntryKind::File
                    && known.modified_ms == entry.metadata.modified_ms
                    && known.size_bytes == entry.metadata.size_bytes
            });
        if !unchanged {
            upserts.push(entry.clone());
        }
    }
    for entry in known {
        let current_shape = current_by_path
            .get(&normalize_path_text_key(&entry.path))
            .map(|entry| entry.metadata.kind);
        if entry.filesystem_kind != FileSystemEntryKind::Directory
            && current_shape != Some(FileSystemEntryKind::File)
        {
            removals.push(PathBuf::from(&entry.path));
        }
    }
}

fn directory_fingerprint(entry: &IndexedEntry) -> DirectoryFingerprint {
    DirectoryFingerprint {
        path: entry.path.clone(),
        parent: (entry.path != entry.root)
            .then(|| entry.parent.clone())
            .filter(|parent| !parent.is_empty()),
        root: entry.root.clone(),
        modified_ms: entry.modified_ms,
    }
}

fn merge_result(target: &mut TargetedScanResult, source: TargetedScanResult) {
    target.upserts.extend(source.upserts);
    target.removals.extend(source.removals);
    target.manifest_upserts.extend(source.manifest_upserts);
    target.manifest_removals.extend(source.manifest_removals);
    target.failures.extend(source.failures);
}

fn sort_result(result: &mut TargetedScanResult) {
    result.upserts.sort_by(|left, right| {
        normalize_path_text_key(&left.path)
            .cmp(&normalize_path_text_key(&right.path))
            .then_with(|| left.path.cmp(&right.path))
    });
    result.upserts.dedup_by(|left, right| {
        normalize_path_text_key(&left.path) == normalize_path_text_key(&right.path)
    });
    result.removals.sort_by(|left, right| {
        normalize_path_key(left)
            .cmp(&normalize_path_key(right))
            .then_with(|| left.cmp(right))
    });
    result
        .removals
        .dedup_by(|left, right| normalize_path_key(left) == normalize_path_key(right));
    result.manifest_upserts.sort_by(|left, right| {
        normalize_path_text_key(&left.path)
            .cmp(&normalize_path_text_key(&right.path))
            .then_with(|| left.path.cmp(&right.path))
    });
    result.manifest_upserts.dedup_by(|left, right| {
        normalize_path_text_key(&left.path) == normalize_path_text_key(&right.path)
    });
    result.manifest_removals.sort_by(|left, right| {
        normalize_path_key(left)
            .cmp(&normalize_path_key(right))
            .then_with(|| left.cmp(right))
    });
    result
        .manifest_removals
        .dedup_by(|left, right| normalize_path_key(left) == normalize_path_key(right));
    result.failures.sort_by(|left, right| {
        normalize_path_text_key(&left.root)
            .cmp(&normalize_path_text_key(&right.root))
            .then_with(|| left.root.cmp(&right.root))
            .then_with(|| left.message.cmp(&right.message))
    });
    result.failures.dedup_by(|left, right| {
        normalize_path_text_key(&left.root) == normalize_path_text_key(&right.root)
            && left.message == right.message
    });
}

fn sort_calibration_result(result: &mut ManifestCalibrationResult) {
    sort_dedup_paths(&mut result.changed_directories);
    sort_dedup_paths(&mut result.new_directories);
    sort_dedup_paths(&mut result.missing_directories);
    let mut subtree_roots: Vec<PathBuf> = Vec::new();
    for path in std::mem::take(&mut result.missing_directories) {
        if !subtree_roots.iter().any(|ancestor| {
            normalized_path_is_same_or_descendant(
                &normalize_path_key(ancestor),
                &normalize_path_key(&path),
            )
        }) {
            subtree_roots.push(path);
        }
    }
    result.missing_directories = subtree_roots;
    result.failures.sort_by(|left, right| {
        normalize_path_text_key(&left.root)
            .cmp(&normalize_path_text_key(&right.root))
            .then_with(|| left.root.cmp(&right.root))
            .then_with(|| left.message.cmp(&right.message))
    });
    result.failures.dedup_by(|left, right| {
        normalize_path_text_key(&left.root) == normalize_path_text_key(&right.root)
            && left.message == right.message
    });
    result.entry_upserts.sort_by(|left, right| {
        normalize_path_key(&left.path)
            .cmp(&normalize_path_key(&right.path))
            .then_with(|| left.path.cmp(&right.path))
    });
    result
        .entry_upserts
        .dedup_by(|left, right| normalize_path_key(&left.path) == normalize_path_key(&right.path));
    sort_dedup_paths(&mut result.entry_removals);
}

fn sorted_unique_paths(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut paths = paths.to_vec();
    sort_dedup_paths(&mut paths);
    paths
}

fn sort_dedup_paths(paths: &mut Vec<PathBuf>) {
    paths.sort_by(|left, right| {
        normalize_path_key(left)
            .cmp(&normalize_path_key(right))
            .then_with(|| left.cmp(right))
    });
    paths.dedup_by(|left, right| normalize_path_key(left) == normalize_path_key(right));
}

fn normalized_path_is_same_or_descendant(root: &str, candidate: &str) -> bool {
    candidate == root
        || candidate
            .strip_prefix(root)
            .is_some_and(|remainder| remainder.starts_with('/'))
}

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::index_entry::IndexedEntryKind;
    use crate::core::index_scanner::{IgnorePathProbe, IndexPathRules, IndexScanPlan};
    use std::cell::{Cell, RefCell};
    use std::collections::BTreeMap;
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tempfile::TempDir;

    fn scan_rules(root: &Path) -> IndexPathRules {
        IndexPathRules::from_plan(&IndexScanPlan {
            include_roots: vec![root.to_path_buf()],
            ..IndexScanPlan::default()
        })
        .unwrap()
    }

    #[test]
    fn scan_changed_file_preserves_configured_root_metadata() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let file = root.join("docs/readme.md");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "hello").unwrap();
        let scanner = TargetedIndexScanner::new(scan_rules(root));

        let delta = scanner
            .scan_changed_paths(std::slice::from_ref(&file))
            .unwrap();

        assert_eq!(delta.upserts.len(), 1);
        assert_eq!(delta.upserts[0].root, root.to_string_lossy());
        assert_eq!(delta.upserts[0].path, file.to_string_lossy());
        assert_eq!(delta.upserts[0].kind, IndexedEntryKind::File);
    }

    #[test]
    fn removed_path_becomes_a_tombstone_without_existing_on_disk() {
        let temp = TempDir::new().unwrap();
        let removed = temp.path().join("gone.md");
        let scanner = TargetedIndexScanner::new(scan_rules(temp.path()));

        let delta = scanner.scan_removed_paths(std::slice::from_ref(&removed));

        assert_eq!(delta.removals, vec![removed.clone()]);
        assert_eq!(delta.manifest_removals, vec![removed]);
        assert!(delta.failures.is_empty());
    }

    #[test]
    fn removed_excluded_path_is_filtered_without_reading_the_file_system() {
        let temp = TempDir::new().unwrap();
        let excluded = temp.path().join("private");
        fs::create_dir_all(&excluded).unwrap();
        let rules = IndexPathRules::from_plan(&IndexScanPlan {
            include_roots: vec![temp.path().to_path_buf()],
            exclude_dirs: vec![excluded.clone()],
            ..IndexScanPlan::default()
        })
        .unwrap();
        fs::remove_dir_all(&excluded).unwrap();
        let scanner = TargetedIndexScanner::new(rules);

        let delta = scanner.scan_removed_paths(&[excluded.join("gone.md")]);

        assert!(delta.removals.is_empty());
    }

    #[test]
    fn rename_removes_old_path_and_scans_new_path() {
        let temp = TempDir::new().unwrap();
        let old = temp.path().join("old.md");
        let new = temp.path().join("new.md");
        fs::write(&new, "new").unwrap();
        let scanner = TargetedIndexScanner::new(scan_rules(temp.path()));

        let delta = scanner.scan_rename(&old, &new).unwrap();

        assert_eq!(delta.removals, vec![old]);
        assert_eq!(delta.upserts.len(), 1);
        assert_eq!(delta.upserts[0].path, new.to_string_lossy());
    }

    #[test]
    fn scan_new_directory_indexes_only_that_subtree_and_builds_manifest() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join("new/nested")).unwrap();
        fs::write(root.join("new/nested/file.md"), "hello").unwrap();
        fs::write(root.join("outside.md"), "outside").unwrap();
        let scanner = TargetedIndexScanner::new(scan_rules(root));

        let delta = scanner.scan_changed_paths(&[root.join("new")]).unwrap();

        assert!(delta.upserts.iter().any(|entry| entry.name == "file.md"));
        assert!(!delta.upserts.iter().any(|entry| entry.name == "outside.md"));
        assert!(delta
            .manifest_upserts
            .iter()
            .any(|fingerprint| fingerprint.path.ends_with("new/nested")));
    }

    #[test]
    fn directory_shaped_application_is_recorded_in_manifest() {
        let temp = TempDir::new().unwrap();
        let app = temp.path().join("Tool.app");
        fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        fs::write(app.join("Contents/MacOS/tool"), "binary").unwrap();
        let scanner = TargetedIndexScanner::new(scan_rules(temp.path()));

        let result = scanner
            .scan_changed_paths(std::slice::from_ref(&app))
            .unwrap();

        assert!(result
            .upserts
            .iter()
            .any(|entry| entry.path == app.to_string_lossy()
                && entry.kind == IndexedEntryKind::Application));
        assert!(result
            .manifest_upserts
            .iter()
            .any(|fingerprint| fingerprint.path == app.to_string_lossy()));
    }

    #[test]
    fn configured_root_manifest_fingerprint_has_no_parent() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("file.md"), "file").unwrap();
        let scanner = TargetedIndexScanner::new(scan_rules(temp.path()));

        let delta = scanner
            .scan_changed_paths(&[temp.path().to_path_buf()])
            .unwrap();
        let root_fingerprint = delta
            .manifest_upserts
            .iter()
            .find(|fingerprint| fingerprint.path == temp.path().to_string_lossy())
            .unwrap();

        assert_eq!(root_fingerprint.parent, None);
    }

    #[test]
    fn cancellation_inside_targeted_subtree_returns_error_without_partial_delta() {
        let temp = TempDir::new().unwrap();
        for index in 0..32 {
            fs::write(temp.path().join(format!("file-{index}.md")), "file").unwrap();
        }
        let scanner = TargetedIndexScanner::new(scan_rules(temp.path()));
        let checks = Cell::new(0);

        let result = scanner.scan_changed_paths_cancellable(&[temp.path().to_path_buf()], || {
            let current = checks.get();
            checks.set(current + 1);
            current >= 2
        });

        assert!(matches!(result, Err(TargetedScanError::Cancelled)));
        assert!(checks.get() >= 3);
    }

    #[test]
    fn cancelled_batch_does_not_return_a_committable_partial_result() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("a.md"), "a").unwrap();
        fs::write(temp.path().join("b.md"), "b").unwrap();
        let scanner = TargetedIndexScanner::new(scan_rules(temp.path()));
        let checks = Cell::new(0);
        let batch = CoordinatorBatch {
            changed_paths: vec![temp.path().join("a.md"), temp.path().join("b.md")],
            ..CoordinatorBatch::default()
        };

        let result = scanner.scan_batch_cancellable(batch, || {
            let current = checks.get();
            checks.set(current + 1);
            current >= 3
        });

        assert!(matches!(result, Err(TargetedScanError::Cancelled)));
    }

    #[test]
    fn non_cancelled_batch_returns_complete_delta() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("a.md"), "a").unwrap();
        fs::write(temp.path().join("b.md"), "b").unwrap();
        let scanner = TargetedIndexScanner::new(scan_rules(temp.path()));
        let batch = CoordinatorBatch {
            changed_paths: vec![temp.path().join("a.md"), temp.path().join("b.md")],
            ..CoordinatorBatch::default()
        };

        let result = scanner.scan_batch_cancellable(batch, || false).unwrap();

        assert_eq!(result.upserts.len(), 2);
    }

    #[test]
    fn targeted_result_deduplicates_windows_path_variants_by_shared_path_key() {
        use crate::core::index_entry::normalize_path_key;

        let mut result = TargetedScanResult {
            upserts: vec![
                IndexedEntry::legacy(r"c:/root/file.md", "file.md", IndexedEntryKind::File),
                IndexedEntry::legacy(r"C:\Root\File.md", "File.md", IndexedEntryKind::File),
            ],
            removals: vec![
                PathBuf::from(r"c:/root/gone.md"),
                PathBuf::from(r"C:\Root\Gone.md"),
            ],
            manifest_upserts: vec![
                DirectoryFingerprint {
                    path: r"c:/root/docs".to_owned(),
                    parent: Some(r"c:/root".to_owned()),
                    root: r"c:/root".to_owned(),
                    modified_ms: Some(1),
                },
                DirectoryFingerprint {
                    path: r"C:\Root\Docs".to_owned(),
                    parent: Some(r"C:\Root".to_owned()),
                    root: r"C:\Root".to_owned(),
                    modified_ms: Some(1),
                },
            ],
            manifest_removals: vec![PathBuf::from(r"c:/root/old"), PathBuf::from(r"C:\Root\Old")],
            ..TargetedScanResult::default()
        };

        sort_result(&mut result);

        assert_eq!(normalize_path_key(r"C:\Root\File.md"), "c:/root/file.md");
        assert_eq!(result.upserts.len(), 1);
        assert_eq!(result.removals.len(), 1);
        assert_eq!(result.manifest_upserts.len(), 1);
        assert_eq!(result.manifest_removals.len(), 1);
        assert_eq!(result.upserts[0].path, r"C:\Root\File.md");
    }

    #[test]
    fn subtree_scan_reuses_project_ignore_for_targeted_directory() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join("new")).unwrap();
        fs::write(root.join(".gitignore"), "new/ignored.md\n").unwrap();
        fs::write(root.join("new/ignored.md"), "ignored").unwrap();
        fs::write(root.join("new/keep.md"), "keep").unwrap();
        let scanner = TargetedIndexScanner::new(scan_rules(root));

        let delta = scanner.scan_changed_paths(&[root.join("new")]).unwrap();

        assert!(delta.upserts.iter().any(|entry| entry.name == "keep.md"));
        assert!(!delta.upserts.iter().any(|entry| entry.name == "ignored.md"));
    }

    #[test]
    fn targeted_single_file_respects_parent_project_ignore() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::write(root.join(".gitignore"), "docs/ignored.md\n").unwrap();
        let ignored = root.join("docs/ignored.md");
        fs::write(&ignored, "ignored").unwrap();
        let scanner = TargetedIndexScanner::new(scan_rules(root));

        let delta = scanner.scan_changed_paths(&[ignored]).unwrap();

        assert!(delta.upserts.is_empty());
    }

    fn assert_targeted_file_matches_full_scan(root: &Path, file: &Path, expected: bool) {
        use crate::core::index_scanner::FileSystemScanner;

        let plan = IndexScanPlan {
            include_roots: vec![root.to_path_buf()],
            ..IndexScanPlan::default()
        };
        let full = IgnoreScanner::default().scan(plan.clone()).unwrap();
        let targeted = TargetedIndexScanner::new(IndexPathRules::from_plan(&plan).unwrap())
            .scan_changed_paths(&[file.to_path_buf()])
            .unwrap();
        let full_contains = full
            .entries
            .iter()
            .any(|entry| entry.path == file.to_string_lossy());
        let targeted_contains = targeted
            .upserts
            .iter()
            .any(|entry| entry.path == file.to_string_lossy());

        assert_eq!(full_contains, expected);
        assert_eq!(targeted_contains, full_contains);
    }

    #[test]
    fn targeted_file_matches_full_scan_for_ignored_parent_directory() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::write(root.join(".gitignore"), "docs/\n").unwrap();
        let file = root.join("docs/readme.md");
        fs::write(&file, "ignored").unwrap();

        assert_targeted_file_matches_full_scan(root, &file, false);
    }

    #[test]
    fn targeted_directory_matches_full_scan_when_target_itself_is_ignored() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        let directory = root.join("ignored");
        fs::create_dir_all(&directory).unwrap();
        fs::write(root.join(".gitignore"), "ignored/\n").unwrap();
        fs::write(directory.join("file.md"), "ignored").unwrap();

        assert_targeted_file_matches_full_scan(root, &directory, false);
    }

    #[test]
    fn targeted_file_matches_full_scan_for_nested_gitignore_negation() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::write(root.join(".gitignore"), "docs/*.md\n").unwrap();
        fs::write(root.join("docs/.gitignore"), "!keep.md\n").unwrap();
        let file = root.join("docs/keep.md");
        fs::write(&file, "included").unwrap();

        assert_targeted_file_matches_full_scan(root, &file, true);
    }

    #[test]
    fn targeted_file_matches_full_scan_for_dot_ignore() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        fs::write(root.join(".ignore"), "ignored.md\n").unwrap();
        let file = root.join("ignored.md");
        fs::write(&file, "ignored").unwrap();

        assert_targeted_file_matches_full_scan(root, &file, false);
    }

    #[test]
    fn targeted_file_matches_full_scan_for_git_info_exclude() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join(".git/info")).unwrap();
        fs::write(root.join(".git/info/exclude"), "local.md\n").unwrap();
        let file = root.join("local.md");
        fs::write(&file, "ignored").unwrap();

        assert_targeted_file_matches_full_scan(root, &file, false);
    }

    #[test]
    fn targeted_file_matches_repo_exclude_above_configured_root() {
        let temp = TempDir::new().unwrap();
        let repo = temp.path();
        fs::create_dir_all(repo.join(".git/info")).unwrap();
        fs::write(repo.join(".git/info/exclude"), "local.md\n").unwrap();
        let root = repo.join("workspace");
        fs::create_dir_all(&root).unwrap();
        let file = root.join("local.md");
        fs::write(&file, "ignored").unwrap();

        assert_targeted_file_matches_full_scan(&root, &file, false);
    }

    #[test]
    fn targeted_file_matches_gitignore_above_configured_root() {
        let temp = TempDir::new().unwrap();
        let repo = temp.path();
        fs::create_dir(repo.join(".git")).unwrap();
        fs::write(repo.join(".gitignore"), "workspace/ignored.md\n").unwrap();
        let root = repo.join("workspace");
        fs::create_dir_all(&root).unwrap();
        let file = root.join("ignored.md");
        fs::write(&file, "ignored").unwrap();

        assert_targeted_file_matches_full_scan(&root, &file, false);
    }

    #[test]
    fn targeted_file_matches_dot_ignore_above_configured_root() {
        let temp = TempDir::new().unwrap();
        let repo = temp.path();
        fs::write(repo.join(".ignore"), "workspace/ignored.md\n").unwrap();
        let root = repo.join("workspace");
        fs::create_dir_all(&root).unwrap();
        let file = root.join("ignored.md");
        fs::write(&file, "ignored").unwrap();

        assert_targeted_file_matches_full_scan(&root, &file, false);
    }

    #[derive(Debug, Default)]
    struct CountingIgnorePathProbe {
        read_dir_calls: AtomicUsize,
        read_file_calls: AtomicUsize,
    }

    impl IgnorePathProbe for CountingIgnorePathProbe {
        fn read_file(&self, path: &Path) -> io::Result<Option<String>> {
            self.read_file_calls.fetch_add(1, Ordering::Relaxed);
            match fs::read_to_string(path) {
                Ok(contents) => Ok(Some(contents)),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(error),
            }
        }

        fn is_directory(&self, path: &Path) -> io::Result<bool> {
            match fs::metadata(path) {
                Ok(metadata) => Ok(metadata.is_dir()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
                Err(error) => Err(error),
            }
        }

        fn global_ignore_path(&self) -> Option<PathBuf> {
            None
        }

        fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
            self.read_dir_calls.fetch_add(1, Ordering::Relaxed);
            fs::read_dir(path)?
                .map(|entry| entry.map(|entry| entry.path()))
                .collect()
        }
    }

    #[test]
    fn targeted_single_file_ignore_decision_never_enumerates_root_or_siblings() {
        let temp = TempDir::new().unwrap();
        let repo = temp.path();
        fs::write(repo.join(".gitignore"), "workspace/target.md\n").unwrap();
        fs::write(repo.join(".ignore"), "!workspace/target.md\n").unwrap();
        let root = repo.join("workspace");
        fs::create_dir_all(&root).unwrap();
        let target = root.join("target.md");
        fs::write(&target, "target").unwrap();
        for index in 0..512 {
            fs::write(root.join(format!("sibling-{index}.md")), "sibling").unwrap();
        }
        let probe = Arc::new(CountingIgnorePathProbe::default());
        let scanner = TargetedIndexScanner::with_scanner(
            scan_rules(&root),
            IgnoreScanner::with_path_probe(probe.clone()),
        );

        let result = scanner.scan_changed_paths(&[target]).unwrap();

        assert_eq!(result.upserts.len(), 1);
        assert_eq!(probe.read_dir_calls.load(Ordering::Relaxed), 0);
        assert!(probe.read_file_calls.load(Ordering::Relaxed) >= 2);
        assert!(probe.read_file_calls.load(Ordering::Relaxed) <= root.ancestors().count() * 3);
    }

    #[test]
    fn cancellation_during_single_file_ignore_evaluation_returns_cancelled() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();
        fs::write(root.join(".gitignore"), "ignored.md\n").unwrap();
        let target = root.join("target.md");
        fs::write(&target, "target").unwrap();
        let probe = Arc::new(CountingIgnorePathProbe::default());
        let scanner = TargetedIndexScanner::with_scanner(
            scan_rules(root),
            IgnoreScanner::with_path_probe(probe.clone()),
        );
        let checks = Cell::new(0);

        let result = scanner.scan_changed_paths_cancellable(&[target], || {
            let current = checks.get();
            checks.set(current + 1);
            current >= 1
        });

        assert!(matches!(result, Err(TargetedScanError::Cancelled)));
        assert_eq!(probe.read_dir_calls.load(Ordering::Relaxed), 0);
        assert_eq!(probe.read_file_calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn configured_root_named_like_a_forced_exclusion_remains_scannable() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("target");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("keep.md"), "keep").unwrap();
        let scanner = TargetedIndexScanner::new(scan_rules(&root));

        let delta = scanner.scan_changed_paths(&[root.join("keep.md")]).unwrap();

        assert_eq!(delta.upserts.len(), 1);
    }

    #[derive(Default)]
    struct MemoryManifest {
        rows: Vec<DirectoryFingerprint>,
    }

    impl DirectoryManifestReader for MemoryManifest {
        fn directories_for_root(&self, _root: &Path) -> Result<Vec<DirectoryFingerprint>, String> {
            Ok(self.rows.clone())
        }
    }

    #[derive(Default)]
    struct MemoryKnownEntries {
        entries: BTreeMap<PathBuf, Vec<KnownIndexedChild>>,
    }

    impl KnownDirectoryEntriesReader for MemoryKnownEntries {
        fn entries_for_directory(
            &self,
            _root: &Path,
            directory: &Path,
        ) -> Result<Vec<KnownIndexedChild>, String> {
            Ok(self.entries.get(directory).cloned().unwrap_or_default())
        }
    }

    #[derive(Default)]
    struct RecordingFileSystem {
        metadata: BTreeMap<PathBuf, Result<Option<FileSystemMetadata>, io::ErrorKind>>,
        entries: BTreeMap<PathBuf, Result<Vec<FileSystemEntry>, io::ErrorKind>>,
        statted: RefCell<Vec<PathBuf>>,
        enumerated: RefCell<Vec<PathBuf>>,
    }

    impl RecordingFileSystem {
        fn directory(&mut self, path: impl Into<PathBuf>, modified_ms: i64) {
            self.metadata.insert(
                path.into(),
                Ok(Some(FileSystemMetadata::directory(Some(modified_ms)))),
            );
        }

        fn missing(&mut self, path: impl Into<PathBuf>) {
            self.metadata.insert(path.into(), Ok(None));
        }

        fn metadata_error(&mut self, path: impl Into<PathBuf>, kind: io::ErrorKind) {
            self.metadata.insert(path.into(), Err(kind));
        }

        fn directory_entries(&mut self, path: impl Into<PathBuf>, entries: Vec<FileSystemEntry>) {
            self.entries.insert(path.into(), Ok(entries));
        }
    }

    impl FileSystemProbe for RecordingFileSystem {
        fn metadata(&self, path: &Path) -> io::Result<Option<FileSystemMetadata>> {
            self.statted.borrow_mut().push(path.to_path_buf());
            self.metadata
                .get(path)
                .cloned()
                .unwrap_or(Ok(None))
                .map_err(io::Error::from)
        }

        fn read_dir(&self, path: &Path) -> io::Result<Vec<FileSystemEntry>> {
            self.enumerated.borrow_mut().push(path.to_path_buf());
            self.entries
                .get(path)
                .cloned()
                .unwrap_or_else(|| Ok(Vec::new()))
                .map_err(io::Error::from)
        }
    }

    fn fingerprint(
        path: &str,
        parent: Option<&str>,
        root: &str,
        modified_ms: i64,
    ) -> DirectoryFingerprint {
        DirectoryFingerprint {
            path: path.to_owned(),
            parent: parent.map(str::to_owned),
            root: root.to_owned(),
            modified_ms: Some(modified_ms),
        }
    }

    #[test]
    fn calibration_stats_every_known_directory_but_enumerates_only_changed_directories() {
        let mut fs = RecordingFileSystem::default();
        fs.directory("/root", 10);
        fs.directory("/root/a", 20);
        fs.directory("/root/b", 30);
        fs.directory_entries(
            "/root/a",
            vec![FileSystemEntry::file(
                "/root/a/changed.md",
                Some(20),
                Some(5),
            )],
        );
        let manifest = MemoryManifest {
            rows: vec![
                fingerprint("/root", None, "/root", 10),
                fingerprint("/root/a", Some("/root"), "/root", 19),
                fingerprint("/root/b", Some("/root"), "/root", 30),
            ],
        };

        let result = calibrate_manifest(
            &fs,
            &manifest,
            &MemoryKnownEntries::default(),
            Path::new("/root"),
        )
        .unwrap();

        assert_eq!(
            fs.statted.borrow().as_slice(),
            [
                PathBuf::from("/root"),
                PathBuf::from("/root/a"),
                PathBuf::from("/root/b")
            ]
        );
        assert_eq!(
            fs.enumerated.borrow().as_slice(),
            [PathBuf::from("/root/a")]
        );
        assert_eq!(result.changed_directories, vec![PathBuf::from("/root/a")]);
    }

    #[test]
    fn calibration_reports_new_changed_and_missing_directory_deltas() {
        let mut fs = RecordingFileSystem::default();
        fs.directory("/root", 11);
        fs.directory("/root/stable", 30);
        fs.missing("/root/gone");
        fs.directory_entries(
            "/root",
            vec![
                FileSystemEntry::directory("/root/new", Some(40)),
                FileSystemEntry::directory("/root/stable", Some(30)),
            ],
        );
        let manifest = MemoryManifest {
            rows: vec![
                fingerprint("/root", None, "/root", 10),
                fingerprint("/root/gone", Some("/root"), "/root", 20),
                fingerprint("/root/stable", Some("/root"), "/root", 30),
            ],
        };

        let result = calibrate_manifest(
            &fs,
            &manifest,
            &MemoryKnownEntries::default(),
            Path::new("/root"),
        )
        .unwrap();

        assert_eq!(result.changed_directories, vec![PathBuf::from("/root")]);
        assert_eq!(result.new_directories, vec![PathBuf::from("/root/new")]);
        assert_eq!(
            result.missing_directories,
            vec![PathBuf::from("/root/gone")]
        );
    }

    #[test]
    fn calibration_collapses_missing_descendants_into_one_subtree_removal() {
        let mut fs = RecordingFileSystem::default();
        fs.directory("/root", 10);
        fs.missing("/root/gone");
        fs.missing("/root/gone/nested");
        let manifest = MemoryManifest {
            rows: vec![
                fingerprint("/root", None, "/root", 10),
                fingerprint("/root/gone", Some("/root"), "/root", 20),
                fingerprint("/root/gone/nested", Some("/root/gone"), "/root", 30),
            ],
        };

        let result = calibrate_manifest(
            &fs,
            &manifest,
            &MemoryKnownEntries::default(),
            Path::new("/root"),
        )
        .unwrap();

        assert_eq!(
            fs.statted.borrow().as_slice(),
            [
                PathBuf::from("/root"),
                PathBuf::from("/root/gone"),
                PathBuf::from("/root/gone/nested")
            ]
        );
        assert_eq!(
            result.missing_directories,
            vec![PathBuf::from("/root/gone")]
        );
    }

    #[test]
    fn calibration_keeps_permission_failed_baseline_and_continues_other_directories() {
        let mut fs = RecordingFileSystem::default();
        fs.directory("/root", 10);
        fs.metadata_error("/root/locked", io::ErrorKind::PermissionDenied);
        fs.directory("/root/changed", 31);
        fs.directory_entries("/root/changed", Vec::new());
        let manifest = MemoryManifest {
            rows: vec![
                fingerprint("/root", None, "/root", 10),
                fingerprint("/root/locked", Some("/root"), "/root", 20),
                fingerprint("/root/changed", Some("/root"), "/root", 30),
            ],
        };

        let result = calibrate_manifest(
            &fs,
            &manifest,
            &MemoryKnownEntries::default(),
            Path::new("/root"),
        )
        .unwrap();

        assert!(result.missing_directories.is_empty());
        assert_eq!(
            result.changed_directories,
            vec![PathBuf::from("/root/changed")]
        );
        assert_eq!(result.failures.len(), 1);
        assert_eq!(result.failures[0].root, "/root/locked");
        assert_eq!(
            fs.statted.borrow().as_slice(),
            [
                PathBuf::from("/root"),
                PathBuf::from("/root/changed"),
                PathBuf::from("/root/locked")
            ]
        );
    }

    fn known_file(path: &str, modified_ms: i64, size_bytes: u64) -> KnownIndexedChild {
        KnownIndexedChild {
            path: path.to_owned(),
            kind: IndexedEntryKind::File,
            filesystem_kind: FileSystemEntryKind::File,
            modified_ms: Some(modified_ms),
            size_bytes: Some(size_bytes),
        }
    }

    fn known_file_shaped_application(
        path: &str,
        modified_ms: i64,
        size_bytes: u64,
    ) -> KnownIndexedChild {
        KnownIndexedChild {
            path: path.to_owned(),
            kind: IndexedEntryKind::Application,
            filesystem_kind: FileSystemEntryKind::File,
            modified_ms: Some(modified_ms),
            size_bytes: Some(size_bytes),
        }
    }

    #[test]
    fn changed_directory_emits_tombstone_for_missing_direct_file() {
        let mut fs = RecordingFileSystem::default();
        fs.directory("/root", 11);
        fs.directory_entries("/root", Vec::new());
        let manifest = MemoryManifest {
            rows: vec![fingerprint("/root", None, "/root", 10)],
        };
        let known = MemoryKnownEntries {
            entries: BTreeMap::from([(
                PathBuf::from("/root"),
                vec![known_file("/root/old.md", 20, 5)],
            )]),
        };

        let result = calibrate_manifest(&fs, &manifest, &known, Path::new("/root")).unwrap();

        assert_eq!(result.entry_removals, vec![PathBuf::from("/root/old.md")]);
        assert!(result.entry_upserts.is_empty());
    }

    #[test]
    fn changed_directory_folds_direct_file_rename_into_remove_and_upsert() {
        let mut fs = RecordingFileSystem::default();
        fs.directory("/root", 11);
        fs.directory_entries(
            "/root",
            vec![FileSystemEntry::file("/root/new.md", Some(20), Some(5))],
        );
        let manifest = MemoryManifest {
            rows: vec![fingerprint("/root", None, "/root", 10)],
        };
        let known = MemoryKnownEntries {
            entries: BTreeMap::from([(
                PathBuf::from("/root"),
                vec![known_file("/root/old.md", 20, 5)],
            )]),
        };

        let result = calibrate_manifest(&fs, &manifest, &known, Path::new("/root")).unwrap();

        assert_eq!(result.entry_removals, vec![PathBuf::from("/root/old.md")]);
        assert_eq!(result.entry_upserts.len(), 1);
        assert_eq!(result.entry_upserts[0].path, PathBuf::from("/root/new.md"));
    }

    #[test]
    fn changed_directory_does_not_rebuild_unchanged_direct_file() {
        let mut fs = RecordingFileSystem::default();
        fs.directory("/root", 11);
        fs.directory_entries(
            "/root",
            vec![FileSystemEntry::file("/root/stable.md", Some(20), Some(5))],
        );
        let manifest = MemoryManifest {
            rows: vec![fingerprint("/root", None, "/root", 10)],
        };
        let known = MemoryKnownEntries {
            entries: BTreeMap::from([(
                PathBuf::from("/root"),
                vec![known_file("/root/stable.md", 20, 5)],
            )]),
        };

        let result = calibrate_manifest(&fs, &manifest, &known, Path::new("/root")).unwrap();

        assert!(result.entry_removals.is_empty());
        assert!(result.entry_upserts.is_empty());
    }

    #[test]
    fn unchanged_file_shaped_application_is_not_rebuilt() {
        let mut fs = RecordingFileSystem::default();
        fs.directory("C:/root", 11);
        fs.directory_entries(
            "C:/root",
            vec![FileSystemEntry::file("C:/root/Tool.exe", Some(20), Some(5))],
        );
        let manifest = MemoryManifest {
            rows: vec![fingerprint("C:/root", None, "C:/root", 10)],
        };
        let known = MemoryKnownEntries {
            entries: BTreeMap::from([(
                PathBuf::from("C:/root"),
                vec![known_file_shaped_application("C:/root/Tool.exe", 20, 5)],
            )]),
        };

        let result = calibrate_manifest(&fs, &manifest, &known, Path::new("C:/root")).unwrap();

        assert!(result.entry_upserts.is_empty());
        assert!(result.entry_removals.is_empty());
    }

    #[test]
    fn deleted_file_shaped_application_emits_tombstone() {
        let mut fs = RecordingFileSystem::default();
        fs.directory("/root", 11);
        fs.directory_entries("/root", Vec::new());
        let manifest = MemoryManifest {
            rows: vec![fingerprint("/root", None, "/root", 10)],
        };
        let known = MemoryKnownEntries {
            entries: BTreeMap::from([(
                PathBuf::from("/root"),
                vec![known_file_shaped_application("/root/tool.desktop", 20, 5)],
            )]),
        };

        let result = calibrate_manifest(&fs, &manifest, &known, Path::new("/root")).unwrap();

        assert_eq!(
            result.entry_removals,
            vec![PathBuf::from("/root/tool.desktop")]
        );
    }

    #[test]
    fn file_shaped_application_becoming_other_emits_tombstone() {
        let mut fs = RecordingFileSystem::default();
        fs.directory("/root", 11);
        fs.directory_entries(
            "/root",
            vec![FileSystemEntry {
                path: PathBuf::from("/root/tool.desktop"),
                metadata: FileSystemMetadata {
                    kind: FileSystemEntryKind::Other,
                    modified_ms: Some(21),
                    size_bytes: None,
                },
            }],
        );
        let manifest = MemoryManifest {
            rows: vec![fingerprint("/root", None, "/root", 10)],
        };
        let known = MemoryKnownEntries {
            entries: BTreeMap::from([(
                PathBuf::from("/root"),
                vec![known_file_shaped_application("/root/tool.desktop", 20, 5)],
            )]),
        };

        let result = calibrate_manifest(&fs, &manifest, &known, Path::new("/root")).unwrap();

        assert_eq!(
            result.entry_removals,
            vec![PathBuf::from("/root/tool.desktop")]
        );
        assert!(result.entry_upserts.is_empty());
    }

    #[test]
    fn file_shaped_application_becoming_directory_is_removed_and_manifested_as_new() {
        let mut fs = RecordingFileSystem::default();
        fs.directory("/root", 11);
        fs.directory_entries(
            "/root",
            vec![FileSystemEntry::directory("/root/Tool.app", Some(21))],
        );
        let manifest = MemoryManifest {
            rows: vec![fingerprint("/root", None, "/root", 10)],
        };
        let known = MemoryKnownEntries {
            entries: BTreeMap::from([(
                PathBuf::from("/root"),
                vec![known_file_shaped_application("/root/Tool.app", 20, 5)],
            )]),
        };

        let result = calibrate_manifest(&fs, &manifest, &known, Path::new("/root")).unwrap();

        assert_eq!(result.entry_removals, vec![PathBuf::from("/root/Tool.app")]);
        assert_eq!(
            result.new_directories,
            vec![PathBuf::from("/root/Tool.app")]
        );
    }
}
