//! What first-run setup checks on this computer: the tools a terminal finds
//! and whether Agent Backbone answers. Read-only; nothing is installed here.

use std::{
    io::Read,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// Commands looked up through the user's own shell.
const COMMANDS: &[&str] = &[
    "tmux", "git", "gh", "backbone", "claude", "codex", "gemini", "opencode",
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
        "curl -fsSL https://opencode.ai/install | bash",
    ),
];

const MARK: &str = "@@agent-interface-probe";
const SHELL_LIMIT: Duration = Duration::from_secs(8);

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
    found: Vec<(&'static str, PathBuf)>,
    pub backbone: Backbone,
}

impl Probe {
    /// Look everything up. Blocks for up to a few seconds; call off the UI thread.
    pub fn run() -> Self {
        let found = shell_lookup().unwrap_or_else(path_lookup);
        Self {
            found,
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
fn shell_lookup() -> Option<Vec<(&'static str, PathBuf)>> {
    let shell = std::env::var_os("SHELL")
        .filter(|shell| !shell.is_empty())
        .unwrap_or_else(|| "/bin/sh".into());
    let script = format!(
        "for c in {}; do printf '{MARK}\\t%s\\t%s\\n' \"$c\" \"$(command -v \"$c\" 2>/dev/null)\"; done",
        COMMANDS.join(" ")
    );
    let mut child = Command::new(shell)
        .args(["-l", "-i", "-c", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < SHELL_LIMIT => {
                std::thread::sleep(Duration::from_millis(50));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut output = String::new();
    child.stdout.take()?.read_to_string(&mut output).ok()?;
    let found = parse(&output);
    // A shell that printed nothing of ours did not run the script.
    output.contains(MARK).then_some(found)
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

/// Without a usable shell: this process's PATH and the usual install places.
fn path_lookup() -> Vec<(&'static str, PathBuf)> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut directories: Vec<PathBuf> =
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect();
    directories.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));
    if let Some(home) = home {
        directories.push(home.join(".local/bin"));
    }
    COMMANDS
        .iter()
        .filter_map(|command| {
            directories
                .iter()
                .map(|directory| directory.join(command))
                .find(|path| path.is_file())
                .map(|path| (*command, path))
        })
        .collect()
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
    fn parse_keeps_only_marked_absolute_paths_for_known_commands() {
        let output = format!(
            "Welcome back!\n{MARK}\ttmux\t/opt/homebrew/bin/tmux\n{MARK}\tgh\t\n\
             {MARK}\tgit\tgit: aliased to hub\n{MARK}\tunknown\t/bin/x\n\
             {MARK}\tclaude\t/Users/me/.local/bin/claude\n"
        );
        assert_eq!(
            parse(&output),
            vec![
                ("tmux", PathBuf::from("/opt/homebrew/bin/tmux")),
                ("claude", PathBuf::from("/Users/me/.local/bin/claude")),
            ]
        );
    }
}
