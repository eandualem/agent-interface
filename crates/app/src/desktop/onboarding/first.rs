//! The explicit first-agent choice. A saved request freezes the choices until
//! its result is known; retry always uses its original engine and identity.

use std::path::PathBuf;

use futures::{StreamExt, channel::mpsc};
use gpui::{
    Context, Entity, EventEmitter, PathPromptOptions, Subscription, Window, div, prelude::*, px,
};

use super::{heading, setup};
use crate::desktop::{
    components::{self as ui, Button},
    design::{colors, tokens},
    name::NameField,
};

pub enum FirstEvent {
    Pending,
    Ready(String),
    Existing,
    OpenTerminal(String),
}

enum Update {
    Progress(setup::Event),
    Finished(Result<setup::Event, &'static str>),
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
    runtimes: Vec<setup::Runtime>,
    selected: Option<String>,
    discovering: bool,
    discovery_generation: u64,
    busy: bool,
    saved: Option<setup::Saved>,
    result: Option<setup::Event>,
    error: Option<String>,
    progress: String,
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
            runtimes: Vec::new(),
            selected: None,
            discovering: false,
            discovery_generation: 0,
            busy: false,
            saved,
            result: None,
            error,
            progress: String::new(),
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
        if self.has_pending_request() {
            return;
        }
        self.executable = setup::active_engine(&self.data_dir)
            .or_else(|| probe.path("backbone").cloned())
            .unwrap_or_default();
        self.path_env = probe.path_env.clone();
        self.existing =
            matches!(probe.backbone, super::probe::Backbone::Running { agents } if agents > 0);
        self.runtimes.clear();
        self.selected = None;
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
                        first.selected = rows
                            .iter()
                            .find(|row| row.can_setup())
                            .map(|row| row.id.clone());
                        first.runtimes = rows;
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
            .selected
            .as_ref()
            .filter(|id| {
                self.runtimes
                    .iter()
                    .any(|row| row.id == **id && row.can_setup())
            })
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
            path_env: self.path_env.clone(),
            executable: self.executable.clone(),
            request: setup::Request {
                schema_version: 1,
                request_id: uuid::Uuid::new_v4().to_string(),
                action: "first_agent".into(),
                data_dir: self.data_dir.clone(),
                hooks: vec![runtime.clone()],
                agent: setup::Agent {
                    name: name.into(),
                    directory,
                    create_directory: true,
                    runtime: runtime.clone(),
                    model: None,
                    brief,
                },
                start_greeting: true,
            },
        })
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        if self.busy {
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
        self.progress = "Starting setup…".into();
        let (sender, mut receiver) = mpsc::unbounded();
        cx.background_executor()
            .spawn(async move {
                let result = setup::run(
                    &saved.executable,
                    &saved.request,
                    &saved.path_env,
                    |event| {
                        let _ = sender.unbounded_send(Update::Progress(event));
                    },
                );
                let _ = sender.unbounded_send(Update::Finished(result));
            })
            .detach();
        cx.spawn(async move |this, cx| {
            while let Some(update) = receiver.next().await {
                if this.update(cx, |first, cx| {
                    match update {
                        Update::Progress(event) if event.kind == "step" => {
                            let label = match event.step.as_str() {
                                "preflight" => "Checking your choices", "hooks" => "Preparing the selected tool",
                                "agent" => "Creating your agent", "launch" => "Opening your agent",
                                "greeting" => "Submitting the introduction", _ => "Preparing setup",
                            };
                            first.progress = format!("{label} · {}", event.status);
                        }
                        Update::Progress(_) => {},
                        Update::Finished(result) => {
                            first.busy = false;
                            match result {
                                Ok(result) => {
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

    fn edit_choices(&mut self, cx: &mut Context<Self>) {
        if self.busy || !self.can_edit_again() {
            return;
        }
        let result = self
            .saved
            .as_ref()
            .ok_or("No saved setup request.")
            .and_then(|saved| saved.archive(&self.save_path));
        if let Err(error) = result {
            self.error = Some(error.into());
        } else {
            self.saved = None;
            self.result = None;
            self.error = None;
            self.progress.clear();
            self.name.update(cx, |field, _| field.set_read_only(false));
            self.brief.update(cx, |field, _| field.set_read_only(false));
            self.discover(cx);
        }
        cx.notify();
    }

    fn can_edit_again(&self) -> bool {
        self.result.as_ref().is_some_and(|result| {
            result.status == "failed"
                && result.retry == "new_request"
                && result
                    .completed_steps
                    .iter()
                    .all(|step| step == "preflight")
                && result.data.get("launch_id").is_none_or(Value::is_null)
        })
    }
}

use serde_json::Value;

impl Render for FirstAgent {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (t, c) = (tokens(), colors());
        let mut page = heading(
            "Your first agent",
            "Choose an agent tool and a coordinating agent to help you get started.",
        )
        .gap(px(t.space.s8))
        .pb(px(t.space.s16));
        if self.existing && self.saved.is_none() {
            page = page.child(
                ui::button("use-existing", "Use my existing agents", Button::Secondary)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(FirstEvent::Existing))),
            );
        }
        if let Some(saved) = &self.saved {
            page = page.child(div().py(px(t.space.s12)).child(format!(
                "{} · {} · {}",
                saved.request.agent.name,
                saved.request.agent.runtime,
                saved.request.agent.directory.display()
            )));
            page = page.child(div().text_color(c.muted).child("These choices are saved. Resume uses the same request and does not create a second agent."));
        } else {
            page = page.child(div().pt(px(t.space.s12)).child("Name")).child(
                div()
                    .h(px(t.size.action))
                    .bg(c.field)
                    .child(self.name.clone()),
            );
            page = page.child(
                div()
                    .pt(px(t.space.s12))
                    .child("Agent tool · uses its default model"),
            );
            let mut tools = div().flex().flex_wrap().gap(px(t.space.s8));
            for runtime in self.runtimes.iter().filter(|row| row.id != "shell") {
                let id = runtime.id.clone();
                let enabled = runtime.can_setup();
                let label = format!(
                    "{}{}",
                    runtime.display_name,
                    if !runtime.setup_supported {
                        " · setup unavailable"
                    } else if !runtime.available {
                        " · not installed"
                    } else {
                        ""
                    }
                );
                tools = tools.child(
                    ui::button(
                        format!("runtime-{id}"),
                        label,
                        if self.selected.as_ref() == Some(&id) {
                            Button::Primary
                        } else {
                            Button::Secondary
                        },
                    )
                    .on_click(cx.listener(move |first, _, _, cx| {
                        if enabled && !first.busy && first.saved.is_none() {
                            first.selected = Some(id.clone());
                            cx.notify();
                        }
                    })),
                );
            }
            page = page.child(tools);
            page = page.child(div().text_color(c.muted).child(if self.discovering {
                "Checking agent tools…"
            } else {
                "Sign in inside your selected tool. Finding it here does not verify login."
            }));
            page = page.child(
                ui::button("refresh-tools", "Check tools again", Button::Secondary)
                    .on_click(cx.listener(|first, _, _, cx| first.discover(cx))),
            );
            let folder = self
                .folder(cx)
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "Choose a folder".into());
            page = page
                .child(
                    div()
                        .pt(px(t.space.s12))
                        .child("Folder · created if missing"),
                )
                .child(div().text_color(c.code).child(folder))
                .child(
                    ui::button("agent-folder", "Choose folder", Button::Secondary)
                        .on_click(cx.listener(|first, _, _, cx| first.choose_folder(cx))),
                );
            page = page.child(div().pt(px(t.space.s12)).child("The coordinating brief learns your goals and proposes project agents for your approval. Starter skills and policies are included."))
                .child(ui::button("agent-brief", if self.custom_brief { "Use coordinating brief" } else { "Write my own instructions" }, Button::Secondary).on_click(cx.listener(|first, _, _, cx| { if !first.busy && first.saved.is_none() { first.custom_brief = !first.custom_brief; cx.notify(); } })));
            if self.custom_brief {
                page = page.child(div().h(px(120.0)).bg(c.field).child(self.brief.clone()));
            }
            page = page.child(div().pt(px(t.space.s12)).text_color(c.muted).child("Start creates the agent and prepares the selected tool's integration, then submits an introduction. It uses your signed-in account; normal permissions still apply."));
        }
        if !self.progress.is_empty() {
            page = page.child(div().py(px(t.space.s12)).child(self.progress.clone()));
        }
        if let Some(error) = &self.error {
            page = page.child(div().text_color(c.attention_text).child(error.clone()));
        }
        if let Some(result) = &self.result
            && result.data.get("agent_name").and_then(|name| name.as_str())
                == self
                    .saved
                    .as_ref()
                    .map(|saved| saved.request.agent.name.as_str())
            && result
                .data
                .get("launch_id")
                .and_then(|id| id.as_str())
                .is_some()
            && let Some(saved) = &self.saved
        {
            let name = saved.request.agent.name.clone();
            page = page.child(
                ui::button("open-agent", "Open agent terminal", Button::Secondary).on_click(
                    cx.listener(move |_, _, _, cx| cx.emit(FirstEvent::OpenTerminal(name.clone()))),
                ),
            );
        }
        if self.can_edit_again() {
            page = page.child(
                ui::button("edit-agent", "Edit choices", Button::Secondary)
                    .on_click(cx.listener(|first, _, _, cx| first.edit_choices(cx))),
            );
        }
        let retry = self
            .result
            .as_ref()
            .is_none_or(|result| result.retry == "same_request");
        if retry {
            page = page.child(
                ui::button(
                    "start-agent",
                    if self.busy {
                        "Setting up…"
                    } else if self.saved.is_some() {
                        "Resume saved setup"
                    } else {
                        "Start my agent"
                    },
                    Button::Primary,
                )
                .on_click(cx.listener(|first, _, _, cx| first.start(cx))),
            );
        }
        page
    }
}
