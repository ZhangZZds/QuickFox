//! Targeted path scanning and directory-manifest calibration.

use crate::core::index_entry::{IndexFailure, IndexedEntry, IndexedEntryKind};
use crate::core::index_scanner::{IgnoreScanner, IndexPathRules};
use crate::core::index_update_coordinator::CoordinatorBatch;
use serde::{Deserialize, Serialize};
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

pub trait DirectoryManifestReader {
    fn directories_for_root(&self, root: &Path) -> Result<Vec<DirectoryFingerprint>, String>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    directory_entries: BTreeMap<PathBuf, Vec<FileSystemEntry>>,
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

    pub fn rules(&self) -> &IndexPathRules {
        &self.rules
    }

    pub fn scan_changed_paths(&self, paths: &[PathBuf]) -> io::Result<TargetedScanResult> {
        self.scan_changed_paths_cancellable(paths, || false)
    }

    pub fn scan_changed_paths_cancellable(
        &self,
        paths: &[PathBuf],
        is_cancelled: impl Fn() -> bool,
    ) -> io::Result<TargetedScanResult> {
        let mut result = TargetedScanResult::default();
        for path in sorted_unique_paths(paths) {
            if is_cancelled() {
                break;
            }
            let Some(root) = self.rules.configured_root_for(&path) else {
                continue;
            };
            if self.rules.is_excluded(&path) {
                continue;
            }
            let report =
                self.scanner
                    .scan_subtree_cancellable(&path, root, &self.rules, &is_cancelled)?;
            result.failures.extend(report.failures);
            for entry in report.entries {
                if entry.kind == IndexedEntryKind::Directory {
                    result.manifest_upserts.push(directory_fingerprint(&entry));
                }
                result.upserts.push(entry);
            }
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

    pub fn scan_rename(&self, old_path: &Path, new_path: &Path) -> io::Result<TargetedScanResult> {
        let mut result = self.scan_removed_paths(&[old_path.to_path_buf()]);
        merge_result(
            &mut result,
            self.scan_changed_paths(&[new_path.to_path_buf()])?,
        );
        sort_result(&mut result);
        Ok(result)
    }

    pub fn scan_batch(&self, batch: CoordinatorBatch) -> io::Result<TargetedScanResult> {
        let mut result = self.scan_removed_paths(&batch.removed_paths);
        merge_result(&mut result, self.scan_changed_paths(&batch.changed_paths)?);
        sort_result(&mut result);
        Ok(result)
    }

    pub fn calibrate_root(
        &self,
        probe: &impl FileSystemProbe,
        manifest: &impl DirectoryManifestReader,
        root: &Path,
    ) -> io::Result<TargetedScanResult> {
        self.calibrate_root_cancellable(probe, manifest, root, || false)
    }

    pub fn calibrate_root_cancellable(
        &self,
        probe: &impl FileSystemProbe,
        manifest: &impl DirectoryManifestReader,
        root: &Path,
        is_cancelled: impl Fn() -> bool,
    ) -> io::Result<TargetedScanResult> {
        let calibration = calibrate_manifest_cancellable(probe, manifest, root, &is_cancelled)?;
        let mut result = TargetedScanResult {
            failures: calibration.failures.clone(),
            removals: calibration.missing_directories.clone(),
            manifest_removals: calibration.missing_directories.clone(),
            ..TargetedScanResult::default()
        };

        for directory in &calibration.changed_directories {
            if is_cancelled() {
                break;
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
            if let Some(entries) = calibration.directory_entries.get(directory) {
                let direct_files: Vec<_> = entries
                    .iter()
                    .filter(|entry| entry.metadata.kind == FileSystemEntryKind::File)
                    .map(|entry| entry.path.clone())
                    .collect();
                merge_result(
                    &mut result,
                    self.scan_changed_paths_cancellable(&direct_files, &is_cancelled)?,
                );
            }
        }

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
    root: &Path,
) -> io::Result<ManifestCalibrationResult> {
    calibrate_manifest_cancellable(probe, manifest, root, &|| false)
}

pub fn calibrate_manifest_cancellable(
    probe: &impl FileSystemProbe,
    manifest: &impl DirectoryManifestReader,
    root: &Path,
    is_cancelled: &impl Fn() -> bool,
) -> io::Result<ManifestCalibrationResult> {
    let mut known = manifest
        .directories_for_root(root)
        .map_err(io::Error::other)?;
    known.retain(|fingerprint| Path::new(&fingerprint.root) == root);
    known.sort_by(|left, right| left.path.cmp(&right.path));
    known.dedup_by(|left, right| left.path == right.path);
    let known_paths: BTreeSet<_> = known
        .iter()
        .map(|fingerprint| PathBuf::from(&fingerprint.path))
        .collect();
    let mut result = ManifestCalibrationResult::default();

    for fingerprint in &known {
        if is_cancelled() {
            break;
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
                        entries.sort_by(|left, right| left.path.cmp(&right.path));
                        entries.dedup_by(|left, right| left.path == right.path);
                        for entry in &entries {
                            if entry.metadata.kind == FileSystemEntryKind::Directory
                                && !known_paths.contains(&entry.path)
                            {
                                result.new_directories.push(entry.path.clone());
                            }
                        }
                        result.directory_entries.insert(path.clone(), entries);
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
    result
        .upserts
        .sort_by(|left, right| left.path.cmp(&right.path));
    result
        .upserts
        .dedup_by(|left, right| left.path == right.path);
    result.removals.sort();
    result.removals.dedup();
    result
        .manifest_upserts
        .sort_by(|left, right| left.path.cmp(&right.path));
    result
        .manifest_upserts
        .dedup_by(|left, right| left.path == right.path);
    result.manifest_removals.sort();
    result.manifest_removals.dedup();
    result.failures.sort_by(|left, right| {
        left.root
            .cmp(&right.root)
            .then_with(|| left.message.cmp(&right.message))
    });
    result
        .failures
        .dedup_by(|left, right| left.root == right.root && left.message == right.message);
}

fn sort_calibration_result(result: &mut ManifestCalibrationResult) {
    result.changed_directories.sort();
    result.changed_directories.dedup();
    result.new_directories.sort();
    result.new_directories.dedup();
    result.missing_directories.sort();
    result.missing_directories.dedup();
    let mut subtree_roots: Vec<PathBuf> = Vec::new();
    for path in std::mem::take(&mut result.missing_directories) {
        if !subtree_roots
            .iter()
            .any(|ancestor| path.starts_with(ancestor))
        {
            subtree_roots.push(path);
        }
    }
    result.missing_directories = subtree_roots;
    result.failures.sort_by(|left, right| {
        left.root
            .cmp(&right.root)
            .then_with(|| left.message.cmp(&right.message))
    });
}

fn sorted_unique_paths(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut paths = paths.to_vec();
    paths.sort();
    paths.dedup();
    paths
}

fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::index_entry::IndexedEntryKind;
    use crate::core::index_scanner::{IndexPathRules, IndexScanPlan};
    use std::cell::{Cell, RefCell};
    use std::collections::BTreeMap;
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};
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
    fn cancellation_is_checked_inside_a_targeted_subtree_walk() {
        let temp = TempDir::new().unwrap();
        for index in 0..32 {
            fs::write(temp.path().join(format!("file-{index}.md")), "file").unwrap();
        }
        let scanner = TargetedIndexScanner::new(scan_rules(temp.path()));
        let checks = Cell::new(0);

        let delta = scanner
            .scan_changed_paths_cancellable(&[temp.path().to_path_buf()], || {
                let current = checks.get();
                checks.set(current + 1);
                current >= 2
            })
            .unwrap();

        assert!(delta.upserts.len() < 32);
        assert!(checks.get() >= 3);
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

        let result = calibrate_manifest(&fs, &manifest, Path::new("/root")).unwrap();

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

        let result = calibrate_manifest(&fs, &manifest, Path::new("/root")).unwrap();

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

        let result = calibrate_manifest(&fs, &manifest, Path::new("/root")).unwrap();

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

        let result = calibrate_manifest(&fs, &manifest, Path::new("/root")).unwrap();

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
}
