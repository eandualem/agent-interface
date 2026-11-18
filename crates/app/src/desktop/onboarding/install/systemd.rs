//! What a systemd user unit of Agent Backbone's login service runs, and the
//! drop-ins that may change it, read as the service itself reads them, so
//! the app restarts only a unit the service would take for its own.
//!
//! Ported from agent-backbone-rs, `crates/cli/src/commands/service/systemd.rs`
//! at 52e38502, under its licence:
//!
//! MIT License
//!
//! Copyright (c) 2026 agent-backbone contributors
//!
//! Permission is hereby granted, free of charge, to any person obtaining a copy
//! of this software and associated documentation files (the "Software"), to deal
//! in the Software without restriction, including without limitation the rights
//! to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
//! copies of the Software, and to permit persons to whom the Software is
//! furnished to do so, subject to the following conditions:
//!
//! The above copyright notice and this permission notice shall be included in all
//! copies or substantial portions of the Software.
//!
//! THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
//! IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
//! FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//! AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
//! LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
//! OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
//! SOFTWARE.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

/// The folders systemd looks for a user's units and their drop-ins in, as
/// the service lists them (systemd.unit(5)), each with whether the system's
/// packages keep it. A relative value is no value.
pub(super) fn unit_dirs(
    home: &Path,
    variable: impl Fn(&str) -> Option<OsString>,
) -> Vec<(PathBuf, bool)> {
    let absolute = |name: &str| {
        variable(name)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    let each = |name: &str, default: &str| -> Vec<PathBuf> {
        let value = variable(name).unwrap_or_else(|| default.into());
        std::env::split_paths(&value)
            .filter(|path| path.is_absolute())
            .collect()
    };
    let config = absolute("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config"));
    let data = absolute("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local/share"));
    let runtime = absolute("XDG_RUNTIME_DIR");
    let at_runtime = |part: &str| runtime.iter().map(|dir| dir.join(part)).collect::<Vec<_>>();
    let mut own = vec![config.join("systemd/user.control")];
    own.extend(at_runtime("systemd/user.control"));
    own.extend(at_runtime("systemd/transient"));
    own.extend(at_runtime("systemd/generator.early"));
    own.push(config.join("systemd/user"));
    own.extend(
        each("XDG_CONFIG_DIRS", "/etc/xdg")
            .into_iter()
            .map(|dir| dir.join("systemd/user")),
    );
    own.push("/etc/systemd/user".into());
    own.extend(at_runtime("systemd/user"));
    own.push("/run/systemd/user".into());
    own.extend(at_runtime("systemd/generator"));
    own.push(data.join("systemd/user"));
    let mut vendor: Vec<PathBuf> = each("XDG_DATA_DIRS", "/usr/local/share:/usr/share")
        .into_iter()
        .map(|dir| dir.join("systemd/user"))
        .collect();
    vendor.push("/usr/local/lib/systemd/user".into());
    vendor.push("/usr/lib/systemd/user".into());
    own.into_iter()
        .map(|dir| (dir, false))
        .chain(vendor.into_iter().map(|dir| (dir, true)))
        .chain(
            at_runtime("systemd/generator.late")
                .into_iter()
                .map(|dir| (dir, false)),
        )
        .collect()
}

/// A drop-in systemd reads for the login service's unit in one of `dirs`:
/// the unit's own, one for its name cut after a dash, or the one for every
/// service, unless that is the system's packages' and sets timeouts only.
pub(super) fn drop_in(dirs: &[(PathBuf, bool)]) -> Option<PathBuf> {
    for (dir, vendor) in dirs {
        for name in [
            "agent-backbone-rs.service.d",
            "agent-backbone-.service.d",
            "agent-.service.d",
        ] {
            let path = dir.join(name);
            if there(&path) {
                return Some(path);
            }
        }
        let every = dir.join("service.d");
        if !there(&every) {
            continue;
        }
        if !vendor {
            return Some(every);
        }
        if let Some(found) = beyond_timeouts(&every) {
            return Some(found);
        }
    }
    None
}

