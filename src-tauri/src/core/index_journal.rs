//! Persistent incremental-index journal and recovery boundary.

use crate::core::index::IndexedEntry;
use crate::core::index_entry::{normalize_path_key, normalize_path_text_key};
use crate::core::layered_index::{CommittedIndexDelta, LayeredSearchIndex};
use crate::core::storage::{IncrementalRuntimeState, SqliteStorage};
use crate::core::targeted_index_scanner::{
    DirectoryFingerprint, DirectoryManifestReader, KnownDirectoryEntriesReader, KnownIndexedChild,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IndexDegradationCode {
    WatcherInitializationFailed,
    WatcherRuntimeFailed,
    WatcherOverflow,
    ChannelOverflow,
    JournalWriteFailed,
    JournalReplayFailed,
    CalibrationFailed,
    FullRefreshFallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IndexFallbackReason {
    JournalRecoveryFailed,
}

#[derive(Debug)]
pub struct IndexRecovery {
    pub index: LayeredSearchIndex,
    pub degradation: Option<IndexDegradationCode>,
    pub fallback_reason: Option<IndexFallbackReason>,
    baseline_entry_count: usize,
}

impl IndexRecovery {
    pub fn baseline_entry_count(&self) -> usize {
        self.baseline_entry_count
    }

    pub fn degradation_code(&self) -> Option<IndexDegradationCode> {
        self.degradation
    }

    pub fn fallback_reason(&self) -> Option<IndexFallbackReason> {
        self.fallback_reason
    }
}

pub trait IndexJournalRepository {
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

    fn runtime_state(&self) -> Result<Option<IncrementalRuntimeState>, String>;

    fn save_runtime_state(&mut self, state: &IncrementalRuntimeState) -> Result<(), String>;
}

impl IndexJournalRepository for SqliteStorage {
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

pub fn recover_layered_index(
    repository: &(impl IndexJournalRepository + ?Sized),
    baseline: Vec<IndexedEntry>,
) -> IndexRecovery {
    let baseline_entry_count = baseline.len();
    let deltas = match repository.committed_index_deltas_after(0) {
        Ok(deltas) => deltas,
        Err(_) => return failed_recovery(baseline, baseline_entry_count),
    };
    let deltas = match ordered_unique_deltas(&deltas) {
        Ok(deltas) => deltas,
        Err(_) => return failed_recovery(baseline, baseline_entry_count),
    };
    let mut index = LayeredSearchIndex::from_baseline(baseline);
    for delta in deltas {
        index.apply_delta(delta);
    }
    IndexRecovery {
        index,
        degradation: None,
        fallback_reason: None,
        baseline_entry_count,
    }
}

fn failed_recovery(baseline: Vec<IndexedEntry>, baseline_entry_count: usize) -> IndexRecovery {
    IndexRecovery {
        index: LayeredSearchIndex::from_baseline(baseline),
        degradation: Some(IndexDegradationCode::JournalReplayFailed),
        fallback_reason: Some(IndexFallbackReason::JournalRecoveryFailed),
        baseline_entry_count,
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
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

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
                "INSERT INTO index_delta_batches (generation, status, committed_at) VALUES (1, 'committed', 1)",
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
        let baseline = vec![entry("/root/baseline.md")];

        let recovery = recover_layered_index(&storage, baseline);

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
