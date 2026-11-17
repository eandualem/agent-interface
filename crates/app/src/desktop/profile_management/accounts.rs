//! The Sign-in section of a profile on the Rust service: a row for each CLI
//! its agents run, saying whether a login is stored, with Sign in, which
//! opens the CLI's own sign-in session in a pane, and, for Claude Code and
//! Codex, Check, which asks the CLI itself. The service does each of these on
//! the profile's connection; the app runs no CLI and no tmux command for
//! them. Nothing is kept: what the section shows lives as long as it does,
//! and an answer that comes after it closed is dropped.

use super::{Open, secrets::CHANGED, ui};
use crate::desktop::design::{colors, tokens};
use agent_interface_roster::profiles::{
    Connection, Error, Profile,
    accounts::{Answer, Checked, Failure, Started, Storage, Stored},
};
use gpui::{Context, EventEmitter, SharedString, Window, div, prelude::*, px};

/// Each CLI's runtime ID, the name people know it by, and whether the
/// service can ask it whether it is signed in, in the order of the rows.
/// Antigravity CLI has no command to ask.
const TOOLS: [(&str, &str, bool); 3] = [
    ("claude", "Claude Code", true),
    ("codex", "Codex", true),
    ("antigravity", "Antigravity", false),
];

/// How agents reach GitHub, which signs in nowhere here.
const GITHUB: &str =
    "Paste a token in the GitHub row under Secrets below; agents use it as GITHUB_TOKEN.";

/// Check's label: asking a CLI may make the system ask for keychain access.
const CHECK: &str = "Check (may ask for keychain access)";
/// What a check refused because another of the same CLI runs says.
const RUNNING: &str = "A check is already running.";
/// What a stored login the service can't tell, and says nothing of, says.
const UNTOLD: &str = "Can't tell whether a login is stored.";
/// What an answer that couldn't be read says.
const UNREADABLE: &str = "Couldn't read the service's answer.";

/// One CLI's row.
#[derive(Default)]
struct Row {
    /// What the service read of its stored login, and the service's note
    /// beside it; `None` while it is read.
    stored: Option<(String, Option<String>)>,
    /// A sign-in is being started.
    starting: bool,
    /// Why the last sign-in didn't start.
    refused: Option<String>,
    /// A check runs.
    checking: bool,
    /// What the last check came to.
    checked: Option<String>,
}

/// The rows whose sign-in pane closed while the section was opening, whose
/// first read may be older than the sign-in.
#[derive(Default)]
struct Owed([bool; 3]);

impl Owed {
    /// The sign-in pane of row `index` closed.
    fn owe(&mut self, index: usize) {
        self.0[index] = true;
    }

    /// The rows to read again once the first answers are in: those owed, or
    /// none when opening failed and there is nothing to read them on.
    fn rows(self, opened: bool) -> Vec<usize> {
        (0..self.0.len())
            .filter(|&index| opened && self.0[index])
            .collect()
    }
}

pub(super) struct Section {
    connection: Option<Connection>,
    /// While the section opens, what it owes once it has.
    opening: Option<Owed>,
    /// In the order of [`TOOLS`].
    rows: [Row; 3],
}
impl EventEmitter<Open> for Section {}

