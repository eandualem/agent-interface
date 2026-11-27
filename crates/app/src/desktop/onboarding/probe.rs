//! What first-run setup checks on this computer: the tools a terminal finds
//! and whether Agent Backbone answers. Read-only; `install` holds the fixes.

use std::{
    os::unix::{fs::FileExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// Commands looked up through the user's own shell. tmux is not among them:
/// terminals use the app's own resolver, so setup checks with that.
const COMMANDS: &[&str] = &["git", "backbone", "claude", "codex", "opencode"];

/// Agent tools Backbone can run: command, name, official install command.
pub const TOOLS: &[(&str, &str, &str)] = &[
    (
        "claude",
        "Claude Code",
        "curl -fsSL https://claude.ai/install.sh | bash",
    ),
    (
        "codex",
        "Codex",
        "curl -fsSL https://chatgpt.com/codex/install.sh | sh",
    ),
    (
        "opencode",
        "OpenCode",
        "curl -fsSL https://opencode.ai/v2/install | bash",
    ),
];

const MARK: &str = "@@agent-interface-probe";
const SHELL_LIMIT: Duration = Duration::from_secs(8);
/// More than enough for our lines after any startup-file noise.
const OUTPUT_LIMIT: usize = 256 * 1024;

/// Whether Agent Backbone is set up and answering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Backbone {
    /// No `backbone` command, or no profile served yet.
    NotSetUp,
    /// Set up, but the last request failed.
    Unreachable(&'static str),
    /// Answering, but older than this app supports or of no version it can
    /// read; the reason says what to do.
    Outdated(String),
    /// Answering.
    Running,
}

/// One complete check.
#[derive(Clone, Debug)]
pub struct Probe {
    pub path_env: String,
    found: Vec<(&'static str, PathBuf)>,
    /// tmux as terminals will find it.
    pub tmux: Option<PathBuf>,
    pub backbone: Backbone,
}

impl Probe {
    /// Look everything up. Blocks for up to a few seconds; call off the UI thread.
    pub fn run() -> Self {
        let (mut found, path_env) = shell_lookup()
            .unwrap_or_else(|| (Vec::new(), std::env::var("PATH").unwrap_or_default()));
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let (fallback, path_env) = path_lookup(&path_env, home.as_deref());
        for (name, path) in fallback {
            if !found.iter().any(|(existing, _)| *existing == name) {
                found.push((name, path));
            }
        }
        let mut probe = Self {
            found,
            path_env,
            tmux: agent_interface_tmux::connection::tmux_binary().ok(),
            backbone: Backbone::NotSetUp,
        };
        // The same executable and PATH a second login shell would find.
        probe.backbone = answers(probe.client());
        probe
    }

    /// A check that found only `backbone` at `path`, with PATH `path_env`.
    #[cfg(test)]
    pub fn with_backbone(path: PathBuf, path_env: String) -> Self {
        Self {
            path_env,
            found: vec![("backbone", path)],
            tmux: None,
            backbone: Backbone::NotSetUp,
        }
    }

    /// A profile client for the Backbone this check found, with its PATH.
    pub fn client(&self) -> Option<crate::profiles::Client> {
        let executable = self.path("backbone")?.clone();
        crate::profiles::Client::new(executable, self.path_env.clone().into()).ok()
    }

    /// Where a terminal finds this command, if it does.
    pub fn path(&self, command: &str) -> Option<&PathBuf> {
        self.found
            .iter()
            .find(|(name, _)| *name == command)
            .map(|(_, path)| path)
    }
}

/// Ask the user's login shell, as a new terminal pane would, so tools added
/// to PATH in shell startup files are found even when the app was opened
/// from the Dock with launchd's short PATH.
fn shell_lookup() -> Option<(Vec<(&'static str, PathBuf)>, String)> {
    let shell = std::env::var_os("SHELL")
        .filter(|shell| !shell.is_empty())
        .unwrap_or_else(|| "/bin/sh".into());
    let script = format!(
        "for c in {}; do printf '{MARK}\\t%s\\t%s\\n' \"$c\" \"$(command -v \"$c\" 2>/dev/null)\"; done",
        COMMANDS.join(" ")
    ) + "\nprintf '@@agent-interface-path\t%s\n' \"$PATH\"";
    // Output goes to a file, not a pipe: a background process started by a
    // startup file can keep a pipe open forever, but cannot stop a file read.
    let file = tempfile::tempfile().ok()?;
    let mut child = probe_command(&shell, &script)
        .stdin(Stdio::null())
        .stdout(file.try_clone().ok()?)
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + SHELL_LIMIT;
    while matches!(child.try_wait(), Ok(None)) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    // Positional reads leave the shared file offset alone, so a process still
    // writing cannot overwrite our lines or skip the reader past them.
    let mut bytes = vec![0; OUTPUT_LIMIT];
    let mut length = 0;
    while length < bytes.len() {
        match file.read_at(&mut bytes[length..], length as u64) {
            Ok(0) => break,
            Ok(read) => length += read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
    bytes.truncate(length);
    // Startup-file noise need not be UTF-8; our lines are.
    let output = String::from_utf8_lossy(&bytes);
    let found = parse(&output);
    // A shell that printed nothing of ours did not run the script.
    let path_env = output
        .lines()
        .find_map(|line| line.strip_prefix("@@agent-interface-path\t"))?
        .to_owned();
    output.contains(MARK).then_some((found, path_env))
}

/// Let the login shell initialize PATH, then parse the probe with POSIX sh.
/// C shells request login mode through argv[0]; their -l must stand alone.
fn probe_command(shell: &std::ffi::OsStr, script: &str) -> Command {
    let mut command = Command::new(shell);
    let name = std::path::Path::new(shell)
        .file_name()
        .and_then(|name| name.to_str());
    if matches!(name, Some("csh" | "tcsh")) {
        command
            .arg0(format!("-{}", name.unwrap()))
            .args(["-i", "-c"]);
    } else {
        command.args(["-l", "-i", "-c"]);
    }
    command
        .env("AGENT_INTERFACE_PROBE", script)
        .arg("exec /bin/sh -c \"$AGENT_INTERFACE_PROBE\"");
    command
}

/// Our marked lines from the shell's output; startup-file noise is ignored.
fn parse(output: &str) -> Vec<(&'static str, PathBuf)> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.strip_prefix(MARK)?.strip_prefix('\t')?.splitn(2, '\t');
            let name = fields.next()?;
            let path = fields.next()?.trim();
            let command = COMMANDS.iter().find(|command| **command == name)?;
            path.starts_with('/')
                .then(|| (*command, PathBuf::from(path)))
        })
        .collect()
}

/// Keep the shell's precedence, but also find tools installed while the app
/// was open. Some login profiles do not source the installer's shell rc file.
/// Use this same PATH for provider discovery and the submitted setup request.
fn path_lookup(path_env: &str, home: Option<&Path>) -> (Vec<(&'static str, PathBuf)>, String) {
    use std::os::unix::fs::PermissionsExt;
    let mut directories: Vec<PathBuf> = std::env::split_paths(path_env).collect();
    let mut extra = Vec::new();
    if let Some(home) = home {
        extra.extend([home.join(".local/bin"), home.join(".opencode/bin")]);
    }
    extra.extend([
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ]);
    for directory in extra {
        if directory.is_dir()
            && !directories.contains(&directory)
            && directory.to_str().is_some_and(|path| !path.contains(':'))
        {
            directories.push(directory);
        }
    }
    let path_env = std::env::join_paths(&directories)
        .ok()
        .and_then(|path| path.into_string().ok())
        .unwrap_or_else(|| path_env.to_owned());
    let found = COMMANDS
        .iter()
        .filter_map(|command| {
            directories
                .iter()
                .map(|directory| directory.join(command))
                .find(|path| {
                    path.metadata()
                        .is_ok_and(|info| info.is_file() && info.permissions().mode() & 0o111 != 0)
                })
                .map(|path| (*command, path))
        })
        .collect();
    (found, path_env)
}

/// Whether the first profile Agent Backbone serves answers, read through its
/// verified connection with a client already found, so the login shell is
/// not asked again. First run makes the Default profile. Blocks for a
/// moment; call off the UI thread.
pub fn answers(client: Option<crate::profiles::Client>) -> Backbone {
    use agent_interface_roster::profiles::State;
    const UNANSWERED: &str = "The first serving profile is not answering";
    let live = || true;
    let Some(client) = client else {
        return Backbone::NotSetUp;
    };
    let items = match client.list(&live) {
        Ok(items) => items,
        Err(_) => return Backbone::Unreachable("Backbone did not list its profiles"),
    };
    let Some(served) = items.iter().find(|item| item.state == State::Ready) else {
        return Backbone::NotSetUp;
    };
    let connected = client
        .show(&served.profile_id, &live)
        .ok()
        .map(|profile| profile.connect());
    let roster = match connected {
        Some(Err(
            error @ (agent_interface_roster::profiles::Error::Outdated(_)
            | agent_interface_roster::profiles::Error::VersionUnknown),
        )) => {
            let error = crate::profiles::Error::Profile(error);
            return Backbone::Outdated(super::super::profile_picker::message(&error).into());
        }
        connected => connected
            .and_then(Result::ok)
            .and_then(|connection| connection.roster().ok()),
    };
    match roster {
        Some(_) => Backbone::Running,
        None => Backbone::Unreachable(UNANSWERED),
    }
}

/// The service's own sentence on why it does not serve `saved`, the first
/// profile made before, when it lists that profile as failed. With nothing
/// saved and none served, the first failed profile's. Blocks for a moment;
/// call off the UI thread.
pub fn saved_failure(
    client: Option<crate::profiles::Client>,
    saved: Option<&str>,
) -> Option<String> {
    use agent_interface_roster::profiles::State;
    let client = client?;
    let Some(saved) = saved else {
        let items = client.list(&|| true).ok()?;
        // With a profile served, its failed neighbours don't explain setup.
        if items.iter().any(|item| item.state == State::Ready) {
            return None;
        }
        let item = items.into_iter().find(|item| item.state == State::Failed)?;
        tracing::warn!(event = "first_profile_not_served", profile_id = %item.profile_id, code = ?item.failure, "First profile not served");
        return item.reason;
    };
    let profile = client.show(saved, &|| true).ok()?;
    let item = profile.item();
    if item.state != State::Failed {
        return None;
    }
    // The stable code and the profile go to the log; the sentence never does.
    tracing::warn!(event = "first_profile_not_served", profile_id = %item.profile_id, code = ?item.failure, "First profile not served");
    item.reason.clone()
}

/// The PATH a new terminal has: the login shell's, plus the usual install
/// folders. Blocks for up to a few seconds; call off the UI thread.
pub fn login_path() -> String {
    let path_env = shell_lookup()
        .map(|(_, path_env)| path_env)
        .unwrap_or_else(|| std::env::var("PATH").unwrap_or_default());
    let home = std::env::var_os("HOME").map(PathBuf::from);
    path_lookup(&path_env, home.as_deref()).1
}

/// Resolve only the provider executable; never probe ambient provider content.
pub(in crate::desktop) fn profile_client() -> Option<crate::profiles::Client> {
    let (mut found, path_env) =
        shell_lookup().unwrap_or_else(|| (Vec::new(), std::env::var("PATH").unwrap_or_default()));
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let (fallback, path_env) = path_lookup(&path_env, home.as_deref());
    found.extend(fallback);
    let executable = found.into_iter().find(|(name, _)| *name == "backbone")?.1;
    crate::profiles::Client::new(executable, path_env.into()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newly_installed_opencode_is_found_and_reaches_the_provider_path() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join(".opencode/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let tool = bin.join("opencode");
        std::fs::write(&tool, "#!/bin/sh\nexit 0\n").unwrap();
        let (found, _) = path_lookup("/usr/bin:/bin", Some(home.path()));
        assert!(!found.iter().any(|(_, path)| path == &tool));
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        let (found, path_env) = path_lookup("/usr/bin:/bin", Some(home.path()));
        assert!(found.contains(&("opencode", tool.clone())));
        let output = Command::new("/bin/sh")
            .args(["-c", "command -v opencode"])
            .env("PATH", path_env)
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8(output.stdout).unwrap().trim(),
            tool.to_str().unwrap()
        );
    }

    #[test]
    fn parse_keeps_only_marked_absolute_paths_for_known_commands() {
        let output = format!(
            "Welcome back!\n{MARK}\tgit\t/usr/bin/git\n{MARK}\tgh\t\n\
             {MARK}\tcodex\tcodex: aliased to x\n{MARK}\ttmux\t/bin/tmux\n{MARK}\tunknown\t/bin/x\n\
             {MARK}\tclaude\t/Users/me/.local/bin/claude\n"
        );
        assert_eq!(
            parse(&output),
            vec![
                ("git", PathBuf::from("/usr/bin/git")),
                ("claude", PathBuf::from("/Users/me/.local/bin/claude")),
            ]
        );
    }

    #[test]
    fn login_shell_startup_path_reaches_the_posix_probe() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let tools = home.path().join("extra tools");
        std::fs::create_dir(&tools).unwrap();
        let tool = tools.join("codex");
        std::fs::write(&tool, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        let posix = format!("export PATH=\"{}:$PATH\"\n", tools.display());
        let csh = format!("setenv PATH \"{}:$PATH\"\n", tools.display());
        for file in [".bash_profile", ".zprofile"] {
            std::fs::write(home.path().join(file), &posix).unwrap();
        }
        for file in [".cshrc", ".tcshrc"] {
            std::fs::write(home.path().join(file), &csh).unwrap();
        }
        std::fs::create_dir_all(home.path().join(".config/fish")).unwrap();
        std::fs::write(
            home.path().join(".config/fish/config.fish"),
            format!("set -gx PATH \"{}\" $PATH\n", tools.display()),
        )
        .unwrap();
        let script = format!(
            "for c in codex; do printf '{MARK}\\t%s\\t%s\\n' \"$c\" \"$(command -v \"$c\")\"; done"
        );
        for shell in [
            "/bin/bash",
            "/bin/zsh",
            "/bin/csh",
            "/bin/tcsh",
            "/usr/bin/fish",
            "/opt/homebrew/bin/fish",
        ] {
            if !std::path::Path::new(shell).exists() {
                continue;
            }
            let output = probe_command(shell.as_ref(), &script)
                .env("HOME", home.path())
                .env("ZDOTDIR", home.path())
                .env("XDG_CONFIG_HOME", home.path().join(".config"))
                .stdin(Stdio::null())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{shell}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(
                parse(&String::from_utf8_lossy(&output.stdout)),
                vec![("codex", tool.clone())],
                "{shell}"
            );
        }
    }

    /// A stub `backbone` in `root` that logs each call and prints `listed` for
    /// a list and `shown` for a show.
    fn stub(root: &std::path::Path, listed: &str, shown: &str) -> crate::profiles::Client {
        use std::os::unix::fs::PermissionsExt;
        // Executing a file that any process holds open for writing fails on Linux
        // with ETXTBSY, and a child another test thread forks while this one writes
        // inherits the descriptor. So `cp` writes the executed file, in its own
        // process, and this process never opens it for writing.
        let source = root.join("provider.sh");
        std::fs::write(
            &source,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$(dirname \"$0\")/calls\"\ncase \"$2\" in\n  list) printf '%s' '{listed}';;\n  show) printf '%s' '{shown}';;\nesac\n"
            ),
        )
        .unwrap();
        let provider = root.join("provider");
        let copied = std::process::Command::new("/bin/cp")
            .arg(&source)
            .arg(&provider)
            .status()
            .unwrap();
        assert!(copied.success());
        std::fs::set_permissions(&provider, std::fs::Permissions::from_mode(0o700)).unwrap();
        crate::profiles::Client::new(provider, "/usr/bin:/bin".into()).unwrap()
    }

    #[test]
    fn a_saved_profile_that_failed_gives_the_services_reason() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let failed = serde_json::json!({"state": "failed", "profile_id": "0a1b2c3d4e5f",
            "code": "start_failed", "reason": "Its folder is not readable."});
        let second = serde_json::json!({"state": "failed", "profile_id": "0a1b2c3d4e60",
            "code": "start_failed", "reason": "Its settings are not readable."});
        let listed = serde_json::json!({"profiles": [failed, second]});
        let client = || stub(&root, &listed.to_string(), &failed.to_string());
        assert_eq!(
            saved_failure(Some(client()), Some("0a1b2c3d4e5f")).as_deref(),
            Some("Its folder is not readable.")
        );
        // A fresh first run, with nothing saved: the first failed profile's.
        assert_eq!(
            saved_failure(Some(client()), None).as_deref(),
            Some("Its folder is not readable.")
        );
        // With a profile served, no failed one explains setup.
        let served = serde_json::json!({"state": "serving", "protocol_version": 1,
            "profile_id": "0a1b2c3d4e61", "display_name": "Work",
            "instance_id": "0190c3a2c1107a3b8c9d0e1f2a3b4c5d", "machine": {"id": "5f2c9e0d1b7a4e38"},
            "socket": root.join("api.sock"), "tmux_socket": root.join("tmux.sock"),
            "credential_reference": {"type": "socket"}});
        let listed = serde_json::json!({"profiles": [served, failed]});
        let client = stub(&root, &listed.to_string(), &failed.to_string());
        assert_eq!(saved_failure(Some(client), None), None);
        assert_eq!(saved_failure(None, Some("0a1b2c3d4e5f")), None);
    }

    #[test]
    fn only_the_first_serving_profile_is_probed() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let entry = |id: &str| {
            serde_json::json!({"state": "serving", "protocol_version": 1, "profile_id": id,
                "display_name": id, "instance_id": "0190c3a2c1107a3b8c9d0e1f2a3b4c5d",
                "machine": {"id": "5f2c9e0d1b7a4e38"}, "socket": root.join(format!("{id}.sock")),
                "tmux_socket": root.join("tmux.sock"), "credential_reference": {"type": "socket"}})
        };
        let failed = serde_json::json!({"state": "failed", "profile_id": "0a1b2c3d4e5e",
            "code": "start_failed", "reason": "It did not start."});
        let listed =
            serde_json::json!({"profiles": [failed, entry("0a1b2c3d4e5f"), entry("0a1b2c3d4e60")]});
        let shown = entry("0a1b2c3d4e5f");
        let client = stub(&root, &listed.to_string(), &shown.to_string());
        // Its socket isn't there, so it doesn't answer, and the next is not asked.
        assert_eq!(
            answers(Some(client)),
            Backbone::Unreachable("The first serving profile is not answering")
        );
        assert_eq!(
            std::fs::read_to_string(root.join("calls")).unwrap(),
            "profiles list --json\nprofiles show --json -- 0a1b2c3d4e5f\n"
        );
    }
}
