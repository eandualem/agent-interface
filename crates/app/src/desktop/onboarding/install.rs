//! What first run fixes: Git or tmux with the system's own installer and an
//! agent tool with its official script, each on the person's click; and the
//! agent service, which first run starts by itself with Agent Backbone's own
//! setup: it keeps a profile the computer has, or sets one up, and this
//! installation's login service is restarted once when setup finds no
//! service answering. Every fix runs off the UI thread with a deadline, and
//! a failure explains itself.

use std::{
    ffi::OsString,
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
/// More than setup prints for `initialize`.
const SETUP_OUTPUT: u64 = 1024 * 1024;

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

/// Restart this installation's login service, the way Agent Backbone's
/// setup starts it, within `deadline`.
pub fn restart_backbone(backbone: &Path, path_env: &str, deadline: Instant) -> Outcome {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Outcome::Failed("The home folder is unknown.".into());
    };
    let definition = match own_service(&home, backbone) {
        Ok(Some(definition)) => definition,
        Ok(None) => return Outcome::Failed(not_installed()),
        Err(reason) => return Outcome::Failed(reason),
    };
    let within = |command: &mut Command| {
        run_within(
            command,
            path_env,
            deadline.saturating_duration_since(Instant::now()),
        )
    };
    if cfg!(target_os = "macos") {
        let domain = format!("gui/{}", uzers::get_current_uid());
        // Stopping fails when it was not running; starting is what counts.
        let _ = within(
            Command::new("/bin/launchctl")
                .args(["bootout", &domain])
                .arg(&definition),
        );
        return finished(within(
            Command::new("/bin/launchctl")
                .args(["bootstrap", &domain])
                .arg(&definition),
        ));
    }
    let Some(systemctl) = find("systemctl", path_env) else {
        return Outcome::Failed("This system has no service manager setup can use.".into());
    };
    finished(within(Command::new(systemctl).args([
        "--user",
        "restart",
        "agent-backbone-rs.service",
    ])))
}

/// This installation's login service: its definition when one is defined,
/// runs `backbone` and serves this app's Backbone directory. `None` when
/// none is defined; why it is left alone otherwise.
fn own_service(home: &Path, backbone: &Path) -> Result<Option<PathBuf>, String> {
    let definition = if cfg!(target_os = "macos") {
        home.join("Library/LaunchAgents/dev.agent-backbone-rs.plist")
    } else {
        // As the service keeps its unit: below $XDG_CONFIG_HOME when absolute.
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|config| config.is_absolute())
            .unwrap_or_else(|| home.join(".config"))
            .join("systemd/user/agent-backbone-rs.service")
    };
    if !definition.is_file() {
        return Ok(None);
    }
    // What may change a unit beside it is not this app's to restart, as
    // setup leaves such a unit as it is.
    if !cfg!(target_os = "macos")
        && let Some(found) = drop_in(&unit_dirs(home, |name| std::env::var_os(name)))
    {
        return Err(format!(
            "{} may change what the agent service runs, so it was left alone.",
            found.display()
        ));
    }
    owned(&definition, home, backbone)?;
    Ok(Some(definition))
}

