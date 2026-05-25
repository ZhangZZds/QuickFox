//! SQLite storage will live here.

use crate::core::index::{IndexedEntry, IndexedEntryKind};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::PathBuf;

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
}

#[derive(Debug)]
pub enum StorageError {
    Sqlite(rusqlite::Error),
}

impl From<rusqlite::Error> for StorageError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

pub struct SqliteStorage {
    connection: Connection,
}

impl SqliteStorage {
    pub fn open(path: PathBuf) -> Result<Self, StorageError> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let connection = Connection::open(path)?;
        let storage = Self { connection };
        storage.migrate()?;
        Ok(storage)
    }

    fn migrate(&self) -> Result<(), StorageError> {
        self.connection.execute_batch(
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
            "#,
        )?;
        Ok(())
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
    ) -> Result<(), StorageError> {
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
                    (batch_id, path, name, kind, search_text, updated_at_ms)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6)
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
                ])?;
            }
        }

        transaction.commit()?;
        Ok(())
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

        let mut statement = self.connection.prepare(
            r#"
            SELECT path, name, kind
            FROM index_entries
            WHERE batch_id = ?1
            ORDER BY path ASC, name ASC
            "#,
        )?;
        let rows = statement.query_map(params![batch_id], |row| {
            Ok(IndexedEntry {
                path: row.get(0)?,
                name: row.get(1)?,
                kind: index_kind_from_storage(row.get::<_, String>(2)?.as_str()),
            })
        })?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }

        Ok(Some(IndexSnapshot {
            completed_at_ms,
            entries,
        }))
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

fn searchable_text(entry: &IndexedEntry) -> String {
    format!("{} {}", entry.name, entry.path).to_lowercase()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::index::{IndexedEntry, IndexedEntryKind};
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

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
                    },
                    IndexedEntry {
                        path: "/home/frank/notes.md".to_owned(),
                        name: "notes.md".to_owned(),
                        kind: IndexedEntryKind::File,
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
            }]
        );

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
