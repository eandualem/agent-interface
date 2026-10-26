use std::{
    env, fs,
    io::{self, Read},
    os::unix::fs::OpenOptionsExt,
    path::Path,
    time::Duration,
};

use rusqlite::{Connection, OpenFlags};
use serde_json::value::RawValue;

use crate::Snapshot;

impl Snapshot {
    /// Read configuration for an already-enabled provider, then fetch its roster.
    ///
    /// Call on an I/O worker with the provider's resolved data directory. This
    /// performs no launch-time discovery or automatic activation. Each call
    /// re-reads the named environment values and, when needed, `.env` and the
    /// read-only settings database, so repaired configuration can recover.
    /// Configuration failures preserve metadata with the same fixed stale error
    /// as a failed request; no secret or underlying error is retained or logged.
    pub fn fetch_configured(&mut self, data_dir: &Path, url: Option<&str>) {
        if let Some((url, key)) = configured(data_dir, url) {
            self.fetch(&url, &key);
        } else {
            self.unavailable();
        }
    }
}

/// The validated address and key for an enabled provider, re-read on each call.
pub(crate) fn configured(data_dir: &Path, url: Option<&str>) -> Option<(String, String)> {
    let key = environment("BACKBONE_API_KEY")?;
    let socket = environment("BACKBONE_SOCKET_PATH")?;
    let target = (!socket.is_empty()).then(|| format!("unix:{socket}"));
    let url = target.as_deref().or(url);
    let port = if url.is_some_and(|url| !url.is_empty()) {
        String::new()
    } else {
        environment("BACKBONE_PORT")?
    };
    read(data_dir, url, &key, &port)
}

fn environment(name: &str) -> Option<String> {
    match env::var(name) {
        Ok(value) => Some(value),
        Err(env::VarError::NotPresent) => Some(String::new()),
        Err(env::VarError::NotUnicode(_)) => None,
    }
}

/// Follow ordinary file symlinks, but never wait for a pipe's writer.
pub(super) fn regular_file(path: &Path) -> io::Result<fs::File> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("not a regular file"));
    }
    Ok(file)
}

fn read(data_dir: &Path, url: Option<&str>, key: &str, port: &str) -> Option<(String, String)> {
    let mut key = key.to_owned();
    if key.is_empty() {
        let path = data_dir.join(".env");
        if path.try_exists().ok()? {
            const LIMIT: usize = 1024 * 1024;
            let mut text = String::new();
            regular_file(&path)
                .ok()?
                .take((LIMIT + 1) as u64)
                .read_to_string(&mut text)
                .ok()?;
            if text.len() > LIMIT {
                return None;
            }
            key = dotenv_key(&text)?;
        }
    }
    let url = if let Some(url) = url.filter(|url| !url.is_empty()) {
        url.to_owned()
    } else {
        let (mut host, saved_port) = settings(&data_dir.join("backbone.db"), !port.is_empty())?;
        if host == "0.0.0.0" {
            host = "127.0.0.1".into();
        }
        let port = if port.is_empty() { &saved_port } else { port };
        format!("http://{host}:{port}")
    };
    if let Some(socket) = url.strip_prefix("unix:") {
        if !Path::new(socket).is_absolute() {
            return None;
        }
    } else {
        crate::transport::endpoint(&url, "/api/agents")?;
    }
    Some((url.trim_end_matches('/').to_owned(), key))
}

fn settings(path: &Path, port_overridden: bool) -> Option<(String, String)> {
    let mut host = "127.0.0.1".to_owned();
    let mut port = "7120".to_owned();
    if path.try_exists().ok()? {
        let connection =
            Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
        connection.busy_timeout(Duration::from_millis(250)).ok()?;
        let mut statement = connection
            .prepare(
                "SELECT key, value FROM settings WHERE key IN ('backbone.host', 'backbone.port')",
            )
            .ok()?;
        let mut rows = statement.query([]).ok()?;
        while let Some(row) = rows.next().ok()? {
            let name: String = row.get(0).ok()?;
            let value: String = row.get(1).ok()?;
            if name == "backbone.host" {
                host = serde_json::from_str(&value).ok()?;
            } else {
                // Check the literal JSON type before parsing so serde_json's
                // internal numeric marker cannot turn an object into a port.
                let raw: &RawValue = serde_json::from_str(&value).ok()?;
                // Python loads each JSON value before applying the environment
                // override, but an overridden value's type does not matter.
                if port_overridden {
                    continue;
                }
                port = match raw.get().as_bytes()[0] {
                    b'"' => serde_json::from_str(raw.get()).ok()?,
                    b'-' | b'0'..=b'9' => raw.get().parse::<serde_json::Number>().ok()?.to_string(),
                    _ => return None,
                };
            }
        }
    }
    Some((host, port))
}

fn whitespace(ch: char) -> bool {
    ch.is_whitespace() || matches!(ch, '\u{1c}'..='\u{1f}')
}