/// Whether something is at `path`, or may be: a look that fails for any
/// reason but its absence can't rule a drop-in out.
fn there(path: &Path) -> bool {
    !matches!(path.symlink_metadata(), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
}

/// A `.conf` drop-in in the system's packages' folder for every service
/// that sets more than timeouts; one that can't be read counts too.
fn beyond_timeouts(folder: &Path) -> Option<PathBuf> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Some(folder.to_path_buf());
    };
    for entry in entries {
        let Ok(entry) = entry else {
            return Some(folder.to_path_buf());
        };
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "conf") {
            continue;
        }
        match super::read_regular(&path, super::DEFINITION_MOST) {
            Ok(text) if timeouts_only(&text) => {}
            _ => return Some(path),
        }
    }
    None
}

/// Whether a drop-in sets only timeouts of `[Service]`, which change
/// neither what a unit runs nor its variables.
fn timeouts_only(text: &str) -> bool {
    const TIMEOUTS: &[&str] = &[
        "TimeoutSec",
        "TimeoutStartSec",
        "TimeoutStopSec",
        "TimeoutAbortSec",
        "TimeoutStartFailureMode",
        "TimeoutStopFailureMode",
    ];
    let mut section = "";
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with(['#', ';']) {
            continue;
        }
        if line.ends_with('\\') {
            return false;
        }
        if let Some(name) = line.strip_prefix('[') {
            match name.strip_suffix(']') {
                Some(name) => section = name,
                None => return false,
            }
            continue;
        }
        let key = line.split_once('=').map(|(key, _)| key.trim_end());
        if section != "Service" || !key.is_some_and(|key| TIMEOUTS.contains(&key)) {
            return false;
        }
    }
    true
}

/// The keys each section of the unit the service writes has. Any other,
/// such as `EnvironmentFile=`, `UnsetEnvironment=` or `ExecStartPre=`, could
/// run something else or with other variables.
const UNIT_KEYS: &[(&str, &[&str])] = &[
    ("Unit", &["Description", "After"]),
    (
        "Service",
        &[
            "ExecStart",
            "Environment",
            "StandardOutput",
            "StandardError",
            "Restart",
            "RestartSec",
            "KillMode",
        ],
    ),
    ("Install", &["WantedBy"]),
];

/// A systemd unit as the service writes it, read as systemd reads that
/// form: one `ExecStart=` of the program, `daemon` and `--raw-output`, and
/// `BACKBONE_DIR` set once by `Environment=` in `[Service]`. A key it
/// doesn't write, an empty `Environment=`, a continued line, a specifier or
/// an escape it doesn't write says nothing.
pub(super) fn runs(text: &str) -> Option<(String, String)> {
    let mut section = "";
    let (mut started, mut dir): (Option<Vec<String>>, Option<String>) = (None, None);
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with(['#', ';']) {
            continue;
        }
        if line.ends_with('\\') {
            return None;
        }
        if let Some(name) = line.strip_prefix('[') {
            section = name.strip_suffix(']')?;
            continue;
        }
        let (key, value) = line.split_once('=')?;
        let key = key.trim_end();
        let (_, keys) = UNIT_KEYS.iter().find(|(name, _)| *name == section)?;
        if !keys.contains(&key) {
            return None;
        }
        match key {
            "ExecStart" if started.is_none() => started = Some(words(value)?),
            "ExecStart" => return None,
            "Environment" => {
                let assigned = words(value)?;
                if assigned.is_empty() {
                    return None;
                }
                for word in assigned {
                    let word = undoubled(&word, '%')?;
                    if let Some(found) = word.strip_prefix("BACKBONE_DIR=")
                        && dir.replace(found.to_owned()).is_some()
                    {
                        return None;
                    }
                }
            }
            _ => {}
        }
    }
    let started = started?;
    let [program, daemon, raw] = &started[..] else {
        return None;
    };
    if daemon != "daemon" || raw != "--raw-output" {
        return None;
    }
    Some((undoubled(&undoubled(program, '%')?, '$')?, dir?))
}

/// `word` with each doubled `mark` read as one, as systemd reads `%%` and
/// `$$`. Any other use of `mark`, which systemd replaces with what it
/// stands for, is not one the service writes.
fn undoubled(word: &str, mark: char) -> Option<String> {
    let mut text = String::with_capacity(word.len());
    let mut chars = word.chars();
    while let Some(c) = chars.next() {
        if c == mark && chars.next() != Some(mark) {
            return None;
        }
        text.push(c);
    }
    Some(text)
}

