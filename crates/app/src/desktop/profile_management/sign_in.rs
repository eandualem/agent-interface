//! Sign in until the Rust service signs CLIs in itself: a shell in the
//! profile's own environment, and the steps to sign each CLI in there. The
//! steps show only when that shell points every CLI's settings, and Git's,
//! inside the profile's folder.

use super::{Open, ui};
use crate::desktop::design::{colors, tokens};
use agent_interface_roster::profiles::Profile;
use gpui::{Context, EventEmitter, Window, div, prelude::*, px};
use std::{
    io,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
};

/// Where each CLI keeps its settings, and Git its name and email.
const OWN: [&str; 4] = [
    "CLAUDE_CONFIG_DIR",
    "CODEX_HOME",
    "GH_CONFIG_DIR",
    "GIT_CONFIG_GLOBAL",
];

/// What to run in the shell for each account, and what to do next.
const STEPS: [(&str, &str, &str); 4] = [
    ("Claude Code", "claude", "then type /login"),
    ("Codex", "codex login", ""),
    (
        "Git name",
        "git config --global user.name \"Your Name\"",
        "",
    ),
    (
        "Git email",
        "git config --global user.email you@example.com",
        "",
    ),
];

/// How agents reach GitHub, which needs no sign-in in the shell.
const GITHUB: &str =
    "GitHub: paste a token in the GitHub row under Secrets below; agents use it as GITHUB_TOKEN.";

enum State {
    Idle,
    Opening,
    /// The shell points every one of [`OWN`] inside the profile's folder.
    Own,
    /// The shell uses this computer's logins for at least one CLI.
    Shared,
    /// The shell's settings couldn't be read.
    Unchecked,
    Failed(&'static str),
}

/// Whether a shell keeps the profile's logins apart from this computer's.
#[derive(Debug, PartialEq)]
enum Check {
    Own,
    Shared,
    Unchecked,
}

pub(super) struct Pane {
    profile: Profile,
    state: State,
}
impl EventEmitter<Open> for Pane {}

impl Pane {
    pub(super) fn new(profile: Profile) -> Self {
        Self {
            profile,
            state: State::Idle,
        }
    }