fn dotenv_key(contents: &str) -> Option<String> {
    let mut key = String::new();
    // Python's splitlines also recognizes these single-character boundaries.
    for line in contents.split([
        '\n', '\r', '\u{b}', '\u{c}', '\u{1c}', '\u{1d}', '\u{1e}', '\u{85}', '\u{2028}',
        '\u{2029}',
    ]) {
        let mut line = line.trim_start_matches(whitespace);
        if let Some(rest) = line.strip_prefix("export")
            && rest.starts_with(whitespace)
        {
            line = rest.trim_start_matches(whitespace);
        }
        let Some(rest) = line.strip_prefix("BACKBONE_API_KEY") else {
            continue;
        };
        let Some(value) = rest.trim_start_matches(whitespace).strip_prefix('=') else {
            continue;
        };
        key = first_word(value.trim_start_matches(whitespace))?;
    }
    Some(key)
}

// Match shlex.split(..., comments=True): preserve backslashes before dollar
// signs/backticks inside double quotes, and start comments even within a word.
// Only the first word is retained, but malformed trailing words still fail.
fn first_word(value: &str) -> Option<String> {
    let mut chars = value.chars();
    let mut quote = None;
    let mut in_word = false;
    let mut finished = false;
    let mut first = String::new();
    while let Some(ch) = chars.next() {
        if quote == Some('\'') {
            if ch == '\'' {
                quote = None;
            } else if !finished {
                first.push(ch);
            }
        } else if ch == '\\' {
            let escaped = chars.next()?;
            if !finished {
                if quote == Some('"') && !matches!(escaped, '"' | '\\') {
                    first.push('\\');
                }
                first.push(escaped);
            }
            in_word = true;
        } else if quote == Some('"') {
            if ch == '"' {
                quote = None;
            } else if !finished {
                first.push(ch);
            }
        } else {
            match ch {
                '\'' | '"' => {
                    quote = Some(ch);
                    in_word = true;
                }
                '#' => break,
                ' ' | '\t' | '\r' | '\n' => {
                    if in_word {
                        finished = true;
                        in_word = false;
                    }
                }
                _ => {
                    if !finished {
                        first.push(ch);
                    }
                    in_word = true;
                }
            }
        }
    }
    quote.is_none().then_some(first)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn database(directory: &Path) -> Connection {
        let connection = Connection::open(directory.join("backbone.db")).unwrap();
        connection
            .execute_batch("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);")
            .unwrap();
        connection
    }

    fn put(connection: &Connection, key: &str, value: &str) {
        connection
            .execute(
                "INSERT OR REPLACE INTO settings VALUES (?1, ?2)",
                [key, value],
            )
            .unwrap();
    }

    #[test]
    fn dotenv_reads_reject_special_and_oversized_files_and_recover() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("source");
        let path = directory.path().join(".env");
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(
            Command::new("mkfifo")
                .arg(&target)
                .status()
                .unwrap()
                .success()
        );
        let read = || super::read(directory.path(), Some("http://127.0.0.1"), "", "");
        assert!(read().is_none());
        fs::remove_file(&target).unwrap();
        fs::write(&target, " ".repeat(1024 * 1024 + 1)).unwrap();
        assert!(read().is_none());
        fs::write(&target, "BACKBONE_API_KEY=fixture").unwrap();
        assert_eq!(read(), Some(("http://127.0.0.1".into(), "fixture".into())));
    }

    #[test]
    fn only_the_named_key_is_read_with_python_quotes_and_comments() {
        let text = "OTHER='unterminated\nBACKBONE_API_KEY=first\n export BACKBONE_API_KEY = 'a b'\"c\"#comment\nBACKBONE_API_KEY_EXTRA=ignored\n";
        assert_eq!(dotenv_key(text).as_deref(), Some("a bc"));
        assert_eq!(
            dotenv_key("BACKBONE_API_KEY=one\\ two extra").as_deref(),
            Some("one two")
        );
        assert_eq!(
            dotenv_key("BACKBONE_API_KEY=\"a\\$b\\`c\"").as_deref(),
            Some("a\\$b\\`c")
        );
        assert_eq!(
            dotenv_key("BACKBONE_API_KEY=before#comment").as_deref(),
            Some("before")
        );
        assert_eq!(
            dotenv_key("BACKBONE_API_KEY=before\rBACKBONE_API_KEY=\"\" later").as_deref(),
            Some("")
        );
        assert_eq!(
            dotenv_key("BACKBONE_API_KEY=x\u{2028}BACKBONE_API_KEY= # cleared").as_deref(),
            Some("")
        );
        assert!(dotenv_key("BACKBONE_API_KEY=first 'unterminated").is_none());
        assert!(dotenv_key("BACKBONE_API_KEY=first second\\").is_none());
    }

    #[test]
    fn configuration_precedence_is_read_only_and_recovers_after_repairs() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        assert_eq!(
            read(root, None, "", ""),
            Some(("http://127.0.0.1:7120".into(), "".into()))
        );
        assert_eq!(fs::read_dir(root).unwrap().count(), 0);
        fs::write(
            root.join(".env"),
            "OTHER=ignored\nBACKBONE_API_KEY='file token'\n",
        )
        .unwrap();
        let connection = database(root);
        put(&connection, "backbone.host", "\"0.0.0.0\"");
        put(&connection, "backbone.port", "8123");
        put(&connection, "unrelated", "not JSON");
        drop(connection);
        let before = fs::read(root.join("backbone.db")).unwrap();
        let env_before = fs::read(root.join(".env")).unwrap();
        assert_eq!(
            read(root, None, "", ""),
            Some(("http://127.0.0.1:8123".into(), "file token".into()))
        );
        assert_eq!(
            read(root, None, "environment token", "8124"),
            Some(("http://127.0.0.1:8124".into(), "environment token".into()))
        );
        assert_eq!(fs::read(root.join("backbone.db")).unwrap(), before);
        assert_eq!(fs::read(root.join(".env")).unwrap(), env_before);
        assert_eq!(fs::read_dir(root).unwrap().count(), 2);

        let connection = Connection::open(root.join("backbone.db")).unwrap();
        put(
            &connection,
            "backbone.port",
            r#"{"$serde_json::private::Number":"80"}"#,
        );
        assert!(read(root, None, "", "").is_none());
        assert!(read(root, None, "", "8124").is_some());
        put(&connection, "backbone.port", "not JSON");
        assert!(read(root, None, "", "8124").is_none());
        // Explicit addresses bypass even an invalid database, but not key parsing.
        assert_eq!(
            read(root, Some("http://localhost:8125/"), "", "bad"),
            Some(("http://localhost:8125".into(), "file token".into()))
        );
        fs::write(root.join(".env"), "BACKBONE_API_KEY='broken").unwrap();
        assert!(read(root, Some("http://localhost:8125"), "", "").is_none());
        assert!(read(root, Some("http://localhost:8125"), "environment token", "").is_some());
        fs::write(root.join(".env"), "BACKBONE_API_KEY=repaired").unwrap();
        put(&connection, "backbone.port", "\"8126\"");
        assert_eq!(
            read(root, Some(""), "", ""),
            Some(("http://127.0.0.1:8126".into(), "repaired".into()))
        );
        assert!(read(root, None, "", "99999").is_none());
        assert!(read(root, Some("http://remote.example"), "", "").is_none());
    }

    #[test]
    fn configured_fetch_uses_only_the_fixture_environment_and_keeps_failures_stale() {
        use crate::transport::tests::{ok, server};
        for (environment_key, expected) in [
            ("env-fixture-token", "env-fixture-token"),
            ("", "file-fixture-token"),
        ] {
            let directory = tempfile::tempdir().unwrap();
            fs::write(
                directory.path().join(".env"),
                "BACKBONE_API_KEY=file-fixture-token",
            )
            .unwrap();
            let connection = database(directory.path());
            put(&connection, "backbone.host", "\"0.0.0.0\"");
            put(&connection, "backbone.port", "1");
            drop(connection);
            let (url, server) = server(
                ok(r#"{"items":[{"name":"worker","state":"busy"}]}"#),
                Duration::ZERO,
            );
            let socket = url.strip_prefix("unix:").unwrap();
            let status = Command::new(env::current_exe().unwrap())
                .args([
                    "--exact",
                    "configuration::tests::configuration_fixture_child",
                    "--nocapture",
                ])
                .env("ROSTER_CONFIG_FIXTURE_DIR", directory.path())
                .env("BACKBONE_API_KEY", environment_key)
                .env("BACKBONE_SOCKET_PATH", socket)
                .status()
                .unwrap();
            assert!(status.success());
            let request = server.join().unwrap().to_ascii_lowercase();
            assert!(request.starts_with("get /api/agents http/1.1\r\n"));
            assert!(request.contains(&format!("authorization: bearer {expected}\r\n")));
        }
    }

    #[test]
    fn configuration_fixture_child() {
        let Some(directory) = env::var_os("ROSTER_CONFIG_FIXTURE_DIR") else {
            return;
        };
        let directory = Path::new(&directory);
        let mut snapshot = Snapshot::default();
        snapshot.fetch_configured(directory, None);
        assert!(snapshot.error.is_none());
        assert_eq!(snapshot.states["worker"], "busy");
        let observed = snapshot.observed_at;
        fs::write(
            directory.join(".env"),
            "BACKBONE_API_KEY=\"invalid fixture key",
        )
        .unwrap();
        snapshot.fetch_configured(directory, None);
        assert_eq!(snapshot.error, Some(crate::UNAVAILABLE));
        assert_eq!(snapshot.states["worker"], "busy");
        assert_eq!(snapshot.observed_at, observed);
    }
}
