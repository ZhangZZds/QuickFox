use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

pub fn normalize_path_key(path: impl AsRef<Path>) -> String {
    normalize_path_text_key(&path.as_ref().to_string_lossy())
}

pub fn normalize_path_text_key(path: &str) -> String {
    normalize_path_text_key_for_mode(path, PathComparisonMode::native())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PathComparisonMode {
    Native,
    Windows,
}

impl PathComparisonMode {
    pub(crate) const fn native() -> Self {
        if cfg!(target_os = "windows") {
            Self::Windows
        } else {
            Self::Native
        }
    }
}

pub(crate) fn normalize_path_key_for_mode(
    path: impl AsRef<Path>,
    mode: PathComparisonMode,
) -> String {
    normalize_path_text_key_for_mode(&path.as_ref().to_string_lossy(), mode)
}

pub(crate) fn normalize_path_text_key_for_mode(path: &str, mode: PathComparisonMode) -> String {
    let normalized = path.replace('\\', "/");
    let normalized = if normalized.len() > 1 {
        normalized.trim_end_matches('/').to_owned()
    } else {
        normalized
    };
    if mode == PathComparisonMode::Windows {
        normalized.to_lowercase()
    } else {
        normalized
    }
}

pub(crate) fn path_is_same_or_descendant_for_mode(
    root: impl AsRef<Path>,
    candidate: impl AsRef<Path>,
    mode: PathComparisonMode,
) -> bool {
    let root = normalize_path_key_for_mode(root, mode);
    let candidate = normalize_path_key_for_mode(candidate, mode);
    candidate == root
        || (root == "/" && candidate.starts_with('/'))
        || candidate
            .strip_prefix(&root)
            .is_some_and(|remainder| remainder.starts_with('/'))
}

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
    #[serde(default)]
    pub parent: String,
    #[serde(default)]
    pub extension: Option<String>,
    #[serde(default)]
    pub depth: usize,
    #[serde(default)]
    pub root: String,
    #[serde(default)]
    pub modified_ms: Option<i64>,
    #[serde(default)]
    pub size_bytes: Option<u64>,
    #[serde(default)]
    pub search_text: String,
    #[serde(default)]
    pub content_index_state: ContentIndexState,
}