impl Section {
    /// Connect to the profile and read each CLI's stored login on an I/O
    /// worker.
    pub(super) fn new(profile: Profile, cx: &mut Context<Self>) -> Self {
        let work = cx.background_executor().spawn(async move {
            let connection = profile.connect()?;
            let read = TOOLS.map(|(runtime, ..)| stored(connection.stored_login(runtime)));
            Ok::<_, Error>((connection, read))
        });
        cx.spawn(async move |this, cx| {
            let opened = work.await;
            let _ = this.update(cx, |section, cx| {
                let owed = section.opening.take().unwrap_or_default();
                let again = owed.rows(opened.is_ok());
                match opened {
                    Ok((connection, read)) => {
                        for (row, read) in section.rows.iter_mut().zip(read) {
                            row.stored = Some(read);
                        }
                        section.connection = Some(connection);
                        for index in again {
                            section.read(index, cx);
                        }
                    }
                    Err(error) => {
                        let why = failed(error.into());
                        for row in &mut section.rows {
                            row.stored = Some((why.clone(), None));
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        Self {
            connection: None,
            opening: Some(Owed::default()),
            rows: Default::default(),
        }
    }

    /// Start the sign-in of the CLI in row `index`, then show its session in
    /// a pane.
    fn sign_in(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(connection) = self.connection.clone() else {
            return;
        };
        let row = &mut self.rows[index];
        if row.starting {
            return;
        }
        row.starting = true;
        row.refused = None;
        let runtime = TOOLS[index].0;
        let work = cx
            .background_executor()
            .spawn(async move { opened(&connection, runtime) });
        cx.spawn(async move |this, cx| {
            let opened = work.await;
            let _ = this.update(cx, |section, cx| {
                let row = &mut section.rows[index];
                row.starting = false;
                match opened {
                    Ok(open) => cx.emit(open),
                    Err(why) => row.refused = Some(why),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Ask the CLI in row `index` whether it is signed in, once at a time.
    fn check(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(connection) = self.connection.clone() else {
            return;
        };
        let (runtime, _, checkable) = TOOLS[index];
        let row = &mut self.rows[index];
        if !checkable || row.checking {
            return;
        }
        row.checking = true;
        row.checked = None;
        let work = cx
            .background_executor()
            .spawn(async move { checked(connection.check_sign_in(runtime)) });
        cx.spawn(async move |this, cx| {
            let checked = work.await;
            let _ = this.update(cx, |section, cx| {
                let row = &mut section.rows[index];
                row.checking = false;
                row.checked = Some(checked);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// A sign-in pane of this profile closed: read that CLI's stored login
    /// again, or, while the section opens, once it has.
    pub(super) fn sign_in_ended(&mut self, runtime: &str, cx: &mut Context<Self>) {
        let Some(index) = TOOLS.iter().position(|(id, ..)| *id == runtime) else {
            return;
        };
        match &mut self.opening {
            Some(owed) => owed.owe(index),
            None => self.read(index, cx),
        }
    }

    /// Read the stored login of the CLI in row `index` again.
    fn read(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(connection) = self.connection.clone() else {
            return;
        };
        self.rows[index].stored = None;
        let runtime = TOOLS[index].0;
        let work = cx
            .background_executor()
            .spawn(async move { stored(connection.stored_login(runtime)) });
        cx.spawn(async move |this, cx| {
            let read = work.await;
            let _ = this.update(cx, |section, cx| {
                section.rows[index].stored = Some(read);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

/// Start `runtime`'s sign-in on the profile's service and say which session
/// to show, whether it started now or was running, or why it can't be shown.
/// Runs on an I/O worker.
fn opened(connection: &Connection, runtime: &str) -> Result<Open, String> {
    let Started { session, .. } = connection.start_sign_in(runtime).map_err(failed)?;
    Ok(Open::Shell {
        profile_id: connection.identity().profile_id.clone(),
        session,
    })
}

/// A stored login as its row says it, and the service's note when it gives
/// one beside `present` or `missing`. When it can't tell, its note is the
/// row's text.
fn stored(read: Result<Stored, Failure>) -> (String, Option<String>) {
    match read {
        Ok(Stored { storage, message }) => match storage {
            Storage::Present => ("Signed in (login stored)".into(), message),
            Storage::Missing => ("Not signed in".into(), message),
            Storage::Unavailable => (message.unwrap_or_else(|| UNTOLD.into()), None),
        },
        Err(failure) => (failed(failure), None),
    }
}

/// What a check came to, and when, in this computer's local time.
fn checked(answer: Result<Checked, Failure>) -> String {
    let checked = match answer {
        Ok(checked) => checked,
        Err(failure) => return failed(failure),
    };
    let Some(at) = local_time(&checked.checked_at) else {
        return UNREADABLE.into();
    };
    let said = match checked.answer {
        Answer::SignedIn => "signed in.".into(),
        Answer::SignedOut => "not signed in.".into(),
        Answer::Unknown => checked
            .message
            .unwrap_or_else(|| "couldn't tell whether it's signed in.".into()),
    };
    format!("Checked at {at}: {said}")
}

/// An RFC 3339 time as this computer's local time of day.
fn local_time(at: &str) -> Option<String> {
    let at = chrono::DateTime::parse_from_rfc3339(at).ok()?;
    Some(
        at.with_timezone(&chrono::Local)
            .format("%H:%M:%S")
            .to_string(),
    )
}

/// Why a read, a sign-in or a check came to nothing: the service's own words
/// when it gave them, else a sentence of the app's.
fn failed(failure: Failure) -> String {
    match failure {
        Failure::Running => RUNNING.into(),
        Failure::Refused(_, message) => message,
        Failure::Connection(
            Error::BindingRequired | Error::BindingMismatch | Error::Unsupported,
        ) => CHANGED.into(),
        Failure::Connection(Error::Invalid) => UNREADABLE.into(),
        Failure::Connection(Error::Refused(status)) => {
            format!("The service refused it ({status}).")
        }
        Failure::Connection(_) => "Couldn't reach the profile's service.".into(),
    }
}

impl Render for Section {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (t, c) = (tokens(), colors());
        let line =
            |text: SharedString| ui::text(div().text_color(c.muted), &t.text.meta).child(text);
        let connected = self.connection.is_some();
        let mut card = ui::group_card();
        for (index, (&(runtime, name, checkable), row)) in TOOLS.iter().zip(&self.rows).enumerate()
        {
            let mut lines = div()
                .flex()
                .flex_col()
                .child(div().text_color(c.text).child(name));
            match &row.stored {
                None => lines = lines.child(line("Looking for a stored login…".into())),
                Some((said, note)) => {
                    lines = lines
                        .child(line(said.clone().into()))
                        .children(note.clone().map(|note| line(note.into())));
                }
            }
            if row.starting {
                lines = lines.child(line("Starting the sign-in…".into()));
            }
            lines = lines.children(row.refused.clone().map(|why| line(why.into())));
            if row.checking {
                lines = lines.child(line("Checking… this can take up to 35 seconds.".into()));
            } else {
                lines = lines.children(row.checked.clone().map(|said| line(said.into())));
            }
            // The buttons sit under the lines and wrap, so a narrow window
            // keeps the lines readable.
            let mut buttons = div().flex().flex_wrap().gap(px(t.space.s8));
            let can_sign_in = connected && !row.starting;
            if can_sign_in {
                buttons = buttons.child(
                    ui::button(
                        SharedString::from(format!("login-sign-in-{runtime}")),
                        "Sign in",
                        ui::Button::Secondary,
                    )
                    .on_click(cx.listener(move |section, _, _, cx| section.sign_in(index, cx))),
                );
            }
            let can_check = connected && checkable && !row.checking;
            if can_check {
                buttons = buttons.child(
                    ui::button(
                        SharedString::from(format!("login-check-{runtime}")),
                        CHECK,
                        ui::Button::Secondary,
                    )
                    .on_click(cx.listener(move |section, _, _, cx| section.check(index, cx))),
                );
            }
            card = card.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(t.space.s8))
                    .py(px(t.space.s8))
                    .px(px(t.space.s14))
                    .when(index > 0, |el| {
                        el.border_t(px(t.size.hairline)).border_color(c.line)
                    })
                    .child(lines)
                    .when(can_sign_in || can_check, |el| el.child(buttons)),
            );
        }
        card.child(
            div()
                .flex()
                .flex_col()
                .py(px(t.space.s8))
                .px(px(t.space.s14))
                .border_t(px(t.size.hairline))
                .border_color(c.line)
                .child(div().text_color(c.text).child("GitHub"))
                .child(line(GITHUB.into())),
        )
    }
}

#[cfg(test)]
mod tests;
