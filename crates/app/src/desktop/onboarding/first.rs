//! First-agent selection with durable retries and explicit tool changes.
//! The provider cancels the previous request before its choices are unlocked.

use std::{collections::HashMap, path::PathBuf};

use futures::{StreamExt, channel::mpsc};
use gpui::{
    Context, Entity, EventEmitter, PathPromptOptions, SharedString, Subscription, Window, div,
    prelude::*, px,
};

use super::{heading, setup};
use crate::desktop::{
    components::{self as ui, Button},
    design::{colors, tokens},
    name::NameField,
};

pub enum FirstEvent {
    RefreshTools,
    Pending,
    /// The agent is ready, or running and waiting for the person in its
    /// terminal; either way setup can move on and open that terminal.
    Ready(String),
    Existing,
    OpenTerminal(String),
}

/// The setup stages in the order the provider reports them.
const STAGES: [(&str, &str); 5] = [
    ("preflight", "Check your choices"),
    ("hooks", "Prepare the selected tool"),
    ("agent", "Create your agent"),
    ("launch", "Open your agent"),
    ("greeting", "Submit the introduction"),
];

const WAITING: &str = "Your agent is waiting for you. Approve its prompt in the terminal, then check again, or continue and it keeps waiting.";

/// What the setup window's main button does on this step. It never sits
/// disabled without saying what it waits for.
pub enum Primary {
    Waiting(SharedString),
    Start(SharedString),
    ChangeTool(&'static str),
    Continue,
}

enum Update {
    Progress(setup::Event),
    Finished(Result<setup::Event, &'static str>),
}

/// Discovery describes tools; only an explicit user choice selects one.
#[derive(Default)]
struct ToolChoice {
    rows: Vec<setup::Runtime>,
    selected: Option<String>,
}

impl ToolChoice {
    fn replace(&mut self, rows: Vec<setup::Runtime>) {
        self.rows = rows;
        if self.current().is_none() {
            self.selected = None;
        }
    }

    fn select(&mut self, id: &str) {
        if self
            .rows
            .iter()
            .any(|row| row.id == id && row.id != "shell")
        {
            self.selected = Some(id.into());
        }
    }

    fn current(&self) -> Option<&setup::Runtime> {
        self.rows
            .iter()
            .find(|row| Some(&row.id) == self.selected.as_ref() && row.id != "shell")
    }

    fn start_runtime(&self) -> Option<&str> {
        self.current()
            .filter(|row| row.can_setup())
            .map(|row| row.id.as_str())
    }
}

pub struct FirstAgent {
    executable: PathBuf,
    path_env: String,
    data_dir: PathBuf,
    save_path: PathBuf,
    existing: bool,
    name: Entity<NameField>,
    brief: Entity<NameField>,
    custom_brief: bool,
    directory: Option<PathBuf>,
    tools: ToolChoice,
    discovering: bool,
    discovery_generation: u64,
    busy: bool,
    saved: Option<setup::Saved>,
    result: Option<setup::Event>,
    error: Option<String>,
    progress: String,
    steps: HashMap<String, String>,
    _fields: Vec<Subscription>,
}

impl EventEmitter<FirstEvent> for FirstAgent {}

impl FirstAgent {
    pub fn new(
        executable: Option<PathBuf>,
        path_env: String,
        data_dir: PathBuf,
        existing: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name = cx.new(|cx| {
            let mut field = NameField::filter(window, cx);
            field.set_value("guide", cx);
            field
        });
        let brief = cx.new(|cx| NameField::feedback(window, cx));
        let fields = vec![
            cx.observe(&name, |_, _, cx| cx.notify()),
            cx.observe(&brief, |_, _, cx| cx.notify()),
        ];
        let save_path =
            crate::desktop::settings::path().map(|path| setup::Saved::path(&path, &data_dir));
        let loaded = save_path
            .as_deref()
            .ok_or("The settings directory is unavailable.")
            .and_then(setup::Saved::load);
        let (saved, error) = match loaded {
            Ok(saved) => (saved, None),
            Err(error) => (None, Some(error.into())),
        };
        if let Some(saved) = &saved {
            name.update(cx, |field, cx| {
                field.set_value(&saved.request.agent.name, cx);
                field.set_read_only(true);
            });
        }
        let (executable, path_env) = match (executable, &saved) {
            (Some(executable), _) => (executable, path_env),
            (None, Some(saved)) => (saved.executable.clone(), saved.path_env.clone()),
            (None, None) => (PathBuf::new(), path_env),
        };
        let mut first = Self {
            executable,
            path_env,
            data_dir,
            save_path: save_path.unwrap_or_default(),
            existing,
            name,
            brief,
            custom_brief: false,
            directory: None,
            tools: ToolChoice::default(),
            discovering: false,
            discovery_generation: 0,
            busy: false,
            saved,
            result: None,
            error,
            progress: String::new(),
            steps: HashMap::new(),
            _fields: fields,
        };
        if first.saved.is_none() {
            first.discover(cx);
        }
        first
    }