    fn open(&mut self, cx: &mut Context<Self>) {
        if matches!(self.state, State::Opening) {
            return;
        }
        self.state = State::Opening;
        let profile = self.profile.clone();
        let work = cx
            .background_executor()
            .spawn(async move { open_shell(&profile) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |pane, cx| {
                pane.state = match result {
                    Ok((session, check)) => {
                        cx.emit(Open::Shell {
                            profile_id: pane.profile.item().profile_id.clone(),
                            session,
                        });
                        match check {
                            Check::Own => State::Own,
                            Check::Shared => State::Shared,
                            Check::Unchecked => State::Unchecked,
                        }
                    }
                    Err(error) => State::Failed(error),
                };
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

/// Start a new shell in the profile and check where it keeps its logins.
/// Runs on an I/O worker.
fn open_shell(profile: &Profile) -> Result<(String, Check), &'static str> {
    let connection = profile
        .connect()
        .map_err(|_| "The profile could not be reached. Start its service, then try again.")?;
    let session = connection
        .ensure_shell(&shell_id(), None)
        .map_err(|_| "The profile's shell could not be started. Try again.")?;
    let socket = connection.tmux_socket();
    let check = match socket.parent() {
        Some(folder) => check(folder, |name| variable(socket, &session, name)),
        None => Check::Unchecked,
    };
    Ok((session, check))
}

/// A new shell's ID: 12 lowercase hexadecimal digits, as the Rust service requires.
fn shell_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..12].to_owned()
}

/// Whether every variable in [`OWN`], as `read` gives it, is inside `folder`.
/// One that is missing or outside it settles it, even if another can't be read.
fn check(folder: &Path, read: impl Fn(&str) -> io::Result<Option<String>>) -> Check {
    let mut check = Check::Own;
    for name in OWN {
        match read(name) {
            Err(_) => check = Check::Unchecked,
            Ok(Some(value)) if inside(Path::new(&value), folder) => {}
            Ok(_) => return Check::Shared,
        }
    }
    check
}

/// The value `session`'s shell started with for `name`: the session's own
/// setting, else the tmux server's. `None` when unset or removed.
fn variable(socket: &Path, session: &str, name: &str) -> io::Result<Option<String>> {
    match setting(socket, &["-t", &format!("={session}")], name)? {
        Some(entry) => Ok(entry),
        None => Ok(setting(socket, &["-g"], name)?.flatten()),
    }
}

/// One `show-environment` entry: `None` when it isn't there, `Some(None)`
/// when it removes `name`.
fn setting(socket: &Path, scope: &[&str], name: &str) -> io::Result<Option<Option<String>>> {
    let output = Command::new(agent_interface_tmux::connection::tmux_binary()?)
        .arg("-S")
        .arg(socket)
        .arg("show-environment")
        .args(scope)
        .arg(name)
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()?;
    let stdout = String::from_utf8(output.stdout)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "not UTF-8"))?;
    if !output.status.success() {
        // tmux says so when the scope doesn't set `name`; anything else is a failure.
        return if String::from_utf8_lossy(&output.stderr).contains("unknown variable") {
            Ok(None)
        } else {
            Err(io::Error::other("tmux show-environment failed"))
        };
    }
    entry(&stdout, name).map(Some)
}

/// Read `show-environment`'s line for `name`: `NAME=value`, or `-NAME` when
/// the scope removes it.
fn entry(output: &str, name: &str) -> io::Result<Option<String>> {
    let line = output.strip_suffix('\n').unwrap_or(output);
    if line.strip_prefix('-') == Some(name) {
        return Ok(None);
    }
    line.strip_prefix(name)
        .and_then(|rest| rest.strip_prefix('='))
        .map(|value| Some(value.to_owned()))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "unexpected entry"))
}

/// Whether `path` is strictly inside `folder`, after following symbolic
/// links. Parts that don't exist yet, such as a CLI's folder before its first
/// login, are taken as written.
fn inside(path: &Path, folder: &Path) -> bool {
    if !path.is_absolute() || path.components().any(|part| part == Component::ParentDir) {
        return false;
    }
    match (resolved(path), folder.canonicalize()) {
        (Ok(path), Ok(folder)) => path != folder && path.starts_with(&folder),
        _ => false,
    }
}

/// `path` with its longest existing start resolved, and the rest appended.
/// A link whose target is missing could point anywhere, so it is an error.
fn resolved(path: &Path) -> io::Result<PathBuf> {
    let mut rest = Vec::new();
    let mut existing = path;
    loop {
        match existing.canonicalize() {
            Ok(real) => return Ok(rest.iter().rev().fold(real, |path, part| path.join(part))),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if existing.symlink_metadata().is_ok() {
                    return Err(io::Error::other("a link to a missing path"));
                }
                rest.push(existing.file_name().ok_or(error)?);
                existing = existing
                    .parent()
                    .ok_or_else(|| io::Error::other("no parent"))?;
            }
            Err(error) => return Err(error),
        }
    }
}

