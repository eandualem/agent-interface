//! What first run fixes: Git or tmux with the system's own installer and an
//! agent tool with its official script, each on the person's click; and the
//! agent service, which first run starts by itself with Agent Backbone's own
//! setup: it keeps a profile the computer has, or sets one up with this
//! installation's login service. Every fix runs off the UI thread with a
//! deadline, and a failure explains itself.

use agent_interface_roster::profiles::setup::{
    EVENT_LIMIT, LINE_LIMIT, OUTPUT_LIMIT, one_line, token, unique_keys,
};
use std::{
    io::Read,
    os::unix::fs::FileExt,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// Long enough for a package manager or an installer script on a slow line.
const LIMIT: Duration = Duration::from_secs(15 * 60);
/// How much of a failed command's output is kept to explain it.
const TAIL: usize = 16 * 1024;
/// Agent Backbone's own setup waits up to 30 s for its service; this leaves
/// room for a slow first start.
const SETUP_LIMIT: Duration = Duration::from_secs(5 * 60);
/// How often one setup request is sent when what came of it isn't known.
const SENDS: usize = 3;

/// How a fix ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// It finished; check the computer again.
    Done,
    /// The system's own installer opened and finishes there; keep checking.
    Opened,
    /// It could not be done, and why, in a sentence.
    Failed(String),
}

/// Find a command in the PATH setup found, or in the usual system places.
fn find(command: &str, path_env: &str) -> Option<PathBuf> {
    std::env::split_paths(path_env)
        .chain(
            [
                "/usr/bin",
                "/bin",
                "/usr/sbin",
                "/opt/homebrew/bin",
                "/usr/local/bin",
            ]
            .map(PathBuf::from),
        )
        .map(|directory| directory.join(command))
        .find(|path| path.is_file())
}

/// Install Git or tmux, by the name both the command and its package share.
pub fn package(name: &'static str, path_env: &str) -> Outcome {
    if cfg!(target_os = "macos") {
        if name == "git" {
            // Apple's own installer for the command line tools, which include Git.
            return match run(Command::new("/usr/bin/xcode-select").arg("--install"), path_env) {
                Ok(()) => Outcome::Opened,
                Err(_) => Outcome::Failed(
                    "The command line tools installer could not open. Open Software Update, then try again.".into(),
                ),
            };
        }
        let Some(brew) = find("brew", path_env) else {
            return Outcome::Failed(format!(
                "{name} installs with Homebrew, which isn't on this Mac yet. Install Homebrew from brew.sh, then try again."
            ));
        };
        return finished(run(Command::new(brew).args(["install", name]), path_env));
    }
    // Linux: the distribution's package manager, behind the desktop's own
    // password prompt. Without that prompt there is no safe way to ask.
    let managers: [(&str, &[&str]); 4] = [
        ("apt-get", &["install", "-y"]),
        ("dnf", &["install", "-y"]),
        ("pacman", &["-S", "--noconfirm"]),
        ("zypper", &["--non-interactive", "install"]),
    ];
    let Some((manager, arguments)) = managers
        .iter()
        .find_map(|(manager, arguments)| find(manager, path_env).map(|path| (path, *arguments)))
    else {
        return Outcome::Failed(format!(
            "This system's package manager isn't one setup knows. Install {name} with it, and setup notices."
        ));
    };
    let Some(prompt) = find("pkexec", path_env) else {
        return Outcome::Failed(format!(
            "Installing {name} needs an administrator password, and this desktop has no prompt for it. Install {name} from a terminal, and setup notices."
        ));
    };
    let mut command = Command::new(prompt);
    command.arg(manager).args(arguments).arg(name);
    finished(run(&mut command, path_env))
}

/// Install an agent tool with its official command. Its download is piped
/// into a shell, so `pipefail` makes a failed download fail the install.
pub fn tool(install: &str, path_env: &str) -> Outcome {
    let Some(bash) = find("bash", path_env) else {
        return Outcome::Failed("Installing it needs bash, which isn't on this computer.".into());
    };
    finished(run(
        Command::new(bash).args(["-o", "pipefail", "-c", install]),
        path_env,
    ))
}

