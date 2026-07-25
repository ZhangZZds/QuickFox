//! Persistent incremental-index journal and recovery boundary.

use crate::core::index::IndexedEntry;
pub use crate::core::index_entry::IndexDegradationCode;
use crate::core::index_entry::{normalize_path_key, normalize_path_text_key};
use crate::core::layered_index::{CommittedIndexDelta, LayeredSearchIndex};
use crate::core::storage::{IncrementalRecoveryBaseline, IncrementalRuntimeState, SqliteStorage};
use crate::core::targeted_index_scanner::{
    DirectoryFingerprint, DirectoryManifestReader, KnownDirectoryEntriesReader, KnownIndexedChild,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IndexFallbackReason {
    JournalRecoveryFailed,
    FullRefreshFallback,
}

#[derive(Debug)]
pub struct IndexRecovery {
    pub index: LayeredSearchIndex,
    pub degradation: Option<IndexDegradationCode>,
    pub fallback_reason: Option<IndexFallbackReason>,
    baseline_entry_count: usize,
    baseline_available: bool,
    manifest_ready: bool,
    needs_manifest_rebuild: bool,
    manifest_roots: std::collections::BTreeSet<String>,
}

impl IndexRecovery {
    pub fn baseline_entry_count(&self) -> usize {
        self.baseline_entry_count
    }

    pub fn baseline_available(&self) -> bool {
        self.baseline_available
    }

    pub fn degradation_code(&self) -> Option<IndexDegradationCode> {
        self.degradation
    }

    pub fn fallback_reason(&self) -> Option<IndexFallbackReason> {
        self.fallback_reason
    }

    pub fn manifest_ready(&self) -> bool {
        self.manifest_ready
    }

    pub fn needs_manifest_rebuild(&self) -> bool {
        self.needs_manifest_rebuild
    }

    pub fn manifest_covers_roots(&self, roots: &[PathBuf]) -> bool {
        self.manifest_ready
            && roots
                .iter()
                .all(|root| self.manifest_roots.contains(&normalize_path_key(root)))
    }
}

pub trait IndexJournalRepository {
    fn incremental_schema_is_ready(&self) -> Result<bool, String>;

    fn incremental_recovery_baseline(&self) -> Result<IncrementalRecoveryBaseline, String>;

    fn latest_completed_recovery_baseline(&self) -> Result<IncrementalRecoveryBaseline, String>;

    fn validate_directory_manifest(&self) -> Result<(), String>;

    fn directory_manifest_roots(&self) -> Result<Vec<PathBuf>, String>;

    fn commit_incremental_batch(
        &mut self,
        delta: &CommittedIndexDelta,
        manifest_upserts: &[DirectoryFingerprint],
        manifest_removals: &[PathBuf],
    ) -> Result<(), String>;

    fn committed_index_deltas_after(
        &self,
        generation: u64,
    ) -> Result<Vec<CommittedIndexDelta>, String>;

    fn highest_committed_generation(&self) -> Result<u64, String>;

    fn replace_directory_manifest(
        &mut self,
        root: &Path,
        rows: &[DirectoryFingerprint],
    ) -> Result<(), String>;

    fn directory_manifest_for_root(&self, root: &Path)
        -> Result<Vec<DirectoryFingerprint>, String>;

    fn known_direct_indexed_children(
        &self,
        root: &Path,
        directory: &Path,
    ) -> Result<Vec<KnownIndexedChild>, String>;

    fn clear_incremental_state_through(&mut self, generation: u64) -> Result<(), String>;

    fn activate_baseline_and_clear_incremental_state(
        &mut self,
        baseline_id: i64,
        baseline_generation: u64,
    ) -> Result<(), String>;

    fn runtime_state(&self) -> Result<Option<IncrementalRuntimeState>, String>;

    fn save_runtime_state(&mut self, state: &IncrementalRuntimeState) -> Result<(), String>;
}

impl IndexJournalRepository for SqliteStorage {
    fn incremental_schema_is_ready(&self) -> Result<bool, String> {
        SqliteStorage::incremental_schema_is_ready(self).map_err(|error| error.to_string())
    }

    fn incremental_recovery_baseline(&self) -> Result<IncrementalRecoveryBaseline, String> {
        SqliteStorage::incremental_recovery_baseline(self).map_err(|error| error.to_string())
    }

    fn latest_completed_recovery_baseline(&self) -> Result<IncrementalRecoveryBaseline, String> {
        SqliteStorage::latest_completed_recovery_baseline(self).map_err(|error| error.to_string())
    }

    fn validate_directory_manifest(&self) -> Result<(), String> {
        SqliteStorage::validate_directory_manifest(self).map_err(|error| error.to_string())
    }

    fn directory_manifest_roots(&self) -> Result<Vec<PathBuf>, String> {
        SqliteStorage::directory_manifest_roots(self).map_err(|error| error.to_string())
    }

    fn commit_incremental_batch(
        &mut self,
        delta: &CommittedIndexDelta,
        manifest_upserts: &[DirectoryFingerprint],
        manifest_removals: &[PathBuf],
    ) -> Result<(), String> {
        SqliteStorage::commit_incremental_batch(self, delta, manifest_upserts, manifest_removals)
            .map_err(|error| error.to_string())
    }

    fn committed_index_deltas_after(
        &self,
        generation: u64,
    ) -> Result<Vec<CommittedIndexDelta>, String> {
        SqliteStorage::committed_index_deltas_after(self, generation)
            .map_err(|error| error.to_string())
    }

    fn highest_committed_generation(&self) -> Result<u64, String> {
        SqliteStorage::highest_committed_generation(self).map_err(|error| error.to_string())
    }

    fn replace_directory_manifest(
        &mut self,
        root: &Path,
        rows: &[DirectoryFingerprint],
    ) -> Result<(), String> {
        SqliteStorage::replace_directory_manifest(self, root, rows)
            .map_err(|error| error.to_string())
    }

    fn directory_manifest_for_root(
        &self,
        root: &Path,
    ) -> Result<Vec<DirectoryFingerprint>, String> {
        SqliteStorage::directory_manifest_for_root(self, root).map_err(|error| error.to_string())
    }

    fn known_direct_indexed_children(
        &self,
        root: &Path,
        directory: &Path,
    ) -> Result<Vec<KnownIndexedChild>, String> {
        SqliteStorage::known_direct_indexed_children(self, root, directory)
            .map_err(|error| error.to_string())
    }

    fn clear_incremental_state_through(&mut self, generation: u64) -> Result<(), String> {
        SqliteStorage::clear_incremental_state_through(self, generation)
            .map_err(|error| error.to_string())
    }

    fn activate_baseline_and_clear_incremental_state(
        &mut self,
        baseline_id: i64,
        baseline_generation: u64,
    ) -> Result<(), String> {
        SqliteStorage::activate_baseline_and_clear_incremental_state(
            self,
            baseline_id,
            baseline_generation,
        )
        .map_err(|error| error.to_string())
    }

    fn runtime_state(&self) -> Result<Option<IncrementalRuntimeState>, String> {
        SqliteStorage::runtime_state(self).map_err(|error| error.to_string())
    }

    fn save_runtime_state(&mut self, state: &IncrementalRuntimeState) -> Result<(), String> {
        SqliteStorage::save_runtime_state(self, state).map_err(|error| error.to_string())
    }
}

impl DirectoryManifestReader for SqliteStorage {
    fn directories_for_root(&self, root: &Path) -> Result<Vec<DirectoryFingerprint>, String> {
        SqliteStorage::directory_manifest_for_root(self, root).map_err(|error| error.to_string())
    }
}

impl KnownDirectoryEntriesReader for SqliteStorage {
    fn entries_for_directory(
        &self,
        root: &Path,
        directory: &Path,
    ) -> Result<Vec<KnownIndexedChild>, String> {
        SqliteStorage::known_direct_indexed_children(self, root, directory)
            .map_err(|error| error.to_string())
    }
}

pub fn replay_deltas(
    mut index: LayeredSearchIndex,
    deltas: &[CommittedIndexDelta],
) -> Result<LayeredSearchIndex, String> {
    let deltas = ordered_unique_deltas(deltas)?;
    for delta in deltas {
        index.apply_delta(delta);
    }
    Ok(index)
}

pub fn recover_layered_index(repository: &(impl IndexJournalRepository + ?Sized)) -> IndexRecovery {
    match repository.incremental_schema_is_ready() {
        Ok(true) => {}
        Ok(false) | Err(_) => return latest_baseline_fallback(repository),
    }
    let baseline = match repository.incremental_recovery_baseline() {
        Ok(baseline) => baseline,
        Err(_) => return latest_baseline_fallback(repository),
    };
    if !baseline.available {
        return unavailable_recovery();
    }
    let requires_full_refresh = baseline.requires_full_refresh;
    let baseline_generation = baseline.generation;
    let baseline = baseline.entries;
    let baseline_roots: std::collections::BTreeSet<PathBuf> = baseline
        .iter()
        .filter(|entry| !entry.root.is_empty())
        .map(|entry| PathBuf::from(&entry.root))
        .collect();
    let baseline_entry_count = baseline.len();
    if requires_full_refresh {
        return baseline_only_recovery(
            baseline,
            baseline_generation,
            baseline_entry_count,
            true,
            IndexDegradationCode::JournalReplayFailed,
            IndexFallbackReason::FullRefreshFallback,
        );
    }
    let manifest_roots = match repository.directory_manifest_roots() {
        Ok(roots) => roots,
        Err(_) => {
            return failed_manifest_recovery(baseline, baseline_generation, baseline_entry_count);
        }
    };
    if repository.validate_directory_manifest().is_err() {
        return failed_manifest_recovery(baseline, baseline_generation, baseline_entry_count);
    }
    let manifest_root_keys: std::collections::BTreeSet<String> =
        manifest_roots.iter().map(normalize_path_key).collect();
    let manifest_ready = !manifest_root_keys.is_empty()
        && baseline_roots.iter().all(|root| {
            repository
                .directory_manifest_for_root(root)
                .is_ok_and(|rows| !rows.is_empty())
        });
    let deltas = match repository.committed_index_deltas_after(baseline_generation) {
        Ok(deltas) => deltas,
        Err(_) => {
            return failed_recovery(baseline, baseline_generation, baseline_entry_count, true);
        }
    };
    let deltas = match ordered_unique_deltas(&deltas) {
        Ok(deltas) => deltas,
        Err(_) => {
            return failed_recovery(baseline, baseline_generation, baseline_entry_count, true);
        }
    };
    let mut index = LayeredSearchIndex::default();
    index.replace_baseline(baseline, baseline_generation);
    for delta in deltas {
        index.apply_delta(delta);
    }
    IndexRecovery {
        index,
        degradation: None,
        fallback_reason: None,
        baseline_entry_count,
        baseline_available: true,
        manifest_ready,
        needs_manifest_rebuild: !manifest_ready,
        manifest_roots: manifest_root_keys,
    }
}

fn latest_baseline_fallback(repository: &(impl IndexJournalRepository + ?Sized)) -> IndexRecovery {
    let baseline = match repository.latest_completed_recovery_baseline() {
        Ok(baseline) => baseline,
        Err(_) => return failed_recovery(Vec::new(), 0, 0, false),
    };
    let baseline_entry_count = baseline.entries.len();
    failed_recovery(
        baseline.entries,
        baseline.generation,
        baseline_entry_count,
        baseline.available,
    )
}

fn unavailable_recovery() -> IndexRecovery {
    IndexRecovery {
        index: LayeredSearchIndex::default(),
        degradation: None,
        fallback_reason: None,
        baseline_entry_count: 0,
        baseline_available: false,
        manifest_ready: false,
        needs_manifest_rebuild: false,
        manifest_roots: std::collections::BTreeSet::new(),
    }
}

fn failed_recovery(
    baseline: Vec<IndexedEntry>,
    baseline_generation: u64,
    baseline_entry_count: usize,
    baseline_available: bool,
) -> IndexRecovery {
    baseline_only_recovery(
        baseline,
        baseline_generation,
        baseline_entry_count,
        baseline_available,
        IndexDegradationCode::JournalReplayFailed,
        IndexFallbackReason::JournalRecoveryFailed,
    )
}

fn failed_manifest_recovery(
    baseline: Vec<IndexedEntry>,
    baseline_generation: u64,
    baseline_entry_count: usize,
) -> IndexRecovery {
    baseline_only_recovery(
        baseline,
        baseline_generation,
        baseline_entry_count,
        true,
        IndexDegradationCode::CalibrationFailed,
        IndexFallbackReason::FullRefreshFallback,
    )
}

fn baseline_only_recovery(
    baseline: Vec<IndexedEntry>,
    baseline_generation: u64,
    baseline_entry_count: usize,
    baseline_available: bool,
    degradation: IndexDegradationCode,
    fallback_reason: IndexFallbackReason,
) -> IndexRecovery {
    let mut index = LayeredSearchIndex::default();
    index.replace_baseline(baseline, baseline_generation);
    IndexRecovery {
        index,
        degradation: Some(degradation),
        fallback_reason: Some(fallback_reason),
        baseline_entry_count,
        baseline_available,
        manifest_ready: false,
        needs_manifest_rebuild: baseline_available,
        manifest_roots: std::collections::BTreeSet::new(),
    }
}

fn ordered_unique_deltas(
    deltas: &[CommittedIndexDelta],
) -> Result<Vec<CommittedIndexDelta>, String> {
    let mut by_generation = BTreeMap::new();
    for delta in deltas {
        validate_delta(delta)?;
        match by_generation.entry(delta.generation) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(delta.clone());
            }
            std::collections::btree_map::Entry::Occupied(entry) if entry.get() == delta => {}
            std::collections::btree_map::Entry::Occupied(_) => {
                return Err(format!(
                    "generation {} has conflicting journal batches",
                    delta.generation
                ));
            }
        }
    }
    Ok(by_generation.into_values().collect())
}

