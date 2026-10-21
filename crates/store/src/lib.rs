//! Access to the Python-compatible schema-v2 `layouts.db` library.
//! Opening is read-only by default. Explicit writable access requires an existing
//! initialized library; no operation creates or migrates a database or starts a shell.

#![forbid(unsafe_code)]

mod json;
mod project;
mod reconcile;
pub use reconcile::Reconciliation;
mod save;
mod schema;

use rusqlite::{Connection, OpenFlags, OptionalExtension, types::ValueRef};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tmux_workspaces_core::Layout;

/// Maximum UTF-8 size of the persisted JSON record.
pub const MAX_RECORD_BYTES: usize = 16 * 1024 * 1024;

/// A library failure. The path identifies the database to preserve.
#[derive(Debug, thiserror::Error)]
#[error("Workspace library {path}: {detail}. No automatic repair was attempted.")]
pub struct Error {
    /// Database path.
    pub path: PathBuf,
    /// SQLite or schema diagnostic.
    pub detail: String,
}

/// A validated schema-v2 document and its independent in-memory model.
#[derive(Clone, Debug)]
pub struct Document {
    raw: String,
    model: Layout,
}

impl Document {
    /// Validate a JSON record without accessing SQLite. `path` is diagnostic only.
    pub fn parse(raw: String, path: &Path) -> Result<Self, Error> {
        let fail = |detail| Error {
            path: path.to_owned(),
            detail,
        };
        if raw.len() > MAX_RECORD_BYTES {
            return Err(fail(
                "layout record must be text no larger than 16 MiB".into(),
            ));
        }
        let value = json::decode(&raw).map_err(|error| fail(error.to_string()))?;
        let model = schema::layout(&value).map_err(fail)?;
        Ok(Self { raw, model })
    }

    /// Apply in-memory model edits while retaining extension fields by stable identity.
    /// Unchanged models retain the exact original JSON. Changed documents may change
    /// JSON whitespace/key order; untouched values and optional defaults are retained.
    /// The complete result is validated and bounded before returning. This performs
    /// no SQLite or filesystem I/O; `path` is diagnostic only.
    pub fn with_model(&self, model: &Layout, path: &Path) -> Result<Self, Error> {
        if model == &self.model {
            return Self::parse(self.raw.clone(), path);
        }
        let projected = project::apply(&self.raw, model).map_err(|detail| Error {
            path: path.to_owned(),
            detail,
        })?;
        Self::parse(projected, path)
    }

    /// Original JSON, preserving unknown fields, number spellings and absent/null distinctions.
    pub fn raw_json(&self) -> &str {
        &self.raw
    }

    /// Validated workspace and split model. Stored navigation only seeds a new window.
    pub fn model(&self) -> &Layout {
        &self.model
    }
}

/// One persistent connection, read-only by default. Statements finish before each call returns;
/// a caller may keep this on its storage worker without holding a read lock.
pub struct Store {
    connection: Connection,
    path: PathBuf,
}

impl Store {
    /// Open `directory/layouts.db` without creating directories, tables or files.
    pub fn open(directory: &Path) -> Result<Self, Error> {
        let path = directory.join("layouts.db");
        let connection = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|error| Error {
                path: path.clone(),
                detail: error.to_string(),
            })?;
        connection
            .busy_timeout(Duration::from_millis(250))
            .map_err(|error| Error {
                path: path.clone(),
                detail: error.to_string(),
            })?;
        Ok(Self { connection, path })
    }

    fn failure(&self, detail: impl ToString) -> Error {
        Error {
            path: self.path.clone(),
            detail: detail.to_string(),
        }
    }

    fn read<T>(&self, mut query: impl FnMut() -> rusqlite::Result<T>) -> Result<T, Error> {
        let deadline = Instant::now() + Duration::from_millis(250);
        loop {
            match query() {
                Ok(value) => return Ok(value),
                Err(rusqlite::Error::SqliteFailure(code, _))
                    if code.extended_code == rusqlite::ffi::SQLITE_READONLY_ROLLBACK =>
                {
                    if Instant::now() >= deadline {
                        return Err(self.failure("database recovery is needed; preserve layouts.db and its journal files"));
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(error) => return Err(self.failure(error)),
            }
        }
    }

    /// Read and validate the current arrangement. Legacy-only libraries require
    /// initialization by the Python tool; malformed records are never repaired.
    pub fn load(&self) -> Result<Document, Error> {
        let initialized = self.read(|| self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='terminal_layout')",
            [], |row| row.get::<_, bool>(0),
        ))?;
        if !initialized {
            return Err(self.failure(
                "terminal_layout table is missing; open this library once with the Python tool",
            ));
        }
        // Check byte length in the same statement that reads the value, preventing
        // a concurrent writer from replacing a small record after a size precheck.
        let raw = self.read(|| self.connection.query_row(
            "SELECT CASE WHEN typeof(value) = 'text' AND length(CAST(value AS BLOB)) <= ?1 THEN value ELSE NULL END FROM terminal_layout WHERE id = 1",
            [MAX_RECORD_BYTES as i64],
            |row| match row.get_ref(0)? {
                ValueRef::Text(text) => Ok(std::str::from_utf8(text).ok().map(str::to_owned)),
                _ => Ok(None),
            },
        ).optional())?
            .ok_or_else(|| self.failure("current layout row is missing"))?
            .ok_or_else(|| self.failure("layout record must be UTF-8 text no larger than 16 MiB"))?;
        Document::parse(raw, &self.path)
    }

    /// A same-connection token that changes when another connection commits.
    /// Read it before `load`; if a writer races that load, the next poll still
    /// observes the change. Tokens from different connections are not comparable.
    pub fn data_version(&self) -> Result<i64, Error> {
        self.read(|| {
            self.connection
                .query_row("PRAGMA data_version", [], |row| row.get(0))
        })
    }
}