/// Set this computer up with Agent Backbone's own setup, `initialize`: it
/// keeps a profile the computer has, or makes the first one with the core
/// starter and this installation's login service. The app restarts no
/// service itself: it can't tell safely whose a login service is, and setup
/// starts this installation's own. `request` is this run's request: it is
/// kept while setup says the same request may follow, so Try again sends
/// it again, and a new one is made otherwise.
pub fn initialize_backbone(
    backbone: &Path,
    path_env: &str,
    request: &mut Option<String>,
) -> Result<(), String> {
    let request_id = request
        .get_or_insert_with(|| uuid::Uuid::now_v7().simple().to_string())
        .clone();
    let stopped = initialize(backbone, path_env, &request_id).err();
    if !stopped.as_ref().is_some_and(|stopped| stopped.again) {
        *request = None;
    }
    stopped.map_or(Ok(()), |stopped| Err(stopped.said))
}

/// Why `initialize` stopped, and whether the same request may follow it.
struct Stopped {
    said: String,
    again: bool,
}

/// The `initialize` request first run sends, under `request_id`.
fn initialize_request(request_id: &str) -> serde_json::Value {
    serde_json::json!({
        "schema_version": 3,
        "request_id": request_id,
        "action": "initialize",
        "profile_name": "Default",
        "service": "user",
        "starter_profile": "core-v1",
    })
}

/// Run one `initialize` request until setup says what came of it. The
/// request is made once: after lost output or an outcome not known, it is
/// sent again as it was, up to [`SENDS`] times. Any other answer stops it,
/// in setup's own words.
fn initialize(backbone: &Path, path_env: &str, request_id: &str) -> Result<(), Stopped> {
    let request = initialize_request(request_id).to_string();
    let deadline = Instant::now() + SETUP_LIMIT;
    let mut last = None;
    for _ in 0..SENDS {
        // What came of a send that failed or ran out of time isn't known.
        let sent = send(backbone, path_env, &request, request_id, deadline)
            .map_err(|said| Stopped { said, again: true })?;
        let Some(answer) = sent else {
            // No readable answer: the same request finds out what came of it.
            continue;
        };
        let said = answer.said();
        match (
            answer.status.as_str(),
            answer.code.as_str(),
            answer.retry.as_str(),
        ) {
            ("completed", "initialized" | "existing_preserved", _) => return Ok(()),
            // Sent, with what came of it not known, or stopped before it
            // changed anything: the same request again.
            (_, "outcome_unknown" | "setup_failed", "same_request") => {}
            // Anything else, service_unavailable among them, is the person's
            // to act on: Try again sends the same request when setup says so.
            _ => {
                let again = answer.retry == "same_request";
                return Err(Stopped { said, again });
            }
        }
        last = Some(said);
    }
    // Lost every time: the same request finds out what came of it.
    Err(Stopped {
        said: last.unwrap_or_else(|| "Agent Backbone's setup did not report a result.".into()),
        again: true,
    })
}

/// What setup answered.
#[derive(Debug, PartialEq, Eq)]
struct Answer {
    status: String,
    code: String,
    message: String,
    retry: String,
}

impl Answer {
    /// Whether `exit` is the code setup ends with for this answer: 0
    /// completed, 1 failed, 2 an invalid request, 3 action required. A
    /// status this app doesn't know has none to match.
    fn exits(&self, exit: Option<i32>) -> bool {
        match self.status.as_str() {
            "completed" | "failed" | "action_required" if self.code == "invalid_request" => {
                exit == Some(2)
            }
            "completed" => exit == Some(0),
            "failed" => exit == Some(1),
            "action_required" => exit == Some(3),
            _ => true,
        }
    }

    /// The answer's own words for a person, else its code.
    fn said(&self) -> String {
        let message = one_line(&self.message);
        format!(
            "Agent Backbone's setup stopped: {}",
            if message.is_empty() {
                &self.code
            } else {
                &message
            }
        )
    }
}

