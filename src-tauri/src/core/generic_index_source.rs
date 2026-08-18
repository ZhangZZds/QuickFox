//! Portable [`IndexSource`] backed by the existing ignore-aware filesystem walker.

use crate::core::index_entry::{IndexReport, IndexScanStats};
use crate::core::index_scanner::{IgnoreScanner, IndexScanPlan};
use crate::core::index_source::{
    compress_covered_roots, IndexBatchCallback, IndexDirectoryCallback, IndexProgressCallback,
    IndexSource, IndexSourceError, IndexSourceKind, IndexSourcePhase, IndexSourceProgress,
    IndexSourceResume,
};

#[derive(Debug, Clone, Default)]
pub struct GenericIndexSource {
    scanner: IgnoreScanner,
}

impl GenericIndexSource {
    pub fn with_scanner(scanner: IgnoreScanner) -> Self {
        Self { scanner }
    }
}

impl IndexSource for GenericIndexSource {
    fn kind(&self) -> IndexSourceKind {
        IndexSourceKind::Generic
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
        if is_cancelled() {
            return Err(IndexSourceError::Cancelled);
        }
        plan.include_roots = compress_covered_roots(&plan.include_roots);
        if resume.is_some() && (plan.respect_project_ignores || plan.include_roots.len() != 1) {
            return Err(IndexSourceError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "generic resume requires exactly one root and project ignore handling disabled",
            )));
        }
        let current_root = plan.include_roots.first().cloned();
        on_progress(IndexSourceProgress::new(
            self.kind(),
            IndexSourcePhase::Enumerating,
            current_root.as_deref(),
            IndexScanStats::default(),
        ))?;

        let stream_batch = |entries: &[crate::core::index_entry::IndexedEntry],
                            stats: &IndexScanStats| {
            on_batch(entries, stats)?;
            on_progress(IndexSourceProgress::new(
                IndexSourceKind::Generic,
                IndexSourcePhase::Enumerating,
                current_root.as_deref(),
                stats.clone(),
            ))
        };
        let report = if !plan.respect_project_ignores && plan.include_roots.len() == 1 {
            let resume = resume.unwrap_or_else(|| {
                IndexSourceResume::fresh(
                    current_root
                        .clone()
                        .expect("single-root generic plan must contain a root"),
                )
            });
            self.scanner.scan_resumable_cancellable_streaming(
                plan,
                resume.pending_directories,
                resume.completed_stats,
                is_cancelled,
                stream_batch,
                on_directory,
            )?
        } else {
            self.scanner
                .scan_cancellable_streaming(plan, is_cancelled, stream_batch)?
        };
        if is_cancelled() {
            return Err(IndexSourceError::Cancelled);
        }
        on_progress(IndexSourceProgress::new(
            self.kind(),
            IndexSourcePhase::Completed,
            current_root.as_deref(),
            report.scan_stats.clone(),
        ))?;
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::index_source::IndexSourcePhase;
    use std::fs;

    #[test]
    fn generic_source_streams_entries_and_progress() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("a.txt"), "a").unwrap();
        let mut paths = Vec::new();
        let mut phases = Vec::new();

        let report = GenericIndexSource::default()
            .scan(
                IndexScanPlan {
                    include_roots: vec![root.path().to_path_buf()],
                    ..IndexScanPlan::default()
                },
                None,
                &|| false,
                &mut |entries, _| {
                    paths.extend(entries.iter().map(|entry| entry.path.clone()));
                    Ok(())
                },
                &mut |progress| {
                    phases.push(progress.phase);
                    Ok(())
                },
                &mut |_| Ok(()),
            )
            .unwrap();

        assert_eq!(report.scan_stats.accepted, 1);
        assert!(report.entries.is_empty());
        assert_eq!(paths.len(), 1);
        assert_eq!(phases.first(), Some(&IndexSourcePhase::Enumerating));
        assert_eq!(phases.last(), Some(&IndexSourcePhase::Completed));
    }

    #[test]
    fn generic_source_reports_cancellation_explicitly() {
        let result = GenericIndexSource::default().scan(
            IndexScanPlan::default(),
            None,
            &|| true,
            &mut |_, _| Ok(()),
            &mut |_| Ok(()),
            &mut |_| Ok(()),
        );
        assert!(matches!(result, Err(IndexSourceError::Cancelled)));
    }

    #[test]
    fn generic_source_does_not_checkpoint_an_interrupted_directory() {
        let root = tempfile::tempdir().unwrap();
        for index in 0..128 {
            fs::write(root.path().join(format!("file-{index}.txt")), "a").unwrap();
        }
        let checks = std::cell::Cell::new(0usize);
        let mut checkpoints = Vec::new();
        let result = GenericIndexSource::default().scan(
            IndexScanPlan {
                include_roots: vec![root.path().to_path_buf()],
                respect_project_ignores: false,
                ..IndexScanPlan::default()
            },
            Some(IndexSourceResume::fresh(root.path().to_path_buf())),
            &|| {
                checks.set(checks.get().saturating_add(1));
                checks.get() > 12
            },
            &mut |_, _| Ok(()),
            &mut |_| Ok(()),
            &mut |checkpoint| {
                checkpoints.push(checkpoint.directory.clone());
                Ok(())
            },
        );

        assert!(matches!(result, Err(IndexSourceError::Cancelled)));
        assert!(checkpoints.is_empty());
    }

    #[test]
    fn generic_source_forwards_directory_checkpoints_for_resumable_plans() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("child")).unwrap();
        fs::write(root.path().join("child").join("a.txt"), "a").unwrap();
        let mut completed = Vec::new();

        GenericIndexSource::default()
            .scan(
                IndexScanPlan {
                    include_roots: vec![root.path().to_path_buf()],
                    respect_project_ignores: false,
                    ..IndexScanPlan::default()
                },
                Some(IndexSourceResume::fresh(root.path().to_path_buf())),
                &|| false,
                &mut |_, _| Ok(()),
                &mut |_| Ok(()),
                &mut |checkpoint| {
                    completed.push(checkpoint.directory.clone());
                    Ok(())
                },
            )
            .unwrap();

        assert!(completed.contains(&root.path().to_path_buf()));
        assert!(completed.contains(&root.path().join("child")));
    }
}