/// The words of a unit's value, unquoted as the service quotes them: bare,
/// in single quotes, or in double quotes with the escapes it writes.
fn words(value: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut rest = value.trim_start();
    while !rest.is_empty() {
        let (word, after) = if let Some(quoted) = rest.strip_prefix('\'') {
            let (word, after) = quoted.split_once('\'')?;
            (!word.contains('\\')).then(|| (word.to_owned(), after))?
        } else if let Some(quoted) = rest.strip_prefix('"') {
            double_quoted(quoted)?
        } else {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            let (word, after) = rest.split_at(end);
            (!word.contains(['\\', '\'', '"'])).then(|| (word.to_owned(), after))?
        };
        if !after.is_empty() && !after.starts_with(char::is_whitespace) {
            return None;
        }
        words.push(word);
        rest = after.trim_start();
    }
    Some(words)
}

/// The text of a word in double quotes, from just after its opening quote,
/// and what follows its closing one. Its escapes are those the service
/// writes, which systemd reads as C escapes: no others are taken.
fn double_quoted(quoted: &str) -> Option<(String, &str)> {
    let mut text = String::new();
    let mut chars = quoted.char_indices();
    while let Some((at, c)) = chars.next() {
        let escaped = match c {
            '"' => return Some((text, &quoted[at + 1..])),
            '\\' => chars.next()?.1,
            c => {
                text.push(c);
                continue;
            }
        };
        let decoded = match escaped {
            '"' | '\\' => escaped,
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            'b' => '\u{8}',
            'f' => '\u{c}',
            'u' => {
                let hex: String = (0..4)
                    .map(|_| chars.next().map(|(_, c)| c))
                    .collect::<Option<_>>()?;
                if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
                    return None;
                }
                char::from_u32(u32::from_str_radix(&hex, 16).ok()?).filter(|c| *c != '\0')?
            }
            _ => return None,
        };
        text.push(decoded);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A word of a unit as the service quotes it.
    fn quote(word: &str) -> String {
        let plain = |c: char| c.is_ascii_alphanumeric() || "@%+=:,./-_".contains(c);
        if !word.is_empty() && word.chars().all(plain) {
            return word.to_owned();
        }
        if word
            .chars()
            .any(|c| c == '\'' || c == '\\' || c.is_control())
        {
            return serde_json::to_string(word).unwrap();
        }
        format!("'{word}'")
    }
    /// The unit the service writes for `program` on `dir`.
    fn unit_of(program: &str, dir: &str) -> String {
        let program = program.replace('%', "%%").replace('$', "$$");
        let assignment = format!("BACKBONE_DIR={}", dir.replace('%', "%%"));
        format!(
            "[Unit]\nDescription=Agent Backbone: this machine's service for its agents\n\
             After=default.target\n\n[Service]\nExecStart={} daemon --raw-output\n\
             Environment={}\nEnvironment=PATH=/usr/bin\nStandardOutput=null\nStandardError=null\n\
             Restart=on-failure\nRestartSec=3\n# Only its own process is stopped.\n\
             KillMode=process\n\n[Install]\nWantedBy=default.target\n",
            quote(&program),
            quote(&assignment)
        )
    }

    #[test]
    fn a_unit_is_this_installations_only_when_systemd_would_run_it_so() {
        let ours = |text: &str, program: &str, dir: &str| {
            runs(text) == Some((program.to_owned(), dir.to_owned()))
        };
        for (program, dir) in [
            ("/opt/bin/backbone", "/home/a/backbone"),
            ("/opt/my tools/backbone", "/home/a/my backbone"),
            ("/opt/100% $HOME/backbone", "/home/a/100% $HOME"),
            ("/opt/it's/back\\bone", "/home/a/\"quoted\"\ttab"),
        ] {
            assert!(
                ours(&unit_of(program, dir), program, dir),
                "{program} {dir}"
            );
        }
        // Another program or directory whose name only starts as this one's.
        let (program, dir) = ("/opt/bin/backbone", "/home/a/backbone");
        assert!(!ours(&unit_of("/opt/bin/backbone-old", dir), program, dir));
        assert!(!ours(&unit_of(program, "/home/a/backbone/2"), program, dir));
        // This one's program and directory in comments, or another section.
        let other = unit_of("/elsewhere/backbone", "/home/b/backbone");
        let commented = other.replace(
            "[Service]\n",
            "[Service]\n# ExecStart=/opt/bin/backbone daemon\n; Environment=BACKBONE_DIR=/home/a/backbone\n",
        );
        assert!(!ours(&commented, program, dir), "{commented}");
        let misplaced = format!(
            "[Unit]\nExecStart=/opt/bin/backbone daemon\nEnvironment=BACKBONE_DIR=/home/a/backbone\n{other}"
        );
        assert!(!ours(&misplaced, program, dir));
        // A second start, a specifier or an escape it doesn't write, or a key
        // that may set, clear or pass on BACKBONE_DIR or run something else.
        let unit = unit_of(program, dir);
        assert!(ours(&unit, program, dir));
        for changed in [
            unit.replace("Restart=", "ExecStart=/bin/sh daemon\nRestart="),
            unit.replace("/opt/bin/backbone", "/opt/%h/backbone"),
            unit.replace(
                "ExecStart=/opt/bin/backbone",
                "ExecStart=\"/opt/bin/backbon\\x65\"",
            ),
            unit.replace(" daemon", " daemon --other"),
            unit.replace(" daemon --raw-output", " daemon"),
            unit.replace("Restart=", "EnvironmentFile=/home/b/env\nRestart="),
            unit.replace("Restart=", "UnsetEnvironment=BACKBONE_DIR\nRestart="),
            unit.replace("Restart=", "PassEnvironment=BACKBONE_DIR\nRestart="),
            unit.replace("Restart=", "Environment=\nRestart="),
            unit.replace("Restart=", "Environment=BACKBONE_DIR=/home/b\nRestart="),
            unit.replace("Restart=", "ExecStartPre=/bin/true\nRestart="),
            unit.replace("After=", "ExecStart=/opt/bin/backbone daemon\nAfter="),
            format!("Environment=BACKBONE_DIR=/home/b\n{unit}"),
            format!("{unit}[X-Other]\nKey=value\n"),
            unit.replace("KillMode=process", "KillMode=process \\"),
        ] {
            assert!(!ours(&changed, program, dir), "{changed}");
        }
    }

    #[test]
    fn a_unit_a_drop_in_may_change_anywhere_systemd_looks_is_left_alone() {
        let (own, vendor) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let dirs = [
            (own.path().to_path_buf(), false),
            (vendor.path().to_path_buf(), true),
        ];
        assert_eq!(drop_in(&dirs), None);
        for (folder, name) in [
            (&own, "agent-backbone-rs.service.d"),
            (&vendor, "agent-.service.d"),
            (&own, "service.d"),
        ] {
            let found = folder.path().join(name);
            std::fs::create_dir(&found).unwrap();
            assert_eq!(drop_in(&dirs), Some(found.clone()), "{name}");
            std::fs::remove_dir(&found).unwrap();
        }
        // The system's packages' drop-in for every service counts only when
        // it sets more than timeouts.
        let every = vendor.path().join("service.d");
        std::fs::create_dir(&every).unwrap();
        let conf = every.join("10-timeout-abort.conf");
        std::fs::write(
            &conf,
            "# Abort hung units\n[Service]\nTimeoutStopFailureMode=abort\n",
        )
        .unwrap();
        assert_eq!(drop_in(&dirs), None);
        std::fs::write(&conf, "[Service]\nEnvironment=BACKBONE_DIR=/x\n").unwrap();
        assert_eq!(drop_in(&dirs), Some(conf.clone()));
        // One that is a pipe counts, at once: it is never waited on.
        std::fs::remove_file(&conf).unwrap();
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&conf)
                .status()
                .unwrap()
                .success()
        );
        let began = std::time::Instant::now();
        assert_eq!(drop_in(&dirs), Some(conf));
        assert!(began.elapsed() < std::time::Duration::from_secs(1));
    }

    #[test]
    fn systemd_is_looked_in_where_the_service_looks() {
        let dirs = unit_dirs(Path::new("/home/a"), |name| match name {
            "XDG_CONFIG_HOME" => Some("/c".into()),
            "XDG_RUNTIME_DIR" => Some("/r".into()),
            // A relative value is no value.
            "XDG_DATA_HOME" => Some("data".into()),
            _ => None,
        });
        for (dir, vendor) in [
            ("/c/systemd/user", false),
            ("/r/systemd/transient", false),
            ("/etc/xdg/systemd/user", false),
            ("/etc/systemd/user", false),
            ("/home/a/.local/share/systemd/user", false),
            ("/usr/share/systemd/user", true),
            ("/usr/lib/systemd/user", true),
            ("/r/systemd/generator.late", false),
        ] {
            assert!(dirs.contains(&(PathBuf::from(dir), vendor)), "{dir}");
        }
    }
}
