//! What a LaunchAgent of Agent Backbone's login service runs, read as a
//! whole property list, as launchd reads it, so the app restarts only one
//! the service would take for its own.
//!
//! Ported from agent-backbone-rs, `crates/cli/src/commands/service/launchd.rs`
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

use serde_json::Value;
use std::{
    io::{Read, Seek, Write},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// How long `plutil` may take to read a definition.
const READ_WITHIN: Duration = Duration::from_secs(5);

/// What the property list `text` runs, its program and `BACKBONE_DIR`,
/// when it is in the form the service writes.
pub(super) fn runs(text: &str) -> Option<(String, String)> {
    runs_of(&json(text)?)
}

/// `text` as JSON, as `plutil` converts a property list: whole, with its
/// comments and entities read as launchd reads them.
fn json(text: &str) -> Option<Value> {
    let mut input = tempfile::tempfile().ok()?;
    input.write_all(text.as_bytes()).ok()?;
    input.rewind().ok()?;
    let mut output = tempfile::tempfile().ok()?;
    let mut child = Command::new("/usr/bin/plutil")
        .args(["-convert", "json", "-o", "-", "-"])
        .stdin(input)
        .stdout(output.try_clone().ok()?)
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + READ_WITHIN;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    if !status.success() {
        return None;
    }
    output.rewind().ok()?;
    let mut converted = String::new();
    output
        .take(4 * super::DEFINITION_MOST)
        .read_to_string(&mut converted)
        .ok()?;
    serde_json::from_str(&converted).ok()
}

/// What a property list, read as JSON, runs when it is the service's form:
/// no `Program`, which launchd runs in place of the arguments' first;
/// `ProgramArguments` of exactly the program, `daemon` and `--raw-output`;
/// and `BACKBONE_DIR` in `EnvironmentVariables`.
fn runs_of(list: &Value) -> Option<(String, String)> {
    let list = list.as_object()?;
    if list.contains_key("Program") {
        return None;
    }
    let arguments: Vec<&str> = list
        .get("ProgramArguments")?
        .as_array()?
        .iter()
        .map(Value::as_str)
        .collect::<Option<_>>()?;
    let [program, "daemon", "--raw-output"] = arguments[..] else {
        return None;
    };
    let dir = list
        .get("EnvironmentVariables")?
        .get("BACKBONE_DIR")?
        .as_str()?;
    Some((program.to_owned(), dir.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The LaunchAgent the service writes for `program` on `dir`.
    #[cfg(target_os = "macos")]
    fn plist_of(program: &str, dir: &str) -> String {
        let escape = |text: &str| -> String {
            text.chars()
                .map(|c| match c {
                    '&' => "&amp;".to_owned(),
                    '<' => "&lt;".to_owned(),
                    '>' => "&gt;".to_owned(),
                    '"' => "&quot;".to_owned(),
                    '\'' => "&apos;".to_owned(),
                    c if c.is_control() => format!("&#{};", u32::from(c)),
                    c => c.to_string(),
                })
                .collect()
        };
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \
             \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\">\n<dict>\n  <key>Label</key><string>dev.agent-backbone-rs</string>\n\
             \x20 <key>ProgramArguments</key>\n  <array>\n    <string>{}</string>\n\
             \x20   <string>daemon</string>\n    <string>--raw-output</string>\n  </array>\n\
             \x20 <key>EnvironmentVariables</key>\n  <dict>\n    <key>BACKBONE_DIR</key><string>{}</string>\n\
             \x20   <key>PATH</key><string>/usr/bin</string>\n  </dict>\n\
             \x20 <key>RunAtLoad</key><true/>\n  <key>KeepAlive</key>\n  <dict>\n\
             \x20   <key>SuccessfulExit</key><false/>\n  </dict>\n\
             \x20 <key>ExitTimeOut</key><integer>10</integer>\n\
             \x20 <key>ProcessType</key><string>Interactive</string>\n</dict>\n</plist>\n",
            escape(program),
            escape(dir)
        )
    }

    #[test]
    fn a_launch_agent_is_this_installations_only_when_launchd_would_run_it_so() {
        let (program, dir) = ("/opt/bin/backbone", "/Users/a/backbone");
        let ours = || {
            json!({"Label": "dev.agent-backbone-rs",
                "ProgramArguments": [program, "daemon", "--raw-output"],
                "EnvironmentVariables": {"BACKBONE_DIR": dir, "PATH": "/usr/bin"},
                "RunAtLoad": true})
        };
        let found = Some((program.to_owned(), dir.to_owned()));
        assert_eq!(runs_of(&ours()), found);
        // A Program launchd would run in place of the one it names, another
        // argument, or no directory among its variables.
        let mut replaced = ours();
        replaced["Program"] = json!("/bin/sh");
        let mut longer = ours();
        longer["ProgramArguments"] = json!([program, "daemon", "--raw-output", "--other"]);
        let mut elsewhere = ours();
        elsewhere["EnvironmentVariables"] = json!({"PATH": "/usr/bin"});
        elsewhere["BACKBONE_DIR"] = json!(dir);
        for changed in [replaced, longer, elsewhere, json!([program, dir])] {
            assert_eq!(runs_of(&changed), None, "{changed}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn plutil_reads_the_services_launch_agent_as_launchd_would() {
        for (program, dir) in [
            ("/opt/my <tools> & 'co'/backbone", "/Users/a/back\"bone\""),
            ("/opt/bin/backbone", "/Users/a/back\tbone"),
        ] {
            assert_eq!(
                runs(&plist_of(program, dir)),
                Some((program.to_owned(), dir.to_owned())),
                "{program} {dir}"
            );
        }
        // This one's program and directory in a comment only, or a Program.
        let other = plist_of("/elsewhere/backbone", "/Users/b/backbone");
        let commented = other.replacen(
            "<dict>",
            "<!-- <string>/opt/bin/backbone</string> <string>/Users/a/backbone</string> -->\n<dict>",
            1,
        );
        assert_eq!(
            runs(&commented),
            Some((
                "/elsewhere/backbone".to_owned(),
                "/Users/b/backbone".to_owned()
            ))
        );
        let replaced = plist_of("/opt/bin/backbone", "/Users/a/backbone").replace(
            "<key>Label</key>",
            "<key>Program</key><string>/bin/sh</string>\n  <key>Label</key>",
        );
        assert_eq!(runs(&replaced), None);
        assert_eq!(runs("not a property list"), None);
    }
}