    fn has_pending_request(&self) -> bool {
        self.saved.is_some() || self.busy
    }

    /// Leaving setup never discards its saved request. Allow terminal use
    /// after any settled outcome, including an operation that needs recovery.
    pub fn can_use_terminals(&self) -> bool {
        !self.busy
    }

    /// Rechecking the computer updates an unsubmitted form, while submitted
    /// requests retain their original launcher and environment for replay.
    pub fn refresh(&mut self, probe: &super::probe::Probe, cx: &mut Context<Self>) {
        if let Some(executable) =
            setup::active_engine(&self.data_dir).or_else(|| probe.path("backbone").cloned())
        {
            self.executable = executable;
            self.path_env = probe.path_env.clone();
        } else if let Some(saved) = &self.saved {
            self.executable = saved.executable.clone();
            self.path_env = saved.path_env.clone();
        } else {
            self.executable = PathBuf::new();
            self.path_env = probe.path_env.clone();
        }
        if self.has_pending_request() {
            return;
        }
        self.existing =
            matches!(probe.backbone, super::probe::Backbone::Running { agents } if agents > 0);
        self.tools.rows.clear();
        self.discovery_generation += 1;
        self.discovering = false;
        self.discover(cx);
    }

    fn discover(&mut self, cx: &mut Context<Self>) {
        if self.discovering || self.busy {
            return;
        }
        if self.executable.as_os_str().is_empty() {
            self.error = Some("The setup engine is unavailable. Return to Check to reconnect it, or use terminals for now.".into());
            cx.notify();
            return;
        }
        self.discovering = true;
        self.discovery_generation += 1;
        let generation = self.discovery_generation;
        let executable = self.executable.clone();
        let path_env = self.path_env.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { setup::runtimes(&executable, &path_env) })
                .await;
            let _ = this.update(cx, |first, cx| {
                if generation != first.discovery_generation {
                    return;
                }
                first.discovering = false;
                match result {
                    Ok(rows) => {
                        first.error = None;
                        first.tools.replace(rows);
                    }
                    Err(error) => first.error = Some(error.into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn folder(&self, cx: &Context<Self>) -> Option<PathBuf> {
        self.directory.clone().or_else(|| {
            let home = std::env::var_os("HOME")?;
            Some(
                PathBuf::from(home)
                    .join("Agents")
                    .join(self.name.read(cx).value().trim()),
            )
        })
    }

    fn choose_folder(&mut self, cx: &mut Context<Self>) {
        let selection = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Agent folder".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = selection.await {
                let _ = this.update(cx, |first, cx| {
                    if first.saved.is_none() && !first.busy {
                        first.directory = paths.into_iter().next();
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn prepare(&self, cx: &Context<Self>) -> Result<setup::Saved, &'static str> {
        let name = self.name.read(cx).value().trim();
        if !name
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
            || name.len() > 80
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        {
            return Err(
                "Use up to 80 letters, numbers, dots, dashes or underscores, starting with a letter or number.",
            );
        }
        let runtime = self
            .tools
            .start_runtime()
            .ok_or("Choose an installed agent tool.")?;
        let directory = self
            .folder(cx)
            .filter(|path| path.is_absolute())
            .ok_or("Choose an absolute agent folder.")?;
        let brief = if self.custom_brief {
            let text = self.brief.read(cx).value().trim().to_owned();
            if text.is_empty() {
                return Err("Enter instructions for the agent, or use the coordinating brief.");
            }
            setup::Brief::Text { text }
        } else {
            setup::Brief::Template {
                template: "orchestrator-v1".into(),
            }
        };
        Ok(setup::Saved {
            cancelling: false,
            path_env: self.path_env.clone(),
            executable: self.executable.clone(),
            request: setup::Request {
                schema_version: 1,
                request_id: uuid::Uuid::new_v4().to_string(),
                action: "first_agent".into(),
                data_dir: self.data_dir.clone(),
                hooks: vec![runtime.into()],
                agent: setup::Agent {
                    name: name.into(),
                    directory,
                    create_directory: true,
                    runtime: runtime.into(),
                    model: None,
                    brief,
                    terminal_defaults: self
                        .tools
                        .current()
                        .filter(|runtime| runtime.terminal_defaults_supported)
                        .map(|_| crate::desktop::terminal::defaults()),
                },
                start_greeting: true,
            },
        })
    }

    pub fn start(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if self.saved.as_ref().is_some_and(|saved| saved.cancelling) {
            self.edit_choices(cx);
            return;
        }
        if self.saved.is_none() {
            let prepared = self.prepare(cx).and_then(|saved| {
                saved.create(&self.save_path)?;
                Ok(saved)
            });
            match prepared {
                Ok(saved) => self.saved = Some(saved),
                Err(error) => {
                    self.error = Some(error.into());
                    cx.notify();
                    return;
                }
            }
        }
        let saved = self.saved.as_ref().unwrap().clone();
        if saved.request.data_dir != self.data_dir {
            self.error = Some("The saved request belongs to a different installation. Reopen setup with its original connection.".into());
            cx.notify();
            return;
        }
        self.name.update(cx, |field, _| field.set_read_only(true));
        self.brief.update(cx, |field, _| field.set_read_only(true));
        self.busy = true;
        cx.emit(FirstEvent::Pending);
        self.error = None;
        self.result = None;
        self.steps.clear();
        self.progress = "Setting up your agent…".into();
        let (sender, mut receiver) = mpsc::unbounded();
        let save_path = self.save_path.clone();
        cx.background_executor()
            .spawn(async move {
                let result = saved.run_saved(&save_path, |event| {
                    let _ = sender.unbounded_send(Update::Progress(event));
                });
                let _ = sender.unbounded_send(Update::Finished(result));
            })
            .detach();
        cx.spawn(async move |this, cx| {
            while let Some(update) = receiver.next().await {
                if this.update(cx, |first, cx| {
                    match update {
                        Update::Progress(event) if event.kind == "step" => {
                            first.steps.insert(event.step, event.status);
                        }
                        Update::Progress(_) => {},
                        Update::Finished(result) => {
                            first.busy = false;
                            match result {
                                Ok(result) => {
                                    for step in &result.completed_steps {
                                        first.steps.insert(step.clone(), "completed".into());
                                    }
                                    let expected = first.saved.as_ref().map(|saved| saved.request.agent.name.as_str());
                                    let named = result.data.get("agent_name").and_then(|name| name.as_str());
                                    if result.status == "completed" && result.code == "agent_ready" && named == expected
                                        && result.data.get("ready").and_then(|ready| ready.as_str()) == Some("ready") {
                                        first.progress = if result.data.get("greeting_delivery_id").is_some_and(|id| !id.is_null()) {
                                            "Your agent is ready. Its introduction has been submitted."
                                        } else {
                                            "Your agent is ready. Open its terminal to continue."
                                        }.into();
                                        cx.emit(FirstEvent::Ready(expected.unwrap().to_owned()));
                                    } else if result.status == "completed" {
                                        first.error = Some(setup::UNKNOWN.into());
                                        cx.notify();
                                        return;
                                    } else if waiting(&result, expected) {
                                        first.progress = WAITING.into();
                                        cx.emit(FirstEvent::Ready(expected.unwrap().to_owned()));
                                    } else {
                                        first.progress = result.message.clone();
                                    }
                                    first.result = Some(result);
                                }
                                Err(error) => first.error = Some(error.into()),
                            }
                        }
                    }
                    cx.notify();
                }).is_err() { break; }
            }
        }).detach();
        cx.notify();
    }

    pub fn edit_choices(&mut self, cx: &mut Context<Self>) {
        if self.busy || self.saved.is_none() || self.is_ready() {
            return;
        }
        if self
            .saved
            .as_ref()
            .is_some_and(|saved| saved.request.data_dir != self.data_dir)
        {
            self.error = Some(
                "Reconnect to the saved setup's original installation before changing tools."
                    .into(),
            );
            cx.notify();
            return;
        }
        let saved = self.saved.as_mut().unwrap();
        if let Err(error) = saved.begin_cancel(&self.save_path) {
            self.error = Some(error.into());
            cx.notify();
            return;
        }
        let saved = saved.clone();
        let path = self.save_path.clone();
        let executable = self.executable.clone();
        let path_env = self.path_env.clone();
        self.busy = true;
        self.error = None;
        self.steps.clear();
        self.progress = "Releasing the previous setup before changing tools…".into();
        cx.emit(FirstEvent::Pending);
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move {
                    let result = saved.cancel_saved(&path, &executable, &path_env);
                (saved, result)
            }).await;
            let _ = this.update(cx, |first, cx| {
                let (saved, result) = result;
                first.busy = false;
                first.progress.clear();
                match result {
                    Ok(result) if result.cancelled(&saved.request.request_id) => {
                        first.directory = Some(saved.request.agent.directory.clone());
                        first.name.update(cx, |field, cx| {
                            field.set_value(&saved.request.agent.name, cx);
                            field.set_read_only(false);
                        });
                        first.custom_brief = matches!(saved.request.agent.brief, setup::Brief::Text { .. });
                        first.brief.update(cx, |field, cx| {
                            if let setup::Brief::Text { text } = &saved.request.agent.brief {
                                field.set_value(text, cx);
                            }
                            field.set_read_only(false);
                        });
                        first.saved = None;
                        first.result = None;
                        first.tools = ToolChoice::default();
                        cx.emit(FirstEvent::RefreshTools);
                    }
                    Ok(result) => {
                        if let Ok(Some(current)) = setup::Saved::load(&first.save_path) {
                            first.saved = Some(current);
                        }
                        first.error = Some(format!("Could not change tools: {} Your saved setup is retained.", result.message));
                    }
                    Err(_) => {
                        first.error = Some("The tool change is not confirmed. Retry changing tools; the previous setup will not be started. If your Backbone version does not support changing tools, update it first.".into());
                    }
                }
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    fn is_ready(&self) -> bool {
        self.result
            .as_ref()
            .is_some_and(|result| result.status == "completed" && result.code == "agent_ready")
    }

    fn is_waiting(&self) -> bool {
        self.result.as_ref().is_some_and(|result| {
            waiting(
                result,
                self.saved
                    .as_ref()
                    .map(|saved| saved.request.agent.name.as_str()),
            )
        })
    }

    /// The main button for this step: start, retry or change tools while
    /// setup needs that, and continue once the agent runs.
    pub fn primary(&self) -> Primary {
        let cancelling = self.saved.as_ref().is_some_and(|saved| saved.cancelling);
        if self.busy {
            return Primary::Waiting(
                if cancelling {
                    "Changing tool…"
                } else {
                    "Setting up…"
                }
                .into(),
            );
        }
        if self.is_ready() || self.is_waiting() {
            return Primary::Continue;
        }
        match (&self.saved, &self.result) {
            (Some(_), _) if cancelling => Primary::ChangeTool("Retry changing tool"),
            (Some(_), Some(result)) if result.retry != "same_request" => {
                Primary::ChangeTool("Change agent tool")
            }
            (Some(_), Some(_)) => Primary::Start("Try again".into()),
            (Some(_), None) => Primary::Start("Resume setup".into()),
            (None, _) if self.discovering => Primary::Waiting("Checking agent tools…".into()),
            (None, _) => match self.tools.current() {
                Some(runtime) if runtime.can_setup() => {
                    Primary::Start(format!("Start with {}", runtime.display_name).into())
                }
                Some(_) => Primary::Waiting("Choose a ready tool".into()),
                None => Primary::Waiting("Choose an agent tool".into()),
            },
        }
    }

    /// The choices, all visible at once: the tool first, then the name,
    /// folder and instructions.
    fn form(&self, cx: &mut Context<Self>) -> gpui::Div {
        let (t, c) = (tokens(), colors());
        let mut tools = div().grid().grid_cols(2).gap(px(t.space.s8));
        for runtime in self.tools.rows.iter().filter(|row| row.id != "shell") {
            let id = runtime.id.clone();
            tools = tools.child(
                tool_card(runtime, self.tools.selected.as_ref() == Some(&id)).on_click(
                    cx.listener(move |first, _, _, cx| {
                        if !first.busy && first.saved.is_none() && !first.discovering {
                            first.tools.select(&id);
                            first.error = None;
                            first.progress.clear();
                            cx.notify();
                        }
                    }),
                ),
            );
        }
        let (note, attention) = match self.tools.current() {
            _ if self.discovering => ("Checking agent tools…".into(), false),
            None => ("Pick the tool your agent runs in.".into(), false),
            Some(runtime) if !runtime.available => (
                format!(
                    "Install {} from the Agent tools step, then check again, or pick another tool.",
                    runtime.display_name
                ),
                true,
            ),
            Some(runtime) if !runtime.setup_supported => (
                format!(
                    "{} does not support first-agent setup. Pick another tool, or use terminals for now.",
                    runtime.display_name
                ),
                true,
            ),
            Some(runtime) => match &runtime.setup_compatibility {
                Some(check) if !check.compatible => (
                    format!(
                        "{}: {}{}",
                        runtime.display_name,
                        check.message,
                        check
                            .path
                            .as_ref()
                            .map(|path| format!(
                                " Detected: {path}{}",
                                check
                                    .version
                                    .as_ref()
                                    .map(|version| format!(" · {version}"))
                                    .unwrap_or_default()
                            ))
                            .unwrap_or_default()
                    ),
                    true,
                ),
                _ => (
                    format!(
                        "Uses its default model. Sign in inside {} if you have not yet.",
                        runtime.display_name
                    ),
                    false,
                ),
            },
        };
        let folder = self
            .folder(cx)
            .map(|path| home_relative(&path))
            .unwrap_or_else(|| "Choose a folder".into());
        div()
            .flex()
            .flex_col()
            .gap(px(t.space.s8))
            .child(tools)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(t.space.s8))
                    .child(
                        ui::text(
                            div().flex_1().min_w_0().text_color(if attention {
                                c.attention_text
                            } else {
                                c.muted
                            }),
                            &t.text.status,
                        )
                        .child(note),
                    )
                    .child(
                        ui::icon_button("refresh-tools", "refresh-cw")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(FirstEvent::RefreshTools))),
                    ),
            )
            .child(field_row(
                "Name",
                div()
                    .flex_1()
                    .h(px(t.size.action))
                    .bg(c.field)
                    .child(self.name.clone()),
            ))
            .child(field_row(
                "Folder",
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(t.space.s8))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_color(c.code)
                            .child(folder),
                    )
                    .child(
                        ui::button("agent-folder", "Change", Button::Secondary)
                            .on_click(cx.listener(|first, _, _, cx| first.choose_folder(cx))),
                    ),
            ))
            .child(field_row(
                "Instructions",
                ui::segments()
                    .child(
                        ui::segment(
                            "brief-coordinating",
                            "Coordinating brief",
                            !self.custom_brief,
                        )
                        .on_click(cx.listener(|first, _, _, cx| first.use_custom_brief(false, cx))),
                    )
                    .child(
                        ui::segment("brief-own", "My own", self.custom_brief).on_click(
                            cx.listener(|first, _, _, cx| first.use_custom_brief(true, cx)),
                        ),
                    ),
            ))
            .when(self.custom_brief, |form| {
                form.child(
                    div()
                        .px(px(t.space.s8))
                        .py(px(t.space.s6))
                        .rounded(px(t.radius.row))
                        .bg(c.field)
                        .border(px(t.size.hairline))
                        .border_color(c.line_strong)
                        .child(self.brief.clone()),
                )
            })
    }

    fn use_custom_brief(&mut self, custom: bool, cx: &mut Context<Self>) {
        if !self.busy && self.saved.is_none() {
            self.custom_brief = custom;
            cx.notify();
        }
    }

    /// After Start: the choices in one line, each stage, the outcome and
    /// the one action that moves setup on.
    fn status(&self, saved: &setup::Saved, cx: &mut Context<Self>) -> gpui::Div {
        let (t, c) = (tokens(), colors());
        let agent = &saved.request.agent;
        let mut stages = div().flex().flex_col().gap(px(t.space.s2));
        // A result that stops short marks its first unfinished stage.
        let mut stopped = self.result.as_ref().is_some_and(|result| {
            result.status != "completed" && !self.steps.values().any(|status| status == "failed")
        }) && !self.busy;
        for (step, label) in STAGES {
            let mark = match self.steps.get(step).map(String::as_str) {
                Some("completed" | "skipped") => ui::Step::Done,
                Some("running") if self.busy => ui::Step::Now,
                Some("failed") => ui::Step::Waits,
                _ if stopped => {
                    stopped = false;
                    ui::Step::Waits
                }
                _ => ui::Step::Ahead,
            };
            stages = stages.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(t.space.s10))
                    .h(px(t.size.action - t.space.s2))
                    .text_color(if mark == ui::Step::Ahead {
                        c.muted
                    } else {
                        c.text
                    })
                    .child(ui::step_dot(mark))
                    .child(label),
            );
        }
        let launched = self.result.as_ref().is_some_and(|result| {
            result.data.get("agent_name").and_then(|name| name.as_str())
                == Some(agent.name.as_str())
                && result
                    .data
                    .get("launch_id")
                    .and_then(|id| id.as_str())
                    .is_some()
        });
        let waiting = self.is_waiting();
        let changeable = !self.busy
            && !self.is_ready()
            && !waiting
            && !matches!(self.primary(), Primary::ChangeTool(_));
        let tool = self
            .tools
            .rows
            .iter()
            .find(|row| row.id == agent.runtime)
            .map_or(agent.runtime.as_str(), |row| row.display_name.as_str());
        div()
            .flex()
            .flex_col()
            .gap(px(t.space.s10))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(t.space.s8))
                    .px(px(t.space.s12))
                    .py(px(t.space.s6))
                    .rounded(px(t.radius.slot))
                    .bg(c.panel)
                    .child(ui::icon("bot", t.size.icon, c.text_2))
                    .child(div().text_color(c.text).child(agent.name.clone()))
                    .child(div().text_color(c.muted).child(format!(
                        "{tool} · {}",
                        home_relative(&agent.directory)
                    ))),
            )
            .child(stages)
            .when(!self.progress.is_empty(), |panel| {
                panel.child(div().text_color(c.text).child(self.progress.clone()))
            })
            .when(
                !saved.cancelling && !self.busy && self.result.is_none() && self.progress.is_empty(),
                |panel| {
                    panel.child(div().text_color(c.muted).child(
                        "This setup was saved. Resume it, or change the agent tool; your name, folder and instructions are kept.",
                    ))
                },
            )
            .when(saved.cancelling && !self.busy, |panel| {
                panel.child(div().text_color(c.muted).child(
                    "A tool change is pending. Retry it to choose another tool; your name, folder and instructions are kept.",
                ))
            })
            .when(launched || changeable || waiting, |panel| {
                let name = agent.name.clone();
                panel.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(t.space.s8))
                        .when(launched, |row| {
                            row.child(
                                ui::button("open-agent", "Open agent terminal", Button::Secondary)
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.emit(FirstEvent::OpenTerminal(name.clone()))
                                    })),
                            )
                        })
                        .when(waiting, |row| {
                            row.child(
                                ui::button("check-agent", "Check again", Button::Secondary)
                                    .on_click(cx.listener(|first, _, _, cx| first.start(cx))),
                            )
                        })
                        .when(changeable, |row| {
                            row.child(
                                ui::button("edit-agent", "Change agent tool", Button::Secondary)
                                    .on_click(cx.listener(|first, _, _, cx| first.edit_choices(cx))),
                            )
                        }),
                )
            })
    }
}