/// The folders systemd looks for a user's units and their drop-ins in, as
/// the service lists them (systemd.unit(5)), each with whether the system's
/// packages keep it. A relative value is no value.
fn unit_dirs(home: &Path, variable: impl Fn(&str) -> Option<OsString>) -> Vec<(PathBuf, bool)> {
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
fn drop_in(dirs: &[(PathBuf, bool)]) -> Option<PathBuf> {
    for (dir, vendor) in dirs {
        for name in [
            "agent-backbone-rs.service.d",
            "agent-backbone-.service.d",
            "agent-.service.d",
        ] {
            let path = dir.join(name);
            if path.symlink_metadata().is_ok() {
                return Some(path);
            }
        }
        let every = dir.join("service.d");
        if every.symlink_metadata().is_err() {
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
        match std::fs::read_to_string(&path) {
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

/// Set this computer up with Agent Backbone's own setup, `initialize`: it
/// keeps a profile the computer has, or makes the first one with the core
/// starter and the login service. When setup finds no service answering,
/// this installation's login service is restarted once.
pub fn initialize_backbone(backbone: &Path, path_env: &str) -> Result<(), String> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("The home folder is unknown.")?;
    let request_id = uuid::Uuid::now_v7().simple().to_string();
    initialize(backbone, path_env, &request_id, &|deadline| {
        matches!(own_service(&home, backbone), Ok(Some(_)))
            .then(|| restart_backbone(backbone, path_env, deadline))
    })
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
/// sent again as it was. `restart` restarts this installation's login
/// service within the deadline it is given, or is `None` when it has none.
fn initialize(
    backbone: &Path,
    path_env: &str,
    request_id: &str,
    restart: &dyn Fn(Instant) -> Option<Outcome>,
) -> Result<(), String> {
    let request = initialize_request(request_id).to_string();
    let deadline = Instant::now() + SETUP_LIMIT;
    let mut restarted = false;
    let mut last = None;
    let mut sends = SENDS;
    while sends > 0 {
        sends -= 1;
        let Some(answer) = send(backbone, path_env, &request, request_id, deadline)? else {
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
            // No service answers, and this request may be sent again:
            // this installation's, restarted once, may answer it.
            (_, "service_unavailable", "same_request") if !restarted => {
                restarted = true;
                match restart(deadline) {
                    None => return Err(said),
                    Some(Outcome::Failed(why)) => {
                        return Err(format!("{said} Restarting its service failed: {why}"));
                    }
                    // One send is kept for after the restart.
                    Some(_) => sends = sends.max(1),
                }
            }
            // Sent, with what came of it not known, or stopped before it
            // changed anything: the same request again.
            (_, "outcome_unknown" | "setup_failed", "same_request") => {}
            _ => return Err(said),
        }
        last = Some(said);
    }
    Err(last.unwrap_or_else(|| "Agent Backbone's setup did not report a result.".into()))
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
            _ if self.code == "invalid_request" => exit == Some(2),
            "completed" => exit == Some(0),
            "failed" => exit == Some(1),
            "action_required" => exit == Some(3),
            _ => true,
        }
    }

    /// The answer's own words for a person, else its code.
    fn said(&self) -> String {
        let message: String = self
            .message
            .chars()
            .filter(|c| !c.is_control())
            .take(400)
            .collect();
        let message = message.trim();
        format!(
            "Agent Backbone's setup stopped: {}",
            if message.is_empty() {
                &self.code
            } else {
                message
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
            .take(SETUP_OUTPUT + 1)
            .read_to_string(&mut printed)
    });
    if read.is_err() || printed.len() as u64 > SETUP_OUTPUT {
        return Ok(None);
    }
    // An answer its exit contradicts is no readable answer.
    Ok(answer(&printed, request_id).filter(|answer| answer.exits(exit)))
}

/// What setup answered `request_id`, from the lines it printed: each an
/// event of that request, in order, and the last its `initialize` result.
/// `None` when what it printed is not that.
fn answer(printed: &str, request_id: &str) -> Option<Answer> {
    #[derive(serde::Deserialize)]
    struct Line {
        schema_version: u32,
        request_id: String,
        seq: u64,
        #[serde(rename = "type")]
        kind: String,
        action: Option<String>,
        step: Option<String>,
        status: Option<String>,
        code: Option<String>,
        message: Option<String>,
        retry: Option<String>,
        data: Option<serde_json::Value>,
        completed_steps: Option<Vec<String>>,
    }
    let mut seq = 0;
    let mut result = None;
    for text in printed.lines().filter(|text| !text.trim().is_empty()) {
        let line: Line = serde_json::from_str(text).ok()?;
        if result.is_some()
            || line.schema_version != 3
            || line.request_id != request_id
            || line.seq <= seq
        {
            return None;
        }
        seq = line.seq;
        match line.kind.as_str() {
            "step" if line.step.is_some() && line.status.is_some() => {}
            "result"
                if line.action.as_deref() == Some("initialize")
                    && line.data.as_ref().is_some_and(serde_json::Value::is_object)
                    && line.completed_steps.is_some() =>
            {
                result = Some(Answer {
                    status: line.status?,
                    code: line.code?,
                    message: line.message?,
                    retry: line.retry?,
                });
            }
            _ => return None,
        }
    }
    result
}

/// The Backbone directory this app's Agent Backbone uses, as the service
/// finds it: `BACKBONE_DIR`, else `$XDG_DATA_HOME/backbone`, else
/// `~/.local/share/backbone`. A relative value is no value.
fn backbone_dir(home: &Path, variable: impl Fn(&str) -> Option<OsString>) -> PathBuf {
    let absolute = |name: &str| {
        variable(name)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    absolute("BACKBONE_DIR")
        .or_else(|| absolute("XDG_DATA_HOME").map(|data| data.join("backbone")))
        .unwrap_or_else(|| home.join(".local/share/backbone"))
}

/// Whether the installed service is this installation's: it runs
/// `backbone`, which the service names by its canonical path, on this app's
/// Backbone directory. Another's, or one that can't be read, is left alone.
fn owned(definition: &Path, home: &Path, backbone: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(definition)
        .map_err(|_| "The agent service's definition could not be read.".to_owned())?;
    let program = backbone
        .canonicalize()
        .unwrap_or_else(|_| backbone.to_path_buf());
    this_installation(
        &text,
        &program,
        &backbone_dir(home, |name| std::env::var_os(name)),
    )
}

/// Whether the login service `text` defines runs `program` on the Backbone
/// directory `dir`, and why it is left alone when it doesn't.
fn this_installation(text: &str, program: &Path, dir: &Path) -> Result<(), String> {
    let Some((runs, served)) = runs(text) else {
        return Err(
            "What the agent service runs could not be read from its definition, so it was left alone."
                .into(),
        );
    };
    if Path::new(&runs) != program {
        return Err(format!(
            "The installed agent service runs {runs}, not {}, so it was left alone.",
            program.display()
        ));
    }
    if Path::new(&served) != dir {
        return Err(format!(
            "The installed agent service serves {served}, not {}, so it was left alone.",
            dir.display()
        ));
    }
    Ok(())
}

/// What a login service's definition runs, read as the service writes it:
/// its program and its `BACKBONE_DIR`. A definition in another form says
/// nothing, so one the service did not write is never taken for its own.
fn runs(text: &str) -> Option<(String, String)> {
    if text.trim_start().starts_with("<?xml") {
        runs_plist(text)
    } else {
        runs_unit(text)
    }
}

/// A launchd plist as the service writes it: `ProgramArguments` of exactly
/// the program, `daemon` and `--raw-output`, and `BACKBONE_DIR` once in
/// `EnvironmentVariables`, each key and its string on one line. `Program`,
/// which launchd runs in place of the arguments' first, or a comment, which
/// may show or hide anything, says nothing.
fn runs_plist(text: &str) -> Option<(String, String)> {
    if text.contains("<!--") {
        return None;
    }
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    let (mut program, mut dir, mut variables) = (None, None, false);
    for (index, line) in lines.iter().enumerate() {
        // launchd runs Program in place of the arguments' first.
        if line.starts_with("<key>Program</key>") {
            return None;
        }
        match *line {
            "<key>ProgramArguments</key>" => {
                let [open, first, daemon, raw, close] = lines.get(index + 1..index + 6)? else {
                    return None;
                };
                if program.is_some()
                    || *open != "<array>"
                    || *daemon != "<string>daemon</string>"
                    || *raw != "<string>--raw-output</string>"
                    || *close != "</array>"
                {
                    return None;
                }
                program = Some(xml_string(first)?);
            }
            "<key>EnvironmentVariables</key>" => {
                if lines.get(index + 1) != Some(&"<dict>") {
                    return None;
                }
                variables = true;
            }
            "</dict>" => variables = false,
            _ => {}
        }
        if let Some(rest) = line.strip_prefix("<key>BACKBONE_DIR</key>") {
            if !variables || dir.is_some() {
                return None;
            }
            dir = Some(xml_string(rest)?);
        }
    }
    Some((program?, dir?))
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
fn runs_unit(text: &str) -> Option<(String, String)> {
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

/// The text of a plist `<string>`, unescaped.
fn xml_string(line: &str) -> Option<String> {
    let text = line.strip_prefix("<string>")?.strip_suffix("</string>")?;
    Some(
        text.replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&apos;", "'")
            .replace("&amp;", "&"),
    )
}

fn not_installed() -> String {
    "Agent Backbone's service isn't installed. Run the Agent Interface installer again to add it."
        .into()
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
    /// The LaunchAgent the service writes for `program` on `dir`.
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
    fn ours(text: &str, program: &str, dir: &str) -> bool {
        this_installation(text, Path::new(program), Path::new(dir)).is_ok()
    }

    #[test]
    fn a_unit_is_this_installations_only_when_systemd_would_run_it_so() {
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
    fn a_launch_agent_is_this_installations_only_when_launchd_would_run_it_so() {
        let (program, dir) = ("/opt/my <tools> & 'co'/backbone", "/Users/a/back\"bone\"");
        assert!(ours(&plist_of(program, dir), program, dir));
        let plain = ("/opt/bin/backbone", "/Users/a/backbone");
        // Another program or directory whose name only starts as this one's.
        assert!(!ours(
            &plist_of("/opt/bin/backbone-old", plain.1),
            plain.0,
            plain.1
        ));
        assert!(!ours(
            &plist_of(plain.0, "/Users/a/backbone/2"),
            plain.0,
            plain.1
        ));
        // This one's program and directory in a comment only, a Program
        // launchd would run in place of the one it names, another argument,
        // or the directory outside its variables or named twice.
        let other = plist_of("/elsewhere/backbone", "/Users/b/backbone");
        let commented = other.replacen(
            "<dict>",
            "<!-- <string>/opt/bin/backbone</string> <string>/Users/a/backbone</string> -->\n<dict>",
            1,
        );
        let text = plist_of(plain.0, plain.1);
        assert!(ours(&text, plain.0, plain.1));
        for changed in [
            commented,
            text.replace(
                "<key>Label</key>",
                "<key>Program</key><string>/bin/sh</string>\n  <key>Label</key>",
            ),
            text.replace(
                "<string>--raw-output</string>",
                "<string>--raw-output</string>\n    <string>--other</string>",
            ),
            text.replace(
                "<key>RunAtLoad</key>",
                "<key>BACKBONE_DIR</key><string>/Users/a/backbone</string>\n  <key>RunAtLoad</key>",
            ),
            text.replace(
                "<key>PATH</key>",
                "<key>BACKBONE_DIR</key><string>/Users/b/backbone</string>\n    <key>PATH</key>",
            ),
        ] {
            assert!(!ours(&changed, plain.0, plain.1), "{changed}");
        }
        // Why another installation's is left alone.
        let refused = this_installation(&other, Path::new(plain.0), Path::new(plain.1));
        assert!(refused.unwrap_err().contains("runs /elsewhere/backbone"));
        let refused = this_installation(&text, Path::new(plain.0), Path::new("/Users/c"));
        assert!(refused.unwrap_err().contains("serves /Users/a/backbone"));
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
        assert_eq!(drop_in(&dirs), Some(conf));
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

    #[test]
    fn the_backbone_directory_is_found_as_the_service_finds_it() {
        let home = Path::new("/home/a");
        let with = |pairs: &'static [(&str, &str)]| {
            backbone_dir(home, |name| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| OsString::from(value))
            })
        };
        assert_eq!(with(&[]), Path::new("/home/a/.local/share/backbone"));
        assert_eq!(
            with(&[("XDG_DATA_HOME", "/data")]),
            Path::new("/data/backbone")
        );
        assert_eq!(
            with(&[("XDG_DATA_HOME", "/data"), ("BACKBONE_DIR", "/b")]),
            Path::new("/b")
        );
        // A relative value is no value.
        assert_eq!(
            with(&[("BACKBONE_DIR", "b"), ("XDG_DATA_HOME", "data")]),
            Path::new("/home/a/.local/share/backbone")
        );
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
        for (field, value) in [
            ("step", serde_json::Value::Null),
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
        assert_eq!(
            initialize(&backbone, "/usr/bin:/bin", ID, &|_| None),
            Ok(())
        );
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
    }

    #[test]
    fn an_answer_not_known_is_asked_for_with_the_same_request() {
        let (directory, backbone) = fake(&[
            None,
            Some(("action_required", "outcome_unknown", "same_request")),
            Some(("completed", "existing_preserved", "none")),
        ]);
        let restarts = std::cell::Cell::new(0);
        let restart = |_: Instant| {
            restarts.set(restarts.get() + 1);
            Some(Outcome::Done)
        };
        assert_eq!(initialize(&backbone, "/usr/bin:/bin", ID, &restart), Ok(()));
        let sent = sent(&directory);
        assert_eq!(sent, vec![fixture("request"); 3]);
        assert_eq!(restarts.get(), 0);
    }

    #[test]
    fn no_service_answering_restarts_this_installations_once_and_asks_again() {
        let unavailable = Some(("action_required", "service_unavailable", "same_request"));
        let (directory, backbone) =
            fake(&[unavailable, Some(("completed", "initialized", "none"))]);
        let restarts = std::cell::Cell::new(0);
        let restart = |deadline: Instant| {
            // Within setup's own limit, not an installer's.
            assert!(deadline <= Instant::now() + SETUP_LIMIT);
            restarts.set(restarts.get() + 1);
            Some(Outcome::Done)
        };
        assert_eq!(initialize(&backbone, "/usr/bin:/bin", ID, &restart), Ok(()));
        assert_eq!((sent(&directory).len(), restarts.get()), (2, 1));
        // A send is kept for after a restart, however many came before it.
        let (directory, backbone) = fake(&[
            None,
            None,
            unavailable,
            Some(("completed", "initialized", "none")),
        ]);
        assert_eq!(initialize(&backbone, "/usr/bin:/bin", ID, &restart), Ok(()));
        assert_eq!((sent(&directory).len(), restarts.get()), (4, 2));
        // Once: still none answering after the restart is what setup says.
        let (_directory, backbone) = fake(&[unavailable, unavailable]);
        let refused = initialize(&backbone, "/usr/bin:/bin", ID, &restart).unwrap_err();
        assert!(
            refused.contains("the service says service_unavailable"),
            "{refused}"
        );
        // No service of this installation's: setup's own words, such as how
        // to start PostgreSQL, and nothing else.
        let (directory, backbone) = fake(&[unavailable]);
        assert_eq!(
            initialize(&backbone, "/usr/bin:/bin", ID, &|_| None),
            Err("Agent Backbone's setup stopped: the service says service_unavailable".into())
        );
        assert_eq!(sent(&directory).len(), 1);
        let (_directory, backbone) = fake(&[unavailable]);
        let failed = initialize(&backbone, "/usr/bin:/bin", ID, &|_| {
            Some(Outcome::Failed("launchd refused it.".into()))
        })
        .unwrap_err();
        assert!(failed.ends_with("Restarting its service failed: launchd refused it."));
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
            // Only the same request is sent after a restart: none here.
            ("action_required", "service_unavailable", "new_request"),
            ("action_required", "service_unavailable", "later"),
        ] {
            let (directory, backbone) = fake(&[Some((status, code, retry))]);
            let restart = |_: Instant| -> Option<Outcome> { panic!("{code}/{retry} restarted") };
            let said = initialize(&backbone, "/usr/bin:/bin", ID, &restart).unwrap_err();
            assert!(said.contains(&format!("the service says {code}")), "{said}");
            assert_eq!(sent(&directory).len(), 1, "{code}");
        }
        // Lost every time: sent a bounded number of times, then said so.
        let (directory, backbone) = fake(&[None, None, None]);
        assert_eq!(
            initialize(&backbone, "/usr/bin:/bin", ID, &|_| None),
            Err("Agent Backbone's setup did not report a result.".into())
        );
        assert_eq!(sent(&directory).len(), 3);
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