fn validate_delta(delta: &CommittedIndexDelta) -> Result<(), String> {
    let mut paths = BTreeMap::new();
    for entry in &delta.upserts {
        let path = normalize_path_text_key(&entry.path);
        if path.is_empty() || paths.insert(path, "upsert").is_some() {
            return Err(format!(
                "generation {} contains an empty or duplicate path",
                delta.generation
            ));
        }
    }
    for removal in &delta.removals {
        let path = normalize_path_key(removal);
        if path.is_empty() || paths.insert(path, "remove").is_some() {
            return Err(format!(
                "generation {} contains an empty or duplicate path",
                delta.generation
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::index::{IndexedEntry, IndexedEntryKind};
    use crate::core::search::QueryParser;
    use crate::core::storage::SqliteStorage;
    use rusqlite::{params, Connection};
    use std::cell::Cell;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    struct CountingRecoveryRepository {
        active_loads: Cell<usize>,
        latest_loads: Cell<usize>,
    }

    impl CountingRecoveryRepository {
        fn new() -> Self {
            Self {
                active_loads: Cell::new(0),
                latest_loads: Cell::new(0),
            }
        }
    }

    impl IndexJournalRepository for CountingRecoveryRepository {
        fn incremental_schema_is_ready(&self) -> Result<bool, String> {
            Ok(true)
        }

        fn incremental_recovery_baseline(&self) -> Result<IncrementalRecoveryBaseline, String> {
            self.active_loads.set(self.active_loads.get() + 1);
            Ok(IncrementalRecoveryBaseline {
                entries: vec![entry("/root/active.md")],
                generation: 0,
                available: true,
                requires_full_refresh: false,
            })
        }

        fn latest_completed_recovery_baseline(
            &self,
        ) -> Result<IncrementalRecoveryBaseline, String> {
            self.latest_loads.set(self.latest_loads.get() + 1);
            Ok(IncrementalRecoveryBaseline {
                entries: vec![entry("/root/latest.md")],
                generation: 0,
                available: true,
                requires_full_refresh: false,
            })
        }

        fn validate_directory_manifest(&self) -> Result<(), String> {
            Ok(())
        }

        fn directory_manifest_roots(&self) -> Result<Vec<PathBuf>, String> {
            Ok(vec![PathBuf::from("/root")])
        }

        fn commit_incremental_batch(
            &mut self,
            _delta: &CommittedIndexDelta,
            _manifest_upserts: &[DirectoryFingerprint],
            _manifest_removals: &[PathBuf],
        ) -> Result<(), String> {
            unreachable!("recovery does not commit")
        }

        fn committed_index_deltas_after(
            &self,
            _generation: u64,
        ) -> Result<Vec<CommittedIndexDelta>, String> {
            Ok(Vec::new())
        }

        fn highest_committed_generation(&self) -> Result<u64, String> {
            Ok(0)
        }

        fn replace_directory_manifest(
            &mut self,
            _root: &Path,
            _rows: &[DirectoryFingerprint],
        ) -> Result<(), String> {
            unreachable!("recovery does not replace manifests")
        }

        fn directory_manifest_for_root(
            &self,
            root: &Path,
        ) -> Result<Vec<DirectoryFingerprint>, String> {
            Ok(vec![DirectoryFingerprint {
                path: root.to_string_lossy().into_owned(),
                parent: None,
                root: root.to_string_lossy().into_owned(),
                modified_ms: None,
            }])
        }

        fn known_direct_indexed_children(
            &self,
            _root: &Path,
            _directory: &Path,
        ) -> Result<Vec<KnownIndexedChild>, String> {
            unreachable!("recovery does not read direct children")
        }

        fn clear_incremental_state_through(&mut self, _generation: u64) -> Result<(), String> {
            unreachable!("recovery does not clear journal state")
        }

        fn activate_baseline_and_clear_incremental_state(
            &mut self,
            _baseline_id: i64,
            _baseline_generation: u64,
        ) -> Result<(), String> {
            unreachable!("recovery does not activate baselines")
        }

        fn runtime_state(&self) -> Result<Option<IncrementalRuntimeState>, String> {
            unreachable!("recovery baseline loading owns runtime-state access")
        }

        fn save_runtime_state(&mut self, _state: &IncrementalRuntimeState) -> Result<(), String> {
            unreachable!("recovery does not save runtime state")
        }
    }

    #[test]
    fn ready_schema_loads_only_the_active_baseline() {
        let repository = CountingRecoveryRepository::new();

        let recovery = recover_layered_index(&repository);

        assert_eq!(repository.active_loads.get(), 1);
        assert_eq!(repository.latest_loads.get(), 0);
        assert_eq!(search_titles(&recovery.index, "active"), vec!["active.md"]);
        assert!(search_titles(&recovery.index, "latest").is_empty());
    }

    #[test]
    fn corrupt_unactivated_latest_snapshot_does_not_block_active_recovery() {
        let path = temp_db_path("corrupt-unactivated-latest");
        let storage = SqliteStorage::open(path.clone()).unwrap();
        let active_id = storage
            .save_completed_index_batch(10, &[entry("/root/active.md")])
            .unwrap();
        storage.activate_baseline(active_id, 0).unwrap();
        let latest_id = storage
            .save_completed_index_batch(20, &[entry("/root/unactivated.md")])
            .unwrap();
        drop(storage);
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "UPDATE index_entries SET depth = 'broken' WHERE batch_id = ?1",
                params![latest_id],
            )
            .unwrap();
        drop(connection);
        let storage = SqliteStorage::open(path.clone()).unwrap();

        let recovery = recover_layered_index(&storage);

        assert!(recovery.baseline_available());
        assert_eq!(recovery.baseline_entry_count(), 1);
        assert_eq!(search_titles(&recovery.index, "active"), vec!["active.md"]);
        assert!(search_titles(&recovery.index, "unactivated").is_empty());
        assert_eq!(recovery.degradation_code(), None);

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn committed_generation_replay_is_ordered_and_idempotent_by_path() {
        let batches = vec![
            delta(
                2,
                entry("/root/current.md"),
                vec![PathBuf::from("/root/old.md")],
            ),
            delta(1, entry("/root/old.md"), Vec::new()),
        ];

        let once = replay_deltas(LayeredSearchIndex::default(), &batches).unwrap();
        let once_count = once.entry_count();
        let once_titles = search_titles(&once, "current");
        let twice = replay_deltas(once, &batches).unwrap();

        assert_eq!(search_titles(&twice, "current"), vec!["current.md"]);
        assert_eq!(once_titles, vec!["current.md"]);
        assert!(search_titles(&twice, "old").is_empty());
        assert_eq!(twice.entry_count(), once_count);
        assert_eq!(twice.generation(), 2);
    }

    #[test]
    fn malformed_journal_returns_recovery_failure_without_deleting_baseline() {
        let path = temp_db_path("malformed-recovery");
        let storage = SqliteStorage::open(path.clone()).unwrap();
        drop(storage);
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "INSERT INTO index_delta_batches (generation, status, committed_at_ms, payload_hash) VALUES (1, 'committed', 1, '')",
                [],
            )
            .unwrap();
        let batch_id = connection.last_insert_rowid();
        connection
            .execute(
                "INSERT INTO index_delta_entries (batch_id, ordinal, operation, path, entry_json) VALUES (?1, 0, 'upsert', '/root/corrupt.md', '{')",
                params![batch_id],
            )
            .unwrap();
        drop(connection);
        let storage = SqliteStorage::open(path.clone()).unwrap();
        storage
            .save_completed_index_batch(1, &[entry("/root/baseline.md")])
            .unwrap();

        let recovery = recover_layered_index(&storage);

        assert_eq!(recovery.baseline_entry_count(), 1);
        assert_eq!(
            recovery.degradation_code(),
            Some(IndexDegradationCode::JournalReplayFailed)
        );
        assert_eq!(
            recovery.fallback_reason(),
            Some(IndexFallbackReason::JournalRecoveryFailed)
        );
        assert_eq!(
            search_titles(&recovery.index, "baseline"),
            vec!["baseline.md"]
        );

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn journal_without_completed_baseline_stays_unavailable_and_unreplayed() {
        let path = temp_db_path("journal-without-baseline");
        let storage = SqliteStorage::open(path.clone()).unwrap();
        storage
            .commit_incremental_batch(
                &delta(1, entry("/root/orphan-journal.md"), Vec::new()),
                &[],
                &[],
            )
            .unwrap();

        let recovery = recover_layered_index(&storage);

        assert!(!recovery.baseline_available());
        assert_eq!(recovery.baseline_entry_count(), 0);
        assert!(search_titles(&recovery.index, "orphan-journal").is_empty());
        assert_eq!(recovery.index.generation(), 0);

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn completed_empty_baseline_is_available_and_replays_journal() {
        let path = temp_db_path("completed-empty-baseline");
        let storage = SqliteStorage::open(path.clone()).unwrap();
        storage.save_completed_index_batch(10, &[]).unwrap();
        storage
            .commit_incremental_batch(
                &delta(1, entry("/root/from-journal.md"), Vec::new()),
                &[],
                &[],
            )
            .unwrap();

        let recovery = recover_layered_index(&storage);

        assert!(recovery.baseline_available());
        assert_eq!(recovery.baseline_entry_count(), 0);
        assert_eq!(
            search_titles(&recovery.index, "from-journal"),
            vec!["from-journal.md"]
        );
        assert_eq!(recovery.index.generation(), 1);

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn missing_active_baseline_falls_back_to_latest_completed_baseline_as_degraded() {
        let path = temp_db_path("missing-active-baseline");
        let storage = SqliteStorage::open(path.clone()).unwrap();
        storage
            .save_completed_index_batch(10, &[entry("/root/latest.md")])
            .unwrap();
        storage
            .save_runtime_state(&IncrementalRuntimeState {
                active_baseline_id: None,
                baseline_generation: 0,
                last_generation: 0,
                degradation_code: None,
                baseline_refresh_reason: None,
            })
            .unwrap();

        let recovery = recover_layered_index(&storage);

        assert!(recovery.baseline_available());
        assert_eq!(recovery.baseline_entry_count(), 1);
        assert_eq!(search_titles(&recovery.index, "latest"), vec!["latest.md"]);
        assert_eq!(
            recovery.degradation_code(),
            Some(IndexDegradationCode::JournalReplayFailed)
        );
        assert_eq!(
            recovery.fallback_reason(),
            Some(IndexFallbackReason::FullRefreshFallback)
        );

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn degradation_codes_have_stable_camel_case_contract() {
        let cases = [
            (
                IndexDegradationCode::WatcherInitializationFailed,
                "\"watcherInitializationFailed\"",
            ),
            (
                IndexDegradationCode::WatcherRuntimeFailed,
                "\"watcherRuntimeFailed\"",
            ),
            (IndexDegradationCode::WatcherOverflow, "\"watcherOverflow\""),
            (IndexDegradationCode::ChannelOverflow, "\"channelOverflow\""),
            (
                IndexDegradationCode::JournalWriteFailed,
                "\"journalWriteFailed\"",
            ),
            (
                IndexDegradationCode::JournalReplayFailed,
                "\"journalReplayFailed\"",
            ),
            (
                IndexDegradationCode::CalibrationFailed,
                "\"calibrationFailed\"",
            ),
            (
                IndexDegradationCode::FullRefreshFallback,
                "\"fullRefreshFallback\"",
            ),
        ];
        for (code, expected) in cases {
            assert_eq!(serde_json::to_string(&code).unwrap(), expected);
        }
    }

    #[test]
    fn sqlite_repository_can_move_into_runtime_worker() {
        let path = temp_db_path("send-repository");
        let storage = SqliteStorage::open(path.clone()).unwrap();

        let repository: Box<dyn IndexJournalRepository + Send> = Box::new(storage);

        drop(repository);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn recovery_skips_journal_already_incorporated_by_active_baseline() {
        let path = temp_db_path("active-baseline-recovery");
        let storage = SqliteStorage::open(path.clone()).unwrap();
        let stale_delta = CommittedIndexDelta {
            generation: 1,
            upserts: vec![entry("/root/stale.md")],
            removals: Vec::new(),
        };
        storage
            .commit_incremental_batch(&stale_delta, &[], &[])
            .unwrap();
        let baseline = vec![entry("/root/fresh.md")];
        let baseline_id = storage.save_completed_index_batch(10, &baseline).unwrap();
        storage.activate_baseline(baseline_id, 1).unwrap();

        let recovery = recover_layered_index(&storage);

        assert!(search_titles(&recovery.index, "stale").is_empty());
        assert_eq!(search_titles(&recovery.index, "fresh"), vec!["fresh.md"]);
        assert_eq!(recovery.index.generation(), 1);

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn recovery_and_known_children_use_only_the_active_baseline() {
        let path = temp_db_path("active-baseline-single-source");
        let storage = SqliteStorage::open(path.clone()).unwrap();
        let old_id = storage
            .save_completed_index_batch(10, &[entry("/root/old.md")])
            .unwrap();
        storage.activate_baseline(old_id, 0).unwrap();
        storage
            .commit_incremental_batch(&delta(1, entry("/root/journal.md"), Vec::new()), &[], &[])
            .unwrap();
        let new_id = storage
            .save_completed_index_batch(20, &[entry("/root/new.md")])
            .unwrap();

        let before_activation = recover_layered_index(&storage);
        let before_children = storage
            .known_direct_indexed_children(Path::new("/root"), Path::new("/root"))
            .unwrap();

        assert_eq!(
            search_titles(&before_activation.index, "old"),
            vec!["old.md"]
        );
        assert_eq!(
            search_titles(&before_activation.index, "journal"),
            vec!["journal.md"]
        );
        assert!(search_titles(&before_activation.index, "new").is_empty());
        assert_eq!(
            before_children
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            vec!["/root/journal.md", "/root/old.md"]
        );

        storage.activate_baseline(new_id, 1).unwrap();
        let after_activation = recover_layered_index(&storage);
        let after_children = storage
            .known_direct_indexed_children(Path::new("/root"), Path::new("/root"))
            .unwrap();

        assert_eq!(
            search_titles(&after_activation.index, "new"),
            vec!["new.md"]
        );
        assert!(search_titles(&after_activation.index, "old").is_empty());
        assert!(search_titles(&after_activation.index, "journal").is_empty());
        assert_eq!(
            after_children
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            vec!["/root/new.md"]
        );

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn corrupt_manifest_keeps_active_baseline_and_requests_full_refresh() {
        let path = temp_db_path("corrupt-manifest-recovery");
        let storage = SqliteStorage::open(path.clone()).unwrap();
        let baseline_id = storage
            .save_completed_index_batch(10, &[entry("/root/baseline.md")])
            .unwrap();
        storage.activate_baseline(baseline_id, 0).unwrap();
        storage
            .replace_directory_manifest(
                Path::new("/root"),
                &[DirectoryFingerprint {
                    path: "/root".to_owned(),
                    parent: None,
                    root: "/root".to_owned(),
                    modified_ms: Some(1),
                }],
            )
            .unwrap();
        drop(storage);
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "INSERT INTO index_directory_manifest (path, parent, root, modified_ms) VALUES ('/root/orphan/child', '/root/orphan', '/root', 2)",
                [],
            )
            .unwrap();
        drop(connection);
        let storage = SqliteStorage::open(path.clone()).unwrap();

        assert!(storage
            .directory_manifest_for_root(Path::new("/root"))
            .is_err());
        let recovery = recover_layered_index(&storage);

        assert_eq!(recovery.baseline_entry_count(), 1);
        assert_eq!(
            recovery.degradation_code(),
            Some(IndexDegradationCode::CalibrationFailed)
        );
        assert_eq!(
            recovery.fallback_reason(),
            Some(IndexFallbackReason::FullRefreshFallback)
        );
        assert_eq!(
            search_titles(&recovery.index, "baseline"),
            vec!["baseline.md"]
        );

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn legacy_baseline_without_manifest_remains_searchable_but_requires_manifest_rebuild() {
        let path = temp_db_path("legacy-empty-manifest");
        let storage = SqliteStorage::open(path.clone()).unwrap();
        let baseline_id = storage
            .save_completed_index_batch(10, &[entry("/root/legacy.md")])
            .unwrap();
        storage.activate_baseline(baseline_id, 0).unwrap();

        let recovery = recover_layered_index(&storage);

        assert_eq!(search_titles(&recovery.index, "legacy"), vec!["legacy.md"]);
        assert!(!recovery.manifest_ready());
        assert!(recovery.needs_manifest_rebuild());
        assert_eq!(recovery.degradation_code(), None);

        drop(storage);
        let _ = fs::remove_file(path);
    }

    fn delta(generation: u64, upsert: IndexedEntry, removals: Vec<PathBuf>) -> CommittedIndexDelta {
        CommittedIndexDelta {
            generation,
            upserts: vec![upsert],
            removals,
        }
    }

    fn entry(path: &str) -> IndexedEntry {
        IndexedEntry {
            path: path.to_owned(),
            name: PathBuf::from(path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            kind: IndexedEntryKind::File,
            parent: "/root".to_owned(),
            root: "/root".to_owned(),
            ..IndexedEntry::legacy("", "", IndexedEntryKind::File)
        }
    }

    fn search_titles(index: &LayeredSearchIndex, query: &str) -> Vec<String> {
        let parser = QueryParser::new(Default::default());
        index
            .search(&parser.parse(query), 20)
            .into_iter()
            .map(|result| result.title)
            .collect()
    }

    fn temp_db_path(label: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("quickfox-{label}-{suffix}.sqlite"))
    }
}
