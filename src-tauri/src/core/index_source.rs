//! Common boundary for full-index entry sources.
//!
//! Sources stream entries so callers can persist and publish progress without retaining a
//! second in-memory snapshot. Cancellation is explicit: a cancelled source must never be
//! mistaken for a completed root.

use crate::core::index_entry::{IndexReport, IndexScanStats, IndexedEntry};
use crate::core::index_scanner::{IndexDirectoryScanCheckpoint, IndexScanPlan};
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IndexSourceKind {
    Generic,
    WindowsNtfsWin32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IndexSourcePhase {
    Probing,
    Enumerating,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexSourceProgress {
    pub source: IndexSourceKind,
    pub phase: IndexSourcePhase,
    pub root: Option<String>,
    pub stats: IndexScanStats,
}

impl IndexSourceProgress {
    pub fn new(
        source: IndexSourceKind,
        phase: IndexSourcePhase,
        root: Option<&Path>,
        stats: IndexScanStats,
    ) -> Self {
        Self {
            source,
            phase,
            root: root.map(|path| path.to_string_lossy().into_owned()),
            stats,
        }
    }
}

#[derive(Debug)]
pub enum IndexSourceError {
    Cancelled,
    Io(io::Error),
}

impl std::fmt::Display for IndexSourceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => formatter.write_str("index source scan was cancelled"),
            Self::Io(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for IndexSourceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Cancelled => None,
            Self::Io(error) => Some(error),
        }
    }
}

impl From<io::Error> for IndexSourceError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub type IndexBatchCallback<'a> =
    dyn FnMut(&[IndexedEntry], &IndexScanStats) -> io::Result<()> + 'a;
pub type IndexProgressCallback<'a> = dyn FnMut(IndexSourceProgress) -> io::Result<()> + 'a;
pub type IndexDirectoryCallback<'a> =
    dyn FnMut(&IndexDirectoryScanCheckpoint) -> io::Result<()> + 'a;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexSourceResume {
    pub pending_directories: Vec<PathBuf>,
    pub completed_stats: IndexScanStats,
}

impl IndexSourceResume {
    pub fn fresh(root: PathBuf) -> Self {
        Self {
            pending_directories: vec![root],
            completed_stats: IndexScanStats::default(),
        }
    }
}

pub trait IndexSource: std::fmt::Debug + Send + Sync {
    fn kind(&self) -> IndexSourceKind;

    fn scan(
        &self,
        plan: IndexScanPlan,
        resume: Option<IndexSourceResume>,
        is_cancelled: &dyn Fn() -> bool,
        on_batch: &mut IndexBatchCallback<'_>,
        on_progress: &mut IndexProgressCallback<'_>,
        on_directory: &mut IndexDirectoryCallback<'_>,
    ) -> Result<IndexReport, IndexSourceError>;
}

/// Removes duplicate roots and roots already covered by another configured ancestor.
///
/// The comparison is lexical and does not require roots to exist. This matters for temporarily
/// disconnected volumes and for Windows roots configured with mixed separator/case spellings.
pub fn compress_covered_roots(roots: &[PathBuf]) -> Vec<PathBuf> {
    compress_covered_roots_by(roots, native_path_key)
}

pub fn compress_windows_covered_roots(roots: &[PathBuf]) -> Vec<PathBuf> {
    compress_covered_roots_by(roots, |path| {
        normalize_windows_path_text(&path.to_string_lossy())
    })
}

fn compress_covered_roots_by(roots: &[PathBuf], key_for: impl Fn(&Path) -> String) -> Vec<PathBuf> {
    let keys: Vec<_> = roots.iter().map(|root| key_for(root)).collect();
    roots
        .iter()
        .enumerate()
        .filter(|(index, _)| {
            !keys.iter().enumerate().any(|(candidate_index, candidate)| {
                candidate_index != *index
                    && (candidate == &keys[*index] && candidate_index < *index
                        || candidate != &keys[*index]
                            && normalized_path_is_same_or_descendant(candidate, &keys[*index]))
            })
        })
        .map(|(_, root)| root.clone())
        .collect()
}

fn native_path_key(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    if cfg!(target_os = "windows") {
        normalize_windows_path_text(&text)
    } else {
        trim_non_root_trailing_slashes(text)
    }
}

/// Normalizes a Windows path key without accessing the filesystem.
///
/// The returned key is for equality/coverage lookup only. It is deliberately not written back as
/// the display path because preserving the user's original path spelling is useful in the UI.
pub fn normalize_windows_path_text(path: &str) -> String {
    let mut normalized = path.replace('\\', "/");
    let lower = normalized.to_ascii_lowercase();
    if lower.starts_with("//?/unc/") {
        normalized = format!("//{}", &normalized[8..]);
    } else if lower.starts_with("//?/") {
        normalized = normalized[4..].to_owned();
    }

    let has_unc_prefix = normalized.starts_with("//");
    let mut collapsed = String::with_capacity(normalized.len());
    for character in normalized.chars() {
        if character != '/' || !collapsed.ends_with('/') {
            collapsed.push(character);
        }
    }
    if has_unc_prefix && !collapsed.starts_with("//") {
        collapsed.insert(0, '/');
    }
    trim_non_root_trailing_slashes(collapsed.to_lowercase())
}

fn trim_non_root_trailing_slashes(mut path: String) -> String {
    while path.len() > 1 && path.ends_with('/') && !(path.len() == 3 && path.as_bytes()[1] == b':')
    {
        path.pop();
    }
    path
}

fn normalized_path_is_same_or_descendant(root: &str, candidate: &str) -> bool {
    candidate == root
        || (root == "/" && candidate.starts_with('/'))
        || (root.ends_with('/') && candidate.starts_with(root))
        || candidate
            .strip_prefix(root)
            .is_some_and(|remainder| remainder.starts_with('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_path_normalization_handles_case_separators_and_device_prefixes() {
        assert_eq!(
            normalize_windows_path_text(r"C:\Users\Frank\"),
            "c:/users/frank"
        );
        assert_eq!(normalize_windows_path_text(r"\\?\C:\DATA\A"), "c:/data/a");
        assert_eq!(
            normalize_windows_path_text(r"\\?\UNC\Server\Share\Folder"),
            "//server/share/folder"
        );
    }

    #[test]
    fn windows_root_compression_removes_nested_and_duplicate_spellings() {
        let roots = vec![
            PathBuf::from(r"C:\Users\Frank\Desktop"),
            PathBuf::from("c:/"),
            PathBuf::from(r"C:\"),
            PathBuf::from(r"D:\Data"),
            PathBuf::from(r"D:\Database"),
        ];
        assert_eq!(
            compress_windows_covered_roots(&roots),
            vec![
                PathBuf::from("c:/"),
                PathBuf::from(r"D:\Data"),
                PathBuf::from(r"D:\Database")
            ]
        );
    }

    #[test]
    fn root_compression_observes_component_boundaries() {
        let roots = vec![
            PathBuf::from("/workspace/app"),
            PathBuf::from("/workspace/application"),
            PathBuf::from("/workspace"),
        ];
        assert_eq!(
            compress_covered_roots(&roots),
            vec![PathBuf::from("/workspace")]
        );

        let siblings = vec![PathBuf::from("/data/app"), PathBuf::from("/data/apple")];
        assert_eq!(compress_covered_roots(&siblings), siblings);
    }
}