/// Send `request` with `backbone setup --file F --jsonl` and read its
/// answer. `None` when what it printed is not one.
fn send(
    backbone: &Path,
    path_env: &str,
    request: &str,
    request_id: &str,
    deadline: Instant,
) -> Result<Option<Answer>, String> {
    let mut file = tempfile::NamedTempFile::new()
        .map_err(|_| "Setup could not create a temporary file.".to_owned())?;
    std::io::Write::write_all(&mut file, request.as_bytes())
        .map_err(|_| "Setup could not write its request.".to_owned())?;
    let output =
        tempfile::tempfile().map_err(|_| "Setup could not create a temporary file.".to_owned())?;
    let mut child = Command::new(backbone)
        .args(["setup", "--file"])
        .arg(file.path())
        .arg("--jsonl")
        .env("PATH", path_env)
        .stdin(Stdio::null())
        .stdout(
            output
                .try_clone()
                .map_err(|_| "Setup could not create a temporary file.".to_owned())?,
        )
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("Agent Backbone's setup could not start: {error}."))?;
    let exit = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(200)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Agent Backbone's setup took too long and was stopped.".into());
            }
        }
    };
    let mut printed = String::new();
    let read = std::io::Seek::rewind(&mut &output).and_then(|_| {
        (&output)
            .take(OUTPUT_LIMIT as u64 + 1)
            .read_to_string(&mut printed)
    });
    if read.is_err() || printed.len() > OUTPUT_LIMIT {
        return Ok(None);
    }
    // An answer its exit contradicts is no readable answer.
    Ok(answer(&printed, request_id).filter(|answer| answer.exits(exit)))
}

/// What setup answered `request_id`, from the lines it printed: each an
/// event of that request, in order, and the last its `initialize` result.
/// `None` when what it printed is not that.
fn answer(printed: &str, request_id: &str) -> Option<Answer> {
    use serde::Deserialize;
    use serde_json::Value;
    // What every line has; then only the fields its own kind defines, so a
    // field of the other kind, of any type, is ignored. Each is read from
    // the line itself, so a field it defines said twice is refused.
    #[derive(Deserialize)]
    struct Line {
        schema_version: u32,
        request_id: String,
        seq: u64,
        #[serde(rename = "type")]
        kind: String,
    }
    #[derive(Deserialize)]
    struct Step {
        step: String,
        status: String,
    }
    #[derive(Deserialize)]
    struct Done {
        action: String,
        status: String,
        code: String,
        message: String,
        retry: String,
        completed_steps: Vec<String>,
        data: Value,
    }
    let mut seq = 0;
    let mut result = None;
    for text in printed.lines() {
        // Bounded as a first agent's events are, nothing after the result,
        // and every value kept as it came a bounded token.
        if text.len() > LINE_LIMIT || result.is_some() {
            return None;
        }
        if text.trim().is_empty() {
            continue;
        }
        // No key said twice anywhere in it, its data among them.
        if !unique_keys(text.as_bytes()) {
            return None;
        }
        let line: Line = serde_json::from_str(text).ok()?;
        if line.schema_version != 3
            || line.request_id != request_id
            || line.seq <= seq
            || line.seq > EVENT_LIMIT
        {
            return None;
        }
        seq = line.seq;
        match line.kind.as_str() {
            "step" => {
                let step: Step = serde_json::from_str(text).ok()?;
                if !token(&step.step) || !token(&step.status) {
                    return None;
                }
            }
            "result" => {
                let done: Done = serde_json::from_str(text).ok()?;
                if done.action != "initialize"
                    || !done.data.is_object()
                    || ![&done.status, &done.code, &done.retry]
                        .into_iter()
                        .all(|value| token(value))
                    || !done.completed_steps.iter().all(|step| token(step))
                {
                    return None;
                }
                result = Some(Answer {
                    status: done.status,
                    code: done.code,
                    message: done.message,
                    retry: done.retry,
                });
            }
            _ => return None,
        }
    }
    result
}

fn finished(result: Result<(), String>) -> Outcome {
    match result {
        Ok(()) => Outcome::Done,
        Err(reason) => Outcome::Failed(reason),
    }
}

/// Run a command to completion with setup's PATH. Output goes to a file, as
/// in the probe, so a background process cannot hold a pipe open.
fn run(command: &mut Command, path_env: &str) -> Result<(), String> {
    run_within(command, path_env, LIMIT)
}

