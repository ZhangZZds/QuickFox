//! SQLite storage will live here.

use crate::core::index::{IndexedEntry, IndexedEntryKind};
use crate::core::index_entry::{
    build_search_text, normalize_path_key_for_mode, normalize_path_text_key_for_mode,
    path_is_same_or_descendant_for_mode, ContentIndexState, PathComparisonMode,
};
use crate::core::layered_index::CommittedIndexDelta;
#[cfg(test)]
use crate::core::targeted_index_scanner::baseline_manifest_from_entries;
use crate::core::targeted_index_scanner::{
    DirectoryFingerprint, FileSystemEntryKind, KnownIndexedChild,
};
use rusqlite::types::Value;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const TARGETED_DIRECT_JOURNAL_SQL: &str = r#"
    SELECT batches.generation, entries.ordinal, entries.operation, entries.path, entries.entry_json
    FROM index_delta_entries AS entries INDEXED BY idx_index_delta_entries_parent_batch
    JOIN index_delta_batches AS batches ON batches.id = entries.batch_id
    WHERE entries.parent_key = ?2
      AND (entries.root_key = ?3 OR entries.root_key IS NULL)
      AND batches.status = 'committed'
      AND batches.generation > ?1
"#;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncrementalRecoveryBaseline {
    pub entries: Vec<IndexedEntry>,
    pub generation: u64,
    pub available: bool,
    pub requires_full_refresh: bool,
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

    pub fn reopen(&self) -> Result<Self, StorageError> {
        let path = self.connection.path().ok_or_else(|| {
            StorageError::InvalidJournal("SQLite storage has no reopenable path".to_owned())
        })?;
        Self::open_with_comparison_mode(PathBuf::from(path), self.comparison_mode)
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
                root_key TEXT,
                parent_key TEXT,
                PRIMARY KEY (batch_id, ordinal),
                FOREIGN KEY (batch_id) REFERENCES index_delta_batches(id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS index_directory_manifest (
                path TEXT PRIMARY KEY NOT NULL,
                parent TEXT,
                root TEXT NOT NULL,
                modified_ms INTEGER,
                path_key TEXT,
                parent_key TEXT,
                root_key TEXT
            );

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
        Self::ensure_delta_entry_key_columns(&transaction)?;
        Self::backfill_delta_entry_keys(&transaction, self.comparison_mode)?;
        Self::ensure_manifest_key_columns(&transaction)?;
        Self::backfill_manifest_keys(&transaction, self.comparison_mode)?;
        transaction.execute_batch(
            r#"
            CREATE INDEX IF NOT EXISTS idx_index_entries_batch_root_parent
                ON index_entries(batch_id, root_key, parent_key);
            "#,
        )?;
        if table_has_columns(&transaction, "index_delta_entries", &["path", "parent_key"])? {
            transaction.execute_batch(
                r#"
                CREATE INDEX IF NOT EXISTS idx_index_delta_entries_path ON index_delta_entries(path);
                CREATE INDEX IF NOT EXISTS idx_index_delta_entries_parent_batch
                    ON index_delta_entries(parent_key, batch_id);
                "#,
            )?;
        }
        if table_has_columns(
            &transaction,
            "index_directory_manifest",
            &["path_key", "root_key", "parent_key"],
        )? {
            transaction.execute_batch(
                r#"
                DROP INDEX IF EXISTS idx_index_directory_manifest_root;
                DROP INDEX IF EXISTS idx_index_directory_manifest_parent;
                CREATE UNIQUE INDEX IF NOT EXISTS idx_index_directory_manifest_path_key
                    ON index_directory_manifest(path_key);
                CREATE INDEX IF NOT EXISTS idx_index_directory_manifest_root
                    ON index_directory_manifest(root_key);
                CREATE INDEX IF NOT EXISTS idx_index_directory_manifest_parent
                    ON index_directory_manifest(parent_key);
                "#,
            )?;
        }
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

    fn ensure_manifest_key_columns(connection: &Connection) -> Result<(), StorageError> {
        if !table_has_columns(
            connection,
            "index_directory_manifest",
            &["path", "parent", "root", "modified_ms"],
        )? {
            return Ok(());
        }
        let columns = table_columns(connection, "index_directory_manifest")?;
        for column in ["path_key", "parent_key", "root_key"] {
            if !columns.contains(column) {
                connection.execute(
                    &format!("ALTER TABLE index_directory_manifest ADD COLUMN {column} TEXT"),
                    [],
                )?;
            }
        }
        Ok(())
    }

    fn ensure_delta_entry_key_columns(connection: &Connection) -> Result<(), StorageError> {
        if !table_has_columns(
            connection,
            "index_delta_entries",
            &["batch_id", "ordinal", "operation", "path", "entry_json"],
        )? {
            return Ok(());
        }
        let columns = table_columns(connection, "index_delta_entries")?;
        for column in ["root_key", "parent_key"] {
            if !columns.contains(column) {
                connection.execute(
                    &format!("ALTER TABLE index_delta_entries ADD COLUMN {column} TEXT"),
                    [],
                )?;
            }
        }
        Ok(())
    }

    fn backfill_delta_entry_keys(
        connection: &Connection,
        mode: PathComparisonMode,
    ) -> Result<(), StorageError> {
        if !table_has_columns(
            connection,
            "index_delta_entries",
            &["operation", "path", "entry_json", "root_key", "parent_key"],
        )? {
            return Ok(());
        }
        let mut select = connection.prepare(
            r#"
            SELECT rowid, operation, path, entry_json
            FROM index_delta_entries
            WHERE parent_key IS NULL AND (?1 IS NULL OR rowid > ?1)
            ORDER BY rowid ASC
            LIMIT 1000
            "#,
        )?;
        let mut update = connection.prepare(
            "UPDATE index_delta_entries SET root_key = ?1, parent_key = ?2 WHERE rowid = ?3",
        )?;
        let mut last_row_id = None;
        loop {
            let mapped = select.query_map(params![last_row_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })?;
            let mut rows = Vec::with_capacity(1_000);
            for row in mapped {
                rows.push(row?);
            }
            if rows.is_empty() {
                break;
            }
            last_row_id = rows.last().map(|row| row.0);
            for (row_id, operation, path, entry_json) in rows {
                let (root_key, parent_key) = if operation == "upsert" {
                    match entry_json
                        .as_deref()
                        .and_then(|json| serde_json::from_str::<IndexedEntry>(json).ok())
                    {
                        Some(entry) => (
                            Some(normalize_path_text_key_for_mode(&entry.root, mode)),
                            Some(normalize_path_text_key_for_mode(&entry.parent, mode)),
                        ),
                        None => (
                            None,
                            normalized_parent(&normalize_path_text_key_for_mode(&path, mode))
                                .map(str::to_owned),
                        ),
                    }
                } else {
                    (
                        None,
                        normalized_parent(&normalize_path_text_key_for_mode(&path, mode))
                            .map(str::to_owned),
                    )
                };
                update.execute(params![root_key, parent_key, row_id])?;
            }
        }
        Ok(())
    }

    fn backfill_manifest_keys(
        connection: &Connection,
        mode: PathComparisonMode,
    ) -> Result<(), StorageError> {
        if !table_has_columns(
            connection,
            "index_directory_manifest",
            &[
                "path",
                "parent",
                "root",
                "path_key",
                "parent_key",
                "root_key",
            ],
        )? {
            return Ok(());
        }
        let mut select = connection.prepare(
            r#"
            SELECT rowid, path, parent, root
            FROM index_directory_manifest
            WHERE (path_key IS NULL OR root_key IS NULL OR (parent IS NOT NULL AND parent_key IS NULL))
              AND (?1 IS NULL OR rowid > ?1)
            ORDER BY rowid ASC
            LIMIT 1000
            "#,
        )?;
        let mut update = connection.prepare(
            "UPDATE index_directory_manifest SET path_key = ?1, parent_key = ?2, root_key = ?3 WHERE rowid = ?4",
        )?;
        let mut last_row_id = None;
        loop {
            let rows = {
                let mapped = select.query_map(params![last_row_id], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                })?;
                let mut rows = Vec::with_capacity(1_000);
                for row in mapped {
                    rows.push(row?);
                }
                rows
            };
            if rows.is_empty() {
                break;
            }
            last_row_id = rows.last().map(|row| row.0);
            for (row_id, path, parent, root) in rows {
                update.execute(params![
                    normalize_path_text_key_for_mode(&path, mode),
                    parent
                        .as_deref()
                        .map(|parent| normalize_path_text_key_for_mode(parent, mode)),
                    normalize_path_text_key_for_mode(&root, mode),
                    row_id,
                ])?;
            }
        }
        Ok(())
    }

    fn backfill_index_entry_keys(
        connection: &Connection,
        mode: PathComparisonMode,
    ) -> Result<(), StorageError> {
        Self::backfill_index_entry_keys_in_batches(connection, mode, |_| {})
    }

    fn backfill_index_entry_keys_in_batches(
        connection: &Connection,
        mode: PathComparisonMode,
        mut on_batch: impl FnMut(usize),
    ) -> Result<(), StorageError> {
        const BATCH_SIZE: i64 = 1_000;
        let mut statement = connection.prepare(
            r#"
            SELECT rowid, path, COALESCE(root, ''), COALESCE(parent, '')
            FROM index_entries
            WHERE (path_key IS NULL OR root_key IS NULL OR parent_key IS NULL)
              AND (?1 IS NULL OR rowid > ?1)
            ORDER BY rowid ASC
            LIMIT ?2
            "#,
        )?;
        let mut update = connection.prepare(
            "UPDATE index_entries SET path_key = ?1, root_key = ?2, parent_key = ?3 WHERE rowid = ?4",
        )?;
        let mut last_row_id = None;
        loop {
            let entries = {
                let rows = statement.query_map(params![last_row_id, BATCH_SIZE], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                })?;
                let mut entries = Vec::with_capacity(BATCH_SIZE as usize);
                for row in rows {
                    entries.push(row?);
                }
                entries
            };
            if entries.is_empty() {
                break;
            }
            last_row_id = entries.last().map(|entry| entry.0);
            on_batch(entries.len());
            for (row_id, path, root, parent) in entries {
                update.execute(params![
                    normalize_path_text_key_for_mode(&path, mode),
                    normalize_path_text_key_for_mode(&root, mode),
                    normalize_path_text_key_for_mode(&parent, mode),
                    row_id,
                ])?;
            }
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
                &[
                    "batch_id",
                    "ordinal",
                    "operation",
                    "path",
                    "entry_json",
                    "root_key",
                    "parent_key",
                ][..],
            ),
            (
                "index_directory_manifest",
                &[
                    "path",
                    "parent",
                    "root",
                    "modified_ms",
                    "path_key",
                    "parent_key",
                    "root_key",
                ][..],
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
            (
                "idx_index_delta_entries_parent_batch",
                &["parent_key", "batch_id"][..],
            ),
            ("idx_index_directory_manifest_path_key", &["path_key"][..]),
            ("idx_index_directory_manifest_root", &["root_key"][..]),
            ("idx_index_directory_manifest_parent", &["parent_key"][..]),
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
        self.commit_incremental_batch_with_manifest_probe(
            delta,
            manifest_upserts,
            manifest_removals,
            |_| {},
        )
    }

    fn commit_incremental_batch_with_manifest_probe(
        &self,
        delta: &CommittedIndexDelta,
        manifest_upserts: &[DirectoryFingerprint],
        manifest_removals: &[PathBuf],
        mut on_manifest_rows: impl FnMut(usize),
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
        validate_manifest_removals(
            &transaction,
            manifest_removals,
            self.comparison_mode,
            &mut on_manifest_rows,
        )?;
        validate_manifest_upsert_ownership(
            &transaction,
            manifest_upserts,
            self.comparison_mode,
            &mut on_manifest_rows,
        )?;

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
                    (batch_id, ordinal, operation, path, entry_json, root_key, parent_key)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
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
                    normalize_path_text_key_for_mode(&entry.root, self.comparison_mode),
                    normalize_path_text_key_for_mode(&entry.parent, self.comparison_mode),
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
                    Option::<String>::None,
                    normalized_parent(&normalize_path_key_for_mode(path, self.comparison_mode)),
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
        validate_affected_manifest_rows(
            &transaction,
            manifest_upserts,
            self.comparison_mode,
            &mut on_manifest_rows,
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn committed_index_deltas_after(
        &self,
        generation: u64,
    ) -> Result<Vec<CommittedIndexDelta>, StorageError> {
        self.committed_index_deltas_after_with_query_probe(generation, || {})
    }

    pub fn highest_committed_generation(&self) -> Result<u64, StorageError> {
        let journal_generation = self.connection.query_row(
            "SELECT MAX(generation) FROM index_delta_batches WHERE status = 'committed'",
            [],
            |row| row.get::<_, Option<i64>>(0),
        )?;
        let journal_generation = journal_generation
            .map(|generation| {
                u64::try_from(generation).map_err(|_| {
                    StorageError::InvalidJournal(
                        "journal generation must not be negative".to_owned(),
                    )
                })
            })
            .transpose()?
            .unwrap_or(0);
        let persisted_generation = self
            .runtime_state()?
            .map(|state| state.last_generation)
            .unwrap_or(0);
        Ok(journal_generation.max(persisted_generation))
    }

    fn committed_index_deltas_after_with_query_probe(
        &self,
        generation: u64,
        mut on_query: impl FnMut(),
    ) -> Result<Vec<CommittedIndexDelta>, StorageError> {
        on_query();
        load_committed_index_deltas_after(&self.connection, generation, self.comparison_mode)
    }

    pub fn replace_directory_manifest(
        &self,
        root: &Path,
        rows: &[DirectoryFingerprint],
    ) -> Result<(), StorageError> {
        validate_manifest_rows(rows, self.comparison_mode)?;
        validate_manifest_tree_rows(rows, self.comparison_mode)?;
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
            "DELETE FROM index_directory_manifest WHERE root_key = ?1",
            params![root],
        )?;
        upsert_manifest_rows(&transaction, rows, self.comparison_mode)?;
        validate_persisted_manifest_tree(&transaction, self.comparison_mode)?;
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
            WHERE root_key = ?1
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
        validate_manifest_rows(&manifest, self.comparison_mode)?;
        validate_manifest_tree_rows(&manifest, self.comparison_mode)?;
        Ok(manifest)
    }

    pub fn validate_directory_manifest(&self) -> Result<(), StorageError> {
        validate_persisted_manifest_tree(&self.connection, self.comparison_mode)
    }

    pub fn directory_manifest_roots(&self) -> Result<Vec<PathBuf>, StorageError> {
        let mut statement = self
            .connection
            .prepare("SELECT DISTINCT root FROM index_directory_manifest ORDER BY root")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        let mut roots = Vec::new();
        for root in rows {
            roots.push(PathBuf::from(root?));
        }
        Ok(roots)
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

    pub fn activate_baseline_with_manifest_and_clear_incremental_state(
        &self,
        baseline_id: i64,
        baseline_generation: u64,
        manifest: &[DirectoryFingerprint],
    ) -> Result<(), StorageError> {
        validate_manifest_rows(manifest, self.comparison_mode)?;
        validate_manifest_tree_rows(manifest, self.comparison_mode)?;
        let transaction = self.connection.unchecked_transaction()?;
        activate_baseline_in_transaction(&transaction, baseline_id, baseline_generation)?;
        let tail = load_committed_index_deltas_after(
            &transaction,
            baseline_generation,
            self.comparison_mode,
        )?;
        let committed_manifest = load_persisted_manifest(&transaction)?;
        let manifest = merge_manifest_with_committed_deltas(
            manifest,
            &tail,
            &committed_manifest,
            self.comparison_mode,
        );
        transaction.execute("DELETE FROM index_directory_manifest", [])?;
        upsert_manifest_rows(&transaction, &manifest, self.comparison_mode)?;
        validate_persisted_manifest_tree(&transaction, self.comparison_mode)?;
        transaction.execute(
            "DELETE FROM index_delta_batches WHERE generation <= ?1",
            params![generation_to_i64(baseline_generation)?],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn restore_baseline_after_failed_revision(
        &self,
        baseline_id: i64,
        baseline_generation: u64,
        manifest: &[DirectoryFingerprint],
    ) -> Result<(), StorageError> {
        validate_manifest_rows(manifest, self.comparison_mode)?;
        validate_manifest_tree_rows(manifest, self.comparison_mode)?;
        let transaction = self.connection.unchecked_transaction()?;
        activate_baseline_in_transaction(&transaction, baseline_id, baseline_generation)?;
        transaction.execute("DELETE FROM index_delta_batches", [])?;
        transaction.execute("DELETE FROM index_directory_manifest", [])?;
        upsert_manifest_rows(&transaction, manifest, self.comparison_mode)?;
        validate_persisted_manifest_tree(&transaction, self.comparison_mode)?;
        transaction.execute(
            "UPDATE index_runtime_state SET last_generation = ?1 WHERE singleton = 1",
            params![generation_to_i64(baseline_generation)?],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn save_runtime_state(&self, state: &IncrementalRuntimeState) -> Result<(), StorageError> {
        if state.baseline_generation > state.last_generation {
            return Err(StorageError::InvalidJournal(
                "baseline generation must not exceed last generation".to_owned(),
            ));
        }
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
                    let baseline_generation = u64::try_from(baseline_generation).map_err(|_| {
                        StorageError::InvalidJournal(
                            "runtime generations must not be negative".to_owned(),
                        )
                    })?;
                    let last_generation = u64::try_from(last_generation).map_err(|_| {
                        StorageError::InvalidJournal(
                            "runtime generations must not be negative".to_owned(),
                        )
                    })?;
                    if baseline_generation > last_generation {
                        return Err(StorageError::InvalidJournal(
                            "baseline generation must not exceed last generation".to_owned(),
                        ));
                    }
                    Ok(IncrementalRuntimeState {
                        active_baseline_id,
                        baseline_generation,
                        last_generation,
                        degradation_code,
                        baseline_refresh_reason,
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
        self.known_direct_indexed_children_with_query_probe(root, directory, || {})
    }

    fn known_direct_indexed_children_with_query_probe(
        &self,
        root: &Path,
        directory: &Path,
        on_query: impl FnMut(),
    ) -> Result<Vec<KnownIndexedChild>, StorageError> {
        self.known_direct_indexed_children_with_probes(root, directory, on_query, |_| {})
    }

    fn known_direct_indexed_children_with_probes(
        &self,
        root: &Path,
        directory: &Path,
        mut on_query: impl FnMut(),
        mut on_journal_rows: impl FnMut(usize),
    ) -> Result<Vec<KnownIndexedChild>, StorageError> {
        let root_key = normalize_path_key_for_mode(root, self.comparison_mode);
        let directory_key = normalize_path_key_for_mode(directory, self.comparison_mode);
        let mut entries = BTreeMap::new();
        let baseline_selection = self.incremental_baseline_selection()?;
        if let Some((batch_id, _)) = baseline_selection {
            on_query();
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

        on_query();
        let mut manifest_statement = self.connection.prepare(
            r#"
            SELECT path_key
            FROM index_directory_manifest
            WHERE root_key = ?1 AND parent_key = ?2
            "#,
        )?;
        let manifest_rows = manifest_statement
            .query_map(params![root_key, directory_key], |row| {
                row.get::<_, String>(0)
            })?;
        let mut manifest_paths = BTreeSet::new();
        for row in manifest_rows {
            manifest_paths.insert(row?);
        }
        for (path, entry) in &mut entries {
            if manifest_paths.contains(path) {
                entry.filesystem_kind = FileSystemEntryKind::Directory;
            }
        }

        let baseline_generation = baseline_selection
            .map(|(_, generation)| generation)
            .unwrap_or(0);
        let baseline_generation = generation_to_i64(baseline_generation)?;
        let mut targeted_rows = Vec::new();

        on_query();
        let mut direct_statement = self.connection.prepare(TARGETED_DIRECT_JOURNAL_SQL)?;
        let direct_rows = direct_statement.query_map(
            params![baseline_generation, directory_key, root_key],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            },
        )?;
        for row in direct_rows {
            targeted_rows.push(row?);
        }

        let ancestor_keys = normalized_ancestor_keys(&directory_key);
        let ancestor_sql = targeted_ancestor_journal_sql(ancestor_keys.len());
        let mut ancestor_parameters = Vec::with_capacity(ancestor_keys.len() + 1);
        ancestor_parameters.push(Value::Integer(baseline_generation));
        ancestor_parameters.extend(ancestor_keys.into_iter().map(Value::Text));
        on_query();
        let mut ancestor_statement = self.connection.prepare(&ancestor_sql)?;
        let ancestor_rows =
            ancestor_statement.query_map(params_from_iter(ancestor_parameters), |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            })?;
        for row in ancestor_rows {
            targeted_rows.push(row?);
        }
        targeted_rows.sort_by_key(|row| (row.0, row.1));
        targeted_rows.dedup_by_key(|row| (row.0, row.1));
        on_journal_rows(targeted_rows.len());

        for (_, _, operation, path, entry_json) in targeted_rows {
            let key = normalize_path_text_key_for_mode(&path, self.comparison_mode);
            match operation.as_str() {
                "remove" => {
                    if entry_json.is_some() {
                        return Err(StorageError::InvalidJournal(
                            "targeted journal removal unexpectedly contains entry JSON".to_owned(),
                        ));
                    }
                    if key == directory_key || path_is_descendant(&key, &directory_key) {
                        entries.clear();
                    } else if normalized_parent(&key) == Some(directory_key.as_str()) {
                        entries.remove(&key);
                    }
                }
                "upsert" => {
                    let json = entry_json.ok_or_else(|| {
                        StorageError::InvalidJournal(
                            "targeted journal upsert is missing entry JSON".to_owned(),
                        )
                    })?;
                    let entry: IndexedEntry = serde_json::from_str(&json)?;
                    if normalize_path_text_key_for_mode(&entry.path, self.comparison_mode) != key {
                        return Err(StorageError::InvalidJournal(
                            "targeted journal path does not match its entry JSON".to_owned(),
                        ));
                    }
                    if (key == directory_key || path_is_descendant(&key, &directory_key))
                        && entry.kind != IndexedEntryKind::Directory
                    {
                        entries.clear();
                    }
                    if normalize_path_text_key_for_mode(&entry.root, self.comparison_mode)
                        == root_key
                        && normalize_path_text_key_for_mode(&entry.parent, self.comparison_mode)
                            == directory_key
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
                _ => {
                    return Err(StorageError::InvalidJournal(
                        "targeted journal contains an unknown operation".to_owned(),
                    ));
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

    pub fn incremental_recovery_baseline(
        &self,
    ) -> Result<IncrementalRecoveryBaseline, StorageError> {
        let runtime_state = self.runtime_state()?;
        let requires_full_refresh =
            matches!(runtime_state, Some(ref state) if state.active_baseline_id.is_none());
        let selection = match runtime_state {
            Some(state) => match state.active_baseline_id {
                Some(batch_id) => Some((batch_id, state.baseline_generation)),
                None => self.latest_index_batch_id()?.map(|batch_id| (batch_id, 0)),
            },
            None => self.latest_index_batch_id()?.map(|batch_id| (batch_id, 0)),
        };
        let Some((batch_id, generation)) = selection else {
            return Ok(IncrementalRecoveryBaseline {
                entries: Vec::new(),
                generation: 0,
                available: false,
                requires_full_refresh,
            });
        };
        let completed_at_ms = self
            .connection
            .query_row(
                "SELECT completed_at_ms FROM index_batches WHERE id = ?1",
                params![batch_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .ok_or_else(|| {
                StorageError::InvalidJournal(
                    "active baseline references a missing index batch".to_owned(),
                )
            })?;
        Ok(IncrementalRecoveryBaseline {
            entries: self.load_index_snapshot(batch_id, completed_at_ms)?.entries,
            generation,
            available: true,
            requires_full_refresh,
        })
    }

    pub fn latest_completed_recovery_baseline(
        &self,
    ) -> Result<IncrementalRecoveryBaseline, StorageError> {
        let Some(batch_id) = self.latest_index_batch_id()? else {
            return Ok(IncrementalRecoveryBaseline {
                entries: Vec::new(),
                generation: 0,
                available: false,
                requires_full_refresh: false,
            });
        };
        let completed_at_ms = self.connection.query_row(
            "SELECT completed_at_ms FROM index_batches WHERE id = ?1",
            params![batch_id],
            |row| row.get::<_, i64>(0),
        )?;
        Ok(IncrementalRecoveryBaseline {
            entries: self.load_index_snapshot(batch_id, completed_at_ms)?.entries,
            generation: 0,
            available: true,
            requires_full_refresh: false,
        })
    }

    fn latest_index_batch_id(&self) -> Result<Option<i64>, StorageError> {
        Ok(self
            .connection
            .query_row(
                "SELECT id FROM index_batches ORDER BY completed_at_ms DESC, id DESC LIMIT 1",
                [],
                |row| row.get::<_, i64>(0),
            )
            .optional()?)
    }

    fn incremental_baseline_selection(&self) -> Result<Option<(i64, u64)>, StorageError> {
        Ok(match self.runtime_state()? {
            Some(state) => state
                .active_baseline_id
                .map(|batch_id| (batch_id, state.baseline_generation)),
            None => self.latest_index_batch_id()?.map(|batch_id| (batch_id, 0)),
        })
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

fn load_committed_index_deltas_after(
    connection: &Connection,
    generation: u64,
    comparison_mode: PathComparisonMode,
) -> Result<Vec<CommittedIndexDelta>, StorageError> {
    let generation = generation_to_i64(generation)?;
    let mut statement = connection.prepare(
        r#"
        SELECT
            batches.generation,
            entries.ordinal,
            entries.operation,
            entries.path,
            entries.entry_json
        FROM index_delta_batches AS batches
        LEFT JOIN index_delta_entries AS entries ON entries.batch_id = batches.id
        WHERE batches.status = 'committed' AND batches.generation > ?1
        ORDER BY batches.generation ASC, entries.ordinal ASC
        "#,
    )?;
    let rows = statement.query_map(params![generation], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, Option<i64>>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<String>>(4)?,
        ))
    })?;
    let mut deltas = Vec::new();
    let mut current_generation = None;
    let mut expected_ordinal = 0_i64;
    let mut upserts = Vec::new();
    let mut removals = Vec::new();
    let mut paths = BTreeSet::new();
    for row in rows {
        let (raw_generation, ordinal, operation, path, entry_json) = row?;
        let row_generation = u64::try_from(raw_generation).map_err(|_| {
            StorageError::InvalidJournal("journal generation must not be negative".to_owned())
        })?;
        if current_generation != Some(row_generation) {
            if let Some(generation) = current_generation {
                deltas.push(CommittedIndexDelta {
                    generation,
                    upserts: std::mem::take(&mut upserts),
                    removals: std::mem::take(&mut removals),
                });
                paths.clear();
                expected_ordinal = 0;
            }
            current_generation = Some(row_generation);
        }
        let Some(ordinal) = ordinal else {
            continue;
        };
        if ordinal != expected_ordinal {
            return Err(StorageError::InvalidJournal(format!(
                "generation {row_generation} has a non-contiguous ordinal"
            )));
        }
        expected_ordinal += 1;
        let operation = operation.ok_or_else(|| {
            StorageError::InvalidJournal(format!(
                "generation {row_generation} has a missing operation"
            ))
        })?;
        let path = normalize_path_text_key_for_mode(
            &path.ok_or_else(|| {
                StorageError::InvalidJournal(format!(
                    "generation {row_generation} has a missing path"
                ))
            })?,
            comparison_mode,
        );
        if path.is_empty() || !paths.insert(path.clone()) {
            return Err(StorageError::InvalidJournal(format!(
                "generation {row_generation} has an empty or duplicate path"
            )));
        }
        match operation.as_str() {
            "upsert" => {
                let json = entry_json.ok_or_else(|| {
                    StorageError::InvalidJournal(format!(
                        "generation {row_generation} upsert is missing entry JSON"
                    ))
                })?;
                let entry: IndexedEntry = serde_json::from_str(&json)?;
                if normalize_path_text_key_for_mode(&entry.path, comparison_mode) != path {
                    return Err(StorageError::InvalidJournal(format!(
                        "generation {row_generation} upsert path does not match its entry JSON"
                    )));
                }
                upserts.push(entry);
            }
            "remove" => {
                if entry_json.is_some() {
                    return Err(StorageError::InvalidJournal(format!(
                        "generation {row_generation} removal unexpectedly contains entry JSON"
                    )));
                }
                removals.push(PathBuf::from(path));
            }
            _ => {
                return Err(StorageError::InvalidJournal(format!(
                    "generation {row_generation} contains an unknown operation"
                )));
            }
        }
    }
    if let Some(generation) = current_generation {
        deltas.push(CommittedIndexDelta {
            generation,
            upserts,
            removals,
        });
    }
    Ok(deltas)
}

fn merge_manifest_with_committed_deltas(
    manifest: &[DirectoryFingerprint],
    deltas: &[CommittedIndexDelta],
    committed_manifest: &[DirectoryFingerprint],
    comparison_mode: PathComparisonMode,
) -> Vec<DirectoryFingerprint> {
    let mut rows: BTreeMap<String, DirectoryFingerprint> = manifest
        .iter()
        .cloned()
        .map(|row| {
            (
                normalize_path_text_key_for_mode(&row.path, comparison_mode),
                row,
            )
        })
        .collect();
    let committed_rows: BTreeMap<String, DirectoryFingerprint> = committed_manifest
        .iter()
        .cloned()
        .map(|row| {
            (
                normalize_path_text_key_for_mode(&row.path, comparison_mode),
                row,
            )
        })
        .collect();
    let mut touched_paths = Vec::new();
    for delta in deltas {
        for removal in &delta.removals {
            touched_paths.push(removal.clone());
            rows.retain(|_, row| {
                !path_is_same_or_descendant_for_mode(removal, Path::new(&row.path), comparison_mode)
            });
        }
        for entry in &delta.upserts {
            let path = PathBuf::from(&entry.path);
            touched_paths.push(path);
            let key = normalize_path_text_key_for_mode(&entry.path, comparison_mode);
            if let Some(committed) = committed_rows.get(&key) {
                rows.insert(key, committed.clone());
            } else if entry.kind == IndexedEntryKind::Directory {
                rows.insert(
                    key,
                    DirectoryFingerprint {
                        path: entry.path.clone(),
                        parent: (entry.path != entry.root)
                            .then(|| entry.parent.clone())
                            .filter(|parent| !parent.is_empty()),
                        root: entry.root.clone(),
                        modified_ms: entry.modified_ms,
                    },
                );
            }
        }
    }
    for committed in committed_manifest {
        if touched_paths.iter().any(|touched| {
            path_is_same_or_descendant_for_mode(
                Path::new(&committed.path),
                touched,
                comparison_mode,
            )
        }) {
            rows.insert(
                normalize_path_text_key_for_mode(&committed.path, comparison_mode),
                committed.clone(),
            );
        }
    }
    rows.into_values().collect()
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

    let mut digest = StableBatchDigest::new();
    digest.add_field(1, b"quickfox-index-batch-v1");
    digest.add_u64(2, delta.generation);
    digest.add_u64(3, upserts.len() as u64);
    for entry in &upserts {
        digest.add_field(4, &serde_json::to_vec(entry)?);
    }
    digest.add_u64(5, removals.len() as u64);
    for removal in &removals {
        digest.add_field(6, removal.as_bytes());
    }
    digest.add_u64(7, manifest_upserts.len() as u64);
    for row in &manifest_upserts {
        digest.add_field(8, row.path.as_bytes());
        match &row.parent {
            Some(parent) => {
                digest.add_field(9, &[1]);
                digest.add_field(10, parent.as_bytes());
            }
            None => digest.add_field(9, &[0]),
        }
        digest.add_field(11, row.root.as_bytes());
        match row.modified_ms {
            Some(modified_ms) => {
                digest.add_field(12, &[1]);
                digest.add_field(13, &modified_ms.to_le_bytes());
            }
            None => digest.add_field(12, &[0]),
        }
    }
    digest.add_u64(14, manifest_removals.len() as u64);
    for removal in &manifest_removals {
        digest.add_field(15, removal.as_bytes());
    }
    Ok(digest.finish())
}

struct StableBatchDigest(u128);

impl StableBatchDigest {
    const FNV_OFFSET_BASIS: u128 = 0x6c62272e07bb014262b821756295c58d;
    const FNV_PRIME: u128 = 0x0000000001000000000000000000013b;

    const fn new() -> Self {
        Self(Self::FNV_OFFSET_BASIS)
    }

    fn add_u64(&mut self, tag: u8, value: u64) {
        self.add_field(tag, &value.to_le_bytes());
    }

    fn add_field(&mut self, tag: u8, value: &[u8]) {
        self.update(&[tag]);
        self.update(&(value.len() as u64).to_le_bytes());
        self.update(value);
    }

    fn update(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u128::from(*byte);
            self.0 = self.0.wrapping_mul(Self::FNV_PRIME);
        }
    }

    fn finish(self) -> String {
        format!("{:032x}", self.0)
    }
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

fn validate_manifest_tree_rows(
    rows: &[DirectoryFingerprint],
    mode: PathComparisonMode,
) -> Result<(), StorageError> {
    let path_roots: BTreeMap<_, _> = rows
        .iter()
        .map(|row| {
            (
                normalize_path_text_key_for_mode(&row.path, mode),
                normalize_path_text_key_for_mode(&row.root, mode),
            )
        })
        .collect();
    for row in rows {
        let path = normalize_path_text_key_for_mode(&row.path, mode);
        let root = normalize_path_text_key_for_mode(&row.root, mode);
        if path_roots.get(&root) != Some(&root) {
            return Err(StorageError::InvalidJournal(
                "directory manifest must contain its root row".to_owned(),
            ));
        }
        if path != root {
            let parent = row
                .parent
                .as_deref()
                .map(|parent| normalize_path_text_key_for_mode(parent, mode));
            let has_parent_in_same_root = parent
                .as_ref()
                .is_some_and(|parent| path_roots.get(parent) == Some(&root));
            if !has_parent_in_same_root {
                return Err(StorageError::InvalidJournal(
                    "directory manifest must contain every parent row in the same root".to_owned(),
                ));
            }
        }
    }
    Ok(())
}

fn validate_persisted_manifest_tree(
    connection: &Connection,
    mode: PathComparisonMode,
) -> Result<(), StorageError> {
    let manifest = load_persisted_manifest(connection)?;
    validate_manifest_rows(&manifest, mode)?;
    validate_manifest_tree_rows(&manifest, mode)
}

fn load_persisted_manifest(
    connection: &Connection,
) -> Result<Vec<DirectoryFingerprint>, StorageError> {
    let mut statement = connection.prepare(
        r#"
        SELECT path, parent, root, modified_ms
        FROM index_directory_manifest
        ORDER BY root ASC, path ASC
        "#,
    )?;
    let rows = statement.query_map([], |row| {
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

fn validate_manifest_removals(
    connection: &Connection,
    removals: &[PathBuf],
    mode: PathComparisonMode,
    on_rows: &mut impl FnMut(usize),
) -> Result<(), StorageError> {
    let mut root_lookup = connection.prepare(
        "SELECT 1 FROM index_directory_manifest WHERE path_key = ?1 AND path_key = root_key LIMIT 1",
    )?;
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
        let mut candidate = Some(path.as_str());
        let mut inside_known_root = false;
        while let Some(ancestor) = candidate {
            let found = root_lookup
                .query_row(params![ancestor], |_| Ok(()))
                .optional()?
                .is_some();
            on_rows(usize::from(found));
            if found {
                inside_known_root = true;
                break;
            }
            candidate = normalized_parent(ancestor);
        }
        if !inside_known_root {
            return Err(StorageError::InvalidJournal(
                "directory manifest removal is outside the known manifest scope".to_owned(),
            ));
        }
    }
    Ok(())
}

fn validate_manifest_upsert_ownership(
    connection: &Connection,
    upserts: &[DirectoryFingerprint],
    mode: PathComparisonMode,
    on_rows: &mut impl FnMut(usize),
) -> Result<(), StorageError> {
    let mut statement =
        connection.prepare("SELECT root_key FROM index_directory_manifest WHERE path_key = ?1")?;
    for row in upserts {
        let path_key = normalize_path_text_key_for_mode(&row.path, mode);
        let root_key = normalize_path_text_key_for_mode(&row.root, mode);
        let existing_root = statement
            .query_row(params![path_key], |row| row.get::<_, String>(0))
            .optional()?;
        on_rows(usize::from(existing_root.is_some()));
        if existing_root
            .as_deref()
            .is_some_and(|root| root != root_key)
        {
            return Err(StorageError::InvalidJournal(
                "incremental manifest upsert cannot change root ownership".to_owned(),
            ));
        }
    }
    Ok(())
}

fn validate_affected_manifest_rows(
    connection: &Connection,
    upserts: &[DirectoryFingerprint],
    mode: PathComparisonMode,
    on_rows: &mut impl FnMut(usize),
) -> Result<(), StorageError> {
    let mut statement =
        connection.prepare("SELECT root_key FROM index_directory_manifest WHERE path_key = ?1")?;
    for row in upserts {
        let path_key = normalize_path_text_key_for_mode(&row.path, mode);
        let root_key = normalize_path_text_key_for_mode(&row.root, mode);
        let persisted_root = statement
            .query_row(params![path_key], |row| row.get::<_, String>(0))
            .optional()?;
        on_rows(usize::from(persisted_root.is_some()));
        if persisted_root.as_deref() != Some(root_key.as_str()) {
            return Err(StorageError::InvalidJournal(
                "incremental manifest upsert is missing from its root".to_owned(),
            ));
        }
        if path_key == root_key {
            continue;
        }
        let parent_key = row
            .parent
            .as_deref()
            .map(|parent| normalize_path_text_key_for_mode(parent, mode))
            .ok_or_else(|| {
                StorageError::InvalidJournal(
                    "incremental manifest upsert is missing its parent".to_owned(),
                )
            })?;
        let parent_root = statement
            .query_row(params![parent_key], |row| row.get::<_, String>(0))
            .optional()?;
        on_rows(usize::from(parent_root.is_some()));
        if parent_root.as_deref() != Some(root_key.as_str()) {
            return Err(StorageError::InvalidJournal(
                "incremental manifest upsert parent is absent from the same root".to_owned(),
            ));
        }
    }
    Ok(())
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
            WHERE path_key = ?1
               OR (?1 = '/' AND substr(path_key, 1, 1) = '/')
               OR (?1 <> '/' AND substr(path_key, 1, length(?1) + 1) = ?1 || '/')
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
        INSERT INTO index_directory_manifest
            (path, parent, root, modified_ms, path_key, parent_key, root_key)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
        ON CONFLICT(path_key) DO UPDATE SET
            path = excluded.path,
            parent = excluded.parent,
            root = excluded.root,
            modified_ms = excluded.modified_ms,
            parent_key = excluded.parent_key,
            root_key = excluded.root_key
        "#,
    )?;
    for row in rows {
        statement.execute(params![
            row.path,
            row.parent,
            row.root,
            row.modified_ms,
            normalize_path_text_key_for_mode(&row.path, mode),
            row.parent
                .as_deref()
                .map(|parent| normalize_path_text_key_for_mode(parent, mode)),
            normalize_path_text_key_for_mode(&row.root, mode),
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

fn normalized_ancestor_keys(path: &str) -> Vec<String> {
    let mut ancestors = Vec::new();
    let mut current = Some(path.to_owned());
    while let Some(path) = current {
        current = normalized_parent(&path).map(str::to_owned);
        ancestors.push(path);
    }
    ancestors
}

fn targeted_ancestor_journal_sql(ancestor_count: usize) -> String {
    let placeholders = (0..ancestor_count)
        .map(|index| format!("?{}", index + 2))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        r#"
        SELECT batches.generation, entries.ordinal, entries.operation, entries.path, entries.entry_json
        FROM index_delta_entries AS entries INDEXED BY idx_index_delta_entries_path
        JOIN index_delta_batches AS batches ON batches.id = entries.batch_id
        WHERE entries.path IN ({placeholders})
          AND batches.status = 'committed'
          AND batches.generation > ?1
        "#
    )
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
    let columns = table_columns(connection, table)?;
    Ok(required.iter().all(|column| columns.contains(*column)))
}

fn table_columns(connection: &Connection, table: &str) -> Result<BTreeSet<String>, StorageError> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info(\"{table}\")"))?;
    let rows = statement.query_map([], |row| row.get::<_, String>(1))?;
    let mut columns = BTreeSet::new();
    for row in rows {
        columns.insert(row?);
    }
    Ok(columns)
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
    use crate::core::index_scanner::{IndexPathRules, IndexScanPlan};
    use crate::core::layered_index::CommittedIndexDelta;
    use crate::core::search::QueryParser;
    use crate::core::targeted_index_scanner::{DirectoryFingerprint, TargetedIndexScanner};
    use std::cell::{Cell, RefCell};
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tempfile::TempDir;

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
        storage
            .save_completed_index_batch(1, &[indexed_entry("/root/baseline.md")])
            .unwrap();

        assert!(!storage.incremental_schema_is_ready().unwrap());
        let recovery = crate::core::index_journal::recover_layered_index(&storage);
        assert_eq!(
            recovery.degradation_code(),
            Some(crate::core::index_journal::IndexDegradationCode::JournalReplayFailed)
        );
        assert_eq!(recovery.baseline_entry_count(), 1);

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn incompatible_incremental_tables_do_not_block_legacy_baseline_recovery() {
        let path = temp_db_path("incompatible-columns-open");
        {
            let connection = Connection::open(&path).unwrap();
            connection
                .execute_batch(
                    r#"
                    PRAGMA user_version = 3;
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
                    INSERT INTO index_batches (completed_at_ms, entry_count) VALUES (10, 1);
                    INSERT INTO index_entries
                        (batch_id, path, name, kind, search_text, updated_at_ms)
                    VALUES (1, '/root/legacy.md', 'legacy.md', 'file', 'legacy', 10);
                    CREATE TABLE index_delta_entries (
                        batch_id INTEGER NOT NULL,
                        ordinal INTEGER NOT NULL,
                        operation TEXT NOT NULL,
                        entry_json TEXT
                    );
                    CREATE TABLE index_directory_manifest (
                        path TEXT PRIMARY KEY,
                        parent TEXT,
                        modified_ms INTEGER
                    );
                    "#,
                )
                .unwrap();
        }

        let storage = SqliteStorage::open(path.clone()).unwrap();

        assert!(!storage.incremental_schema_is_ready().unwrap());
        let recovery = crate::core::index_journal::recover_layered_index(&storage);
        assert!(recovery.baseline_available());
        assert_eq!(recovery.baseline_entry_count(), 1);
        assert_eq!(
            recovery.degradation_code(),
            Some(crate::core::index_journal::IndexDegradationCode::JournalReplayFailed)
        );

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn recovery_reads_legacy_baseline_before_incompatible_runtime_state() {
        let path = temp_db_path("runtime-state-missing-generation");
        {
            let connection = Connection::open(&path).unwrap();
            connection
                .execute_batch(
                    r#"
                    PRAGMA user_version = 3;
                    CREATE TABLE index_runtime_state (
                        singleton INTEGER PRIMARY KEY,
                        active_baseline_id INTEGER,
                        last_generation INTEGER NOT NULL
                    );
                    "#,
                )
                .unwrap();
        }
        let storage = SqliteStorage::open(path.clone()).unwrap();
        storage
            .save_completed_index_batch(10, &[indexed_entry("/root/legacy.md")])
            .unwrap();

        let recovery = crate::core::index_journal::recover_layered_index(&storage);

        assert_eq!(recovery.baseline_entry_count(), 1);
        assert_eq!(
            recovery.degradation_code(),
            Some(crate::core::index_journal::IndexDegradationCode::JournalReplayFailed)
        );

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
    fn committed_delta_recovery_streams_many_batches_with_one_query() {
        let path = temp_db_path("streamed-delta-recovery");
        let storage = SqliteStorage::open(path.clone()).unwrap();
        for generation in 1..=200 {
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
        let query_count = Cell::new(0);

        let batches = storage
            .committed_index_deltas_after_with_query_probe(0, || {
                query_count.set(query_count.get() + 1)
            })
            .unwrap();

        assert_eq!(batches.len(), 200);
        assert_eq!(query_count.get(), 1);

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
    fn one_entry_commit_validates_only_affected_manifest_rows() {
        let path = temp_db_path("bounded-manifest-commit-validation");
        let storage = SqliteStorage::open(path.clone()).unwrap();
        let mut manifest = vec![fingerprint("/root", None, "/root", 1)];
        for ordinal in 0..5_000 {
            manifest.push(fingerprint(
                &format!("/root/dir-{ordinal}"),
                Some("/root"),
                "/root",
                ordinal,
            ));
        }
        storage
            .replace_directory_manifest(Path::new("/root"), &manifest)
            .unwrap();
        let rows_read = Cell::new(0_usize);

        storage
            .commit_incremental_batch_with_manifest_probe(
                &CommittedIndexDelta {
                    generation: 1,
                    upserts: Vec::new(),
                    removals: Vec::new(),
                },
                &[fingerprint("/root/new-dir", Some("/root"), "/root", 9_999)],
                &[],
                |rows| rows_read.set(rows_read.get() + rows),
            )
            .unwrap();

        assert!(
            rows_read.get() <= 4,
            "read {} manifest rows",
            rows_read.get()
        );
        assert_eq!(
            storage
                .directory_manifest_for_root(Path::new("/root"))
                .unwrap()
                .len(),
            5_002
        );

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
    fn manifest_replace_requires_root_and_complete_parent_chain() {
        let path = temp_db_path("manifest-tree-closure");
        let storage = SqliteStorage::open(path.clone()).unwrap();

        let missing_root = storage.replace_directory_manifest(
            Path::new("/root"),
            &[fingerprint("/root/child", Some("/root"), "/root", 1)],
        );
        let missing_parent = storage.replace_directory_manifest(
            Path::new("/root"),
            &[
                fingerprint("/root", None, "/root", 1),
                fingerprint("/root/child/grandchild", Some("/root/child"), "/root", 2),
            ],
        );

        assert!(matches!(missing_root, Err(StorageError::InvalidJournal(_))));
        assert!(matches!(
            missing_parent,
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
    fn overlapping_root_replace_rolls_back_when_it_orphans_existing_rows() {
        let path = temp_db_path("overlapping-root-replace-rollback");
        let storage = SqliteStorage::open(path.clone()).unwrap();
        let original = vec![
            fingerprint("/outer", None, "/outer", 1),
            fingerprint("/outer/nested", Some("/outer"), "/outer", 2),
            fingerprint("/outer/nested/leaf", Some("/outer/nested"), "/outer", 3),
        ];
        storage
            .replace_directory_manifest(Path::new("/outer"), &original)
            .unwrap();

        let takeover = storage.replace_directory_manifest(
            Path::new("/outer/nested"),
            &[fingerprint("/outer/nested", None, "/outer/nested", 20)],
        );

        assert!(matches!(takeover, Err(StorageError::InvalidJournal(_))));
        assert_eq!(
            storage
                .directory_manifest_for_root(Path::new("/outer"))
                .unwrap(),
            original
        );
        assert!(storage
            .directory_manifest_for_root(Path::new("/outer/nested"))
            .unwrap()
            .is_empty());

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn incremental_manifest_upsert_requires_parent_in_final_tree() {
        let path = temp_db_path("incremental-manifest-parent-closure");
        let storage = SqliteStorage::open(path.clone()).unwrap();
        storage
            .replace_directory_manifest(
                Path::new("/root"),
                &[fingerprint("/root", None, "/root", 1)],
            )
            .unwrap();

        let result = storage.commit_incremental_batch(
            &CommittedIndexDelta {
                generation: 1,
                upserts: Vec::new(),
                removals: Vec::new(),
            },
            &[fingerprint(
                "/root/missing/child",
                Some("/root/missing"),
                "/root",
                2,
            )],
            &[],
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
    fn persisted_manifest_rejects_parent_rows_owned_by_another_root() {
        let path = temp_db_path("manifest-cross-root-parent-closure");
        let storage = SqliteStorage::open(path.clone()).unwrap();
        storage
            .replace_directory_manifest(
                Path::new("/outer"),
                &[
                    fingerprint("/outer", None, "/outer", 1),
                    fingerprint("/outer/nested", Some("/outer"), "/outer", 2),
                ],
            )
            .unwrap();
        storage
            .connection
            .execute(
                "INSERT INTO index_directory_manifest (path, parent, root, modified_ms) VALUES ('/outer/nested/child', '/outer/nested', '/outer/nested', 3)",
                [],
            )
            .unwrap();

        assert!(matches!(
            storage.validate_directory_manifest(),
            Err(StorageError::InvalidJournal(_))
        ));

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
    fn targeted_scanner_file_removal_commits_without_a_manifest_row() {
        let path = temp_db_path("targeted-file-removal-contract");
        let storage = SqliteStorage::open(path.clone()).unwrap();
        let root = TempDir::new().unwrap();
        let root_text = root.path().to_string_lossy().into_owned();
        storage
            .replace_directory_manifest(
                root.path(),
                &[fingerprint(&root_text, None, &root_text, 1)],
            )
            .unwrap();
        let scanner = TargetedIndexScanner::new(
            IndexPathRules::from_plan(&IndexScanPlan {
                include_roots: vec![root.path().to_path_buf()],
                ..IndexScanPlan::default()
            })
            .unwrap(),
        );
        let removed = root.path().join("gone.md");
        let scan = scanner.scan_removed_paths(std::slice::from_ref(&removed));

        storage
            .commit_incremental_batch(
                &CommittedIndexDelta {
                    generation: 1,
                    upserts: scan.upserts,
                    removals: scan.removals,
                },
                &scan.manifest_upserts,
                &scan.manifest_removals,
            )
            .unwrap();

        assert_eq!(
            storage.committed_index_deltas_after(0).unwrap()[0].removals,
            vec![removed]
        );
        assert_eq!(
            storage.directory_manifest_for_root(root.path()).unwrap(),
            vec![fingerprint(&root_text, None, &root_text, 1)]
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
    fn payload_hash_is_a_fixed_hex_digest_without_path_content() {
        let path = temp_db_path("journal-payload-digest-privacy");
        let storage = SqliteStorage::open(path.clone()).unwrap();
        let private_path = "/root/private/customer-secret-document.md";
        storage
            .commit_incremental_batch(
                &committed_delta(1, indexed_entry(private_path), Vec::new()),
                &[],
                &[],
            )
            .unwrap();

        let digest = storage
            .connection
            .query_row(
                "SELECT payload_hash FROM index_delta_batches WHERE generation = 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap();

        assert_eq!(digest.len(), 32);
        assert!(digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert!(!digest.contains(private_path));
        assert!(!digest.contains("customer-secret-document"));

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
    fn runtime_state_rejects_baseline_generation_ahead_of_last_generation() {
        let path = temp_db_path("invalid-runtime-generation-order");
        let storage = SqliteStorage::open(path.clone()).unwrap();

        let error = storage
            .save_runtime_state(&IncrementalRuntimeState {
                active_baseline_id: None,
                baseline_generation: 2,
                last_generation: 1,
                degradation_code: None,
                baseline_refresh_reason: None,
            })
            .unwrap_err();

        assert!(matches!(error, StorageError::InvalidJournal(_)));
        assert!(storage.runtime_state().unwrap().is_none());

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
            .activate_baseline_with_manifest_and_clear_incremental_state(
                baseline_id,
                2,
                &[fingerprint("/root", None, "/root", 10)],
            )
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
        assert_eq!(
            storage
                .directory_manifest_for_root(Path::new("/root"))
                .unwrap(),
            vec![fingerprint("/root", None, "/root", 10)]
        );
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
    fn highest_committed_generation_includes_queued_publish_before_refresh_handoff() {
        let path = temp_db_path("refresh-generation-handoff");
        let storage = SqliteStorage::open(path.clone()).unwrap();
        let old_id = storage
            .save_completed_index_batch(1, &[indexed_entry("/root/old.md")])
            .unwrap();
        storage.activate_baseline(old_id, 0).unwrap();
        storage
            .commit_incremental_batch(
                &committed_delta(1, indexed_entry("/root/new.md"), Vec::new()),
                &[],
                &[],
            )
            .unwrap();

        let stable_generation = storage.highest_committed_generation().unwrap();
        assert_eq!(stable_generation, 1);
        let refreshed_id = storage
            .save_completed_index_batch(2, &[indexed_entry("/root/new.md")])
            .unwrap();
        storage
            .activate_baseline_with_manifest_and_clear_incremental_state(
                refreshed_id,
                stable_generation,
                &[fingerprint("/root", None, "/root", 2)],
            )
            .unwrap();

        assert!(storage.committed_index_deltas_after(0).unwrap().is_empty());
        assert_eq!(
            storage
                .runtime_state()
                .unwrap()
                .unwrap()
                .baseline_generation,
            stable_generation
        );

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn activation_replays_successor_directory_create_into_stale_manifest_before_restart() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let created = root.join("created");
        fs::create_dir_all(&created).unwrap();
        let database_path = temp.path().join("activation-create.sqlite");
        let storage = SqliteStorage::open(database_path.clone()).unwrap();
        let initial_baseline_id = storage.save_completed_index_batch(1, &[]).unwrap();
        let stale_manifest = baseline_manifest_from_entries(&[], std::slice::from_ref(&root));
        storage
            .activate_baseline_with_manifest_and_clear_incremental_state(
                initial_baseline_id,
                0,
                &stale_manifest,
            )
            .unwrap();
        let created_entry =
            IndexedEntry::from_path_metadata(&created, &root, IndexedEntryKind::Directory);
        storage
            .commit_incremental_batch(
                &CommittedIndexDelta {
                    generation: 1,
                    upserts: vec![created_entry],
                    removals: Vec::new(),
                },
                &[fingerprint(
                    &created.to_string_lossy(),
                    Some(&root.to_string_lossy()),
                    &root.to_string_lossy(),
                    1,
                )],
                &[],
            )
            .unwrap();
        let baseline_id = storage.save_completed_index_batch(2, &[]).unwrap();

        storage
            .activate_baseline_with_manifest_and_clear_incremental_state(
                baseline_id,
                0,
                &stale_manifest,
            )
            .unwrap();
        drop(storage);

        let reopened = SqliteStorage::open(database_path).unwrap();
        let manifest = reopened.directory_manifest_for_root(&root).unwrap();
        assert!(manifest
            .iter()
            .any(|row| row.path == created.to_string_lossy()));
        assert_eq!(reopened.committed_index_deltas_after(0).unwrap().len(), 1);
    }

    #[test]
    fn activation_replays_successor_directory_delete_into_stale_manifest_before_restart() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let deleted = root.join("deleted");
        fs::create_dir_all(&deleted).unwrap();
        let database_path = temp.path().join("activation-delete.sqlite");
        let storage = SqliteStorage::open(database_path.clone()).unwrap();
        let deleted_entry =
            IndexedEntry::from_path_metadata(&deleted, &root, IndexedEntryKind::Directory);
        let initial_baseline_id = storage
            .save_completed_index_batch(1, std::slice::from_ref(&deleted_entry))
            .unwrap();
        let stale_manifest =
            baseline_manifest_from_entries(&[deleted_entry], std::slice::from_ref(&root));
        storage
            .activate_baseline_with_manifest_and_clear_incremental_state(
                initial_baseline_id,
                0,
                &stale_manifest,
            )
            .unwrap();
        storage
            .commit_incremental_batch(
                &CommittedIndexDelta {
                    generation: 1,
                    upserts: Vec::new(),
                    removals: vec![deleted.clone()],
                },
                &[],
                std::slice::from_ref(&deleted),
            )
            .unwrap();
        fs::remove_dir_all(&deleted).unwrap();
        let baseline_id = storage
            .save_completed_index_batch(
                2,
                &[IndexedEntry::from_path_metadata(
                    &deleted,
                    &root,
                    IndexedEntryKind::Directory,
                )],
            )
            .unwrap();

        storage
            .activate_baseline_with_manifest_and_clear_incremental_state(
                baseline_id,
                0,
                &stale_manifest,
            )
            .unwrap();
        drop(storage);

        let reopened = SqliteStorage::open(database_path).unwrap();
        let manifest = reopened.directory_manifest_for_root(&root).unwrap();
        assert!(!manifest
            .iter()
            .any(|row| row.path == deleted.to_string_lossy()));
        assert_eq!(reopened.committed_index_deltas_after(0).unwrap().len(), 1);
    }

    #[test]
    fn activation_preserves_committed_parent_fingerprint_for_successor_file_change() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        fs::create_dir_all(&root).unwrap();
        let database_path = temp.path().join("activation-file-parent.sqlite");
        let storage = SqliteStorage::open(database_path.clone()).unwrap();
        let stale_manifest = vec![fingerprint(
            &root.to_string_lossy(),
            None,
            &root.to_string_lossy(),
            1,
        )];
        let initial_baseline_id = storage.save_completed_index_batch(1, &[]).unwrap();
        storage
            .activate_baseline_with_manifest_and_clear_incremental_state(
                initial_baseline_id,
                0,
                &stale_manifest,
            )
            .unwrap();

        let created = root.join("created.txt");
        fs::write(&created, "created after snapshot").unwrap();
        storage
            .commit_incremental_batch(
                &CommittedIndexDelta {
                    generation: 1,
                    upserts: vec![IndexedEntry::from_path_metadata(
                        &created,
                        &root,
                        IndexedEntryKind::File,
                    )],
                    removals: Vec::new(),
                },
                &[fingerprint(
                    &root.to_string_lossy(),
                    None,
                    &root.to_string_lossy(),
                    99,
                )],
                &[],
            )
            .unwrap();
        let baseline_id = storage.save_completed_index_batch(2, &[]).unwrap();

        storage
            .activate_baseline_with_manifest_and_clear_incremental_state(
                baseline_id,
                0,
                &stale_manifest,
            )
            .unwrap();
        drop(storage);

        let reopened = SqliteStorage::open(database_path).unwrap();
        let manifest = reopened.directory_manifest_for_root(&root).unwrap();
        assert_eq!(manifest.len(), 1);
        assert_eq!(manifest[0].modified_ms, Some(99));
        assert_eq!(reopened.committed_index_deltas_after(0).unwrap().len(), 1);
    }

    #[test]
    fn activation_preserves_committed_parent_fingerprint_for_file_delete_and_rename() {
        for (case, upsert_name, removals, expected_modified) in [
            ("delete", None, vec!["old.txt"], 101),
            ("rename", Some("new.txt"), vec!["old.txt"], 102),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("root");
            fs::create_dir_all(&root).unwrap();
            let old = root.join("old.txt");
            fs::write(&old, "old").unwrap();
            let database_path = temp.path().join(format!("activation-file-{case}.sqlite"));
            let storage = SqliteStorage::open(database_path.clone()).unwrap();
            let old_entry = IndexedEntry::from_path_metadata(&old, &root, IndexedEntryKind::File);
            let stale_manifest = vec![fingerprint(
                &root.to_string_lossy(),
                None,
                &root.to_string_lossy(),
                1,
            )];
            let initial_baseline_id = storage
                .save_completed_index_batch(1, std::slice::from_ref(&old_entry))
                .unwrap();
            storage
                .activate_baseline_with_manifest_and_clear_incremental_state(
                    initial_baseline_id,
                    0,
                    &stale_manifest,
                )
                .unwrap();
            fs::remove_file(&old).unwrap();
            let upserts = upsert_name
                .map(|name| {
                    let path = root.join(name);
                    fs::write(&path, "new").unwrap();
                    vec![IndexedEntry::from_path_metadata(
                        &path,
                        &root,
                        IndexedEntryKind::File,
                    )]
                })
                .unwrap_or_default();
            storage
                .commit_incremental_batch(
                    &CommittedIndexDelta {
                        generation: 1,
                        upserts,
                        removals: removals.iter().map(|name| root.join(name)).collect(),
                    },
                    &[fingerprint(
                        &root.to_string_lossy(),
                        None,
                        &root.to_string_lossy(),
                        expected_modified,
                    )],
                    &[],
                )
                .unwrap();
            let baseline_id = storage
                .save_completed_index_batch(2, std::slice::from_ref(&old_entry))
                .unwrap();
            storage
                .activate_baseline_with_manifest_and_clear_incremental_state(
                    baseline_id,
                    0,
                    &stale_manifest,
                )
                .unwrap();
            drop(storage);

            let reopened = SqliteStorage::open(database_path).unwrap();
            let manifest = reopened.directory_manifest_for_root(&root).unwrap();
            assert_eq!(manifest[0].modified_ms, Some(expected_modified));
            assert_eq!(reopened.committed_index_deltas_after(0).unwrap().len(), 1);
        }
    }

    #[test]
    fn failed_revision_restore_discards_candidate_tail_and_rewinds_generation() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        fs::create_dir_all(&root).unwrap();
        let storage = SqliteStorage::open(temp.path().join("revision-rollback.sqlite")).unwrap();
        let old_manifest = vec![fingerprint(
            &root.to_string_lossy(),
            None,
            &root.to_string_lossy(),
            7,
        )];
        let rollback_id = storage.save_completed_index_batch(1, &[]).unwrap();
        storage
            .activate_baseline_with_manifest_and_clear_incremental_state(
                rollback_id,
                3,
                &old_manifest,
            )
            .unwrap();
        storage
            .commit_incremental_batch(
                &CommittedIndexDelta {
                    generation: 4,
                    upserts: vec![indexed_entry(&root.join("candidate.txt").to_string_lossy())],
                    removals: Vec::new(),
                },
                &[fingerprint(
                    &root.to_string_lossy(),
                    None,
                    &root.to_string_lossy(),
                    99,
                )],
                &[],
            )
            .unwrap();

        storage
            .restore_baseline_after_failed_revision(rollback_id, 3, &old_manifest)
            .unwrap();

        assert!(storage.committed_index_deltas_after(3).unwrap().is_empty());
        assert_eq!(storage.highest_committed_generation().unwrap(), 3);
        assert_eq!(
            storage.directory_manifest_for_root(&root).unwrap(),
            old_manifest
        );
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
    fn known_direct_children_use_constant_queries_for_many_unrelated_batches() {
        let path = temp_db_path("known-direct-targeted-queries");
        let storage = SqliteStorage::open(path.clone()).unwrap();
        storage
            .save_completed_index_batch(10, &[indexed_entry("/root/direct.md")])
            .unwrap();
        for generation in 1..=200 {
            let mut entry = indexed_entry(&format!("/other/dir-{generation}/file.md"));
            entry.root = "/other".to_owned();
            entry.parent = format!("/other/dir-{generation}");
            storage
                .commit_incremental_batch(&committed_delta(generation, entry, Vec::new()), &[], &[])
                .unwrap();
        }
        let query_count = Cell::new(0);

        let children = storage
            .known_direct_indexed_children_with_query_probe(
                Path::new("/root"),
                Path::new("/root"),
                || query_count.set(query_count.get() + 1),
            )
            .unwrap();

        assert_eq!(children.len(), 1);
        assert_eq!(children[0].path, "/root/direct.md");
        assert_eq!(query_count.get(), 4);

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn targeted_journal_query_plan_does_not_walk_batches_to_scan_unrelated_entries() {
        let path = temp_db_path("known-direct-targeted-query-plan");
        let storage = SqliteStorage::open(path.clone()).unwrap();
        let direct_explain = format!("EXPLAIN QUERY PLAN {TARGETED_DIRECT_JOURNAL_SQL}");
        let mut direct_statement = storage.connection.prepare(&direct_explain).unwrap();
        let direct_rows = direct_statement
            .query_map(params![0, "/changed/nested", "/"], |row| {
                row.get::<_, String>(3)
            })
            .unwrap();
        let direct_plan = direct_rows.collect::<Result<Vec<_>, _>>().unwrap();

        let ancestors = normalized_ancestor_keys("/changed/nested");
        let ancestor_explain = format!(
            "EXPLAIN QUERY PLAN {}",
            targeted_ancestor_journal_sql(ancestors.len())
        );
        let mut parameters = vec![Value::Integer(0)];
        parameters.extend(ancestors.into_iter().map(Value::Text));
        let mut ancestor_statement = storage.connection.prepare(&ancestor_explain).unwrap();
        let ancestor_rows = ancestor_statement
            .query_map(params_from_iter(parameters), |row| row.get::<_, String>(3))
            .unwrap();
        let ancestor_plan = ancestor_rows.collect::<Result<Vec<_>, _>>().unwrap();

        assert!(
            direct_plan
                .iter()
                .any(|detail| detail.contains("idx_index_delta_entries_parent_batch")),
            "direct journal plan must use the parent index: {direct_plan:?}"
        );
        assert!(
            ancestor_plan
                .iter()
                .any(|detail| detail.contains("idx_index_delta_entries_path")),
            "ancestor journal plan must use the path index: {ancestor_plan:?}"
        );

        drop(direct_statement);
        drop(ancestor_statement);
        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn many_unrelated_journal_rows_are_not_returned_for_multiple_changed_directories() {
        let path = temp_db_path("known-direct-production-scale-targeting");
        let storage = SqliteStorage::open(path.clone()).unwrap();
        let baseline = (0..5)
            .map(|ordinal| {
                let mut entry = indexed_entry(&format!("/root/changed-{ordinal}/direct.md"));
                entry.parent = format!("/root/changed-{ordinal}");
                entry
            })
            .collect::<Vec<_>>();
        storage.save_completed_index_batch(10, &baseline).unwrap();
        for generation in 1..=5_000 {
            let mut entry = indexed_entry(&format!("/other/dir-{generation}/file.md"));
            entry.root = "/other".to_owned();
            entry.parent = format!("/other/dir-{generation}");
            storage
                .commit_incremental_batch(&committed_delta(generation, entry, Vec::new()), &[], &[])
                .unwrap();
        }
        let query_count = Cell::new(0);
        let journal_rows = Cell::new(0);

        for ordinal in 0..5 {
            let children = storage
                .known_direct_indexed_children_with_probes(
                    Path::new("/root"),
                    Path::new(&format!("/root/changed-{ordinal}")),
                    || query_count.set(query_count.get() + 1),
                    |rows| journal_rows.set(journal_rows.get() + rows),
                )
                .unwrap();
            assert_eq!(children.len(), 1);
        }

        assert_eq!(query_count.get(), 20);
        assert_eq!(journal_rows.get(), 0);

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn posix_root_removal_clears_known_children_of_a_nested_directory() {
        let path = temp_db_path("known-direct-posix-root-removal");
        let storage = SqliteStorage::open(path.clone()).unwrap();
        let mut child = indexed_entry("/nested/direct.md");
        child.root = "/".to_owned();
        child.parent = "/nested".to_owned();
        storage.save_completed_index_batch(10, &[child]).unwrap();
        storage
            .commit_incremental_batch(
                &CommittedIndexDelta {
                    generation: 1,
                    upserts: Vec::new(),
                    removals: vec![PathBuf::from("/")],
                },
                &[],
                &[],
            )
            .unwrap();

        let children = storage
            .known_direct_indexed_children(Path::new("/"), Path::new("/nested"))
            .unwrap();

        assert!(children.is_empty());

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn posix_root_file_upsert_clears_known_children_of_a_nested_directory() {
        let path = temp_db_path("known-direct-posix-root-upsert");
        let storage = SqliteStorage::open(path.clone()).unwrap();
        let mut child = indexed_entry("/nested/direct.md");
        child.root = "/".to_owned();
        child.parent = "/nested".to_owned();
        storage.save_completed_index_batch(10, &[child]).unwrap();
        let mut root_file = indexed_entry("/placeholder");
        root_file.path = "/".to_owned();
        root_file.name = "/".to_owned();
        root_file.root = "/".to_owned();
        root_file.parent.clear();
        storage
            .commit_incremental_batch(
                &CommittedIndexDelta {
                    generation: 1,
                    upserts: vec![root_file],
                    removals: Vec::new(),
                },
                &[],
                &[],
            )
            .unwrap();

        let children = storage
            .known_direct_indexed_children(Path::new("/"), Path::new("/nested"))
            .unwrap();

        assert!(children.is_empty());

        drop(storage);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn targeted_journal_ancestor_lookup_respects_path_segment_boundaries() {
        let path = temp_db_path("known-direct-ancestor-segment-boundary");
        let storage = SqliteStorage::open(path.clone()).unwrap();
        let mut child = indexed_entry("/foobar/direct.md");
        child.root = "/".to_owned();
        child.parent = "/foobar".to_owned();
        storage.save_completed_index_batch(10, &[child]).unwrap();
        storage
            .commit_incremental_batch(
                &CommittedIndexDelta {
                    generation: 1,
                    upserts: Vec::new(),
                    removals: vec![PathBuf::from("/foo")],
                },
                &[],
                &[],
            )
            .unwrap();

        let children = storage
            .known_direct_indexed_children(Path::new("/"), Path::new("/foobar"))
            .unwrap();

        assert_eq!(children.len(), 1);
        assert_eq!(children[0].path, "/foobar/direct.md");

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
    fn windows_manifest_round_trips_display_paths_for_drives_and_unc_roots() {
        let path = temp_db_path("windows-manifest-display-paths");
        let storage =
            SqliteStorage::open_with_comparison_mode(path.clone(), PathComparisonMode::Windows)
                .unwrap();
        let fixtures = [
            (
                r"C:\Users\Alice",
                vec![
                    fingerprint(r"C:\Users\Alice", None, r"C:\Users\Alice", 1),
                    fingerprint(
                        r"C:\Users\Alice\Docs",
                        Some(r"C:\Users\Alice"),
                        r"C:\Users\Alice",
                        2,
                    ),
                ],
            ),
            (
                r"D:\Data",
                vec![fingerprint(r"D:\Data", None, r"D:\Data", 3)],
            ),
            (
                r"\\Server\Share",
                vec![fingerprint(r"\\Server\Share", None, r"\\Server\Share", 4)],
            ),
        ];
        for (root, rows) in &fixtures {
            storage
                .replace_directory_manifest(Path::new(root), rows)
                .unwrap();
        }

        assert_eq!(
            storage
                .directory_manifest_for_root(Path::new("c:/users/alice"))
                .unwrap(),
            fixtures[0].1.clone()
        );
        assert_eq!(
            storage
                .directory_manifest_for_root(Path::new("d:/DATA"))
                .unwrap(),
            fixtures[1].1.clone()
        );
        assert_eq!(
            storage
                .directory_manifest_for_root(Path::new("//server/share"))
                .unwrap(),
            fixtures[2].1.clone()
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

    #[test]
    fn baseline_key_backfill_processes_rows_in_bounded_batches() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                r#"
                CREATE TABLE index_entries (
                    path TEXT NOT NULL,
                    root TEXT,
                    parent TEXT,
                    path_key TEXT,
                    root_key TEXT,
                    parent_key TEXT
                );
                "#,
            )
            .unwrap();
        {
            let transaction = connection.unchecked_transaction().unwrap();
            let mut insert = transaction
                .prepare(
                    "INSERT INTO index_entries (path, root, parent) VALUES (?1, '/root', '/root')",
                )
                .unwrap();
            for ordinal in 0..2_505 {
                insert
                    .execute(params![format!("/root/{ordinal}.md")])
                    .unwrap();
            }
            drop(insert);
            transaction.commit().unwrap();
        }
        let batches = RefCell::new(Vec::new());

        SqliteStorage::backfill_index_entry_keys_in_batches(
            &connection,
            PathComparisonMode::Native,
            |batch_size| batches.borrow_mut().push(batch_size),
        )
        .unwrap();

        assert_eq!(batches.borrow().iter().sum::<usize>(), 2_505);
        assert!(batches.borrow().iter().all(|batch| *batch <= 1_000));
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM index_entries WHERE path_key IS NOT NULL AND root_key IS NOT NULL AND parent_key IS NOT NULL",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            2_505
        );
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
