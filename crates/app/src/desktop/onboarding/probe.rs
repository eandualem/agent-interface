//! What first-run setup checks on this computer: the tools a terminal finds
//! and whether Agent Backbone answers. Read-only; nothing is installed here.

use std::{
    os::unix::{fs::FileExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// Commands looked up through the user's own shell. tmux is not among them:
/// terminals use the app's own resolver, so setup checks with that.
const COMMANDS: &[&str] = &[
    "git", "gh", "backbone", "claude", "codex", "gemini", "opencode",
];

/// Agent tools Backbone can run: command, name, official install command.
pub const TOOLS: &[(&str, &str, &str)] = &[
    (
        "claude",
        "Claude Code",
        "curl -fsSL https://claude.ai/install.sh | bash",
    ),
    ("codex", "Codex", "npm install -g @openai/codex"),
    ("gemini", "Gemini CLI", "npm install -g @google/gemini-cli"),
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
    /// No Backbone database at its data directory.
    NotSetUp,
    /// Set up, but the last request failed.
    Unreachable(&'static str),
    /// Answering, with this many registered agents.
    Running { agents: usize },
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
        Self {
            found,
            path_env,
            tmux: agent_interface_tmux::connection::tmux_binary().ok(),
            backbone: backbone(),
        }
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

fn backbone() -> Backbone {
    match crate::roster::provider_directory(None, None, None, false) {
        Ok(Some(directory)) => {
            let mut snapshot = agent_interface_roster::Snapshot::default();
            snapshot.fetch_configured(&directory, None);
            match snapshot.error {
                Some(error) => Backbone::Unreachable(error),
                None => Backbone::Running {
                    agents: snapshot.states.len(),
                },
            }
        }
        _ => Backbone::NotSetUp,
    }
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
}