fn run_within(command: &mut Command, path_env: &str, limit: Duration) -> Result<(), String> {
    let file =
        tempfile::tempfile().map_err(|_| "Setup could not create a temporary file.".to_owned())?;
    let output = |file: &std::fs::File| -> Result<Stdio, String> {
        file.try_clone()
            .map(Stdio::from)
            .map_err(|_| "Setup could not create a temporary file.".to_owned())
    };
    let mut child = command
        .env("PATH", path_env)
        .stdin(Stdio::null())
        .stdout(output(&file)?)
        .stderr(output(&file)?)
        // Its own group, so a timeout stops the downloader and installer too.
        .process_group(0)
        .spawn()
        .map_err(|error| format!("It could not start: {error}."))?;
    let deadline = Instant::now() + limit;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(200)),
            _ => {
                let group = rustix::process::Pid::from_child(&child);
                let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    match status {
        Some(status) if status.success() => Ok(()),
        Some(_) => Err(explain(&last_line(&file))),
        None => Err("It took too long and was stopped. Try again.".into()),
    }
}

/// The last meaningful line a failed command printed.
fn last_line(file: &std::fs::File) -> String {
    let length = file.metadata().map_or(0, |info| info.len());
    let start = length.saturating_sub(TAIL as u64);
    let mut bytes = vec![0; (length - start) as usize];
    let read = file.read_at(&mut bytes, start).unwrap_or(0);
    bytes.truncate(read);
    let text = String::from_utf8_lossy(&bytes);
    text.lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .map(|line| line.chars().take(TAIL).collect())
        .unwrap_or_default()
}

/// A failed command's explanation, in a sentence the window can show.
fn explain(line: &str) -> String {
    if line.is_empty() {
        return "It stopped with an error. Try again.".into();
    }
    if line.contains("npm: not found") || line.contains("npm: command not found") {
        return "It needs Node.js 20 or newer with npm. Install Node.js, then try again.".into();
    }
    let line: String = line.chars().take(200).collect();
    format!("It stopped with an error: {line}")
}