impl Render for Pane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (t, c) = (tokens(), colors());
        let mut body = div().flex().flex_col().gap(px(t.space.s8));
        let opening = matches!(self.state, State::Opening);
        body = body.child(
            div()
                .flex()
                .items_center()
                .gap(px(t.space.s12))
                .child(div().flex_1().min_w_0().child(ui::group_note(if opening {
                    "Opening a shell in this profile…"
                } else {
                    "Opens a shell in this profile's own environment."
                })))
                .when(!opening, |row| {
                    row.child(
                        ui::button("profile-sign-in", "Sign in", ui::Button::Secondary)
                            .on_click(cx.listener(|pane, _, _, cx| pane.open(cx))),
                    )
                }),
        );
        match &self.state {
            State::Idle | State::Opening => {}
            State::Own => {
                body = body.child(ui::group_note("In the new shell, run each of these:"));
                let mut card = ui::group_card();
                for (index, (account, command, then)) in STEPS.iter().enumerate() {
                    card = card.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(t.space.s10))
                            .h(px(t.size.group_row))
                            .px(px(t.space.s14))
                            .when(index > 0, |el| {
                                el.border_t(px(t.size.hairline)).border_color(c.line)
                            })
                            .child(div().w(px(96.)).text_color(c.text).child(*account))
                            .child(ui::mono(*command).min_w_0().text_color(c.text))
                            .when(!then.is_empty(), |el| {
                                el.child(div().text_color(c.muted).child(*then))
                            }),
                    );
                }
                body = body.child(card).child(ui::group_note(GITHUB));
            }
            State::Shared => {
                body = body.child(ui::group_note(
                    "This profile shares this computer's logins. Signing in from its shell \
                     would change them for this computer too, so it isn't suggested here.",
                ))
            }
            State::Unchecked => {
                body = body.child(ui::group_note(
                    "Couldn't check whether this profile keeps its own logins, so signing \
                     in from its shell isn't suggested.",
                ))
            }
            State::Failed(error) => body = body.child(ui::group_note(*error)),
        }
        body
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_login_counts_only_strictly_inside_the_profile_folder() {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("profile");
        std::fs::create_dir_all(folder.join("accounts")).unwrap();
        std::os::unix::fs::symlink(root.path(), folder.join("escape")).unwrap();
        let dangling = folder.join("accounts-link");
        std::os::unix::fs::symlink(root.path().join("outside-missing"), &dangling).unwrap();
        let own = |path: PathBuf| inside(&path, &folder);
        assert!(own(folder.join("accounts")));
        assert!(own(folder.join("accounts/codex/not-yet")));
        assert!(!own(folder.clone()));
        assert!(!own(root.path().join("other")));
        assert!(!own(folder.join("accounts/../../other")));
        assert!(!own(folder.join("escape/other")));
        assert!(!own(dangling.clone()));
        assert!(!own(dangling.join("codex")));
        assert!(!own(PathBuf::from("profile/accounts")));
    }

    #[test]
    fn every_login_must_be_the_profiles_own() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("accounts").to_str().unwrap().to_owned();
        let all = |name: &str| Ok(Some(format!("{path}/{name}")));
        assert_eq!(check(folder.path(), all), Check::Own);
        let missing = |name: &str| Ok((name != "GH_CONFIG_DIR").then(|| format!("{path}/{name}")));
        assert_eq!(check(folder.path(), missing), Check::Shared);
        let outside = |name: &str| Ok(Some(format!("/elsewhere/{name}")));
        assert_eq!(check(folder.path(), outside), Check::Shared);
        let unread = |_: &str| Err(io::Error::other("tmux"));
        assert_eq!(check(folder.path(), unread), Check::Unchecked);
        let partly = |name: &str| match name {
            "CODEX_HOME" => Err(io::Error::other("tmux")),
            "GIT_CONFIG_GLOBAL" => Ok(None),
            _ => Ok(Some(format!("{path}/{name}"))),
        };
        assert_eq!(check(folder.path(), partly), Check::Shared);
    }

    #[test]
    fn show_environment_lines_read_as_set_or_removed() {
        assert_eq!(
            entry("CODEX_HOME=/p/accounts/codex\n", "CODEX_HOME").unwrap(),
            Some("/p/accounts/codex".into())
        );
        assert_eq!(entry("-CODEX_HOME\n", "CODEX_HOME").unwrap(), None);
        assert!(entry("CODEX_HOMEX=/p\n", "CODEX_HOME").is_err());
    }
}
