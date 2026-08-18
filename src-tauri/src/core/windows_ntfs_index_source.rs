//! Windows NTFS index source and safe fallback routing.
//!
//! Standard-user installations use `FindFirstFileExW` with large-fetch hints for fixed NTFS
//! volumes. Raw-volume MFT/USN access is capability-probed but is not used by this build: doing so
//! reliably requires a separately reviewed privileged-service design. No elevation prompt or
//! service installation is attempted here.

use crate::core::generic_index_source::GenericIndexSource;
use crate::core::index_entry::{IndexReport, IndexScanStats};
use crate::core::index_scanner::IndexScanPlan;
use crate::core::index_source::{
    compress_windows_covered_roots, IndexBatchCallback, IndexDirectoryCallback,
    IndexProgressCallback, IndexSource, IndexSourceError, IndexSourceKind, IndexSourcePhase,
    IndexSourceProgress, IndexSourceResume,
};
use serde::{Deserialize, Serialize};
use std::path::Path;
#[cfg(any(target_os = "windows", test))]
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "status")]
pub enum MftUsnCapability {
    UnsupportedPlatform,
    NotNtfs,
    RawVolumeUnavailable {
        os_error: Option<u32>,
    },
    JournalUnavailable {
        os_error: Option<u32>,
    },
    /// The journal can be queried, but raw MFT enumeration remains disabled until the privileged
    /// service threat model, installation and update path have been approved.
    JournalReadableEnumerationDisabled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowsNtfsCapability {
    pub root: String,
    pub fixed_drive: bool,
    pub file_system: Option<String>,
    pub selected_source: IndexSourceKind,
    pub mft_usn: MftUsnCapability,
    pub reason: String,
}

impl WindowsNtfsCapability {
    fn generic(root: &Path, reason: impl Into<String>, mft_usn: MftUsnCapability) -> Self {
        Self {
            root: root.to_string_lossy().into_owned(),
            fixed_drive: false,
            file_system: None,
            selected_source: IndexSourceKind::Generic,
            mft_usn,
            reason: reason.into(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct WindowsNtfsIndexSource {
    generic: GenericIndexSource,
}

impl WindowsNtfsIndexSource {
    pub fn probe(&self, root: &Path) -> WindowsNtfsCapability {
        probe_windows_ntfs(root)
    }

    pub fn selection_for(&self, plan: &IndexScanPlan) -> Vec<WindowsNtfsCapability> {
        compress_windows_covered_roots(&plan.include_roots)
            .iter()
            .map(|root| self.capability_for_plan(root, plan.respect_project_ignores))
            .collect()
    }

    fn capability_for_plan(
        &self,
        root: &Path,
        respect_project_ignores: bool,
    ) -> WindowsNtfsCapability {
        let mut capability = self.probe(root);
        if respect_project_ignores {
            capability.selected_source = IndexSourceKind::Generic;
            capability.reason =
                "启用项目 ignore 规则时使用 Generic Scanner 以保持语义一致".to_owned();
        }
        capability
    }
}

impl IndexSource for WindowsNtfsIndexSource {
    fn kind(&self) -> IndexSourceKind {
        IndexSourceKind::WindowsNtfsWin32
    }

    fn scan(
        &self,
        mut plan: IndexScanPlan,
        resume: Option<IndexSourceResume>,
        is_cancelled: &dyn Fn() -> bool,
        on_batch: &mut IndexBatchCallback<'_>,
        on_progress: &mut IndexProgressCallback<'_>,
        on_directory: &mut IndexDirectoryCallback<'_>,
    ) -> Result<IndexReport, IndexSourceError> {
        plan.include_roots = compress_windows_covered_roots(&plan.include_roots);
        let roots = plan.include_roots.clone();
        let mut combined = IndexReport::default();

        for root in roots {
            if is_cancelled() {
                return Err(IndexSourceError::Cancelled);
            }
            let capability = self.capability_for_plan(&root, plan.respect_project_ignores);
            on_progress(IndexSourceProgress::new(
                capability.selected_source,
                IndexSourcePhase::Probing,
                Some(&root),
                IndexScanStats::default(),
            ))?;

            let mut root_plan = plan.clone();
            root_plan.include_roots = vec![root.clone()];
            let root_resume = if plan.include_roots.len() == 1 {
                resume.clone()
            } else {
                None
            };
            let report = if capability.selected_source == IndexSourceKind::WindowsNtfsWin32 {
                scan_windows_root(
                    root_plan,
                    root_resume,
                    is_cancelled,
                    on_batch,
                    on_progress,
                    on_directory,
                )?
            } else {
                self.generic.scan(
                    root_plan,
                    root_resume,
                    is_cancelled,
                    on_batch,
                    on_progress,
                    on_directory,
                )?
            };
            merge_report(&mut combined, report);
        }
        Ok(combined)
    }
}

fn merge_report(combined: &mut IndexReport, mut report: IndexReport) {
    combined.entries.append(&mut report.entries);
    combined.failures.append(&mut report.failures);
    combined.scan_events.append(&mut report.scan_events);
    combined.scan_stats.scanned = combined
        .scan_stats
        .scanned
        .saturating_add(report.scan_stats.scanned);
    combined.scan_stats.accepted = combined
        .scan_stats
        .accepted
        .saturating_add(report.scan_stats.accepted);
    combined.scan_stats.skipped = combined
        .scan_stats
        .skipped
        .saturating_add(report.scan_stats.skipped);
    combined.scan_stats.failures = combined
        .scan_stats
        .failures
        .saturating_add(report.scan_stats.failures);
}

#[cfg(not(target_os = "windows"))]
fn probe_windows_ntfs(root: &Path) -> WindowsNtfsCapability {
    WindowsNtfsCapability::generic(
        root,
        "当前平台不是 Windows，使用 Generic Scanner",
        MftUsnCapability::UnsupportedPlatform,
    )
}

#[cfg(not(target_os = "windows"))]
fn scan_windows_root(
    _plan: IndexScanPlan,
    _resume: Option<IndexSourceResume>,
    _is_cancelled: &dyn Fn() -> bool,
    _on_batch: &mut IndexBatchCallback<'_>,
    _on_progress: &mut IndexProgressCallback<'_>,
    _on_directory: &mut IndexDirectoryCallback<'_>,
) -> Result<IndexReport, IndexSourceError> {
    Err(IndexSourceError::Io(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Win32 enumeration is unavailable on this platform",
    )))
}

#[cfg(target_os = "windows")]
mod windows {
    use super::*;
    use crate::core::index_entry::{
        build_search_text, IndexFailure, IndexedEntry, IndexedEntryKind, ScanEvent,
    };
    use crate::core::index_scanner::{IndexDirectoryScanCheckpoint, IndexPathRules};
    use crate::core::index_source::normalize_windows_path_text;
    use std::collections::{HashSet, VecDeque};
    use std::ffi::{c_void, OsString};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::{io, mem, ptr};
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FindClose, FindExInfoBasic, FindExSearchNameMatch, FindFirstFileExW,
        FindNextFileW, GetDriveTypeW, GetVolumeInformationW, GetVolumePathNameW, WIN32_FIND_DATAW,
    };
    use windows_sys::Win32::System::IO::DeviceIoControl;

    const DRIVE_FIXED: u32 = 3;
    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    const FILE_SHARE_READ: u32 = 0x1;
    const FILE_SHARE_WRITE: u32 = 0x2;
    const FILE_SHARE_DELETE: u32 = 0x4;
    const OPEN_EXISTING: u32 = 3;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FIND_FIRST_EX_LARGE_FETCH: u32 = 0x2;
    const FSCTL_QUERY_USN_JOURNAL: u32 = 0x0009_00f4;
    const ERROR_NO_MORE_FILES: u32 = 18;
    const WINDOWS_TO_UNIX_EPOCH_100NS: u64 = 116_444_736_000_000_000;
    const STREAM_BATCH_SIZE: usize = 2_048;

    pub(super) fn probe_windows_ntfs(root: &Path) -> WindowsNtfsCapability {
        let root_text = root.to_string_lossy().into_owned();
        let Some(volume_root) = volume_root_for(root) else {
            return WindowsNtfsCapability::generic(
                root,
                "无法解析 Windows 卷根，使用 Generic Scanner",
                MftUsnCapability::RawVolumeUnavailable {
                    os_error: last_error_code(),
                },
            );
        };
        let fixed_drive =
            unsafe { GetDriveTypeW(wide_null(volume_root.as_os_str()).as_ptr()) } == DRIVE_FIXED;
        let file_system = volume_file_system(&volume_root);
        if !fixed_drive {
            return WindowsNtfsCapability {
                root: root_text,
                fixed_drive,
                file_system,
                selected_source: IndexSourceKind::Generic,
                mft_usn: MftUsnCapability::NotNtfs,
                reason: "网络盘、可移动盘或未知卷类型使用 Generic Scanner".to_owned(),
            };
        }
        if !file_system
            .as_deref()
            .is_some_and(|name| name.eq_ignore_ascii_case("NTFS"))
        {
            return WindowsNtfsCapability {
                root: root_text,
                fixed_drive,
                file_system,
                selected_source: IndexSourceKind::Generic,
                mft_usn: MftUsnCapability::NotNtfs,
                reason: "非 NTFS 固定卷使用 Generic Scanner".to_owned(),
            };
        }

        let mft_usn = probe_usn_journal(&volume_root);
        let reason = match &mft_usn {
            MftUsnCapability::JournalReadableEnumerationDisabled => {
                "USN Journal 可读；当前版本仍使用无服务 Win32 批量枚举，禁止静默提权"
            }
            MftUsnCapability::RawVolumeUnavailable { .. }
            | MftUsnCapability::JournalUnavailable { .. } => {
                "标准用户无法可靠读取 MFT/USN；使用无服务 Win32 批量枚举"
            }
            _ => "使用无服务 Win32 批量枚举",
        };
        WindowsNtfsCapability {
            root: root_text,
            fixed_drive,
            file_system,
            selected_source: IndexSourceKind::WindowsNtfsWin32,
            mft_usn,
            reason: reason.to_owned(),
        }
    }

    fn volume_root_for(path: &Path) -> Option<PathBuf> {
        let path = wide_null(path.as_os_str());
        let mut buffer = vec![0u16; 261];
        let success =
            unsafe { GetVolumePathNameW(path.as_ptr(), buffer.as_mut_ptr(), buffer.len() as u32) };
        if success == 0 {
            return None;
        }
        Some(PathBuf::from(OsString::from_wide(nul_terminated_slice(
            &buffer,
        ))))
    }

    fn volume_file_system(volume_root: &Path) -> Option<String> {
        let root = wide_null(volume_root.as_os_str());
        let mut file_system = vec![0u16; 64];
        let success = unsafe {
            GetVolumeInformationW(
                root.as_ptr(),
                ptr::null_mut(),
                0,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                file_system.as_mut_ptr(),
                file_system.len() as u32,
            )
        };
        (success != 0).then(|| {
            OsString::from_wide(nul_terminated_slice(&file_system))
                .to_string_lossy()
                .into_owned()
        })
    }

    fn probe_usn_journal(volume_root: &Path) -> MftUsnCapability {
        let Some(device_path) = volume_device_path(volume_root) else {
            return MftUsnCapability::RawVolumeUnavailable { os_error: None };
        };
        let device_path = wide_null(OsString::from(device_path).as_os_str());
        let handle = unsafe {
            CreateFileW(
                device_path.as_ptr(),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return MftUsnCapability::RawVolumeUnavailable {
                os_error: last_error_code(),
            };
        }

        let mut journal_data = [0u8; 128];
        let mut returned = 0u32;
        let success = unsafe {
            DeviceIoControl(
                handle,
                FSCTL_QUERY_USN_JOURNAL,
                ptr::null(),
                0,
                journal_data.as_mut_ptr().cast::<c_void>(),
                journal_data.len() as u32,
                &mut returned,
                ptr::null_mut(),
            )
        };
        let error = (success == 0).then(last_error_code).flatten();
        unsafe {
            CloseHandle(handle);
        }
        if success == 0 {
            MftUsnCapability::JournalUnavailable { os_error: error }
        } else {
            MftUsnCapability::JournalReadableEnumerationDisabled
        }
    }

    fn volume_device_path(volume_root: &Path) -> Option<String> {
        let text = volume_root.to_string_lossy();
        let drive = text.get(..2)?;
        (drive.as_bytes().get(1) == Some(&b':')).then(|| format!(r"\\.\{drive}"))
    }

    pub(super) fn scan_windows_root(
        plan: IndexScanPlan,
        resume: Option<IndexSourceResume>,
        is_cancelled: &dyn Fn() -> bool,
        on_batch: &mut IndexBatchCallback<'_>,
        on_progress: &mut IndexProgressCallback<'_>,
        on_directory: &mut IndexDirectoryCallback<'_>,
    ) -> Result<IndexReport, IndexSourceError> {
        let rules = IndexPathRules::from_plan(&plan)?;
        let Some(root) = rules.roots.first().cloned() else {
            return Ok(IndexReport::default());
        };
        let stage = plan.stage.as_ref().map(|stage| stage.name.clone());
        let root_text = root.to_string_lossy().into_owned();
        let resume = resume.unwrap_or_else(|| IndexSourceResume::fresh(root.clone()));
        let mut report = IndexReport {
            scan_stats: resume.completed_stats,
            ..IndexReport::default()
        };
        report.scan_events.push(ScanEvent::RootStarted {
            root: root_text.clone(),
            stage: stage.clone(),
        });
        on_progress(IndexSourceProgress::new(
            IndexSourceKind::WindowsNtfsWin32,
            IndexSourcePhase::Enumerating,
            Some(&root),
            report.scan_stats.clone(),
        ))?;

        let mut directories = VecDeque::from(resume.pending_directories);
        let mut queued: HashSet<_> = directories
            .iter()
            .map(|path| normalize_windows_path_text(&path.to_string_lossy()))
            .collect();
        let mut batch = Vec::with_capacity(STREAM_BATCH_SIZE);
        while let Some(directory) = directories.pop_front() {
            if is_cancelled() {
                flush_batch(&mut batch, &report.scan_stats, on_batch)?;
                return Err(IndexSourceError::Cancelled);
            }
            queued.remove(&normalize_windows_path_text(&directory.to_string_lossy()));
            let mut directory_stats = IndexScanStats::default();
            let mut discovered_directories = Vec::new();
            let pattern = directory.join("*");
            let pattern = wide_null(pattern.as_os_str());
            let mut find_data: WIN32_FIND_DATAW = unsafe { mem::zeroed() };
            let handle = unsafe {
                FindFirstFileExW(
                    pattern.as_ptr(),
                    FindExInfoBasic,
                    (&mut find_data as *mut WIN32_FIND_DATAW).cast::<c_void>(),
                    FindExSearchNameMatch,
                    ptr::null(),
                    FIND_FIRST_EX_LARGE_FETCH,
                )
            };
            if handle == INVALID_HANDLE_VALUE {
                let failure = push_failure(&mut report, &directory, io::Error::last_os_error());
                directory_stats.failures = 1;
                on_directory(&IndexDirectoryScanCheckpoint {
                    root: root.clone(),
                    directory,
                    discovered_directories,
                    stats: directory_stats,
                    failure: Some(failure),
                })?;
                continue;
            }
            let find_handle = FindHandle(handle);
            let mut directory_failure = None;
            loop {
                if is_cancelled() {
                    flush_batch(&mut batch, &report.scan_stats, on_batch)?;
                    return Err(IndexSourceError::Cancelled);
                }
                let name = OsString::from_wide(nul_terminated_slice(&find_data.cFileName));
                if name != "." && name != ".." {
                    let path = directory.join(&name);
                    report.scan_stats.scanned = report.scan_stats.scanned.saturating_add(1);
                    directory_stats.scanned = directory_stats.scanned.saturating_add(1);
                    if rules.is_excluded(&path) {
                        report.scan_stats.skipped = report.scan_stats.skipped.saturating_add(1);
                        directory_stats.skipped = directory_stats.skipped.saturating_add(1);
                    } else {
                        let attributes = find_data.dwFileAttributes;
                        let is_directory = attributes & FILE_ATTRIBUTE_DIRECTORY != 0;
                        let is_reparse = attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0;
                        let kind = if is_application_path(&path) {
                            IndexedEntryKind::Application
                        } else if is_directory {
                            IndexedEntryKind::Directory
                        } else {
                            IndexedEntryKind::File
                        };
                        batch.push(indexed_entry_from_find_data(&path, &root, kind, &find_data));
                        report.scan_stats.accepted = report.scan_stats.accepted.saturating_add(1);
                        directory_stats.accepted = directory_stats.accepted.saturating_add(1);
                        if is_directory && !is_reparse && !is_application_path(&path) {
                            discovered_directories.push(path);
                        }
                        if batch.len() >= STREAM_BATCH_SIZE {
                            flush_batch(&mut batch, &report.scan_stats, on_batch)?;
                            on_progress(IndexSourceProgress::new(
                                IndexSourceKind::WindowsNtfsWin32,
                                IndexSourcePhase::Enumerating,
                                Some(&root),
                                report.scan_stats.clone(),
                            ))?;
                        }
                    }
                }
                if unsafe { FindNextFileW(find_handle.0, &mut find_data) } == 0 {
                    let error_code = unsafe { GetLastError() };
                    if error_code != ERROR_NO_MORE_FILES {
                        let failure = push_failure(
                            &mut report,
                            &directory,
                            io::Error::from_raw_os_error(error_code as i32),
                        );
                        directory_stats.failures = directory_stats.failures.saturating_add(1);
                        directory_failure = Some(failure);
                    }
                    break;
                }
            }
            if directory_failure.is_some() {
                discovered_directories.clear();
            } else {
                discovered_directories
                    .sort_by_key(|path| normalize_windows_path_text(&path.to_string_lossy()));
                discovered_directories.dedup_by(|left, right| {
                    normalize_windows_path_text(&left.to_string_lossy())
                        == normalize_windows_path_text(&right.to_string_lossy())
                });
            }
            on_directory(&IndexDirectoryScanCheckpoint {
                root: root.clone(),
                directory: directory.clone(),
                discovered_directories: discovered_directories.clone(),
                stats: directory_stats,
                failure: directory_failure,
            })?;
            for discovered in discovered_directories {
                let key = normalize_windows_path_text(&discovered.to_string_lossy());
                if queued.insert(key) {
                    directories.push_back(discovered);
                }
            }
        }
        flush_batch(&mut batch, &report.scan_stats, on_batch)?;
        report.scan_events.push(ScanEvent::RootFinished {
            root: root_text,
            stage,
            stats: report.scan_stats.clone(),
        });
        on_progress(IndexSourceProgress::new(
            IndexSourceKind::WindowsNtfsWin32,
            IndexSourcePhase::Completed,
            Some(&root),
            report.scan_stats.clone(),
        ))?;
        Ok(report)
    }

    struct FindHandle(HANDLE);

    impl Drop for FindHandle {
        fn drop(&mut self) {
            unsafe {
                FindClose(self.0);
            }
        }
    }

    fn flush_batch(
        batch: &mut Vec<IndexedEntry>,
        stats: &IndexScanStats,
        on_batch: &mut IndexBatchCallback<'_>,
    ) -> io::Result<()> {
        if !batch.is_empty() {
            on_batch(batch, stats)?;
            batch.clear();
        }
        Ok(())
    }

    fn indexed_entry_from_find_data(
        path: &Path,
        root: &Path,
        kind: IndexedEntryKind,
        data: &WIN32_FIND_DATAW,
    ) -> IndexedEntry {
        let path_text = path.to_string_lossy().into_owned();
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path_text.clone());
        let size = ((data.nFileSizeHigh as u64) << 32) | data.nFileSizeLow as u64;
        let ticks = ((data.ftLastWriteTime.dwHighDateTime as u64) << 32)
            | data.ftLastWriteTime.dwLowDateTime as u64;
        IndexedEntry {
            parent: path
                .parent()
                .map(|parent| parent.to_string_lossy().into_owned())
                .unwrap_or_default(),
            extension: path
                .extension()
                .and_then(|extension| extension.to_str())
                .map(str::to_ascii_lowercase),
            depth: path
                .strip_prefix(root)
                .map(|relative| relative.components().count())
                .unwrap_or_else(|_| path.components().count()),
            root: root.to_string_lossy().into_owned(),
            modified_ms: ticks
                .checked_sub(WINDOWS_TO_UNIX_EPOCH_100NS)
                .map(|ticks| (ticks / 10_000) as i64),
            size_bytes: (kind == IndexedEntryKind::File).then_some(size),
            search_text: build_search_text(&name, &path_text),
            path: path_text,
            name,
            kind,
            content_index_state: Default::default(),
        }
    }

    fn push_failure(report: &mut IndexReport, path: &Path, error: io::Error) -> IndexFailure {
        let failure = IndexFailure {
            root: path.to_string_lossy().into_owned(),
            message: error.to_string(),
        };
        report.scan_stats.failures = report.scan_stats.failures.saturating_add(1);
        report.scan_events.push(ScanEvent::Failure(failure.clone()));
        report.failures.push(failure.clone());
        failure
    }

    fn is_application_path(path: &Path) -> bool {
        path.file_name()
            .and_then(|name| name.to_str())
            .map(str::to_ascii_lowercase)
            .is_some_and(|name| {
                name.ends_with(".exe") || name.ends_with(".lnk") || name.ends_with(".desktop")
            })
    }

    fn wide_null(value: &std::ffi::OsStr) -> Vec<u16> {
        value.encode_wide().chain(std::iter::once(0)).collect()
    }

    fn nul_terminated_slice(buffer: &[u16]) -> &[u16] {
        &buffer[..buffer
            .iter()
            .position(|character| *character == 0)
            .unwrap_or(buffer.len())]
    }

    fn last_error_code() -> Option<u32> {
        let code = unsafe { GetLastError() };
        (code != 0).then_some(code)
    }
}

#[cfg(target_os = "windows")]
use windows::{probe_windows_ntfs, scan_windows_root};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::index_source::IndexSourcePhase;
    use std::fs;

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn non_windows_probe_selects_generic_without_claiming_ntfs_support() {
        let capability = WindowsNtfsIndexSource::default().probe(Path::new("/tmp"));
        assert_eq!(capability.selected_source, IndexSourceKind::Generic);
        assert_eq!(capability.mft_usn, MftUsnCapability::UnsupportedPlatform);
    }

    #[test]
    fn source_falls_back_to_generic_and_preserves_streaming_contract() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("file.txt"), "content").unwrap();
        let mut paths = Vec::new();
        let mut phases = Vec::new();
        let result = WindowsNtfsIndexSource::default().scan(
            IndexScanPlan {
                include_roots: vec![root.path().to_path_buf()],
                respect_project_ignores: true,
                ..IndexScanPlan::default()
            },
            None,
            &|| false,
            &mut |entries, _| {
                paths.extend(entries.iter().map(|entry| entry.path.clone()));
                Ok(())
            },
            &mut |progress| {
                phases.push((progress.source, progress.phase));
                Ok(())
            },
            &mut |_| Ok(()),
        );

        assert_eq!(result.unwrap().scan_stats.accepted, 1);
        assert_eq!(paths.len(), 1);
        assert!(phases.contains(&(IndexSourceKind::Generic, IndexSourcePhase::Completed)));
    }

    #[test]
    fn selection_compresses_windows_covered_roots_before_probing() {
        let selection = WindowsNtfsIndexSource::default().selection_for(&IndexScanPlan {
            include_roots: vec![PathBuf::from(r"C:\Users\a"), PathBuf::from(r"c:\")],
            respect_project_ignores: true,
            ..IndexScanPlan::default()
        });
        assert_eq!(selection.len(), 1);
        assert_eq!(selection[0].root.to_ascii_lowercase(), r"c:\");
    }
}
