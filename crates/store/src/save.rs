//! Transactional saving of an already initialized version-2 library.

use std::{path::Path, time::Duration};

use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior};
use tmux_workspaces_core::Layout;

use crate::{Document, Error, Reconciliation, Store};

impl Store {
    /// Open an existing initialized library for transactional updates. This never
    /// creates a database, imports legacy tables or changes the journal mode.
    /// The product's read-only connection is never upgraded implicitly.
    pub fn open_existing_writable(directory: &Path) -> Result<Self, Error> {
        let path = directory.join("layouts.db");
        let connection = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_WRITE)
            .map_err(|error| Error {
                path: path.clone(),
                detail: error.to_string(),
            })?;
        let store = Self { connection, path };
        store
            .connection
            .set_limit(
                rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH,
                (crate::MAX_RECORD_BYTES + 1024) as i32,
            )
            .map_err(|error| store.failure(error))?;
        store
            .connection
            .busy_timeout(Duration::from_secs(5))
            .map_err(|error| store.failure(error))?;
        // Bundled SQLite enables this by default; Python's connection does not.
        // REPLACE must not cascade deletes into unrelated extension tables.
        store
            .connection
            .pragma_update(None, "foreign_keys", false)
            .map_err(|error| store.failure(error))?;
        store.check_writable_schema()?;
        store.load()?;
        Ok(store)
    }

    /// Save edits against the current row under one immediate transaction.
    /// On success, apply the returned document to the local model. A conflict
    /// writes nothing, advances `base` to the latest row and returns that row with
    /// local navigation retained; tell the user to retry the conflicting edit.
    /// Other errors leave `base` and the caller's local model untouched. A successful
    /// commit advances `base` only after SQLite has committed the merged document.
    pub fn save(&mut self, base: &mut Document, local: &Layout) -> Result<Reconciliation, Error> {
        // Validate local edits before acquiring the write lock.
        base.with_model(local, &self.path)?;
        let transaction =
            Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)
                .map_err(|error| self.failure(error))?;
        self.check_writable_schema()?;
        let latest = self.load()?;
        let result = base.reconcile(local, &latest, &self.path)?;
        if result.conflict {
            transaction
                .rollback()
                .map_err(|error| self.failure(error))?;
            *base = latest;
        } else {
            // Allocate the next base before committing, so no fallible operation
            // remains between a successful commit and updating the caller's base.
            let committed = result.document.clone();
            transaction
                .execute(
                    "INSERT OR REPLACE INTO terminal_layout (id, value) VALUES (1, ?1)",
                    [committed.raw_json()],
                )
                .map_err(|error| self.failure(error))?;
            transaction.commit().map_err(|error| self.failure(error))?;
            *base = committed;
        }
        Ok(result)
    }

    fn check_writable_schema(&self) -> Result<(), Error> {
        let mut statement = self
            .connection
            .prepare("PRAGMA main.table_xinfo('terminal_layout')")
            .map_err(|error| self.failure(error))?;
        let columns = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?.to_ascii_uppercase(),
                    row.get::<_, bool>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                ))
            })
            .map_err(|error| self.failure(error))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|error| self.failure(error))?;
        let expected = vec![
            ("id".into(), "INTEGER".into(), false, None, 1, 0),
            ("value".into(), "TEXT".into(), false, None, 0, 0),
        ];
        let extra_objects: bool = self
            .connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE tbl_name='terminal_layout' COLLATE NOCASE AND type IN ('trigger', 'index'))",
                [],
                |row| row.get(0),
            )
            .map_err(|error| self.failure(error))?;
        let definition: String = self
            .connection
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE type='table' AND name='terminal_layout'",
                [],
                |row| row.get(0),
            )
            .map_err(|error| self.failure(error))?;
        // Column metadata cannot reveal executable CHECK constraints. Only the
        // supported table declaration may be written; preserve other libraries.
        let canonical = definition
            .chars()
            .filter(|c| !c.is_ascii_whitespace())
            .collect::<String>()
            .to_ascii_lowercase();
        if columns != expected
            || extra_objects
            || canonical != "createtableterminal_layout(idintegerprimarykey,valuetext)"
        {
            return Err(self
                .failure("transactional saving requires the initialized version-2 table schema"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writer_foreign_key_setting_matches_the_reference_runtime() {
        let directory = tempfile::tempdir().unwrap();
        let database = Connection::open(directory.path().join("layouts.db")).unwrap();
        database.execute_batch(r#"CREATE TABLE terminal_layout(id INTEGER PRIMARY KEY, value TEXT);
            INSERT INTO terminal_layout VALUES(1, '{"version":2,"selected":"000000000001","focus":false,"workspaces":[{"id":"000000000001","name":"Workspace","selected":"","tabs":[]}]}');"#).unwrap();
        let writer = Store::open_existing_writable(directory.path()).unwrap();
        let enabled: bool = writer
            .connection
            .pragma_query_value(None, "foreign_keys", |row| row.get(0))
            .unwrap();
        println!(
            "Rust SQLite {}, writer foreign_keys={enabled}",
            rusqlite::version()
        );
        assert!(!enabled);
    }
}
