//! Local, immutable HTML artifacts. Publishing needs no GUI, tmux or daemon.

use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// Maximum UTF-8 HTML document size, including inline assets.
pub const HTML_LIMIT: usize = 4 * 1024 * 1024;
const META_LIMIT: usize = 8192;
const CATALOG_LIMIT: usize = 200;

/// An artifact's retained metadata. The body is a sibling `index.html` file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    /// Opaque publication ID.
    pub id: String,
    /// Human-readable title.
    pub title: String,
    /// Author-supplied attribution, not an authenticated identity.
    pub author: String,
    /// Publication time in Unix milliseconds.
    pub created_ms: u64,
}

/// The newest retained artifact metadata, bounded independently of HTML bodies.
#[derive(Default)]
pub struct Catalog {
    /// Available documents, newest first.
    pub entries: Vec<Entry>,
    /// Whether the catalog exceeded the number displayed.
    pub more: bool,
    /// Files that could not be read as artifact metadata.
    pub unreadable: usize,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn root(directory: Option<&Path>) -> io::Result<PathBuf> {
    crate::library::location::library_location(directory)
        .map(|path| path.join("artifacts"))
        .map_err(|error| invalid(&error.to_string()))
}

fn read_bounded(path: &Path, limit: usize) -> io::Result<Vec<u8>> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > limit as u64 {
        return Err(invalid(
            "Expected a regular file within the artifact size limit.",
        ));
    }
    let mut data = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut data)?;
    if data.len() > limit {
        return Err(invalid("Artifact exceeds the size limit."));
    }
    Ok(data)
}

fn valid_id(id: &str) -> bool {
    id.len() == 36 && uuid::Uuid::parse_str(id).is_ok()
}

fn valid_label(text: &str, limit: usize) -> bool {
    !text.trim().is_empty() && text.chars().count() <= limit && !text.chars().any(char::is_control)
}

/// Copy a self-contained HTML file into the selected library atomically.
/// The document can use inline CSS, JavaScript, SVG, canvas and data URLs.
pub fn publish(
    directory: Option<&Path>,
    file: &Path,
    title: &str,
    author: &str,
) -> io::Result<Entry> {
    if !valid_label(title, 200) || !valid_label(author, 100) {
        return Err(invalid(
            "Use a nonempty title (up to 200 characters) and author (up to 100), without control characters.",
        ));
    }
    let bytes = read_bounded(file, HTML_LIMIT)?;
    std::str::from_utf8(&bytes).map_err(|_| invalid("HTML must be UTF-8."))?;
    if bytes.is_empty() {
        return Err(invalid("HTML is empty."));
    }
    let root = root(directory)?;
    fs::create_dir_all(&root)?;
    let entry = Entry {
        id: uuid::Uuid::new_v4().to_string(),
        title: title.into(),
        author: author.into(),
        created_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
    };
    let staged = tempfile::Builder::new()
        .prefix(".publish-")
        .tempdir_in(&root)?;
    for (name, data) in [
        ("index.html", bytes),
        ("metadata.json", serde_json::to_vec(&entry)?),
    ] {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(staged.path().join(name))?;
        file.write_all(&data)?;
        file.sync_all()?;
    }
    fs::rename(staged.path(), root.join(&entry.id))?;
    Ok(entry)
}

/// Read metadata only. Missing storage is an empty catalog, never initialized.
pub fn catalog(directory: Option<&Path>) -> io::Result<Catalog> {
    let root = root(directory)?;
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Catalog::default()),
        Err(error) => return Err(error),
    };
    let mut catalog = Catalog::default();
    for entry in entries {
        let entry = entry?;
        let Some(id) = entry
            .file_name()
            .to_str()
            .map(str::to_owned)
            .filter(|id| valid_id(id))
        else {
            continue;
        };
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let meta = read_bounded(&entry.path().join("metadata.json"), META_LIMIT)
            .and_then(|bytes| serde_json::from_slice::<Entry>(&bytes).map_err(io::Error::other));
        match meta {
            Ok(meta)
                if meta.id == id
                    && valid_label(&meta.title, 200)
                    && valid_label(&meta.author, 100) =>
            {
                catalog.entries.push(meta);
                catalog
                    .entries
                    .sort_by(|a, b| b.created_ms.cmp(&a.created_ms).then(b.id.cmp(&a.id)));
                if catalog.entries.len() > CATALOG_LIMIT {
                    catalog.entries.pop();
                    catalog.more = true;
                }
            }
            _ => catalog.unreadable += 1,
        }
    }
    Ok(catalog)
}

/// Load one validated publication's HTML off the UI thread.
pub fn read(directory: Option<&Path>, id: &str) -> io::Result<String> {
    if !valid_id(id) {
        return Err(invalid("Invalid artifact ID."));
    }
    let folder = root(directory)?.join(id);
    if !fs::symlink_metadata(&folder)?.is_dir() {
        return Err(invalid("Invalid artifact directory."));
    }
    let bytes = read_bounded(&folder.join("index.html"), HTML_LIMIT)?;
    String::from_utf8(bytes).map_err(|_| invalid("HTML must be UTF-8."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publishing_copies_html_and_catalogs_metadata_without_starting_the_app() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.html");
        fs::write(
            &source,
            "<!doctype html><button>Click</button><script>let count=0;</script>",
        )
        .unwrap();
        let directory = temp.path().join("library");
        assert!(catalog(Some(&directory)).unwrap().entries.is_empty());
        let entry = publish(Some(&directory), &source, "A report", "worker").unwrap();
        fs::remove_file(source).unwrap();
        assert_eq!(
            catalog(Some(&directory)).unwrap().entries,
            vec![entry.clone()]
        );
        assert!(
            read(Some(&directory), &entry.id)
                .unwrap()
                .contains("<script>")
        );
        assert!(read(Some(&directory), "../outside").is_err());
        let link = temp.path().join("link.html");
        std::os::unix::fs::symlink(
            directory
                .join("artifacts")
                .join(&entry.id)
                .join("index.html"),
            &link,
        )
        .unwrap();
        assert!(publish(Some(&directory), &link, "Link", "worker").is_err());
    }
}
