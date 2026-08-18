use quickfox_lib::core::index_scanner::IndexScanPlan;
use quickfox_lib::core::index_source::{IndexSource, IndexSourceProgress};
use quickfox_lib::core::windows_ntfs_index_source::WindowsNtfsIndexSource;
use serde::Serialize;
use std::path::PathBuf;
use std::time::Instant;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BenchmarkResult {
    root: String,
    capability: quickfox_lib::core::windows_ntfs_index_source::WindowsNtfsCapability,
    elapsed_ms: u128,
    entries: usize,
    batches: usize,
    failures: usize,
    last_progress: Option<IndexSourceProgress>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::args_os().nth(1).map(PathBuf::from).ok_or(
        "usage: index-source-benchmark <root> (run on a real Windows NTFS volume in release mode)",
    )?;
    let source = WindowsNtfsIndexSource::default();
    let capability = source.probe(&root);
    let mut entries = 0usize;
    let mut batches = 0usize;
    let mut last_progress = None;
    let started = Instant::now();
    let report = source.scan(
        IndexScanPlan {
            include_roots: vec![root.clone()],
            respect_project_ignores: false,
            ..IndexScanPlan::default()
        },
        None,
        &|| false,
        &mut |batch, _| {
            entries = entries.saturating_add(batch.len());
            batches = batches.saturating_add(1);
            Ok(())
        },
        &mut |progress| {
            last_progress = Some(progress);
            Ok(())
        },
        &mut |_| Ok(()),
    )?;
    let result = BenchmarkResult {
        root: root.to_string_lossy().into_owned(),
        capability,
        elapsed_ms: started.elapsed().as_millis(),
        entries,
        batches,
        failures: report.failures.len(),
        last_progress,
    };
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}