/// Shorten a path under the home folder to `~`.
pub fn home_relative(path: &Path) -> String {
    std::env::var_os("HOME")
        .and_then(|home| path.strip_prefix(home).ok())
        .map(|rest| format!("~/{}", rest.display()))
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_path() -> (tempfile::TempDir, String) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().display().to_string();
        (directory, format!("{path}:/usr/bin:/bin"))
    }

    /// The service's own fixtures, copied byte for byte.
    fn fixture(name: &str) -> serde_json::Value {
        serde_json::from_str(match name {
            "request" => include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../roster/src/profiles/setup/fixtures/setup-initialize.json"
            )),
            "step" => include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../roster/src/profiles/setup/fixtures/setup-event-step.json"
            )),
            "result" => include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../roster/src/profiles/setup/fixtures/setup-event-result.json"
            )),
            _ => unreachable!("{name}"),
        })
        .unwrap()
    }
    /// The request ID of the service's initialize fixture.
    const ID: &str = "019a8f3e7c2b7d4e9a1f2b3c4d5e6f70";

    #[test]
    fn initialize_is_the_services_request_and_reads_its_events() {
        assert_eq!(initialize_request(ID), fixture("request"));
        let printed = format!("{}\n{}\n", fixture("step"), fixture("result"));
        let read = answer(&printed, ID).unwrap();
        assert_eq!(
            (
                read.status.as_str(),
                read.code.as_str(),
                read.retry.as_str()
            ),
            ("completed", "initialized", "none")
        );
        assert_eq!(read.message, fixture("result")["message"]);
        // Another request's, out of order, after the result, another schema or action.
        let changed = |field: &str, value: serde_json::Value| {
            let mut result = fixture("result");
            result[field] = value;
            format!("{}\n{result}\n", fixture("step"))
        };
        for printed in [
            changed("request_id", "019a8f3e7c2b7d4e9a1f2b3c4d5e6f71".into()),
            changed("seq", 1.into()),
            changed("schema_version", 2.into()),
            changed("action", "first_agent".into()),
            changed("data", serde_json::Value::Null),
            changed("data", "none".into()),
            changed("completed_steps", serde_json::Value::Null),
            changed("completed_steps", 3.into()),
            changed("message", serde_json::Value::Null),
            format!("{}\n{}\n", fixture("result"), fixture("step")),
            fixture("step").to_string(),
            "not json".into(),
        ] {
            assert_eq!(answer(&printed, ID), None, "{printed}");
        }
        // A value over its bound, or not a token, as a first agent's are read.
        let long = serde_json::Value::from("c".repeat(129));
        for (field, value) in [
            ("retry", long.clone()),
            ("code", "a b".into()),
            ("status", long.clone()),
            ("completed_steps", serde_json::json!([long.clone()])),
        ] {
            let mut result = fixture("result");
            result[field] = value;
            let printed = format!("{}\n{result}\n", fixture("step"));
            assert_eq!(answer(&printed, ID), None, "{printed}");
        }
        // A line over its bound, or an event past the most a run has.
        let mut padded = fixture("step");
        padded["pad"] = "x".repeat(LINE_LIMIT).into();
        let late = changed("seq", (EVENT_LIMIT + 1).into());
        for printed in [format!("{padded}\n{}\n", fixture("result")), late] {
            assert_eq!(answer(&printed, ID), None);
        }
        // A field said twice, the right one last, is refused.
        let twice = fixture("result").to_string().replacen(
            '{',
            "{\"request_id\":\"019a8f3e7c2b7d4e9a1f2b3c4d5e6f71\",",
            1,
        );
        assert_eq!(answer(&format!("{}\n{twice}\n", fixture("step")), ID), None);
        let twice = fixture("step")
            .to_string()
            .replacen('{', "{\"status\":\"failed\",", 1);
        assert_eq!(
            answer(&format!("{twice}\n{}\n", fixture("result")), ID),
            None
        );
        let twice =
            fixture("result")
                .to_string()
                .replacen('{', "{\"code\":\"installation_conflict\",", 1);
        assert_eq!(answer(&format!("{}\n{twice}\n", fixture("step")), ID), None);
        // Nothing after the result, not even a blank line, and no line,
        // blank or not, over its bound.
        let printed = format!("{}\n{}\n\n", fixture("step"), fixture("result"));
        assert_eq!(answer(&printed, ID), None);
        let blank = " ".repeat(LINE_LIMIT + 1);
        let printed = format!("{}\n{blank}\n{}\n", fixture("step"), fixture("result"));
        assert_eq!(answer(&printed, ID), None);
        // A key said twice inside the data, which the app doesn't read.
        let mut twice = fixture("result");
        twice["data"]["profile"] = serde_json::json!({"profile_id": "0a1b2c3d4e5f"});
        let twice = twice.to_string().replacen(
            "\"profile\":{",
            "\"profile\":{\"profile_id\":\"0a1b2c3d4e60\",",
            1,
        );
        assert_eq!(answer(&format!("{}\n{twice}\n", fixture("step")), ID), None);
        // A step with a result's fields, of any type, is read as a step.
        let mut step = fixture("step");
        step["retry"] = serde_json::json!({"not": "a token"});
        step["data"] = 3.into();
        let printed = format!("{step}\n{}\n", fixture("result"));
        assert!(answer(&printed, ID).is_some());
        for (field, value) in [
            ("step", serde_json::Value::Null),
            ("step", long),
            ("status", serde_json::Value::Null),
            ("status", 3.into()),
        ] {
            let mut step = fixture("step");
            step[field] = value;
            let printed = format!("{step}\n{}\n", fixture("result"));
            assert_eq!(answer(&printed, ID), None, "{printed}");
        }
    }

    /// A fake `backbone`: it keeps each request it is given and prints the
    /// next of `answers`, a result of `initialize` for [`ID`] each, or
    /// nothing for a lost answer, and exits as setup does for it.
    fn fake(answers: &[Option<(&str, &str, &str)>]) -> (tempfile::TempDir, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        for (at, answer) in answers.iter().enumerate() {
            let printed = answer.map_or(String::new(), |(status, code, retry)| {
                let mut result = fixture("result");
                result["status"] = status.into();
                result["code"] = code.into();
                result["retry"] = retry.into();
                result["message"] = format!("the service says {code}").into();
                format!("{}\n{result}\n", fixture("step"))
            });
            std::fs::write(directory.path().join(format!("answer-{}", at + 1)), printed).unwrap();
            let exit = match answer {
                Some((_, "invalid_request", _)) => 2,
                Some(("completed", ..)) => 0,
                Some(("action_required", ..)) => 3,
                _ => 1,
            };
            std::fs::write(
                directory.path().join(format!("exit-{}", at + 1)),
                exit.to_string(),
            )
            .unwrap();
        }
        let script = directory.path().join("backbone");
        let root = directory.path().display();
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n[ \"$1 $2 $4\" = 'setup --file --jsonl' ] || exit 9\n\
                 /bin/cat \"$3\" >> '{root}/sent'; printf '\\n' >> '{root}/sent'\n\
                 n=$(($(/bin/cat '{root}/count' 2>/dev/null || echo 0) + 1)); echo $n > '{root}/count'\n\
                 /bin/cat '{root}/answer-'$n\n\
                 exit $(/bin/cat '{root}/exit-'$n)\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        // A test thread that forked while this one had the script open
        // keeps it open for writing until its child execs, and until then
        // running it fails with ETXTBSY. Wait for one run to start: it exits
        // 9 at once, on arguments setup never takes.
        loop {
            match std::process::Command::new(&script).status() {
                Err(error) if error.raw_os_error() == Some(libc::ETXTBSY) => {
                    std::thread::yield_now()
                }
                started => {
                    assert_eq!(started.unwrap().code(), Some(9));
                    break;
                }
            }
        }
        (directory, script)
    }
    /// The requests the fake was given.
    fn sent(directory: &tempfile::TempDir) -> Vec<serde_json::Value> {
        std::fs::read_to_string(directory.path().join("sent"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn an_answer_its_exit_contradicts_is_asked_for_again() {
        let initialized = Some(("completed", "initialized", "none"));
        let (directory, backbone) = fake(&[initialized, initialized]);
        std::fs::write(directory.path().join("exit-1"), "1").unwrap();
        assert_eq!(run(&backbone, &mut Some(ID.into())), Ok(()));
        assert_eq!(sent(&directory).len(), 2);
        let answer = |status: &str, code: &str| Answer {
            status: status.into(),
            code: code.into(),
            message: String::new(),
            retry: "none".into(),
        };
        for (status, code, exit) in [
            ("completed", "initialized", 0),
            ("failed", "installation_conflict", 1),
            ("failed", "invalid_request", 2),
            ("action_required", "runtime_missing", 3),
        ] {
            let answer = answer(status, code);
            assert!(answer.exits(Some(exit)), "{code}");
            assert!(!answer.exits(Some((exit + 1) % 4)), "{code}");
            assert!(!answer.exits(None), "{code}");
        }
        // A status this app doesn't know has no exit to match.
        assert!(answer("paused", "new_code").exits(Some(7)));
        assert!(answer("paused", "invalid_request").exits(Some(1)));
    }

    /// First run's setup of this computer, under the request `request` holds.
    fn run(backbone: &Path, request: &mut Option<String>) -> Result<(), String> {
        initialize_backbone(backbone, "/usr/bin:/bin", request)
    }

    #[test]
    fn an_answer_not_known_is_asked_for_with_the_same_request() {
        let (directory, backbone) = fake(&[
            None,
            Some(("action_required", "outcome_unknown", "same_request")),
            Some(("completed", "existing_preserved", "none")),
        ]);
        assert_eq!(run(&backbone, &mut Some(ID.into())), Ok(()));
        assert_eq!(sent(&directory), vec![fixture("request"); 3]);
    }

    #[test]
    fn no_service_answering_is_said_and_try_again_sends_the_same_request() {
        let unavailable = Some(("action_required", "service_unavailable", "same_request"));
        let (directory, backbone) =
            fake(&[unavailable, Some(("completed", "initialized", "none"))]);
        // Setup's own words, such as how to start the service; nothing is
        // restarted, and nothing sent again by itself.
        let mut request = Some(ID.to_owned());
        assert_eq!(
            run(&backbone, &mut request),
            Err("Agent Backbone's setup stopped: the service says service_unavailable".into())
        );
        assert_eq!(sent(&directory).len(), 1);
        // Try again sends the same request, as setup said it may.
        assert_eq!(request.as_deref(), Some(ID));
        assert_eq!(run(&backbone, &mut request), Ok(()));
        assert_eq!(sent(&directory), vec![fixture("request"); 2]);
        assert_eq!(request, None);
        // After one that may not follow, Try again makes a new request.
        for retry in ["new_request", "none", "later"] {
            let answer = Some(("action_required", "service_unavailable", retry));
            let (directory, backbone) = fake(&[answer]);
            let mut request = Some(ID.to_owned());
            assert!(run(&backbone, &mut request).is_err());
            assert_eq!(request, None, "{retry}");
            assert_eq!(sent(&directory).len(), 1, "{retry}");
        }
    }

    #[test]
    fn what_needs_the_person_is_said_and_not_sent_again() {
        for (status, code, retry) in [
            ("action_required", "runtime_missing", "same_request"),
            ("action_required", "profile_unavailable", "same_request"),
            ("failed", "installation_conflict", "none"),
            ("failed", "operator_required", "none"),
            // A retry this app doesn't know is no resend.
            ("action_required", "outcome_unknown", "later"),
            ("paused", "new_code", "none"),
        ] {
            let (directory, backbone) = fake(&[Some((status, code, retry))]);
            let said = run(&backbone, &mut Some(ID.into())).unwrap_err();
            assert!(said.contains(&format!("the service says {code}")), "{said}");
            assert_eq!(sent(&directory).len(), 1, "{code}");
        }
        // Lost every time: sent a bounded number of times, then said so;
        // Try again asks with the same request.
        let (directory, backbone) = fake(&[None, None, None]);
        let mut request = Some(ID.to_owned());
        assert_eq!(
            run(&backbone, &mut request),
            Err("Agent Backbone's setup did not report a result.".into())
        );
        assert_eq!(sent(&directory).len(), 3);
        assert_eq!(request.as_deref(), Some(ID));
    }

    #[test]
    fn a_tool_installer_that_succeeds_is_done() {
        let (_directory, path) = temporary_path();
        assert_eq!(tool("printf installed", &path), Outcome::Done);
    }

    #[test]
    fn a_timeout_stops_what_the_installer_started() {
        let (directory, path) = temporary_path();
        let pid = directory.path().join("pid");
        let script = format!("sleep 30 & echo $! > '{}'; wait", pid.display());
        let outcome = run_within(
            Command::new("/bin/sh").args(["-c", &script]),
            &path,
            Duration::from_millis(500),
        );
        assert_eq!(
            outcome,
            Err("It took too long and was stopped. Try again.".into())
        );
        let pid: i32 = std::fs::read_to_string(&pid)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let pid = rustix::process::Pid::from_raw(pid).unwrap();
        // The background sleep was in the installer's group, so it is gone.
        let deadline = Instant::now() + Duration::from_secs(5);
        while rustix::process::test_kill_process(pid).is_ok() {
            assert!(
                Instant::now() < deadline,
                "the installer's child kept running"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn a_download_that_fails_fails_the_install_it_feeds() {
        let (_directory, path) = temporary_path();
        assert_ne!(
            tool("echo 'download failed' >&2; exit 22 | sh", &path),
            Outcome::Done
        );
        assert_ne!(tool("false | sh", &path), Outcome::Done);
    }

    #[test]
    fn a_failing_installer_explains_itself_with_its_last_line() {
        let (_directory, path) = temporary_path();
        let outcome = tool(
            "printf 'downloading\\nno space left on device\\n' >&2; exit 3",
            &path,
        );
        assert_eq!(
            outcome,
            Outcome::Failed("It stopped with an error: no space left on device".into())
        );
        let silent = tool("exit 1", &path);
        assert_eq!(
            silent,
            Outcome::Failed("It stopped with an error. Try again.".into())
        );
    }

    #[test]
    fn a_missing_npm_says_what_to_install() {
        let (_directory, path) = temporary_path();
        let outcome = tool("echo 'sh: 1: npm: not found' >&2; exit 127", &path);
        assert_eq!(
            outcome,
            Outcome::Failed(
                "It needs Node.js 20 or newer with npm. Install Node.js, then try again.".into()
            )
        );
    }

    #[test]
    fn installers_see_the_path_setup_found() {
        let (directory, path) = temporary_path();
        let marker = directory.path().join("seen");
        let script = directory.path().join("fixture-tool");
        std::fs::write(
            &script,
            format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(tool("fixture-tool", &path), Outcome::Done);
        assert!(marker.is_file());
    }
}