/// Whether the agent launched and now waits for the person in its terminal,
/// such as for a permission prompt.
fn waiting(result: &setup::Event, expected: Option<&str>) -> bool {
    result.status == "action_required"
        && result.data.get("agent_name").and_then(|name| name.as_str()) == expected
        && result.data.get("ready").and_then(|ready| ready.as_str()) == Some("waiting_for_human")
        && result
            .data
            .get("launch_id")
            .and_then(|id| id.as_str())
            .is_some()
}

/// A path with the home directory shortened to `~`.
fn home_relative(path: &std::path::Path) -> String {
    std::env::var_os("HOME")
        .and_then(|home| path.strip_prefix(home).ok())
        .map(|rest| format!("~/{}", rest.display()))
        .unwrap_or_else(|| path.display().to_string())
}

/// A tool to pick, on one line: a radio mark, its name and whether it can
/// run setup.
fn tool_card(runtime: &setup::Runtime, selected: bool) -> gpui::Stateful<gpui::Div> {
    let (t, c) = (tokens(), colors());
    let ready = runtime.can_setup();
    let state = if !runtime.setup_supported {
        "Setup unavailable"
    } else if !runtime.available {
        "Not installed"
    } else if !ready {
        "Needs attention"
    } else {
        "Ready"
    };
    div()
        .id(SharedString::from(format!("runtime-{}", runtime.id)))
        .relative()
        .min_w_0()
        .flex()
        .items_center()
        .gap(px(t.space.s8))
        .h(px(t.size.choice_row))
        .px(px(t.space.s10))
        .rounded(px(t.radius.slot))
        .border(px(t.size.hairline))
        .cursor_pointer()
        .when(selected, |card| {
            card.bg(c.accent_soft)
                .border_color(c.focus)
                .child(ui::selection_ring(t.radius.slot, t.size.hairline))
        })
        .when(!selected, |card| {
            card.bg(c.field)
                .border_color(c.line_strong)
                .hover(|card| card.bg(colors().hover))
        })
        .child(ui::icon(
            if selected { "circle-check" } else { "circle" },
            t.size.icon + 2.0,
            if selected { c.accent_text } else { c.muted },
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_color(c.text)
                .child(runtime.display_name.clone()),
        )
        .child(
            ui::text(
                div()
                    .flex_shrink_0()
                    .text_color(if ready { c.muted } else { c.attention_text }),
                &t.text.meta,
            )
            .child(state),
        )
}

/// A labelled row of the form: the label column, then its control.
fn field_row(label: &'static str, control: impl IntoElement) -> gpui::Div {
    let (t, c) = (tokens(), colors());
    div()
        .flex()
        .items_center()
        .gap(px(t.space.s12))
        .min_h(px(t.size.action + t.space.s4))
        .child(
            div()
                .w(px(t.size.fact_label - t.space.s14))
                .flex_shrink_0()
                .text_color(c.text_2)
                .child(label),
        )
        .child(control)
}

impl Render for FirstAgent {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (t, c) = (tokens(), colors());
        let page = heading(
            "Your first agent",
            "It learns your goals and proposes project agents for you to approve.",
        )
        .gap(px(t.space.s8))
        .pb(px(t.space.s16))
        .when(self.existing && self.saved.is_none(), |page| {
            page.child(
                div().child(
                    ui::button("use-existing", "Use my existing agents", Button::Secondary)
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(FirstEvent::Existing))),
                ),
            )
        });
        let page = match self.saved.clone() {
            Some(saved) => page.child(self.status(&saved, cx)),
            None => page.child(self.form(cx)),
        };
        page.when_some(self.error.clone(), |page, error| {
            page.child(div().text_color(c.attention_text).child(error))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> Vec<setup::Runtime> {
        serde_json::from_value(serde_json::json!([
            {"id":"codex","display_name":"Codex","available":true,
             "setup_supported":true,"models":[],"authentication":"unknown"},
            {"id":"opencode","display_name":"OpenCode","available":true,
             "setup_supported":true,"models":[],"authentication":"unknown",
             "setup_compatibility":{"compatible":false,"message":"Update OpenCode.",
                 "path":"/fixture/opencode","version":"1.19.5"}},
            {"id":"shell","display_name":"Plain shell","available":true,
             "setup_supported":false,"models":[],"authentication":"unknown",
             "setup_compatibility":{"compatible":false,"message":"Setup unavailable."}}
        ]))
        .unwrap()
    }

    #[test]
    fn discovery_does_not_choose_a_tool_for_the_user() {
        let mut tools = ToolChoice::default();
        tools.replace(catalog());
        assert!(tools.current().is_none());
        assert!(tools.start_runtime().is_none());
    }

    #[test]
    fn switching_from_incompatible_opencode_to_codex_changes_the_entire_choice() {
        let mut tools = ToolChoice::default();
        tools.replace(catalog());
        tools.select("opencode");
        assert_eq!(
            tools
                .current()
                .unwrap()
                .setup_compatibility
                .as_ref()
                .unwrap()
                .message,
            "Update OpenCode."
        );
        assert!(tools.start_runtime().is_none());
        tools.select("codex");
        assert_eq!(tools.current().unwrap().display_name, "Codex");
        assert!(tools.current().unwrap().setup_compatibility.is_none());
        assert_eq!(tools.start_runtime(), Some("codex"));
        tools.select("opencode");
        assert!(tools.start_runtime().is_none());
    }

    #[test]
    fn refresh_keeps_explicit_choice_even_when_it_needs_attention() {
        let mut tools = ToolChoice::default();
        tools.replace(catalog());
        tools.select("codex");
        tools.rows.clear(); // The asynchronous refresh must not allow a stale start.
        assert!(tools.start_runtime().is_none());
        let mut rows = catalog();
        rows.reverse();
        tools.replace(rows);
        assert_eq!(tools.start_runtime(), Some("codex"));
        let mut rows = catalog();
        rows[0].available = false;
        tools.replace(rows);
        assert_eq!(tools.current().unwrap().id, "codex");
        assert!(tools.start_runtime().is_none());
    }

    #[test]
    fn an_agent_waiting_in_its_terminal_lets_setup_continue() {
        let fixture = include_str!("fixtures/first-agent-action-required.jsonl");
        let result: setup::Event = serde_json::from_str(fixture.lines().last().unwrap()).unwrap();
        assert!(waiting(&result, Some("guide")));
        assert!(!waiting(&result, Some("another-agent")));
        let mut stopped = result.clone();
        stopped.data["ready"] = "exited".into();
        assert!(!waiting(&stopped, Some("guide")));
    }

    #[test]
    fn removed_tools_do_not_silently_select_a_replacement() {
        let mut tools = ToolChoice::default();
        tools.replace(catalog());
        tools.select("codex");
        let mut rows = catalog();
        rows.retain(|row| row.id != "codex");
        tools.replace(rows);
        assert!(tools.current().is_none());
        tools.select("shell");
        assert!(tools.current().is_none());
        tools.select("missing");
        assert!(tools.current().is_none());
    }
}
