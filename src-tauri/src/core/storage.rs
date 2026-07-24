//! SQLite storage will live here.

use crate::core::index::{IndexedEntry, IndexedEntryKind};
use crate::core::index_entry::{
    build_search_text, normalize_path_key_for_mode, normalize_path_text_key_for_mode,
    ContentIndexState, PathComparisonMode,
};
use crate::core::layered_index::CommittedIndexDelta;
use crate::core::targeted_index_scanner::{
    DirectoryFingerprint, FileSystemEntryKind, KnownIndexedChild,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexState {
    pub root: String,
    pub refreshed_at_ms: i64,
    pub entry_count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathUsage {
    pub path: String,
    pub open_count: i64,
    pub last_opened_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexSnapshot {
    pub completed_at_ms: i64,
    pub entries: Vec<IndexedEntry>,
    pub needs_full_refresh: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncrementalRuntimeState {
    pub active_baseline_id: Option<i64>,
    pub baseline_generation: u64,
    pub last_generation: u64,
    pub degradation_code: Option<String>,
    pub baseline_refresh_reason: Option<String>,
}

#[derive(Debug)]
pub enum StorageError {
    Sqlite(rusqlite::Error),
    Json(serde_json::Error),
    InvalidJournal(String),
}

impl From<rusqlite::Error> for StorageError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

impl From<serde_json::Error> for StorageError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sqlite(error) => error.fmt(formatter),
            Self::Json(error) => error.fmt(formatter),
            Self::InvalidJournal(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for StorageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sqlite(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::InvalidJournal(_) => None,
        }
    }
}

pub struct SqliteStorage {
    connection: Connection,
    comparison_mode: PathComparisonMode,
}

impl SqliteStorage {
    pub fn open(path: PathBuf) -> Result<Self, StorageError> {
        Self::open_with_comparison_mode(path, PathComparisonMode::native())
    }

    pub(crate) fn open_with_comparison_mode(
        path: PathBuf,
        comparison_mode: PathComparisonMode,
    ) -> Result<Self, StorageError> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let connection = Connection::open(path)?;
        connection.pragma_update(None, "foreign_keys", true)?;
        let storage = Self {
            connection,
            comparison_mode,
        };
        storage.migrate()?;
        Ok(storage)
    }

    fn migrate(&self) -> Result<(), StorageError> {
        let transaction = self.connection.unchecked_transaction()?;
        transaction.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS index_state (
                root TEXT PRIMARY KEY NOT NULL,
                refreshed_at_ms INTEGER NOT NULL,
                entry_count INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS path_usage (
                path TEXT PRIMARY KEY NOT NULL,
                open_count INTEGER NOT NULL,
                last_opened_at_ms INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS command_history (
                command TEXT PRIMARY KEY NOT NULL,
                last_used_at_ms INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS input_history (
                input TEXT PRIMARY KEY NOT NULL,
                last_used_at_ms INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS index_batches (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                completed_at_ms INTEGER NOT NULL,
                entry_count INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS index_entries (
                batch_id INTEGER NOT NULL,
                path TEXT NOT NULL,
                name TEXT NOT NULL,
                kind TEXT NOT NULL,
                search_text TEXT NOT NULL,
                updated_at_ms INTEGER NOT NULL,
                PRIMARY KEY (batch_id, path),
                FOREIGN KEY (batch_id) REFERENCES index_batches(id) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_index_entries_batch_id
                ON index_entries(batch_id);

            CREATE TABLE IF NOT EXISTS index_delta_batches (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                generation INTEGER NOT NULL UNIQUE,
                status TEXT NOT NULL CHECK (status IN ('pending', 'committed')),
                committed_at_ms INTEGER NOT NULL,
                payload_hash TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS index_delta_entries (
                batch_id INTEGER NOT NULL,
                ordinal INTEGER NOT NULL,
                operation TEXT NOT NULL CHECK (operation IN ('upsert', 'remove')),
                path TEXT NOT NULL,
                entry_json TEXT,
                PRIMARY KEY (batch_id, ordinal),
                FOREIGN KEY (batch_id) REFERENCES index_delta_batches(id) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_index_delta_entries_path
                ON index_delta_entries(path);

            CREATE TABLE IF NOT EXISTS index_directory_manifest (
                path TEXT PRIMARY KEY NOT NULL,
                parent TEXT,
                root TEXT NOT NULL,
                modified_ms INTEGER
            );

            CREATE INDEX IF NOT EXISTS idx_index_directory_manifest_root
                ON index_directory_manifest(root);

            CREATE INDEX IF NOT EXISTS idx_index_directory_manifest_parent
                ON index_directory_manifest(parent);

            CREATE TABLE IF NOT EXISTS index_runtime_state (
                singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                active_baseline_id INTEGER,
                baseline_generation INTEGER NOT NULL,
                last_generation INTEGER NOT NULL,
                degradation_code TEXT,
                baseline_refresh_reason TEXT,
                FOREIGN KEY (active_baseline_id) REFERENCES index_batches(id) ON DELETE SET NULL
            );
            "#,
        )?;
        Self::ensure_index_entry_metadata_columns(&transaction)?;
        Self::backfill_index_entry_keys(&transaction, self.comparison_mode)?;
        transaction.execute_batch(
            r#"
            CREATE INDEX IF NOT EXISTS idx_index_entries_batch_root_parent
                ON index_entries(batch_id, root_key, parent_key);
            "#,
        )?;
        let user_version: i64 =
            transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if user_version < 3 {
            transaction.pragma_update(None, "user_version", 3)?;
        }
        transaction.commit()?;
        Ok(())
    }

    fn ensure_index_entry_metadata_columns(connection: &Connection) -> Result<(), StorageError> {
        let existing = index_entry_columns(connection)?;
        let columns = [
            ("parent", "TEXT"),
            ("extension", "TEXT"),
            ("depth", "INTEGER"),
            ("root", "TEXT"),
            ("modified_ms", "INTEGER"),
            ("size_bytes", "INTEGER"),
            ("content_index_state", "TEXT"),
            ("path_key", "TEXT"),
            ("root_key", "TEXT"),
            ("parent_key", "TEXT"),
        ];
        for (name, sql_type) in columns {
            if !existing.contains(&name.to_owned()) {
                connection.execute(
                    &format!("ALTER TABLE index_entries ADD COLUMN {name} {sql_type}"),
                    [],
                )?;
            }
        }
        Ok(())
    }

    fn backfill_index_entry_keys(
        connection: &Connection,
        mode: PathComparisonMode,
    ) -> Result<(), StorageError> {
        let mut statement = connection.prepare(
            r#"
            SELECT rowid, path, COALESCE(root, ''), COALESCE(parent, '')
            FROM index_entries
            WHERE path_key IS NULL OR root_key IS NULL OR parent_key IS NULL
            "#,
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }
        drop(statement);
        let mut update = connection.prepare(
            "UPDATE index_entries SET path_key = ?1, root_key = ?2, parent_key = ?3 WHERE rowid = ?4",
        )?;
        for (row_id, path, root, parent) in entries {
            update.execute(params![
                normalize_path_text_key_for_mode(&path, mode),
                normalize_path_text_key_for_mode(&root, mode),
                normalize_path_text_key_for_mode(&parent, mode),
                row_id,
            ])?;
        }
        Ok(())
    }

    pub fn incremental_schema_is_ready(&self) -> Result<bool, StorageError> {
        let user_version: i64 =
            self.connection
                .pragma_query_value(None, "user_version", |row| row.get(0))?;
        if user_version < 3 {
            return Ok(false);
        }
        let required_tables = [
            ("index_entries", &["path_key", "root_key", "parent_key"][..]),
            (
                "index_delta_batches",
                &[
                    "id",
                    "generation",
                    "status",
                    "committed_at_ms",
                    "payload_hash",
                ][..],
            ),
            (
                "index_delta_entries",
                &["batch_id", "ordinal", "operation", "path", "entry_json"][..],
            ),
            (
                "index_directory_manifest",
                &["path", "parent", "root", "modified_ms"][..],
            ),
            (
                "index_runtime_state",
                &[
                    "singleton",
                    "active_baseline_id",
                    "baseline_generation",
                    "last_generation",
                    "degradation_code",
                    "baseline_refresh_reason",
                ][..],
            ),
        ];
        for (table, columns) in required_tables {
            if !table_has_columns(&self.connection, table, columns)? {
                return Ok(false);
            }
        }
        if !foreign_key_matches(
            &self.connection,
            "index_delta_entries",
            "batch_id",
            "index_delta_batches",
            "id",
            "CASCADE",
        )? {
            return Ok(false);
        }
        if !foreign_key_matches(
            &self.connection,
            "index_runtime_state",
            "active_baseline_id",
            "index_batches",
            "id",
            "SET NULL",
        )? {
            return Ok(false);
        }
        let required_indexes = [
            ("idx_index_delta_entries_path", &["path"][..]),
            ("idx_index_directory_manifest_root", &["root"][..]),
            ("idx_index_directory_manifest_parent", &["parent"][..]),
            (
                "idx_index_entries_batch_root_parent",
                &["batch_id", "root_key", "parent_key"][..],
            ),
        ];
        for (index, columns) in required_indexes {
            if !index_has_columns(&self.connection, index, columns)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub fn commit_incremental_batch(
        &self,
        delta: &CommittedIndexDelta,
        manifest_upserts: &[DirectoryFingerprint],
        manifest_removals: &[PathBuf],
    ) -> Result<(), StorageError> {
        validate_delta(delta, self.comparison_mode)?;
        validate_manifest_rows(manifest_upserts, self.comparison_mode)?;
        let payload_identity = canonical_batch_identity(
            delta,
            manifest_upserts,
            manifest_removals,
            self.comparison_mode,
        )?;
        let generation = generation_to_i64(delta.generation)?;
        let transaction = self.connection.unchecked_transaction()?;

        let existing_batch = transaction
            .query_row(
                "SELECT status, payload_hash FROM index_delta_batches WHERE generation = ?1",
                params![generation],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        if let Some((status, existing_identity)) = existing_batch {
            if status != "committed" {
                return Err(StorageError::InvalidJournal(format!(
                    "generation {} already exists without a committed status",
                    delta.generation
                )));
            }
            if existing_identity == payload_identity {
                return Ok(());
            }
            return Err(StorageError::InvalidJournal(format!(
                "generation {} was committed with a different batch payload",
                delta.generation
            )));
        }
        validate_manifest_removals(&transaction, manifest_removals, self.comparison_mode)?;

        transaction.execute(
            r#"
            INSERT INTO index_delta_batches
                (generation, status, committed_at_ms, payload_hash)
            VALUES (?1, 'committed', ?2, ?3)
            "#,
            params![generation, unix_timestamp_ms(), payload_identity],
        )?;
        let batch_id = transaction.last_insert_rowid();
        {
            let mut statement = transaction.prepare(
                r#"
                INSERT INTO index_delta_entries
                    (batch_id, ordinal, operation, path, entry_json)
                VALUES (?1, ?2, ?3, ?4, ?5)
                "#,
            )?;
            let mut ordinal = 0_i64;
            for entry in &delta.upserts {
                statement.execute(params![
                    batch_id,
                    ordinal,
                    "upsert",
                    normalize_path_text_key_for_mode(&entry.path, self.comparison_mode),
                    serde_json::to_string(entry)?,
                ])?;
                ordinal += 1;
            }
            for path in &delta.removals {
                statement.execute(params![
                    batch_id,
                    ordinal,
                    "remove",
                    normalize_path_key_for_mode(path, self.comparison_mode),
                    Option::<String>::None,
                ])?;
                ordinal += 1;
            }
        }
        apply_manifest_changes(
            &transaction,
            manifest_upserts,
            manifest_removals,
            self.comparison_mode,
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn committed_index_deltas_after(
        &self,
        generation: u64,
    ) -> Result<Vec<CommittedIndexDelta>, StorageError> {
        let generation = generation_to_i64(generation)?;
        let mut statement = self.connection.prepare(
            r#"
            SELECT id, generation
            FROM index_delta_batches
            WHERE status = 'committed' AND generation > ?1
            ORDER BY generation ASC
            "#,
        )?;
        let rows = statement.query_map(params![generation], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })?;
        let mut batch_rows = Vec::new();
        for row in rows {
            batch_rows.push(row?);
        }
        drop(statement);

        let mut deltas = Vec::with_capacity(batch_rows.len());
        for (batch_id, generation) in batch_rows {
            let generation = u64::try_from(generation).map_err(|_| {
                StorageError::InvalidJournal("journal generation must not be negative".to_owned())
            })?;
            deltas.push(load_delta_entries(
                &self.connection,
                batch_id,
                generation,
                self.comparison_mode,
            )?);
        }
        Ok(deltas)
    }

    pub fn replace_directory_manifest(
        &self,
        root: &Path,
        rows: &[DirectoryFingerprint],
    ) -> Result<(), StorageError> {
        validate_manifest_rows(rows, self.comparison_mode)?;
        let root = normalize_path_key_for_mode(root, self.comparison_mode);
        if rows
            .iter()
            .any(|row| normalize_path_text_key_for_mode(&row.root, self.comparison_mode) != root)
        {
            return Err(StorageError::InvalidJournal(
                "directory manifest row belongs to a different root".to_owned(),
            ));
        }
        let transaction = self.connection.unchecked_transaction()?;
        transaction.execute(
            "DELETE FROM index_directory_manifest WHERE root = ?1",
            params![root],
        )?;
        upsert_manifest_rows(&transaction, rows, self.comparison_mode)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn directory_manifest_for_root(
        &self,
        root: &Path,
    ) -> Result<Vec<DirectoryFingerprint>, StorageError> {
        let root = normalize_path_key_for_mode(root, self.comparison_mode);
        let mut statement = self.connection.prepare(
            r#"
            SELECT path, parent, root, modified_ms
            FROM index_directory_manifest
            WHERE root = ?1
            ORDER BY path ASC
            "#,
        )?;
        let rows = statement.query_map(params![root], |row| {
            Ok(DirectoryFingerprint {
                path: row.get(0)?,
                parent: row.get(1)?,
                root: row.get(2)?,
                modified_ms: row.get(3)?,
            })
        })?;
        let mut manifest = Vec::new();
        for row in rows {
            manifest.push(row?);
        }
        Ok(manifest)
    }

    pub fn clear_incremental_state_through(&self, generation: u64) -> Result<(), StorageError> {
        self.connection.execute(
            "DELETE FROM index_delta_batches WHERE generation <= ?1",
            params![generation_to_i64(generation)?],
        )?;
        Ok(())
    }

    pub fn activate_baseline(
        &self,
        baseline_id: i64,
        baseline_generation: u64,
    ) -> Result<(), StorageError> {
        let transaction = self.connection.unchecked_transaction()?;
        activate_baseline_in_transaction(&transaction, baseline_id, baseline_generation)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn activate_baseline_and_clear_incremental_state(
        &self,
        baseline_id: i64,
        baseline_generation: u64,
    ) -> Result<(), StorageError> {
        let transaction = self.connection.unchecked_transaction()?;
        activate_baseline_in_transaction(&transaction, baseline_id, baseline_generation)?;
        transaction.execute(
            "DELETE FROM index_delta_batches WHERE generation <= ?1",
            params![generation_to_i64(baseline_generation)?],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn save_runtime_state(&self, state: &IncrementalRuntimeState) -> Result<(), StorageError> {
        self.connection.execute(
            r#"
            INSERT INTO index_runtime_state
                (
                    singleton,
                    active_baseline_id,
                    baseline_generation,
                    last_generation,
                    degradation_code,
                    baseline_refresh_reason
                )
            VALUES (1, ?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(singleton) DO UPDATE SET
                active_baseline_id = excluded.active_baseline_id,
                baseline_generation = excluded.baseline_generation,
                last_generation = excluded.last_generation,
                degradation_code = excluded.degradation_code,
                baseline_refresh_reason = excluded.baseline_refresh_reason
            "#,
            params![
                state.active_baseline_id,
                generation_to_i64(state.baseline_generation)?,
                generation_to_i64(state.last_generation)?,
                state.degradation_code,
                state.baseline_refresh_reason,
            ],
        )?;
        Ok(())
    }

    pub fn runtime_state(&self) -> Result<Option<IncrementalRuntimeState>, StorageError> {
        self.connection
            .query_row(
                r#"
                SELECT
                    active_baseline_id,
                    baseline_generation,
                    last_generation,
                    degradation_code,
                    baseline_refresh_reason
                FROM index_runtime_state
                WHERE singleton = 1
                "#,
                [],
                |row| {
                    Ok((
                        row.get::<_, Option<i64>>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .optional()?
            .map(
                |(
                    active_baseline_id,
                    baseline_generation,
                    last_generation,
                    degradation_code,
                    baseline_refresh_reason,
                )| {
                    u64::try_from(baseline_generation)
                        .and_then(|baseline_generation| {
                            u64::try_from(last_generation).map(|last_generation| {
                                IncrementalRuntimeState {
                                    active_baseline_id,
                                    baseline_generation,
                                    last_generation,
                                    degradation_code,
                                    baseline_refresh_reason,
                                }
                            })
                        })
                        .map_err(|_| {
                            StorageError::InvalidJournal(
                                "runtime generations must not be negative".to_owned(),
                            )
                        })
                },
            )
            .transpose()
    }

    pub fn known_direct_indexed_children(
        &self,
        root: &Path,
        directory: &Path,
    ) -> Result<Vec<KnownIndexedChild>, StorageError> {
        let root_key = normalize_path_key_for_mode(root, self.comparison_mode);
        let directory_key = normalize_path_key_for_mode(directory, self.comparison_mode);
        let mut entries = BTreeMap::new();
        let latest_batch_id = self
            .connection
            .query_row(
                "SELECT id FROM index_batches ORDER BY completed_at_ms DESC, id DESC LIMIT 1",
                [],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        if let Some(batch_id) = latest_batch_id {
            let mut statement = self.connection.prepare(
                r#"
                SELECT path, kind, modified_ms, size_bytes
                FROM index_entries
                WHERE batch_id = ?1 AND root_key = ?2 AND parent_key = ?3
                ORDER BY path ASC
                "#,
            )?;
            let rows = statement.query_map(params![batch_id, root_key, directory_key], |row| {
                let path = row.get::<_, String>(0)?;
                let kind = index_kind_from_storage(&row.get::<_, String>(1)?);
                Ok((
                    normalize_path_text_key_for_mode(&path, self.comparison_mode),
                    KnownIndexedChild {
                        path,
                        filesystem_kind: if kind == IndexedEntryKind::Directory {
                            FileSystemEntryKind::Directory
                        } else {
                            FileSystemEntryKind::File
                        },
                        kind,
                        modified_ms: row.get(2)?,
                        size_bytes: row.get::<_, Option<i64>>(3)?.map(|size| size.max(0) as u64),
                    },
                ))
            })?;
            for row in rows {
                let (path, entry) = row?;
                entries.insert(path, entry);
            }
        }

        let root = normalize_path_key_for_mode(root, self.comparison_mode);
        let directory = normalize_path_key_for_mode(directory, self.comparison_mode);
        let manifest_paths: BTreeSet<_> = self
            .directory_manifest_for_root(Path::new(&root))?
            .into_iter()
            .map(|row| normalize_path_text_key_for_mode(&row.path, self.comparison_mode))
            .collect();
        for (path, entry) in &mut entries {
            if manifest_paths.contains(path) {
                entry.filesystem_kind = FileSystemEntryKind::Directory;
            }
        }

        for delta in self.committed_index_deltas_after(0)? {
            for removal in delta.removals {
                let key = normalize_path_key_for_mode(&removal, self.comparison_mode);
                if key == directory || path_is_descendant(&key, &directory) {
                    entries.clear();
                } else if normalized_parent(&key) == Some(directory.as_str()) {
                    entries.remove(&key);
                }
            }
            for entry in delta.upserts {
                let key = normalize_path_text_key_for_mode(&entry.path, self.comparison_mode);
                if (key == directory || path_is_descendant(&key, &directory))
                    && entry.kind != IndexedEntryKind::Directory
                {
                    entries.clear();
                }
                if normalize_path_text_key_for_mode(&entry.root, self.comparison_mode) == root
                    && normalize_path_text_key_for_mode(&entry.parent, self.comparison_mode)
                        == directory
                {
                    entries.insert(
                        key.clone(),
                        KnownIndexedChild {
                            path: entry.path,
                            kind: entry.kind.clone(),
                            filesystem_kind: if entry.kind == IndexedEntryKind::Directory
                                || manifest_paths.contains(&key)
                            {
                                FileSystemEntryKind::Directory
                            } else {
                                FileSystemEntryKind::File
                            },
                            modified_ms: entry.modified_ms,
                            size_bytes: entry.size_bytes,
                        },
                    );
                }
            }
        }
        Ok(entries.into_values().collect())
    }

    pub fn save_index_state(&self, state: &IndexState) -> Result<(), StorageError> {
        self.connection.execute(
            r#"
            INSERT INTO index_state (root, refreshed_at_ms, entry_count)
            VALUES (?1, ?2, ?3)
            ON CONFLICT(root) DO UPDATE SET
                refreshed_at_ms = excluded.refreshed_at_ms,
                entry_count = excluded.entry_count
            "#,
            params![state.root, state.refreshed_at_ms, state.entry_count],
        )?;
        Ok(())
    }

    pub fn index_state(&self, root: &str) -> Result<Option<IndexState>, StorageError> {
        Ok(self
            .connection
            .query_row(
                r#"
                SELECT root, refreshed_at_ms, entry_count
                FROM index_state
                WHERE root = ?1
                "#,
                params![root],
                |row| {
                    Ok(IndexState {
                        root: row.get(0)?,
                        refreshed_at_ms: row.get(1)?,
                        entry_count: row.get(2)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn save_completed_index_batch(
        &self,
        completed_at_ms: i64,
        entries: &[IndexedEntry],
    ) -> Result<i64, StorageError> {
        let transaction = self.connection.unchecked_transaction()?;
        transaction.execute(
            r#"
            INSERT INTO index_batches (completed_at_ms, entry_count)
            VALUES (?1, ?2)
            "#,
            params![completed_at_ms, entries.len() as i64],
        )?;
        let batch_id = transaction.last_insert_rowid();

        {
            let mut statement = transaction.prepare(
                r#"
                INSERT INTO index_entries
                    (
                        batch_id,
                        path,
                        name,
                        kind,
                        search_text,
                        updated_at_ms,
                        parent,
                        extension,
                        depth,
                        root,
                        modified_ms,
                        size_bytes,
                        content_index_state,
                        path_key,
                        root_key,
                        parent_key
                    )
                VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                    ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16
                )
                "#,
            )?;
            for entry in entries {
                statement.execute(params![
                    batch_id,
                    entry.path,
                    entry.name,
                    index_kind_to_storage(&entry.kind),
                    searchable_text(entry),
                    completed_at_ms,
                    nullable_text(&entry.parent),
                    entry.extension.as_deref(),
                    entry.depth as i64,
                    nullable_text(&entry.root),
                    entry.modified_ms,
                    entry.size_bytes.map(|size| size as i64),
                    content_index_state_to_storage(&entry.content_index_state),
                    normalize_path_text_key_for_mode(&entry.path, self.comparison_mode),
                    normalize_path_text_key_for_mode(&entry.root, self.comparison_mode),
                    normalize_path_text_key_for_mode(&entry.parent, self.comparison_mode),
                ])?;
            }
        }

        transaction.commit()?;
        Ok(batch_id)
    }

    pub fn latest_index_snapshot(&self) -> Result<Option<IndexSnapshot>, StorageError> {
        let Some((batch_id, completed_at_ms)) = self
            .connection
            .query_row(
                r#"
                SELECT id, completed_at_ms
                FROM index_batches
                ORDER BY completed_at_ms DESC, id DESC
                LIMIT 1
                "#,
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()?
        else {
            return Ok(None);
        };
        Ok(Some(self.load_index_snapshot(batch_id, completed_at_ms)?))
    }

    pub fn active_index_snapshot(&self) -> Result<Option<IndexSnapshot>, StorageError> {
        let active = self
            .connection
            .query_row(
                r#"
                SELECT batches.id, batches.completed_at_ms
                FROM index_runtime_state AS state
                JOIN index_batches AS batches ON batches.id = state.active_baseline_id
                WHERE state.singleton = 1
                "#,
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()?;
        active
            .map(|(batch_id, completed_at_ms)| self.load_index_snapshot(batch_id, completed_at_ms))
            .transpose()
    }

    fn load_index_snapshot(
        &self,
        batch_id: i64,
        completed_at_ms: i64,
    ) -> Result<IndexSnapshot, StorageError> {
        let mut statement = self.connection.prepare(
            r#"
            SELECT
                path,
                name,
                kind,
                COALESCE(parent, ''),
                extension,
                COALESCE(depth, 0),
                COALESCE(root, ''),
                modified_ms,
                size_bytes,
                COALESCE(search_text, ''),
                COALESCE(content_index_state, 'not_indexed')
            FROM index_entries
            WHERE batch_id = ?1
            ORDER BY path ASC, name ASC
            "#,
        )?;
        let rows = statement.query_map(params![batch_id], |row| {
            let path: String = row.get(0)?;
            let name: String = row.get(1)?;
            let search_text: String = row.get(9)?;
            Ok(IndexedEntry {
                path: path.clone(),
                name: name.clone(),
                kind: index_kind_from_storage(row.get::<_, String>(2)?.as_str()),
                parent: row.get(3)?,
                extension: row.get(4)?,
                depth: row.get::<_, i64>(5)?.max(0) as usize,
                root: row.get(6)?,
                modified_ms: row.get(7)?,
                size_bytes: row.get::<_, Option<i64>>(8)?.map(|size| size.max(0) as u64),
                search_text: if search_text.is_empty() {
                    build_search_text(&name, &path)
                } else {
                    search_text
                },
                content_index_state: content_index_state_from_storage(
                    row.get::<_, String>(10)?.as_str(),
                ),
            })
        })?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }

        Ok(IndexSnapshot {
            completed_at_ms,
            entries,
            needs_full_refresh: snapshot_needs_full_refresh(&self.connection, batch_id)?,
        })
    }

    pub fn record_path_usage(&self, path: &str, opened_at_ms: i64) -> Result<(), StorageError> {
        self.connection.execute(
            r#"
            INSERT INTO path_usage (path, open_count, last_opened_at_ms)
            VALUES (?1, 1, ?2)
            ON CONFLICT(path) DO UPDATE SET
                open_count = path_usage.open_count + 1,
                last_opened_at_ms = excluded.last_opened_at_ms
            "#,
            params![path, opened_at_ms],
        )?;
        Ok(())
    }

    pub fn path_usage(&self, path: &str) -> Result<Option<PathUsage>, StorageError> {
        Ok(self
            .connection
            .query_row(
                r#"
                SELECT path, open_count, last_opened_at_ms
                FROM path_usage
                WHERE path = ?1
                "#,
                params![path],
                |row| {
                    Ok(PathUsage {
                        path: row.get(0)?,
                        open_count: row.get(1)?,
                        last_opened_at_ms: row.get(2)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn record_command(
        &self,
        command: &str,
        used_at_ms: i64,
        enabled: bool,
        max_entries: usize,
    ) -> Result<(), StorageError> {
        self.record_input(command, used_at_ms, enabled, max_entries)?;
        if !enabled || max_entries == 0 {
            return Ok(());
        }

        self.connection.execute(
            r#"
            INSERT INTO command_history (command, last_used_at_ms)
            VALUES (?1, ?2)
            ON CONFLICT(command) DO UPDATE SET
                last_used_at_ms = excluded.last_used_at_ms
            "#,
            params![command, used_at_ms],
        )?;
        self.connection.execute(
            r#"
            DELETE FROM command_history
            WHERE command NOT IN (
                SELECT command
                FROM command_history
                ORDER BY last_used_at_ms DESC
                LIMIT ?1
            )
            "#,
            params![max_entries as i64],
        )?;
        Ok(())
    }

    pub fn recent_commands(&self) -> Result<Vec<String>, StorageError> {
        let mut statement = self.connection.prepare(
            r#"
            SELECT command
            FROM command_history
            ORDER BY last_used_at_ms DESC
            "#,
        )?;
        let rows = statement.query_map([], |row| row.get(0))?;
        let mut commands = Vec::new();
        for row in rows {
            commands.push(row?);
        }
        Ok(commands)
    }

    pub fn clear_command_history(&self) -> Result<(), StorageError> {
        self.connection.execute("DELETE FROM command_history", [])?;
        Ok(())
    }

    pub fn record_input(
        &self,
        input: &str,
        used_at_ms: i64,
        enabled: bool,
        max_entries: usize,
    ) -> Result<(), StorageError> {
        if !enabled || max_entries == 0 || input.trim().is_empty() {
            return Ok(());
        }

        self.connection.execute(
            r#"
            INSERT INTO input_history (input, last_used_at_ms)
            VALUES (?1, ?2)
            ON CONFLICT(input) DO UPDATE SET
                last_used_at_ms = excluded.last_used_at_ms
            "#,
            params![input.trim(), used_at_ms],
        )?;
        self.connection.execute(
            r#"
            DELETE FROM input_history
            WHERE input NOT IN (
                SELECT input
                FROM input_history
                ORDER BY last_used_at_ms DESC
                LIMIT ?1
            )
            "#,
            params![max_entries as i64],
        )?;
        Ok(())
    }

    pub fn recent_inputs(&self) -> Result<Vec<String>, StorageError> {
        let mut statement = self.connection.prepare(
            r#"
            SELECT input
            FROM input_history
            ORDER BY last_used_at_ms DESC
            "#,
        )?;
        let rows = statement.query_map([], |row| row.get(0))?;
        let mut inputs = Vec::new();
        for row in rows {
            inputs.push(row?);
        }
        Ok(inputs)
    }

    pub fn clear_input_history(&self) -> Result<(), StorageError> {
        self.connection.execute("DELETE FROM input_history", [])?;
        Ok(())
    }
}

fn validate_delta(
    delta: &CommittedIndexDelta,
    mode: PathComparisonMode,
) -> Result<(), StorageError> {
    generation_to_i64(delta.generation)?;
    let mut paths = BTreeSet::new();
    for entry in &delta.upserts {
        let path = normalize_path_text_key_for_mode(&entry.path, mode);
        if path.is_empty() {
            return Err(StorageError::InvalidJournal(
                "journal upsert path must not be empty".to_owned(),
            ));
        }
        if !paths.insert(path) {
            return Err(StorageError::InvalidJournal(
                "journal batch contains duplicate normalized paths".to_owned(),
            ));
        }
    }
    for removal in &delta.removals {
        let path = normalize_path_key_for_mode(removal, mode);
        if path.is_empty() {
            return Err(StorageError::InvalidJournal(
                "journal removal path must not be empty".to_owned(),
            ));
        }
        if !paths.insert(path) {
            return Err(StorageError::InvalidJournal(
                "journal batch contains duplicate normalized paths".to_owned(),
            ));
        }
    }
    Ok(())
}

#[derive(Serialize)]
struct CanonicalBatchIdentity {
    generation: u64,
    upserts: Vec<IndexedEntry>,
    removals: Vec<String>,
    manifest_upserts: Vec<DirectoryFingerprint>,
    manifest_removals: Vec<String>,
}

fn canonical_batch_identity(
    delta: &CommittedIndexDelta,
    manifest_upserts: &[DirectoryFingerprint],
    manifest_removals: &[PathBuf],
    mode: PathComparisonMode,
) -> Result<String, StorageError> {
    let mut upserts = delta.upserts.clone();
    for entry in &mut upserts {
        entry.path = normalize_path_text_key_for_mode(&entry.path, mode);
        entry.root = normalize_path_text_key_for_mode(&entry.root, mode);
        entry.parent = normalize_path_text_key_for_mode(&entry.parent, mode);
    }
    upserts.sort_by(|left, right| left.path.cmp(&right.path));
    let mut removals: Vec<_> = delta
        .removals
        .iter()
        .map(|path| normalize_path_key_for_mode(path, mode))
        .collect();
    removals.sort();
    let mut manifest_upserts = manifest_upserts.to_vec();
    for row in &mut manifest_upserts {
        row.path = normalize_path_text_key_for_mode(&row.path, mode);
        row.root = normalize_path_text_key_for_mode(&row.root, mode);
        row.parent = row
            .parent
            .as_deref()
            .map(|parent| normalize_path_text_key_for_mode(parent, mode));
    }
    manifest_upserts.sort_by(|left, right| left.path.cmp(&right.path));
    let mut manifest_removals: Vec<_> = manifest_removals
        .iter()
        .map(|path| normalize_path_key_for_mode(path, mode))
        .collect();
    manifest_removals.sort();
    Ok(serde_json::to_string(&CanonicalBatchIdentity {
        generation: delta.generation,
        upserts,
        removals,
        manifest_upserts,
        manifest_removals,
    })?)
}

fn validate_manifest_rows(
    rows: &[DirectoryFingerprint],
    mode: PathComparisonMode,
) -> Result<(), StorageError> {
    let mut paths = BTreeSet::new();
    for row in rows {
        let path = normalize_path_text_key_for_mode(&row.path, mode);
        let root = normalize_path_text_key_for_mode(&row.root, mode);
        if path.is_empty() || root.is_empty() {
            return Err(StorageError::InvalidJournal(
                "directory manifest path and root must not be empty".to_owned(),
            ));
        }
        if !paths.insert(path.clone()) {
            return Err(StorageError::InvalidJournal(
                "directory manifest contains duplicate normalized paths".to_owned(),
            ));
        }
        if path == root {
            if row.parent.is_some() {
                return Err(StorageError::InvalidJournal(
                    "directory manifest root must not have a parent".to_owned(),
                ));
            }
            continue;
        }
        if !path_is_descendant(&root, &path) {
            return Err(StorageError::InvalidJournal(
                "directory manifest path must be inside its root".to_owned(),
            ));
        }
        let expected_parent = normalized_parent(&path);
        let actual_parent = row
            .parent
            .as_deref()
            .map(|parent| normalize_path_text_key_for_mode(parent, mode));
        if actual_parent.as_deref() != expected_parent {
            return Err(StorageError::InvalidJournal(
                "directory manifest parent must be the normalized direct parent".to_owned(),
            ));
        }
    }
    Ok(())
}

fn validate_manifest_removals(
    connection: &Connection,
    removals: &[PathBuf],
    mode: PathComparisonMode,
) -> Result<(), StorageError> {
    let mut paths = BTreeSet::new();
    for removal in removals {
        let path = normalize_path_key_for_mode(removal, mode);
        if path.is_empty() {
            return Err(StorageError::InvalidJournal(
                "directory manifest removal must not be empty".to_owned(),
            ));
        }
        if !paths.insert(path.clone()) {
            return Err(StorageError::InvalidJournal(
                "directory manifest removals contain a duplicate path".to_owned(),
            ));
        }
        let exists = connection.query_row(
            "SELECT EXISTS (SELECT 1 FROM index_directory_manifest WHERE path = ?1)",
            params![path],
            |row| row.get::<_, i64>(0),
        )? != 0;
        if !exists {
            return Err(StorageError::InvalidJournal(
                "directory manifest removal is outside the known manifest scope".to_owned(),
            ));
        }
    }
    Ok(())
}

fn load_delta_entries(
    connection: &Connection,
    batch_id: i64,
    generation: u64,
    mode: PathComparisonMode,
) -> Result<CommittedIndexDelta, StorageError> {
    let mut statement = connection.prepare(
        r#"
        SELECT ordinal, operation, path, entry_json
        FROM index_delta_entries
        WHERE batch_id = ?1
        ORDER BY ordinal ASC
        "#,
    )?;
    let rows = statement.query_map(params![batch_id], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
        ))
    })?;
    let mut upserts = Vec::new();
    let mut removals = Vec::new();
    let mut paths = BTreeSet::new();
    for (expected_ordinal, row) in rows.enumerate() {
        let (ordinal, operation, path, entry_json) = row?;
        if ordinal != expected_ordinal as i64 {
            return Err(StorageError::InvalidJournal(format!(
                "generation {generation} has a non-contiguous ordinal"
            )));
        }
        let path = normalize_path_text_key_for_mode(&path, mode);
        if path.is_empty() || !paths.insert(path.clone()) {
            return Err(StorageError::InvalidJournal(format!(
                "generation {generation} has an empty or duplicate path"
            )));
        }
        match operation.as_str() {
            "upsert" => {
                let json = entry_json.ok_or_else(|| {
                    StorageError::InvalidJournal(format!(
                        "generation {generation} upsert is missing entry JSON"
                    ))
                })?;
                let entry: IndexedEntry = serde_json::from_str(&json)?;
                if normalize_path_text_key_for_mode(&entry.path, mode) != path {
                    return Err(StorageError::InvalidJournal(format!(
                        "generation {generation} upsert path does not match its entry JSON"
                    )));
                }
                upserts.push(entry);
            }
            "remove" => {
                if entry_json.is_some() {
                    return Err(StorageError::InvalidJournal(format!(
                        "generation {generation} removal unexpectedly contains entry JSON"
                    )));
                }
                removals.push(PathBuf::from(path));
            }
            _ => {
                return Err(StorageError::InvalidJournal(format!(
                    "generation {generation} contains an unknown operation"
                )));
            }
        }
    }
    Ok(CommittedIndexDelta {
        generation,
        upserts,
        removals,
    })
}

fn apply_manifest_changes(
    connection: &Connection,
    upserts: &[DirectoryFingerprint],
    removals: &[PathBuf],
    mode: PathComparisonMode,
) -> Result<(), StorageError> {
    for removal in removals {
        let path = normalize_path_key_for_mode(removal, mode);
        connection.execute(
            r#"
            DELETE FROM index_directory_manifest
            WHERE path = ?1
               OR (?1 = '/' AND substr(path, 1, 1) = '/')
               OR (?1 <> '/' AND substr(path, 1, length(?1) + 1) = ?1 || '/')
            "#,
            params![path],
        )?;
    }
    upsert_manifest_rows(connection, upserts, mode)
}

fn upsert_manifest_rows(
    connection: &Connection,
    rows: &[DirectoryFingerprint],
    mode: PathComparisonMode,
) -> Result<(), StorageError> {
    let mut statement = connection.prepare(
        r#"
        INSERT INTO index_directory_manifest (path, parent, root, modified_ms)
        VALUES (?1, ?2, ?3, ?4)
        ON CONFLICT(path) DO UPDATE SET
            parent = excluded.parent,
            root = excluded.root,
            modified_ms = excluded.modified_ms
        "#,
    )?;
    for row in rows {
        statement.execute(params![
            normalize_path_text_key_for_mode(&row.path, mode),
            row.parent
                .as_deref()
                .map(|parent| normalize_path_text_key_for_mode(parent, mode)),
            normalize_path_text_key_for_mode(&row.root, mode),
            row.modified_ms,
        ])?;
    }
    Ok(())
}

fn generation_to_i64(generation: u64) -> Result<i64, StorageError> {
    i64::try_from(generation).map_err(|_| {
        StorageError::InvalidJournal("journal generation exceeds SQLite range".to_owned())
    })
}

fn activate_baseline_in_transaction(
    connection: &Connection,
    baseline_id: i64,
    baseline_generation: u64,
) -> Result<(), StorageError> {
    let baseline_exists = connection.query_row(
        "SELECT EXISTS (SELECT 1 FROM index_batches WHERE id = ?1)",
        params![baseline_id],
        |row| row.get::<_, i64>(0),
    )? != 0;
    if !baseline_exists {
        return Err(StorageError::InvalidJournal(
            "active baseline must reference a persisted index batch".to_owned(),
        ));
    }
    let generation = generation_to_i64(baseline_generation)?;
    connection.execute(
        r#"
        INSERT INTO index_runtime_state
            (
                singleton,
                active_baseline_id,
                baseline_generation,
                last_generation,
                degradation_code,
                baseline_refresh_reason
            )
        VALUES (1, ?1, ?2, ?2, NULL, NULL)
        ON CONFLICT(singleton) DO UPDATE SET
            active_baseline_id = excluded.active_baseline_id,
            baseline_generation = excluded.baseline_generation,
            last_generation = MAX(index_runtime_state.last_generation, excluded.last_generation)
        "#,
        params![baseline_id, generation],
    )?;
    Ok(())
}

fn unix_timestamp_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or_default()
}

fn normalized_parent(path: &str) -> Option<&str> {
    let separator = path.rfind('/')?;
    if separator == 0 {
        (path != "/").then_some("/")
    } else {
        Some(&path[..separator])
    }
}

fn path_is_descendant(root: &str, candidate: &str) -> bool {
    if root.ends_with('/') {
        candidate.starts_with(root) && candidate.len() > root.len()
    } else {
        candidate
            .strip_prefix(root)
            .is_some_and(|suffix| suffix.starts_with('/'))
    }
}

fn searchable_text(entry: &IndexedEntry) -> String {
    if entry.search_text.is_empty() {
        build_search_text(&entry.name, &entry.path)
    } else {
        entry.search_text.clone()
    }
}

fn nullable_text(value: &str) -> Option<&str> {
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn index_kind_to_storage(kind: &IndexedEntryKind) -> &'static str {
    match kind {
        IndexedEntryKind::Application => "application",
        IndexedEntryKind::File => "file",
        IndexedEntryKind::Directory => "directory",
    }
}

fn index_kind_from_storage(kind: &str) -> IndexedEntryKind {
    match kind {
        "application" => IndexedEntryKind::Application,
        "directory" => IndexedEntryKind::Directory,
        _ => IndexedEntryKind::File,
    }
}

fn content_index_state_to_storage(state: &ContentIndexState) -> &'static str {
    match state {
        ContentIndexState::NotIndexed => "not_indexed",
        ContentIndexState::Indexed => "indexed",
        ContentIndexState::SkippedTooLarge => "skipped_too_large",
        ContentIndexState::SkippedBinary => "skipped_binary",
        ContentIndexState::ReadFailed => "read_failed",
    }
}

fn content_index_state_from_storage(state: &str) -> ContentIndexState {
    match state {
        "indexed" => ContentIndexState::Indexed,
        "skipped_too_large" => ContentIndexState::SkippedTooLarge,
        "skipped_binary" => ContentIndexState::SkippedBinary,
        "read_failed" => ContentIndexState::ReadFailed,
        _ => ContentIndexState::NotIndexed,
    }
}

fn index_entry_columns(connection: &Connection) -> Result<Vec<String>, StorageError> {
    let mut statement = connection.prepare("PRAGMA table_info(index_entries)")?;
    let rows = statement.query_map([], |row| row.get::<_, String>(1))?;
    let mut columns = Vec::new();
    for row in rows {
        columns.push(row?);
    }
    Ok(columns)
}

fn table_has_columns(
    connection: &Connection,
    table: &str,
    required: &[&str],
) -> Result<bool, StorageError> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info(\"{table}\")"))?;
    let rows = statement.query_map([], |row| row.get::<_, String>(1))?;
    let mut columns = BTreeSet::new();
    for row in rows {
        columns.insert(row?);
    }
    Ok(required.iter().all(|column| columns.contains(*column)))
}

fn foreign_key_matches(
    connection: &Connection,
    table: &str,
    from: &str,
    target_table: &str,
    target_column: &str,
    on_delete: &str,
) -> Result<bool, StorageError> {
    let mut statement = connection.prepare(&format!("PRAGMA foreign_key_list(\"{table}\")"))?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(6)?,
        ))
    })?;
    for row in rows {
        let (found_table, found_from, found_target, found_delete) = row?;
        if found_table == target_table
            && found_from == from
            && found_target == target_column
            && found_delete.eq_ignore_ascii_case(on_delete)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn index_has_columns(
    connection: &Connection,
    index: &str,
    required: &[&str],
) -> Result<bool, StorageError> {
    let exists = connection.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = ?1)",
        params![index],
        |row| row.get::<_, i64>(0),
    )? != 0;
    if !exists {
        return Ok(false);
    }
    let mut statement = connection.prepare(&format!("PRAGMA index_info(\"{index}\")"))?;
    let rows = statement.query_map([], |row| row.get::<_, String>(2))?;
    let mut columns = Vec::new();
    for row in rows {
        columns.push(row?);
    }
    Ok(columns == required)
}

fn snapshot_needs_full_refresh(
    connection: &Connection,
    batch_id: i64,
) -> Result<bool, StorageError> {
    Ok(connection.query_row(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM index_entries
            WHERE batch_id = ?1
              AND (
                parent IS NULL
                OR root IS NULL
                OR depth IS NULL
                OR content_index_state IS NULL
              )
        )
        "#,
        params![batch_id],
        |row| row.get::<_, i64>(0),
    )? != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::index::{IndexedEntry, IndexedEntryKind, SearchIndex};
    use crate::core::index_entry::PathComparisonMode;
    use crate::core::layered_index::CommittedIndexDelta;
    use crate::core::search::QueryParser;
    use crate::core::targeted_index_scanner::DirectoryFingerprint;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn migration_creates_incremental_index_tables_without_losing_legacy_snapshot() {
        let path = temp_db_path("incremental-schema");
        {
            let connection = Connection::open(&path).unwrap();
            connection
                .execute_batch(
                    r#"
                    PRAGMA user_version = 1;
                    CREATE TABLE index_batches (
                        id INTEGER PRIMARY KEY AUTOINCREMENT,
                        completed_at_ms INTEGER NOT NULL,
                        entry_count INTEGER NOT NULL
                    );
                    CREATE TABLE index_entries (
                        batch_id INTEGER NOT NULL,
                        path TEXT NOT NULL,
                        name TEXT NOT NULL,
                        kind TEXT NOT NULL,
                        search_text TEXT NOT NULL,
                        updated_at_ms INTEGER NOT NULL,
                        PRIMARY KEY (batch_id, path)
                    );
                    INSERT INTO index_batches (completed_at_ms, entry_count)
                    VALUES (700, 1);
                    INSERT INTO index_entries
                        (batch_id, path, name, kind, search_text, updated_at_ms)
                    VALUES
                        (1, '/root/legacy.md', 'legacy.md', 'file',
                         'legacy.md /root/legacy.md', 700);
                    "#,
                )
                .unwrap();
        }

        let storage = SqliteStorage::open(path.clone()).unwrap();

        assert!(storage.incremental_schema_is_ready().unwrap());
        let snapshot = storage.latest_index_snapshot().unwrap().unwrap();
        assert_eq!(snapshot.entries.len(), 1);
        assert_eq!(snapshot.entries[0].path, "/root/legacy.md");

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn same_name_incremental_tables_without_required_foreign_key_are_not_ready() {
        let path = temp_db_path("incompatible-incremental-schema");
        {
            let connection = Connection::open(&path).unwrap();
            connection
                .execute_batch(
                    r#"
                    PRAGMA user_version = 3;
                    CREATE TABLE index_delta_batches (
                        id INTEGER PRIMARY KEY,
                        generation INTEGER NOT NULL UNIQUE,
                        status TEXT NOT NULL,
                        committed_at_ms INTEGER NOT NULL,
                        payload_hash TEXT NOT NULL
                    );
                    CREATE TABLE index_delta_entries (
                        batch_id INTEGER NOT NULL,
                        ordinal INTEGER NOT NULL,
                        operation TEXT NOT NULL,
                        path TEXT NOT NULL,
                        entry_json TEXT,
                        PRIMARY KEY (batch_id, ordinal)
                    );
                    CREATE TABLE index_directory_manifest (
                        path TEXT PRIMARY KEY,
                        parent TEXT,
                        root TEXT NOT NULL,
                        modified_ms INTEGER
                    );
                    CREATE TABLE index_runtime_state (
                        singleton INTEGER PRIMARY KEY,
                        active_baseline_id INTEGER,
                        baseline_generation INTEGER NOT NULL,
                        last_generation INTEGER NOT NULL,
                        degradation_code TEXT,
                        baseline_refresh_reason TEXT
                    );
                    "#,
                )
                .unwrap();
        }

        let storage = SqliteStorage::open(path.clone()).unwrap();

        assert!(!storage.incremental_schema_is_ready().unwrap());
        let recovery = crate::core::index_journal::recover_layered_index(
            &storage,
            vec![indexed_entry("/root/baseline.md")],
        );
        assert_eq!(
            recovery.degradation_code(),
            Some(crate::core::index_journal::IndexDegradationCode::JournalReplayFailed)
        );
        assert_eq!(recovery.baseline_entry_count(), 1);

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn committed_deltas_round_trip_in_generation_order_and_duplicate_commit_is_idempotent() {
        let path = temp_db_path("delta-order");
        let storage = SqliteStorage::open(path.clone()).unwrap();
        let second = committed_delta(2, indexed_entry("/root/two.md"), Vec::new());
        let first = committed_delta(1, indexed_entry("/root/one.md"), Vec::new());

        storage.commit_incremental_batch(&second, &[], &[]).unwrap();
        storage.commit_incremental_batch(&first, &[], &[]).unwrap();
        storage.commit_incremental_batch(&first, &[], &[]).unwrap();

        let batches = storage.committed_index_deltas_after(0).unwrap();
        assert_eq!(
            batches
                .iter()
                .map(|batch| batch.generation)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(batches[0], first);
        assert_eq!(batches[1], second);

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn incremental_batch_commits_manifest_changes_atomically() {
        let path = temp_db_path("delta-manifest-transaction");
        let storage = SqliteStorage::open(path.clone()).unwrap();
        storage
            .replace_directory_manifest(
                Path::new("/root"),
                &[
                    fingerprint("/root", None, "/root", 1),
                    fingerprint("/root/old", Some("/root"), "/root", 2),
                ],
            )
            .unwrap();

        storage
            .commit_incremental_batch(
                &committed_delta(1, indexed_entry("/root/new/file.md"), Vec::new()),
                &[fingerprint("/root/new", Some("/root"), "/root", 3)],
                &[PathBuf::from("/root/old")],
            )
            .unwrap();

        assert_eq!(
            storage
                .directory_manifest_for_root(Path::new("/root"))
                .unwrap(),
            vec![
                fingerprint("/root", None, "/root", 1),
                fingerprint("/root/new", Some("/root"), "/root", 3),
            ]
        );
        assert_eq!(storage.committed_index_deltas_after(0).unwrap().len(), 1);

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn manifest_rejects_cross_root_and_non_direct_parent_rows() {
        let path = temp_db_path("manifest-structure");
        let storage = SqliteStorage::open(path.clone()).unwrap();

        let cross_root = storage.replace_directory_manifest(
            Path::new("/root"),
            &[fingerprint("/outside", Some("/"), "/root", 1)],
        );
        let wrong_parent = storage.replace_directory_manifest(
            Path::new("/root"),
            &[fingerprint(
                "/root/child/grandchild",
                Some("/root"),
                "/root",
                1,
            )],
        );
        let root_with_parent = storage.replace_directory_manifest(
            Path::new("/root"),
            &[fingerprint("/root", Some("/"), "/root", 1)],
        );

        assert!(matches!(cross_root, Err(StorageError::InvalidJournal(_))));
        assert!(matches!(wrong_parent, Err(StorageError::InvalidJournal(_))));
        assert!(matches!(
            root_with_parent,
            Err(StorageError::InvalidJournal(_))
        ));
        assert!(storage
            .directory_manifest_for_root(Path::new("/root"))
            .unwrap()
            .is_empty());

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn manifest_removal_outside_known_root_rejects_entire_batch() {
        let path = temp_db_path("manifest-removal-scope");
        let storage = SqliteStorage::open(path.clone()).unwrap();
        storage
            .replace_directory_manifest(
                Path::new("/root"),
                &[fingerprint("/root", None, "/root", 1)],
            )
            .unwrap();

        let result = storage.commit_incremental_batch(
            &committed_delta(1, indexed_entry("/root/new.md"), Vec::new()),
            &[],
            &[PathBuf::from("/outside")],
        );

        assert!(matches!(result, Err(StorageError::InvalidJournal(_))));
        assert!(storage.committed_index_deltas_after(0).unwrap().is_empty());
        assert_eq!(
            storage
                .directory_manifest_for_root(Path::new("/root"))
                .unwrap(),
            vec![fingerprint("/root", None, "/root", 1)]
        );

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn posix_root_manifest_removal_deletes_all_descendants() {
        let path = temp_db_path("manifest-posix-root-removal");
        let storage = SqliteStorage::open(path.clone()).unwrap();
        storage
            .replace_directory_manifest(
                Path::new("/"),
                &[
                    fingerprint("/", None, "/", 1),
                    fingerprint("/a", Some("/"), "/", 2),
                    fingerprint("/a/b", Some("/a"), "/", 3),
                ],
            )
            .unwrap();

        storage
            .commit_incremental_batch(
                &CommittedIndexDelta {
                    generation: 1,
                    upserts: Vec::new(),
                    removals: Vec::new(),
                },
                &[],
                &[PathBuf::from("/")],
            )
            .unwrap();

        assert!(storage
            .directory_manifest_for_root(Path::new("/"))
            .unwrap()
            .is_empty());

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn pending_delta_is_ignored_and_malformed_committed_entry_is_rejected() {
        let path = temp_db_path("delta-validation");
        let storage = SqliteStorage::open(path.clone()).unwrap();
        storage
            .connection
            .execute(
                "INSERT INTO index_delta_batches (generation, status, committed_at_ms, payload_hash) VALUES (1, 'pending', 1, '')",
                [],
            )
            .unwrap();
        storage
            .connection
            .execute(
                "INSERT INTO index_delta_batches (generation, status, committed_at_ms, payload_hash) VALUES (2, 'committed', 2, '')",
                [],
            )
            .unwrap();
        let committed_batch_id = storage.connection.last_insert_rowid();
        storage
            .connection
            .execute(
                "INSERT INTO index_delta_entries (batch_id, ordinal, operation, path, entry_json) VALUES (?1, 0, 'upsert', '/root/wrong.md', ?2)",
                params![
                    committed_batch_id,
                    serde_json::to_string(&indexed_entry("/root/right.md")).unwrap()
                ],
            )
            .unwrap();

        let error = storage.committed_index_deltas_after(0).unwrap_err();
        assert!(matches!(error, StorageError::InvalidJournal(_)));

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn pending_delta_is_not_returned_for_recovery() {
        let path = temp_db_path("pending-delta");
        let storage = SqliteStorage::open(path.clone()).unwrap();
        storage
            .connection
            .execute(
                "INSERT INTO index_delta_batches (generation, status, committed_at_ms, payload_hash) VALUES (1, 'pending', 1, '')",
                [],
            )
            .unwrap();
        let batch_id = storage.connection.last_insert_rowid();
        storage
            .connection
            .execute(
                "INSERT INTO index_delta_entries (batch_id, ordinal, operation, path, entry_json) VALUES (?1, 0, 'remove', '/root/ignored.md', NULL)",
                params![batch_id],
            )
            .unwrap();

        assert!(storage.committed_index_deltas_after(0).unwrap().is_empty());

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn manifest_failure_rolls_back_journal_batch() {
        let path = temp_db_path("journal-rollback");
        let storage = SqliteStorage::open(path.clone()).unwrap();
        storage
            .connection
            .execute_batch(
                r#"
                CREATE TRIGGER reject_manifest_insert
                BEFORE INSERT ON index_directory_manifest
                BEGIN
                    SELECT RAISE(ABORT, 'simulated manifest failure');
                END;
                "#,
            )
            .unwrap();

        let result = storage.commit_incremental_batch(
            &committed_delta(1, indexed_entry("/root/new.md"), Vec::new()),
            &[fingerprint("/root", None, "/root", 1)],
            &[],
        );

        assert!(matches!(result, Err(StorageError::Sqlite(_))));
        assert!(storage.committed_index_deltas_after(0).unwrap().is_empty());

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn conflicting_duplicate_generation_keeps_original_batch() {
        let path = temp_db_path("journal-generation-conflict");
        let storage = SqliteStorage::open(path.clone()).unwrap();
        let original = committed_delta(1, indexed_entry("/root/original.md"), Vec::new());
        let conflict = committed_delta(1, indexed_entry("/root/conflict.md"), Vec::new());
        storage
            .commit_incremental_batch(&original, &[], &[])
            .unwrap();

        let result = storage.commit_incremental_batch(&conflict, &[], &[]);

        assert!(matches!(result, Err(StorageError::InvalidJournal(_))));
        assert_eq!(
            storage.committed_index_deltas_after(0).unwrap(),
            vec![original]
        );

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn duplicate_generation_requires_identical_delta_and_manifest_payload() {
        let path = temp_db_path("journal-full-payload-identity");
        let storage = SqliteStorage::open(path.clone()).unwrap();
        storage
            .replace_directory_manifest(
                Path::new("/root"),
                &[
                    fingerprint("/root", None, "/root", 1),
                    fingerprint("/root/old", Some("/root"), "/root", 2),
                ],
            )
            .unwrap();
        let delta = committed_delta(1, indexed_entry("/root/new.md"), Vec::new());
        let upserts = [fingerprint("/root/new", Some("/root"), "/root", 3)];
        let removals = [PathBuf::from("/root/old")];
        storage
            .commit_incremental_batch(&delta, &upserts, &removals)
            .unwrap();

        storage
            .commit_incremental_batch(&delta, &upserts, &removals)
            .unwrap();
        let conflicting_manifest = [fingerprint("/root/new", Some("/root"), "/root", 999)];
        let conflict = storage.commit_incremental_batch(&delta, &conflicting_manifest, &removals);

        assert!(matches!(conflict, Err(StorageError::InvalidJournal(_))));
        assert_eq!(
            storage.committed_index_deltas_after(0).unwrap(),
            vec![delta]
        );
        assert_eq!(
            storage
                .directory_manifest_for_root(Path::new("/root"))
                .unwrap(),
            vec![
                fingerprint("/root", None, "/root", 1),
                fingerprint("/root/new", Some("/root"), "/root", 3),
            ]
        );

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn clear_through_keeps_newer_journal_and_runtime_state_round_trips() {
        let path = temp_db_path("delta-clear");
        let storage = SqliteStorage::open(path.clone()).unwrap();
        for generation in 1..=3 {
            storage
                .commit_incremental_batch(
                    &committed_delta(
                        generation,
                        indexed_entry(&format!("/root/{generation}.md")),
                        Vec::new(),
                    ),
                    &[],
                    &[],
                )
                .unwrap();
        }
        let state = IncrementalRuntimeState {
            active_baseline_id: None,
            baseline_generation: 0,
            last_generation: 3,
            degradation_code: Some("journalReplayFailed".to_owned()),
            baseline_refresh_reason: Some("journalRecovery".to_owned()),
        };
        storage.save_runtime_state(&state).unwrap();

        storage.clear_incremental_state_through(2).unwrap();

        assert_eq!(
            storage
                .committed_index_deltas_after(0)
                .unwrap()
                .iter()
                .map(|delta| delta.generation)
                .collect::<Vec<_>>(),
            vec![3]
        );
        assert_eq!(storage.runtime_state().unwrap(), Some(state));
        let remaining_entry_rows = storage
            .connection
            .query_row("SELECT COUNT(*) FROM index_delta_entries", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap();
        assert_eq!(remaining_entry_rows, 1);

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn baseline_activation_and_journal_clear_share_one_transaction() {
        let path = temp_db_path("baseline-activation-clear");
        let storage = SqliteStorage::open(path.clone()).unwrap();
        for generation in 1..=3 {
            storage
                .commit_incremental_batch(
                    &committed_delta(
                        generation,
                        indexed_entry(&format!("/root/{generation}.md")),
                        Vec::new(),
                    ),
                    &[],
                    &[],
                )
                .unwrap();
        }
        let baseline_id = storage
            .save_completed_index_batch(10, &[indexed_entry("/root/baseline.md")])
            .unwrap();

        let invalid = storage.activate_baseline_and_clear_incremental_state(999_999, 2);
        assert!(matches!(invalid, Err(StorageError::InvalidJournal(_))));
        assert_eq!(storage.committed_index_deltas_after(0).unwrap().len(), 3);

        storage
            .activate_baseline_and_clear_incremental_state(baseline_id, 2)
            .unwrap();

        assert_eq!(
            storage
                .committed_index_deltas_after(0)
                .unwrap()
                .into_iter()
                .map(|delta| delta.generation)
                .collect::<Vec<_>>(),
            vec![3]
        );
        let state = storage.runtime_state().unwrap().unwrap();
        assert_eq!(state.active_baseline_id, Some(baseline_id));
        assert_eq!(state.baseline_generation, 2);
        assert_eq!(state.last_generation, 2);
        storage
            .save_completed_index_batch(20, &[indexed_entry("/root/not-active.md")])
            .unwrap();
        assert_eq!(
            storage.active_index_snapshot().unwrap().unwrap().entries[0].path,
            "/root/baseline.md"
        );

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn known_direct_children_reflect_baseline_and_committed_journal() {
        let path = temp_db_path("known-direct-children");
        let storage = SqliteStorage::open(path.clone()).unwrap();
        let direct = indexed_entry("/root/direct.md");
        let mut directory = indexed_entry("/root/dir");
        directory.kind = IndexedEntryKind::Directory;
        directory.extension = None;
        let mut nested = indexed_entry("/root/dir/nested.md");
        nested.parent = "/root/dir".to_owned();
        storage
            .save_completed_index_batch(10, &[direct.clone(), directory, nested])
            .unwrap();
        let new_entry = indexed_entry("/root/new.md");
        storage
            .commit_incremental_batch(
                &CommittedIndexDelta {
                    generation: 1,
                    upserts: vec![new_entry],
                    removals: vec![PathBuf::from(&direct.path)],
                },
                &[],
                &[],
            )
            .unwrap();

        let children = storage
            .known_direct_indexed_children(Path::new("/root"), Path::new("/root"))
            .unwrap();

        assert_eq!(
            children
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            vec!["/root/dir", "/root/new.md"]
        );
        assert_eq!(
            children[0].filesystem_kind,
            crate::core::targeted_index_scanner::FileSystemEntryKind::Directory
        );

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn windows_known_direct_children_use_persisted_normalized_keys_for_mixed_paths() {
        let path = temp_db_path("windows-known-children");
        let storage =
            SqliteStorage::open_with_comparison_mode(path.clone(), PathComparisonMode::Windows)
                .unwrap();
        let mut old = indexed_entry(r"C:\Root\Old.md");
        old.parent = r"C:\ROOT".to_owned();
        old.root = r"C:\Root".to_owned();
        storage.save_completed_index_batch(10, &[old]).unwrap();
        let mut new = indexed_entry("c:/ROOT/New.md");
        new.parent = "c:/Root".to_owned();
        new.root = "C:/ROOT".to_owned();
        storage
            .commit_incremental_batch(
                &CommittedIndexDelta {
                    generation: 1,
                    upserts: vec![new],
                    removals: vec![PathBuf::from("c:/root/OLD.md")],
                },
                &[],
                &[],
            )
            .unwrap();

        let children = storage
            .known_direct_indexed_children(Path::new("c:/ROOT"), Path::new(r"C:\root"))
            .unwrap();

        assert_eq!(
            children
                .iter()
                .map(|entry| entry.path.as_str())
                .collect::<Vec<_>>(),
            vec!["c:/ROOT/New.md"]
        );

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn windows_migration_backfills_normalized_keys_for_existing_baseline_rows() {
        let path = temp_db_path("windows-key-backfill");
        {
            let connection = Connection::open(&path).unwrap();
            connection
                .execute_batch(
                    r#"
                    PRAGMA user_version = 2;
                    CREATE TABLE index_batches (
                        id INTEGER PRIMARY KEY AUTOINCREMENT,
                        completed_at_ms INTEGER NOT NULL,
                        entry_count INTEGER NOT NULL
                    );
                    CREATE TABLE index_entries (
                        batch_id INTEGER NOT NULL,
                        path TEXT NOT NULL,
                        name TEXT NOT NULL,
                        kind TEXT NOT NULL,
                        search_text TEXT NOT NULL,
                        updated_at_ms INTEGER NOT NULL,
                        parent TEXT,
                        extension TEXT,
                        depth INTEGER,
                        root TEXT,
                        modified_ms INTEGER,
                        size_bytes INTEGER,
                        content_index_state TEXT,
                        PRIMARY KEY (batch_id, path)
                    );
                    INSERT INTO index_batches (completed_at_ms, entry_count) VALUES (10, 1);
                    INSERT INTO index_entries (
                        batch_id, path, name, kind, search_text, updated_at_ms,
                        parent, depth, root, content_index_state
                    ) VALUES (
                        1, 'C:\Root\Legacy.md', 'Legacy.md', 'file', 'legacy', 10,
                        'C:\ROOT', 1, 'C:\Root', 'not_indexed'
                    );
                    "#,
                )
                .unwrap();
        }
        let storage =
            SqliteStorage::open_with_comparison_mode(path.clone(), PathComparisonMode::Windows)
                .unwrap();

        let children = storage
            .known_direct_indexed_children(Path::new("c:/root"), Path::new("C:/ROOT"))
            .unwrap();

        assert_eq!(children.len(), 1);
        assert_eq!(children[0].path, r"C:\Root\Legacy.md");

        drop(storage);
        let _ = fs::remove_file(path);
    }

    fn committed_delta(
        generation: u64,
        upsert: IndexedEntry,
        removals: Vec<PathBuf>,
    ) -> CommittedIndexDelta {
        CommittedIndexDelta {
            generation,
            upserts: vec![upsert],
            removals,
        }
    }

    fn indexed_entry(path: &str) -> IndexedEntry {
        let name = Path::new(path)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        IndexedEntry {
            path: path.to_owned(),
            name,
            kind: IndexedEntryKind::File,
            parent: Path::new(path)
                .parent()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            root: "/root".to_owned(),
            modified_ms: Some(10),
            size_bytes: Some(20),
            ..IndexedEntry::legacy("", "", IndexedEntryKind::File)
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
    fn storage_migrates_and_persists_index_state() {
        let path = temp_db_path("index-state");
        let storage = SqliteStorage::open(path.clone()).unwrap();

        storage
            .save_index_state(&IndexState {
                root: "/home/frank".to_owned(),
                refreshed_at_ms: 123,
                entry_count: 42,
            })
            .unwrap();

        let loaded = storage.index_state("/home/frank").unwrap().unwrap();
        assert_eq!(loaded.root, "/home/frank");
        assert_eq!(loaded.refreshed_at_ms, 123);
        assert_eq!(loaded.entry_count, 42);

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn storage_returns_no_index_snapshot_before_first_completed_batch() {
        let path = temp_db_path("index-snapshot-empty");
        let storage = SqliteStorage::open(path.clone()).unwrap();

        assert!(storage.latest_index_snapshot().unwrap().is_none());

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn storage_persists_and_loads_latest_completed_index_snapshot() {
        let path = temp_db_path("index-snapshot");
        let storage = SqliteStorage::open(path.clone()).unwrap();

        storage
            .save_completed_index_batch(
                100,
                &[
                    IndexedEntry {
                        path: "/home/frank/Documents".to_owned(),
                        name: "Documents".to_owned(),
                        kind: IndexedEntryKind::Directory,
                        ..IndexedEntry::legacy("", "", IndexedEntryKind::Directory)
                    },
                    IndexedEntry {
                        path: "/home/frank/notes.md".to_owned(),
                        name: "notes.md".to_owned(),
                        kind: IndexedEntryKind::File,
                        ..IndexedEntry::legacy("", "", IndexedEntryKind::File)
                    },
                ],
            )
            .unwrap();
        storage
            .save_completed_index_batch(
                200,
                &[IndexedEntry {
                    path: "/home/frank/Downloads".to_owned(),
                    name: "Downloads".to_owned(),
                    kind: IndexedEntryKind::Directory,
                    ..IndexedEntry::legacy("", "", IndexedEntryKind::Directory)
                }],
            )
            .unwrap();

        let snapshot = storage.latest_index_snapshot().unwrap().unwrap();
        assert_eq!(snapshot.completed_at_ms, 200);
        assert_eq!(
            snapshot.entries,
            vec![IndexedEntry {
                path: "/home/frank/Downloads".to_owned(),
                name: "Downloads".to_owned(),
                kind: IndexedEntryKind::Directory,
                ..IndexedEntry::legacy("", "", IndexedEntryKind::Directory)
            }]
        );

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn latest_snapshot_restores_index_metadata() {
        let path = temp_db_path("index-snapshot-metadata");
        let storage = SqliteStorage::open(path.clone()).unwrap();
        let entry = IndexedEntry {
            path: "/home/frank/Documents/report.PDF".to_owned(),
            name: "report.PDF".to_owned(),
            kind: IndexedEntryKind::File,
            parent: "/home/frank/Documents".to_owned(),
            extension: Some("pdf".to_owned()),
            depth: 2,
            root: "/home/frank".to_owned(),
            modified_ms: Some(1234),
            size_bytes: Some(4096),
            search_text: "report.pdf /home/frank/documents/report.pdf".to_owned(),
            content_index_state: crate::core::index_entry::ContentIndexState::SkippedTooLarge,
        };

        storage
            .save_completed_index_batch(300, std::slice::from_ref(&entry))
            .unwrap();

        let snapshot = storage.latest_index_snapshot().unwrap().unwrap();
        assert_eq!(snapshot.entries, vec![entry]);
        assert!(!snapshot.needs_full_refresh);

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn legacy_index_snapshot_without_metadata_still_loads() {
        let path = temp_db_path("index-snapshot-legacy");
        {
            let connection = Connection::open(&path).unwrap();
            connection
                .execute_batch(
                    r#"
                    CREATE TABLE index_batches (
                        id INTEGER PRIMARY KEY AUTOINCREMENT,
                        completed_at_ms INTEGER NOT NULL,
                        entry_count INTEGER NOT NULL
                    );
                    CREATE TABLE index_entries (
                        batch_id INTEGER NOT NULL,
                        path TEXT NOT NULL,
                        name TEXT NOT NULL,
                        kind TEXT NOT NULL,
                        search_text TEXT NOT NULL,
                        updated_at_ms INTEGER NOT NULL,
                        PRIMARY KEY (batch_id, path)
                    );
                    INSERT INTO index_batches (completed_at_ms, entry_count)
                    VALUES (500, 1);
                    INSERT INTO index_entries
                        (batch_id, path, name, kind, search_text, updated_at_ms)
                    VALUES
                        (1, '/tmp/legacy.txt', 'legacy.txt', 'file', 'legacy.txt /tmp/legacy.txt', 500);
                    "#,
                )
                .unwrap();
        }

        let storage = SqliteStorage::open(path.clone()).unwrap();
        let snapshot = storage.latest_index_snapshot().unwrap().unwrap();

        assert_eq!(snapshot.completed_at_ms, 500);
        assert_eq!(snapshot.entries.len(), 1);
        assert_eq!(snapshot.entries[0].path, "/tmp/legacy.txt");
        assert_eq!(snapshot.entries[0].parent, "");
        assert_eq!(
            snapshot.entries[0].content_index_state,
            crate::core::index_entry::ContentIndexState::NotIndexed
        );
        assert!(snapshot.needs_full_refresh);

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn legacy_index_snapshot_builds_searchable_compact_index() {
        let path = temp_db_path("index-snapshot-legacy-compact");
        {
            let connection = Connection::open(&path).unwrap();
            connection
                .execute_batch(
                    r#"
                    CREATE TABLE index_batches (
                        id INTEGER PRIMARY KEY AUTOINCREMENT,
                        completed_at_ms INTEGER NOT NULL,
                        entry_count INTEGER NOT NULL
                    );
                    CREATE TABLE index_entries (
                        batch_id INTEGER NOT NULL,
                        path TEXT NOT NULL,
                        name TEXT NOT NULL,
                        kind TEXT NOT NULL,
                        search_text TEXT NOT NULL,
                        updated_at_ms INTEGER NOT NULL,
                        PRIMARY KEY (batch_id, path)
                    );
                    INSERT INTO index_batches (completed_at_ms, entry_count)
                    VALUES (600, 1);
                    INSERT INTO index_entries
                        (batch_id, path, name, kind, search_text, updated_at_ms)
                    VALUES
                        (1, 'D:\workspace\QuickFox\AGENTS.md', 'AGENTS.md', 'file', 'agents.md d:\workspace\quickfox\agents.md', 600);
                    "#,
                )
                .unwrap();
        }

        let storage = SqliteStorage::open(path.clone()).unwrap();
        let snapshot = storage.latest_index_snapshot().unwrap().unwrap();
        let index = SearchIndex::from_entries(snapshot.entries);
        let parser = QueryParser::new(Default::default());
        let results = index.search(&parser.parse("type:md agents"));

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "AGENTS.md");

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn storage_records_file_and_directory_usage_history() {
        let path = temp_db_path("path-history");
        let storage = SqliteStorage::open(path.clone()).unwrap();

        storage
            .record_path_usage("/home/frank/notes.md", 100)
            .unwrap();
        storage
            .record_path_usage("/home/frank/notes.md", 200)
            .unwrap();

        let usage = storage.path_usage("/home/frank/notes.md").unwrap().unwrap();
        assert_eq!(usage.path, "/home/frank/notes.md");
        assert_eq!(usage.open_count, 2);
        assert_eq!(usage.last_opened_at_ms, 200);

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn command_history_keeps_most_recent_entries_with_configured_limit() {
        let path = temp_db_path("command-history");
        let storage = SqliteStorage::open(path.clone()).unwrap();

        storage.record_command("first", 100, true, 2).unwrap();
        storage.record_command("second", 200, true, 2).unwrap();
        storage.record_command("third", 300, true, 2).unwrap();

        assert_eq!(storage.recent_commands().unwrap(), vec!["third", "second"]);

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn command_history_can_be_disabled_and_cleared() {
        let path = temp_db_path("command-privacy");
        let storage = SqliteStorage::open(path.clone()).unwrap();

        storage
            .record_command("secret-token", 100, false, 15)
            .unwrap();
        assert!(storage.recent_commands().unwrap().is_empty());

        storage.record_command("git status", 200, true, 15).unwrap();
        storage.clear_command_history().unwrap();
        assert!(storage.recent_commands().unwrap().is_empty());

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn input_history_keeps_recent_confirmed_inputs_with_configured_limit() {
        let path = temp_db_path("input-history");
        let storage = SqliteStorage::open(path.clone()).unwrap();

        storage.record_input("first", 100, true, 2).unwrap();
        storage.record_input("second", 200, true, 2).unwrap();
        storage.record_input("third", 300, true, 2).unwrap();

        assert_eq!(storage.recent_inputs().unwrap(), vec!["third", "second"]);

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn input_history_can_be_disabled_and_cleared() {
        let path = temp_db_path("input-history-privacy");
        let storage = SqliteStorage::open(path.clone()).unwrap();

        storage
            .record_input("secret query", 100, false, 15)
            .unwrap();
        assert!(storage.recent_inputs().unwrap().is_empty());

        storage.record_input("g 1234", 200, true, 15).unwrap();
        storage.clear_input_history().unwrap();
        assert!(storage.recent_inputs().unwrap().is_empty());

        drop(storage);
        let _ = fs::remove_file(path);
    }

    fn temp_db_path(label: &str) -> std::path::PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("quickfox-{label}-{suffix}.sqlite"))
    }
}