impl IndexedEntry {
    pub fn from_path_metadata(
        path: impl AsRef<Path>,
        root: impl AsRef<Path>,
        kind: IndexedEntryKind,
    ) -> Self {
        let path = path.as_ref();
        let root = root.as_ref();
        let path_text = path.to_string_lossy().to_string();
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| path_text.clone());
        let parent = path
            .parent()
            .map(|parent| parent.to_string_lossy().to_string())
            .unwrap_or_default();
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension.to_ascii_lowercase());
        let depth = path
            .strip_prefix(root)
            .ok()
            .map(|relative| relative.components().count())
            .unwrap_or_else(|| path.components().count());
        let root = root.to_string_lossy().to_string();
        let metadata = std::fs::metadata(path).ok();
        let modified_ms = metadata
            .as_ref()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis() as i64);
        let size_bytes = metadata
            .as_ref()
            .filter(|metadata| metadata.is_file())
            .map(|metadata| metadata.len());
        let search_text = build_search_text(&name, &path_text);

        Self {
            path: path_text,
            name,
            kind,
            parent,
            extension,
            depth,
            root,
            modified_ms,
            size_bytes,
            search_text,
            content_index_state: ContentIndexState::NotIndexed,
        }
    }

    pub fn legacy(
        path: impl Into<String>,
        name: impl Into<String>,
        kind: IndexedEntryKind,
    ) -> Self {
        let path = path.into();
        let name = name.into();
        Self {
            search_text: build_search_text(&name, &path),
            path,
            name,
            kind,
            parent: String::new(),
            extension: None,
            depth: 0,
            root: String::new(),
            modified_ms: None,
            size_bytes: None,
            content_index_state: ContentIndexState::NotIndexed,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ContentIndexState {
    #[default]
    NotIndexed,
    Indexed,
    SkippedTooLarge,
    SkippedBinary,
    ReadFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexFailure {
    pub root: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct IndexScanStats {
    pub scanned: usize,
    pub accepted: usize,
    pub skipped: usize,
    pub failures: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScanEvent {
    RootStarted {
        root: String,
        stage: Option<String>,
    },
    RootFinished {
        root: String,
        stage: Option<String>,
        stats: IndexScanStats,
    },
    EntryAccepted {
        path: String,
        kind: IndexedEntryKind,
    },
    EntrySkipped {
        path: String,
        reason: String,
    },
    Failure(IndexFailure),
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct IndexReport {
    pub entries: Vec<IndexedEntry>,
    pub failures: Vec<IndexFailure>,
    #[serde(default)]
    pub scan_stats: IndexScanStats,
    #[serde(default)]
    pub scan_events: Vec<ScanEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IndexStatusKind {
    Unbuilt,
    Building,
    Ready,
    Refreshing,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IndexAvailability {
    Unavailable,
    QuickAvailable,
    Completing,
    ContentIndexing,
    Complete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexStatus {
    pub kind: IndexStatusKind,
    #[serde(default = "default_index_availability")]
    pub availability: IndexAvailability,
    pub entry_count: usize,
    pub message: Option<String>,
    pub generation: u64,
    pub completed_at_ms: Option<i64>,
    #[serde(default)]
    pub stage: String,
    #[serde(default)]
    pub current_root: Option<String>,
    #[serde(default)]
    pub scanned: usize,
    #[serde(default)]
    pub accepted: usize,
    #[serde(default)]
    pub skipped: usize,
    #[serde(default)]
    pub failures: usize,
}

fn default_index_availability() -> IndexAvailability {
    IndexAvailability::Unavailable
}

#[derive(Debug, Clone)]
pub struct IndexLifecycle {
    generation: u64,
    status: IndexStatus,
}

impl Default for IndexLifecycle {
    fn default() -> Self {
        Self {
            generation: 0,
            status: IndexStatus {
                kind: IndexStatusKind::Unbuilt,
                availability: IndexAvailability::Unavailable,
                entry_count: 0,
                message: None,
                generation: 0,
                completed_at_ms: None,
                stage: String::new(),
                current_root: None,
                scanned: 0,
                accepted: 0,
                skipped: 0,
                failures: 0,
            },
        }
    }
}

impl IndexLifecycle {
    pub fn from_ready(entry_count: usize, completed_at_ms: i64) -> Self {
        Self {
            generation: 0,
            status: IndexStatus {
                kind: IndexStatusKind::Ready,
                availability: IndexAvailability::Complete,
                entry_count,
                message: None,
                generation: 0,
                completed_at_ms: Some(completed_at_ms),
                stage: String::new(),
                current_root: None,
                scanned: 0,
                accepted: 0,
                skipped: 0,
                failures: 0,
            },
        }
    }

    pub fn status(&self) -> &IndexStatus {
        &self.status
    }

    pub fn start_refresh(&mut self, has_existing_index: bool) -> u64 {
        self.generation = self.generation.saturating_add(1);
        self.status.kind = if has_existing_index {
            IndexStatusKind::Refreshing
        } else {
            IndexStatusKind::Building
        };
        self.status.availability = if has_existing_index {
            IndexAvailability::Complete
        } else {
            IndexAvailability::Unavailable
        };
        self.status.message = None;
        self.status.generation = self.generation;
        self.status.stage.clear();
        self.status.current_root = None;
        self.status.scanned = 0;
        self.status.accepted = 0;
        self.status.skipped = 0;
        self.status.failures = 0;
        self.generation
    }

    pub fn update_progress(
        &mut self,
        generation: u64,
        stage: impl Into<String>,
        current_root: Option<String>,
        stats: IndexScanStats,
        entry_count: usize,
    ) -> bool {
        if generation != self.generation {
            return false;
        }

        let stage = stage.into();
        self.status.availability = availability_for_progress_stage(&stage, entry_count);
        self.status.stage = stage;
        self.status.current_root = current_root;
        self.status.scanned = stats.scanned;
        self.status.accepted = stats.accepted;
        self.status.skipped = stats.skipped;
        self.status.failures = stats.failures;
        self.status.entry_count = entry_count;
        true
    }

    pub fn complete_refresh(
        &mut self,
        generation: u64,
        entry_count: usize,
        completed_at_ms: i64,
    ) -> bool {
        if generation != self.generation {
            return false;
        }

        self.status = IndexStatus {
            kind: IndexStatusKind::Ready,
            availability: IndexAvailability::Complete,
            entry_count,
            message: None,
            generation,
            completed_at_ms: Some(completed_at_ms),
            stage: String::new(),
            current_root: None,
            scanned: self.status.scanned,
            accepted: self.status.accepted,
            skipped: self.status.skipped,
            failures: self.status.failures,
        };
        true
    }

    pub fn fail_refresh(&mut self, generation: u64, message: String) -> bool {
        if generation != self.generation {
            return false;
        }

        self.status.kind = IndexStatusKind::Failed;
        if self.status.entry_count == 0 {
            self.status.availability = IndexAvailability::Unavailable;
        }
        self.status.message = Some(message);
        self.status.generation = generation;
        true
    }
}

fn availability_for_progress_stage(stage: &str, entry_count: usize) -> IndexAvailability {
    if entry_count == 0 {
        return IndexAvailability::Unavailable;
    }

    match stage {
        "configured-roots" | "remaining-drives" => IndexAvailability::Completing,
        "content-index" => IndexAvailability::ContentIndexing,
        _ => IndexAvailability::QuickAvailable,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexScanOptions {
    pub include_dirs: Vec<PathBuf>,
    pub exclude_dirs: Vec<PathBuf>,
    pub exclude_patterns: Vec<String>,
    pub respect_project_ignores: bool,
}

impl Default for IndexScanOptions {
    fn default() -> Self {
        Self {
            include_dirs: Vec::new(),
            exclude_dirs: Vec::new(),
            exclude_patterns: Vec::new(),
            respect_project_ignores: true,
        }
    }
}

pub fn build_search_text(name: &str, path: &str) -> String {
    format!("{name} {path}").to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn unix_backslash_paths_preserve_case() {
        assert_eq!(normalize_path_text_key(r"A\B"), "A/B");
        assert_eq!(normalize_path_text_key(r"a\b"), "a/b");
        assert_ne!(
            normalize_path_text_key(r"A\B"),
            normalize_path_text_key(r"a\b")
        );
    }

    #[test]
    fn filesystem_roots_match_descendants_without_crossing_path_kinds() {
        assert!(path_is_same_or_descendant_for_mode(
            "/",
            "/tmp/a",
            PathComparisonMode::Native,
        ));
        assert!(!path_is_same_or_descendant_for_mode(
            "/",
            "tmp/a",
            PathComparisonMode::Native,
        ));
        assert!(!path_is_same_or_descendant_for_mode(
            "/",
            r"C:\tmp\a",
            PathComparisonMode::Native,
        ));

        assert!(path_is_same_or_descendant_for_mode(
            "C:/",
            r"c:\tmp\a",
            PathComparisonMode::Windows,
        ));
        assert!(!path_is_same_or_descendant_for_mode(
            "C:/",
            "C:tmp/a",
            PathComparisonMode::Windows,
        ));
        assert!(!path_is_same_or_descendant_for_mode(
            "C:/",
            "D:/tmp/a",
            PathComparisonMode::Windows,
        ));
    }

    #[test]
    fn entry_metadata_from_path() {
        let root = temp_dir("entry-metadata");
        let docs = root.join("docs");
        fs::create_dir_all(&docs).unwrap();
        let file = docs.join("Report.PDF");
        fs::write(&file, "metadata").unwrap();

        let entry = IndexedEntry::from_path_metadata(&file, &root, IndexedEntryKind::File);

        assert_eq!(entry.path, file.to_string_lossy());
        assert_eq!(entry.name, "Report.PDF");
        assert_eq!(entry.parent, docs.to_string_lossy());
        assert_eq!(entry.extension.as_deref(), Some("pdf"));
        assert_eq!(entry.depth, 2);
        assert_eq!(entry.root, root.to_string_lossy());
        assert!(entry.modified_ms.is_some());
        assert_eq!(entry.size_bytes, Some(8));
        assert!(entry.search_text.contains("report.pdf"));
        assert!(entry.search_text.contains(&entry.path.to_lowercase()));
        assert_eq!(entry.content_index_state, ContentIndexState::NotIndexed);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn legacy_json_entry_defaults_metadata() {
        let entry: IndexedEntry = serde_json::from_value(json!({
            "path": "/tmp/notes.md",
            "name": "notes.md",
            "kind": "File"
        }))
        .unwrap();

        assert_eq!(entry.parent, "");
        assert_eq!(entry.extension, None);
        assert_eq!(entry.depth, 0);
        assert_eq!(entry.root, "");
        assert_eq!(entry.modified_ms, None);
        assert_eq!(entry.size_bytes, None);
        assert_eq!(entry.search_text, "");
        assert_eq!(entry.content_index_state, ContentIndexState::NotIndexed);
    }

    #[test]
    fn index_status_serializes_stage_progress_payload() {
        let status = IndexStatus {
            kind: IndexStatusKind::Refreshing,
            availability: IndexAvailability::Completing,
            entry_count: 7,
            message: None,
            generation: 2,
            completed_at_ms: None,
            stage: "configured-roots".to_owned(),
            current_root: Some("/tmp".to_owned()),
            scanned: 11,
            accepted: 7,
            skipped: 3,
            failures: 1,
        };

        let value = serde_json::to_value(status).unwrap();

        assert_eq!(value["stage"], "configured-roots");
        assert_eq!(value["currentRoot"], "/tmp");
        assert_eq!(value["scanned"], 11);
        assert_eq!(value["accepted"], 7);
        assert_eq!(value["skipped"], 3);
        assert_eq!(value["failures"], 1);
    }

    #[test]
    fn index_lifecycle_marks_quick_available_before_background_completion() {
        let mut lifecycle = IndexLifecycle::default();
        let generation = lifecycle.start_refresh(false);

        assert!(lifecycle.update_progress(
            generation,
            "user-hot-paths",
            Some("/Users/frank/Downloads".to_owned()),
            IndexScanStats {
                scanned: 12,
                accepted: 3,
                skipped: 1,
                failures: 0,
            },
            3,
        ));

        assert_eq!(lifecycle.status().kind, IndexStatusKind::Building);
        assert_eq!(
            lifecycle.status().availability,
            IndexAvailability::QuickAvailable
        );

        assert!(lifecycle.update_progress(
            generation,
            "configured-roots",
            Some("/Volumes/Data".to_owned()),
            IndexScanStats {
                scanned: 120,
                accepted: 40,
                skipped: 10,
                failures: 1,
            },
            40,
        ));

        assert_eq!(
            lifecycle.status().availability,
            IndexAvailability::Completing
        );
    }

    #[test]
    fn failed_refresh_keeps_available_entry_count() {
        let mut lifecycle = IndexLifecycle::default();
        let generation = lifecycle.start_refresh(false);
        lifecycle.update_progress(
            generation,
            "user-hot-paths",
            None,
            IndexScanStats {
                scanned: 4,
                accepted: 2,
                skipped: 0,
                failures: 0,
            },
            2,
        );

        lifecycle.fail_refresh(generation, "permission denied".to_owned());

        assert_eq!(lifecycle.status().kind, IndexStatusKind::Failed);
        assert_eq!(lifecycle.status().entry_count, 2);
        assert_eq!(
            lifecycle.status().availability,
            IndexAvailability::QuickAvailable
        );
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
