//! Native workspace layout and existing tmux session connections.

use std::{cell::RefCell, collections::HashMap, path::PathBuf, rc::Rc, sync::Arc};

use futures::{StreamExt, channel::mpsc};
use gpui::{
    AnyElement, App, Bounds, Context, KeyBinding, Menu, MenuItem, PathPromptOptions, SharedString,
    Task, TitlebarOptions, Window, WindowBounds, WindowOptions, actions, div, prelude::*, px, rgb,
    size,
};
use gpui_platform::application;
use tmux_workspaces_core::{
    Attachment, AttachmentTarget, Direction, Layout, NameEdit, Node, RenameOutcome, Tab,
};
use tmux_workspaces_store::{Document, Store};

use crate::library::{Library, Update, WindowLayout};
mod chooser;
mod name;
use chooser::{Choice, SessionChooser};
mod terminal;
use name::{NameEvent, NameField};
use terminal::{LiveTerminal, Target};
use tmux_workspaces_tmux::connection::SessionWindow;

actions!(
    library_browser,
    [
        /// Close the library browser without changing saved layouts or sessions.
        Quit,
        /// Open an independent native window on the current library.
        NewWindow
    ]
);

/// Open the native workspace window and connect its saved, existing sessions.
/// An absent directory uses the shared library's normal path lookup.
pub fn run(directory: Option<PathBuf>) -> Result<(), String> {
    let source = crate::sessions::source_socket(None).map_err(|e| e.to_string());
    let failure = Rc::new(RefCell::new(None));
    let opened = Rc::clone(&failure);
    application().run(move |cx: &mut App| {
        if cfg!(target_os = "linux") && cx.graphical_environment().is_none() {
            *opened.borrow_mut() =
                Some("the workspace browser requires a graphical desktop".into());
            // Linux's event loop resets its stop flag when run() begins, after
            // this launch callback. Request exit on its first dispatched task.
            cx.spawn(async |cx| cx.update(|cx| cx.quit())).detach();
            return;
        }
        cx.bind_keys([
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("cmd-n", NewWindow, None),
        ]);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.on_action(|_: &NewWindow, cx| {
            if let Some(window) = cx
                .active_window()
                .and_then(|window| window.downcast::<Browser>())
            {
                // Dispatch currently borrows the originating window. Open after it
                // returns, retaining that window's identity even if focus changes.
                cx.defer(move |cx| {
                    let _ = window.update(cx, |browser, _, cx| browser.open_another(cx));
                });
            }
        });
        cx.set_menus(vec![Menu {
            name: "tmux-workspaces".into(),
            items: vec![
                MenuItem::action("New Window", NewWindow),
                MenuItem::action("Quit tmux-workspaces", Quit),
            ],
            disabled: false,
        }]);
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        if let Err(error) = open_browser(directory, source, cx) {
            *opened.borrow_mut() = Some(format!("cannot open window: {error}"));
            cx.spawn(async |cx| cx.update(|cx| cx.quit())).detach();
            return;
        }
        cx.activate(true);
    });
    failure.borrow_mut().take().map_or(Ok(()), Err)
}

fn open_browser(
    directory: Option<PathBuf>,
    source: Result<PathBuf, String>,
    cx: &mut App,
) -> Result<(), String> {
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(1040.0), px(700.0)),
            cx,
        ))),
        window_min_size: Some(size(px(640.0), px(420.0))),
        titlebar: Some(TitlebarOptions {
            title: Some("tmux-workspaces".into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    cx.open_window(options, |_, cx| {
        cx.new(|cx| {
            let mut browser = Browser {
                source: Some(source),
                navigation: Some(cx.focus_handle()),
                ..Browser::default()
            };
            browser.open(directory, cx);
            browser
        })
    })
    .map(|_| ())
    .map_err(|error| error.to_string())
}

struct EditingName {
    target: NameEdit,
    tab: bool,
    field: gpui::Entity<NameField>,
    _events: gpui::Subscription,
}

#[derive(Clone)]
enum TabAction {
    Rename,
    Attach,
    Move(isize),
    Transfer(String),
}

#[derive(Clone)]
struct TabMenu {
    workspace: String,
    tab: Tab,
}

impl TabMenu {
    fn current(&self, layout: &tmux_workspaces_core::Layout) -> bool {
        layout.selected == self.workspace && layout.tab() == Some(&self.tab)
    }
}

struct AttachmentEdit {
    tab: String,
    pane: String,
    empty: bool,
    target: AttachmentTarget,
}

impl AttachmentEdit {
    fn capture(model: &Layout) -> Option<Self> {
        Some(Self {
            tab: model.tab()?.id.clone(),
            pane: model.pane()?.id.clone(),
            empty: model.pane()?.empty,
            target: model.attachment_target()?,
        })
    }

    fn apply(&self, model: &mut Layout, attachment: Attachment) -> bool {
        if self.empty {
            let displayed = model
                .tab()
                .is_some_and(|tab| !model.focus || tab.focus == self.pane);
            displayed && model.fill_pane(&self.tab, &self.pane, Some(attachment))
        } else {
            self.target.apply(model, Some(attachment))
        }
    }

    fn displayed(&self, model: &Layout) -> bool {
        model.tab().is_some_and(|tab| {
            tab.id == self.tab
                && (!model.focus || tab.focus == self.pane)
                && tab.tree.leaves().iter().any(|leaf| leaf.id == self.pane)
        })
    }

    fn propose(
        &self,
        layout: &mut WindowLayout,
        attachment: Attachment,
    ) -> Option<(Document, Layout)> {
        let (base, mut proposed) = layout.save_snapshot();
        if !self.apply(&mut proposed, attachment) {
            return None;
        }
        // Confirmation returns selection to the captured destination, but keeps
        // its original terminal until the detached proposal has committed.
        // The caller explicitly returns keyboard focus to navigation on confirmation.
        let _ = layout.model_mut().retain_navigation(&proposed);
        Some((base, proposed))
    }
}

struct Attaching {
    destination: AttachmentEdit,
    source: String,
    view: gpui::Entity<SessionChooser>,
    _events: gpui::Subscription,
}

#[derive(Default)]
struct Browser {
    library: Option<Library>,
    directory: Option<PathBuf>,
    source: Option<Result<PathBuf, String>>,
    terminals: HashMap<String, gpui::Entity<LiveTerminal>>,
    window_choices: HashMap<String, (Target, SessionWindow)>,
    layout: Option<WindowLayout>,
    error: Option<String>,
    loading: bool,
    workspace_menu: bool,
    reset_focus: bool,
    navigation: Option<gpui::FocusHandle>,
    updates: Option<Task<()>>,
    editing: Option<EditingName>,
    saving: bool,
    save_failed: bool,
    save_notice: Option<String>,
    name_click: Option<(String, bool)>,
    tab_menu: Option<TabMenu>,
    chooser: Option<Attaching>,
    // A failed proposed edit remains detached until its transaction succeeds.
    pending_save: Option<(Document, Layout)>,
}

impl Browser {
    fn open_another(&mut self, cx: &mut Context<Self>) {
        let source = self
            .source
            .clone()
            .unwrap_or_else(|| Err("Source socket unavailable".into()));
        if let Err(error) = open_browser(self.directory.clone(), source, cx) {
            self.error = Some(format!("Cannot open window: {error}"));
            cx.notify();
        }
    }

    fn stop_reader(&mut self, cx: &mut Context<Self>) {
        self.updates = None;
        if let Some(previous) = self.library.take() {
            cx.background_executor()
                .spawn(async move { drop(previous) })
                .detach();
        }
    }

    fn open(&mut self, directory: Option<PathBuf>, cx: &mut Context<Self>) {
        if self.saving || self.save_failed {
            return;
        }
        self.editing = None;
        self.chooser = None;
        self.name_click = None;
        self.tab_menu = None;
        self.save_notice = None;
        self.stop_reader(cx);
        self.terminals.clear();
        self.window_choices.clear();
        self.reset_focus = true;
        self.directory = directory.clone();
        self.layout = None;
        self.error = None;
        self.loading = true;
        self.start_reader(cx);
    }

    fn start_reader(&mut self, cx: &mut Context<Self>) {
        let directory = self.directory.clone();
        let (wake, mut woken) = mpsc::unbounded();
        match Library::start(directory, move || {
            let _ = wake.unbounded_send(());
        }) {
            Ok(library) => self.library = Some(library),
            Err(error) => {
                self.error = Some(error.to_string());
                self.loading = false;
            }
        }
        self.updates = Some(cx.spawn(async move |this, cx| {
            while woken.next().await.is_some() {
                if this
                    .update(cx, |browser, cx| {
                        if let Some(update) =
                            browser.library.as_ref().and_then(Library::take_update)
                        {
                            browser.apply(update);
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
        cx.notify();
    }

    fn apply(&mut self, update: Update) {
        self.loading = false;
        self.error = update.error.map(|error| error.to_string());
        if let Some(document) = update.document {
            if let Some(layout) = &mut self.layout {
                match layout.refresh(document, "layouts.db".as_ref()) {
                    Ok(result) if result.conflict => {
                        self.reset_focus = true;
                        self.error =
                            Some("The library changed. Showing the latest saved layout.".into());
                    }
                    Ok(result) => {
                        self.reset_focus |= result.active_tree_changed;
                    }
                    Err(error) => self.error = Some(error.to_string()),
                }
            } else {
                self.layout = Some(WindowLayout::new(document));
            }
        }
    }

    fn begin_name(&mut self, tab: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving || self.save_failed || self.editing.is_some() {
            return;
        }
        if let Some(update) = self.library.as_ref().and_then(Library::take_update) {
            self.apply(update);
        }
        let target = self.layout.as_ref().and_then(|layout| {
            if tab {
                layout.model().edit_tab_name()
            } else {
                layout.model().edit_workspace_name()
            }
        });
        let Some(target) = target else {
            return;
        };
        let field = cx.new(|cx| NameField::new(target.name(), window, cx));
        let events = cx.subscribe_in(
            &field,
            window,
            |browser, field, event, window, cx| match event {
                NameEvent::Changed(_) => {}
                NameEvent::Save => browser.submit_name(window, cx),
                NameEvent::Cancel => {
                    if field.read(cx).focus.is_focused(window)
                        && let Some(focus) = &browser.navigation
                    {
                        window.focus(focus, cx);
                    }
                    browser.editing = None;
                    cx.notify();
                }
            },
        );
        self.workspace_menu = false;
        self.tab_menu = None;
        self.chooser = None;
        self.reset_focus = false;
        self.save_notice = None;
        self.editing = Some(EditingName {
            target,
            tab,
            field,
            _events: events,
        });
        cx.notify();
    }

    fn submit_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(update) = self.library.as_ref().and_then(Library::take_update) {
            self.apply(update);
        }
        let (Some(edit), Some(layout)) = (&self.editing, &mut self.layout) else {
            return;
        };
        match edit
            .target
            .apply(layout.model_mut(), edit.field.read(cx).value())
        {
            RenameOutcome::Empty => return,
            RenameOutcome::Changed => {
                self.editing = None;
                self.save_notice =
                    Some("The name or selection changed. Rename it again to retry.".into());
            }
            RenameOutcome::Applied => {
                self.editing = None;
                self.save(cx);
            }
        }
        if let Some(focus) = &self.navigation {
            window.focus(focus, cx);
        }
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let Some(layout) = &self.layout else {
            return;
        };
        let (mut base, local) = self
            .pending_save
            .take()
            .unwrap_or_else(|| layout.save_snapshot());
        let directory = self.directory.clone();
        // Discard all pending reads before saving. A fresh reader starts only
        // after applying the result, so no older snapshot can undo the commit.
        self.stop_reader(cx);
        self.saving = true;
        self.save_failed = false;
        self.save_notice = None;
        self.error = None;
        // Closing this window does not cancel a save while the app is running.
        // SQLite and its busy timeout never run on the UI thread.
        let work = cx.background_executor().spawn(async move {
            let result = crate::library::library_directory(directory.as_deref())
                .and_then(|directory| Store::open_existing_writable(&directory))
                .and_then(|mut store| store.save(&mut base, &local));
            (base, local, result)
        });
        cx.spawn(async move |this, cx| {
            let (base, local, result) = work.await;
            let _ = this.update(cx, |browser, cx| {
                browser.saving = false;
                match result {
                    Ok(result) => {
                        if let Some(layout) = &mut browser.layout {
                            browser.reset_focus |= layout.saved(base, &result);
                        }
                        browser.start_reader(cx);
                        if result.conflict {
                            browser.save_notice = Some("The library changed. Your edit was not saved; repeat it on the latest layout to retry.".into());
                        }
                    }
                    Err(error) => {
                        browser.save_failed = true;
                        browser.pending_save = Some((base, local));
                        browser.error = Some(format!("Changes not saved: {error}. Retry saving."));
                    }
                }
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    fn discard_save(&mut self, cx: &mut Context<Self>) {
        if !self.save_failed || self.saving {
            return;
        }
        self.pending_save = None;
        self.save_failed = false;
        self.save_notice = None;
        self.error = None;
        if let Some(layout) = &mut self.layout {
            self.reset_focus |= layout.discard_edits();
        }
        self.start_reader(cx);
        cx.notify();
    }

    fn open_tab_menu(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving || self.save_failed {
            return;
        }
        if let Some(update) = self.library.as_ref().and_then(Library::take_update) {
            self.apply(update);
        }
        let Some(layout) = &mut self.layout else {
            return;
        };
        let model = layout.model_mut();
        if !model.choose_tab(id) {
            return;
        }
        self.tab_menu = model.tab().cloned().map(|tab| TabMenu {
            workspace: model.selected.clone(),
            tab,
        });
        self.editing = None;
        self.chooser = None;
        self.workspace_menu = false;
        if let Some(focus) = &self.navigation {
            window.focus(focus, cx);
        }
        cx.notify();
    }

    fn tab_action(&mut self, action: TabAction, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.tab_menu.take() else {
            return;
        };
        if self.saving || self.save_failed {
            return;
        }
        if let Some(update) = self.library.as_ref().and_then(Library::take_update) {
            self.apply(update);
        }
        let Some(layout) = &mut self.layout else {
            return;
        };
        if !menu.current(layout.model()) {
            self.save_notice = Some("The tab changed. Open its options again to retry.".into());
            cx.notify();
            return;
        }
        match action {
            TabAction::Rename => self.begin_name(true, window, cx),
            TabAction::Attach => {
                if let Some(pane) = layout.model().pane().map(|pane| pane.id.clone()) {
                    self.open_attach(&pane, window, cx);
                }
            }
            TabAction::Move(offset) => {
                layout.model_mut().move_tab(offset);
                self.save(cx);
            }
            TabAction::Transfer(destination) => {
                if layout.model_mut().transfer_tab(&destination) {
                    self.save(cx);
                }
            }
        }
        cx.notify();
    }

    fn tab_options(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut options = div()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .on_mouse_down_out(cx.listener(|browser, _, _, cx| {
                browser.tab_menu = None;
                cx.notify();
            }))
            .child(div().text_xs().child("TAB OPTIONS"));
        let mut actions = vec![
            ("Rename tab".to_owned(), TabAction::Rename),
            ("Attach session".to_owned(), TabAction::Attach),
        ];
        if let (Some(menu), Some(layout)) = (&self.tab_menu, &self.layout) {
            if let Some(space) = layout.model().workspace()
                && let Some(index) = space.tabs.iter().position(|tab| tab.id == menu.tab.id)
            {
                if index > 0 {
                    actions.push(("Move up".into(), TabAction::Move(-1)));
                }
                if index + 1 < space.tabs.len() {
                    actions.push(("Move down".into(), TabAction::Move(1)));
                }
            }
            for space in &layout.model().workspaces {
                if space.id != menu.workspace {
                    actions.push((
                        format!("Move to {}", space.name),
                        TabAction::Transfer(space.id.clone()),
                    ));
                }
            }
        }
        for (index, (label, action)) in actions.into_iter().enumerate() {
            options = options.child(
                div()
                    .id(("tab-action", index))
                    .px_2()
                    .py_1()
                    .cursor_pointer()
                    .hover(|el| el.bg(rgb(0x25334a)))
                    .child(label)
                    .on_click(cx.listener(move |browser, _, window, cx| {
                        browser.tab_action(action.clone(), window, cx)
                    })),
            );
        }
        options.into_any_element()
    }

    fn open_attach(&mut self, pane: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving || self.save_failed {
            return;
        }
        if let Some(update) = self.library.as_ref().and_then(Library::take_update) {
            self.apply(update);
        }
        let Some(layout) = &mut self.layout else {
            return;
        };
        if !layout
            .model()
            .tab()
            .is_some_and(|tab| !layout.model().focus || tab.focus == pane)
            || !layout.model_mut().choose_pane(pane)
        {
            return;
        }
        let Some(destination) = AttachmentEdit::capture(layout.model()) else {
            return;
        };
        let source = match &self.source {
            Some(Ok(path)) => path.clone(),
            _ => {
                self.error = Some("Source socket unavailable".into());
                cx.notify();
                return;
            }
        };
        let Some(saved_source) = source.to_str().map(str::to_owned) else {
            self.error = Some("The source socket path cannot be saved as text.".into());
            cx.notify();
            return;
        };
        self.editing = None;
        self.tab_menu = None;
        self.chooser = None;
        self.reset_focus = false;
        let view = cx.new(|cx| SessionChooser::new(source, window, cx));
        let events = cx.subscribe_in(&view, window, |browser, chooser, event, window, cx| {
            if !browser
                .chooser
                .as_ref()
                .is_some_and(|active| active.view == *chooser)
            {
                return;
            }
            match event {
                Choice::Cancel => {
                    if chooser.read(cx).focused(window, cx)
                        && let Some(focus) = &browser.navigation
                    {
                        window.focus(focus, cx);
                    }
                    browser.chooser = None;
                }
                Choice::Session(name) => browser.attach_session(name, window, cx),
            }
            cx.notify();
        });
        self.chooser = Some(Attaching {
            destination,
            source: saved_source,
            view,
            _events: events,
        });
        cx.notify();
    }

    fn attach_session(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(chooser) = self.chooser.take() else {
            return;
        };
        if self.saving || self.save_failed {
            return;
        }
        if let Some(update) = self.library.as_ref().and_then(Library::take_update) {
            self.apply(update);
        }
        let Some(layout) = &mut self.layout else {
            return;
        };
        let attachment = Attachment {
            session: name.into(),
            source_socket: Some(chooser.source),
        };
        if let Some(proposal) = chooser.destination.propose(layout, attachment) {
            // The displayed model retains its original terminal until saving
            // completes. Failed or conflicting edits never start a replacement.
            self.pending_save = Some(proposal);
            self.save(cx);
        } else {
            self.save_notice =
                Some("The destination changed. Open Attach session again to retry.".into());
        }
        if let Some(focus) = &self.navigation {
            window.focus(focus, cx);
        }
        cx.notify();
    }

    fn name_editor(&self, cx: &mut Context<Self>) -> AnyElement {
        let field = self
            .editing
            .as_ref()
            .expect("active name editor")
            .field
            .clone();
        div()
            .flex()
            .flex_col()
            .min_w_0()
            .w_full()
            .on_mouse_down_out(cx.listener(|browser, _, _, cx| {
                browser.editing = None;
                cx.notify();
            }))
            .child(field)
            .child(
                div()
                    .id("save-name")
                    .cursor_pointer()
                    .text_xs()
                    .child("Save name")
                    .on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
                        window.prevent_default()
                    })
                    .on_click(cx.listener(|browser, _, window, cx| {
                        cx.stop_propagation();
                        browser.submit_name(window, cx);
                    })),
            )
            .into_any_element()
    }

    fn pick_library(&mut self, cx: &mut Context<Self>) {
        if self.saving || self.save_failed {
            return;
        }
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open library".into()),
        });
        cx.spawn(async move |this, cx| match paths.await {
            Ok(Ok(Some(paths))) => {
                if let Some(path) = paths.into_iter().next() {
                    let _ = this.update(cx, |browser, cx| browser.open(Some(path), cx));
                }
            }
            Ok(Err(error)) => {
                let _ = this.update(cx, |browser, cx| {
                    browser.error = Some(error.to_string());
                    cx.notify();
                });
            }
            _ => {}
        })
        .detach();
    }

    fn tree(
        node: &Node,
        focus: &str,
        terminals: &HashMap<String, gpui::Entity<LiveTerminal>>,
        chooser: Option<&Attaching>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match node {
            Node::Leaf(leaf) => {
                let id = leaf.id.clone();
                let title = leaf.attachment.as_ref().map_or_else(
                    || {
                        if leaf.empty {
                            "Empty pane".to_owned()
                        } else {
                            "Terminal".to_owned()
                        }
                    },
                    |attachment| attachment.session.clone(),
                );
                div()
                    .id(SharedString::from(format!("pane-{id}")))
                    .size_full()
                    .min_w_0()
                    .min_h_0()
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .gap_0()
                    .bg(rgb(0x171d28))
                    .border_1()
                    .border_color(rgb(if id == focus { 0x627a9c } else { 0x303b4d }))
                    .cursor_pointer()
                    .on_click(cx.listener(move |browser, _, _, cx| {
                        if let Some(layout) = &mut browser.layout {
                            layout.model_mut().choose_pane(&id);
                            cx.notify();
                        }
                    }))
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .text_xs()
                            .px_1()
                            .child(title)
                            .child(
                                div()
                                    .id(SharedString::from(format!("attach-{}", leaf.id)))
                                    .cursor_pointer()
                                    .child("Attach session")
                                    .on_click(cx.listener({
                                        let id = leaf.id.clone();
                                        move |browser, _, window, cx| {
                                            cx.stop_propagation();
                                            browser.open_attach(&id, window, cx);
                                        }
                                    })),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("zoom-{}", leaf.id)))
                                    .cursor_pointer()
                                    .child("⤢")
                                    .on_click(cx.listener({
                                        let id = leaf.id.clone();
                                        move |browser, _, _, cx| {
                                            if let Some(layout) = &mut browser.layout {
                                                let model = layout.model_mut();
                                                model.choose_pane(&id);
                                                model.focus = !model.focus;
                                                cx.notify();
                                            }
                                        }
                                    })),
                            ),
                    )
                    .child(div().flex_1().min_h_0().overflow_hidden().child(
                        if let Some(chooser) =
                            chooser.filter(|chooser| chooser.destination.pane == leaf.id)
                        {
                            chooser.view.clone().into_any_element()
                        } else if let Some(terminal) = terminals.get(&leaf.id) {
                            terminal.clone().into_any_element()
                        } else {
                            div()
                                .p_2()
                                .child("Choose Attach session to fill this empty pane.")
                                .into_any_element()
                        },
                    ))
                    .into_any_element()
            }
            Node::Split {
                direction,
                ratio,
                first,
                second,
                ..
            } => div()
                .size_full()
                .min_w_0()
                .min_h_0()
                .flex()
                .gap_0()
                .when(*direction == Direction::Below, |el| el.flex_col())
                .child(
                    div()
                        .flex_basis(px(0.0))
                        .flex_grow(*ratio as f32)
                        .min_w_0()
                        .min_h_0()
                        .overflow_hidden()
                        .child(Self::tree(first, focus, terminals, chooser, cx)),
                )
                .child(
                    div()
                        .flex_basis(px(0.0))
                        .flex_grow((1.0 - ratio) as f32)
                        .min_w_0()
                        .min_h_0()
                        .overflow_hidden()
                        .child(Self::tree(second, focus, terminals, chooser, cx)),
                )
                .into_any_element(),
        }
    }
}

impl Render for Browser {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.chooser.as_ref().is_some_and(|chooser| {
            !self
                .layout
                .as_ref()
                .is_some_and(|layout| chooser.destination.displayed(layout.model()))
        }) {
            self.chooser = None;
            self.reset_focus = true;
        }
        if std::mem::take(&mut self.reset_focus)
            && let Some(focus) = &self.navigation
        {
            window.focus(focus, cx);
        }
        // A new entity owns each connection generation. Results from a removed
        // leaf cannot install a view or accept keys in its replacement.
        let leaves = self
            .layout
            .as_ref()
            .and_then(|layout| layout.model().tab())
            .map(|tab| {
                tab.tree
                    .leaves()
                    .into_iter()
                    .filter(|leaf| !leaf.empty)
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        // Keep only the source-bound choice, not hidden clients or renderers.
        for (id, terminal) in &self.terminals {
            let terminal = terminal.read(cx);
            if let Some(selected) = terminal.selection() {
                self.window_choices
                    .insert(id.clone(), (terminal.target.clone(), selected));
            }
        }
        self.window_choices.retain(|_, (target, _)| {
            self.layout.as_ref().is_some_and(|layout| {
                layout.model().workspaces.iter().any(|workspace| {
                    workspace.tabs.iter().any(|tab| {
                        tab.tree
                            .leaves()
                            .into_iter()
                            .any(|leaf| *leaf == target.leaf)
                    })
                })
            })
        });
        self.terminals
            .retain(|id, _| leaves.iter().any(|leaf| &leaf.id == id));
        for leaf in leaves {
            let target = Target {
                leaf: leaf.clone(),
                directory: self.directory.clone(),
                source: self
                    .source
                    .clone()
                    .unwrap_or_else(|| Err("Source socket unavailable".into())),
            };
            if !self
                .terminals
                .get(&leaf.id)
                .is_some_and(|view| view.read(cx).target == target)
            {
                let selected = self
                    .window_choices
                    .get(&leaf.id)
                    .filter(|(previous, _)| *previous == target)
                    .map(|(_, selected)| selected.clone());
                self.terminals.insert(
                    leaf.id,
                    cx.new(|cx| LiveTerminal::new(target, selected, cx)),
                );
            }
        }
        let mut tabs = div().id("tabs").flex_1().min_h_0().overflow_y_scroll();
        let mut heading = "Workspaces".to_owned();
        let mut status = "Opening library…".to_owned();
        let mut content = div().flex_1().min_w_0().h_full().overflow_hidden();
        if let Some(layout) = &self.layout {
            let model = layout.model();
            if let Some(space) = model.workspace() {
                heading = space.name.clone();
                if self.tab_menu.is_some() {
                    tabs = tabs.child(self.tab_options(cx));
                } else if self.workspace_menu {
                    tabs = tabs.child(
                        div()
                            .id("rename-workspace")
                            .px_2()
                            .py_1()
                            .cursor_pointer()
                            .child("Rename workspace")
                            .on_click(cx.listener(|browser, _, window, cx| {
                                browser.begin_name(false, window, cx)
                            })),
                    );
                    tabs = tabs.child(div().px_2().py_1().text_xs().child("WORKSPACES"));
                    for space in &model.workspaces {
                        let id = space.id.clone();
                        tabs = tabs.child(
                            div()
                                .id(SharedString::from(format!("workspace-{id}")))
                                .px_2()
                                .py_1()
                                .text_ellipsis()
                                .cursor_pointer()
                                .when(model.selected == id, |el| el.bg(rgb(0x2a3c58)))
                                .hover(|el| el.bg(rgb(0x25334a)))
                                .child(space.name.clone())
                                .on_click(cx.listener(move |browser, _, window, cx| {
                                    if let Some(layout) = &mut browser.layout {
                                        layout.model_mut().choose_workspace(&id);
                                    }
                                    browser.workspace_menu = false;
                                    if let Some(focus) = &browser.navigation {
                                        window.focus(focus, cx);
                                    }
                                    cx.notify();
                                })),
                        );
                    }
                } else {
                    tabs = tabs.child(
                        div()
                            .px_2()
                            .py_1()
                            .text_xs()
                            .text_color(rgb(0x9aa9bf))
                            .child("TABS"),
                    );
                    for (index, tab) in space.tabs.iter().enumerate() {
                        let id = tab.id.clone();
                        tabs = tabs.child(
                            div()
                                .id(SharedString::from(format!("tab-{id}")))
                                .on_mouse_down(gpui::MouseButton::Right, cx.listener({
                                    let id = id.clone();
                                    move |browser, _, window, cx| browser.open_tab_menu(&id, window, cx)
                                }))
                                .px_2()
                                .py_1()
                                .text_ellipsis()
                                .cursor_pointer()
                                .when(space.selected == id, |el| el.bg(rgb(0x2a3c58)))
                                .hover(|el| el.bg(rgb(0x25334a)))
                                .flex().gap_2()
                                .child(format!("{}", index + 1))
                                .child(if self.editing.as_ref().is_some_and(|edit| edit.tab) && space.selected == id {
                                    self.name_editor(cx)
                                } else {
                                    let tab_id = id.clone();
                                    div().id(SharedString::from(format!("tab-name-{id}"))).flex_1().min_w_0().text_ellipsis()
                                        .child(tab.name.clone())
                                        .on_click(cx.listener(move |browser, event: &gpui::ClickEvent, window, cx| {
                                            cx.stop_propagation();
                                            let active = browser.layout.as_ref().is_some_and(|layout| layout.model().tab().is_some_and(|tab| tab.id == tab_id));
                                            if event.click_count() == 1 {
                                                browser.name_click = Some((tab_id.clone(), active));
                                            }
                                            let edit = event.click_count() == 2 && browser.name_click.as_ref().is_some_and(|(id, active)| id == &tab_id && *active);
                                            if let Some(layout) = &mut browser.layout { layout.model_mut().choose_tab(&tab_id); }
                                            if edit { browser.begin_name(true, window, cx); }
                                            else if let Some(focus) = &browser.navigation { window.focus(focus, cx); }
                                            cx.notify();
                                        })).into_any_element()
                                })
                                .child("▮".repeat(tab.tree.leaves().len()))
                                .when(space.selected == id, |el| {
                                    let id = id.clone();
                                    el.child(div().id("tab-options").px_1().child("⋯")
                                        .on_click(cx.listener(move |browser, _, window, cx| {
                                            cx.stop_propagation(); browser.open_tab_menu(&id, window, cx);
                                        })))
                                })
                                .on_click(cx.listener(move |browser, _, window, cx| {
                                    if let Some(layout) = &mut browser.layout {
                                        layout.model_mut().choose_tab(&id);
                                    }
                                    if let Some(focus) = &browser.navigation {
                                        window.focus(focus, cx);
                                    }
                                    cx.notify();
                                })),
                        );
                    }
                }
                status = format!("{} · no tabs", space.name);
            }
            if let Some(tab) = model.tab() {
                let leaves = tab.tree.leaves();
                let selected = leaves
                    .iter()
                    .position(|leaf| leaf.id == tab.focus)
                    .unwrap_or(0);
                status = format!(
                    "{} · {} · pane {}/{}",
                    heading,
                    tab.name,
                    selected + 1,
                    leaves.len()
                );
                let tree = if model.focus {
                    leaves.get(selected).map(|leaf| Node::Leaf((*leaf).clone()))
                } else {
                    None
                };
                content = content.child(Self::tree(
                    tree.as_ref().unwrap_or(&tab.tree),
                    &tab.focus,
                    &self.terminals,
                    self.chooser.as_ref(),
                    cx,
                ));
            }
        } else if !self.loading {
            content = content.child(
                div()
                    .p_4()
                    .child("Open a folder containing layouts.db to use your saved workspaces."),
            );
            status = "No library open".into();
        }
        if self.saving {
            status = "Saving changes…".into();
        }
        if let Some(notice) = &self.save_notice {
            status = notice.clone();
        }
        if let Some(error) = &self.error {
            status = error.clone();
        }
        let sidebar = div()
            .w(px(210.0))
            .flex_shrink_0()
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(0x161e2b))
            .border_r_1()
            .border_color(rgb(0x303b4d))
            .child(if self.editing.as_ref().is_some_and(|edit| !edit.tab) {
                self.name_editor(cx)
            } else {
                div()
                    .flex()
                    .bg(rgb(0x25334a))
                    .child(
                        div()
                            .id("workspace-name")
                            .flex_1()
                            .min_w_0()
                            .px_2()
                            .py_2()
                            .text_ellipsis()
                            .cursor_pointer()
                            .child(heading.clone())
                            .on_click(cx.listener(
                                |browser, event: &gpui::ClickEvent, window, cx| {
                                    if event.click_count() == 2 {
                                        browser.begin_name(false, window, cx);
                                    } else {
                                        browser.tab_menu = None;
                                        browser.workspace_menu = !browser.workspace_menu;
                                        if let Some(focus) = &browser.navigation {
                                            window.focus(focus, cx);
                                        }
                                    }
                                    cx.notify();
                                },
                            )),
                    )
                    .child(
                        div()
                            .id("workspace-menu")
                            .px_2()
                            .py_2()
                            .cursor_pointer()
                            .child("▾")
                            .on_click(cx.listener(|browser, _, window, cx| {
                                browser.tab_menu = None;
                                browser.workspace_menu = !browser.workspace_menu;
                                if let Some(focus) = &browser.navigation {
                                    window.focus(focus, cx);
                                }
                                cx.notify();
                            })),
                    )
                    .into_any_element()
            })
            .child(tabs)
            .when(self.save_failed, |el| {
                el.child(
                    div()
                        .id("retry-save")
                        .px_2()
                        .py_2()
                        .cursor_pointer()
                        .child("Retry saving changes")
                        .on_click(cx.listener(|browser, _, _, cx| browser.save(cx))),
                )
            })
            .when(self.save_failed, |el| {
                el.child(
                    div()
                        .id("discard-save")
                        .px_2()
                        .py_2()
                        .cursor_pointer()
                        .child("Discard unsaved changes")
                        .on_click(cx.listener(|browser, _, _, cx| browser.discard_save(cx))),
                )
            })
            .child(
                div()
                    .id("open-library")
                    .px_2()
                    .py_2()
                    .text_sm()
                    .cursor_pointer()
                    .hover(|el| el.bg(rgb(0x25334a)))
                    .child("Open library…")
                    .on_click(cx.listener(|browser, _, _, cx| browser.pick_library(cx))),
            );
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x101620))
            .text_color(rgb(0xe2e9f3))
            .font_family(Arc::<str>::from(".SystemUIFont"))
            .when_some(self.navigation.clone(), |el, focus| el.track_focus(&focus))
            .capture_key_down(
                cx.listener(|browser, event: &gpui::KeyDownEvent, window, cx| {
                    if browser.editing.is_some() {
                        return;
                    }
                    let key = &event.keystroke;
                    if let Some(chooser) = &browser.chooser {
                        if key.key == "escape" && !chooser.view.read(cx).composing(cx) {
                            browser.chooser = None;
                            if let Some(focus) = &browser.navigation {
                                window.focus(focus, cx);
                            }
                            cx.stop_propagation();
                            cx.notify();
                        }
                        return;
                    }
                    if key.modifiers.platform
                        && key.modifiers.shift
                        && !key.modifiers.control
                        && !key.modifiers.alt
                        && key.key == "a"
                    {
                        if let Some(pane) = browser
                            .layout
                            .as_ref()
                            .and_then(|layout| layout.model().pane())
                            .map(|pane| pane.id.clone())
                        {
                            browser.open_attach(&pane, window, cx);
                        }
                        cx.stop_propagation();
                        return;
                    }
                    if browser.tab_menu.is_some() && key.key == "escape" {
                        browser.tab_menu = None;
                        cx.stop_propagation();
                        cx.notify();
                        return;
                    }
                    if key.modifiers.platform
                        && !key.modifiers.control
                        && !key.modifiers.alt
                        && key.key == "r"
                    {
                        browser.begin_name(!key.modifiers.shift, window, cx);
                        cx.stop_propagation();
                        return;
                    }
                    let sidebar = key.modifiers.platform
                        && !key.modifiers.control
                        && !key.modifiers.alt
                        && !key.modifiers.shift
                        && key.key == "b";
                    let navigated = browser
                        .layout
                        .as_mut()
                        .is_some_and(|layout| navigate_key(layout.model_mut(), key));
                    if sidebar || navigated {
                        if let Some(focus) = &browser.navigation {
                            window.focus(focus, cx);
                        }
                        if navigated
                            && let Some(leaf) = browser
                                .layout
                                .as_ref()
                                .and_then(|layout| layout.model().pane())
                            && let Some(terminal) = browser.terminals.get(&leaf.id)
                            && terminal.read(cx).target.leaf == *leaf
                        {
                            terminal.update(cx, |terminal, cx| terminal.focus_terminal(window, cx));
                        }
                        browser.workspace_menu = false;
                        browser.tab_menu = None;
                        cx.stop_propagation();
                        cx.notify();
                    }
                }),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(sidebar)
                    .child(content),
            )
            .child(
                div()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .text_color(rgb(0x9aa9bf))
                    .bg(rgb(0x161e2b))
                    .child(format!(
                        "{status}    ⌘1–9 tabs · ⌘⌥1–9 workspaces · click terminal to type"
                    )),
            )
    }
}

// Navigation changes this window's selection only. An unavailable terminal keeps
// focus on navigation; no keys are queued for a future connection.
fn navigate_key(model: &mut tmux_workspaces_core::Layout, key: &gpui::Keystroke) -> bool {
    let mods = key.modifiers;
    if !mods.platform || mods.control || mods.function {
        return false;
    }
    if !mods.shift
        && let Ok(number @ 1..=9) = key.key.parse::<usize>()
    {
        let id = if mods.alt {
            model
                .workspaces
                .get(number - 1)
                .map(|space| space.id.clone())
        } else {
            model
                .workspace()
                .and_then(|space| space.tabs.get(number - 1))
                .map(|tab| tab.id.clone())
        };
        if let Some(id) = id {
            if mods.alt {
                model.choose_workspace(&id);
            } else {
                model.choose_tab(&id);
            }
        }
        return true;
    }
    match (mods.alt, mods.shift, key.key.as_str()) {
        (false, false, "]") => {
            model.next_pane(1);
        }
        (false, false, "[") => {
            model.next_pane(-1);
        }
        // macOS normalizes shifted punctuation to its resulting character.
        (false, true, "]") | (false, false, "}") => {
            model.next_tab(1);
        }
        (false, true, "[") | (false, false, "{") => {
            model.next_tab(-1);
        }
        (true, false, "right") => {
            model.next_workspace(1);
        }
        (true, false, "left") => {
            model.next_workspace(-1);
        }
        (false, true, "enter") => {
            if model.pane().is_some() {
                model.focus = !model.focus;
            }
        }
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use tmux_workspaces_store::{Document, Error};

    #[test]
    fn attachment_guards_empty_displayed_panes_and_keeps_the_original_destination() {
        let mut model = Layout::initial("/tmp".into());
        model.split(Direction::Right, "/tmp".into(), true);
        let pane = model.pane().unwrap().id.clone();
        let edit = AttachmentEdit::capture(&model).unwrap();
        let attachment = Attachment {
            session: "existing worker".into(),
            source_socket: Some("/fixture/source".into()),
        };
        model.next_pane(1);
        assert!(edit.apply(&mut model, attachment.clone()));
        assert_eq!(model.pane().unwrap().id, pane);
        assert_eq!(model.pane().unwrap().cwd.as_deref(), Some("/tmp"));
        assert!(!model.pane().unwrap().empty);

        model.split(Direction::Right, "/tmp".into(), true);
        let empty = AttachmentEdit::capture(&model).unwrap();
        model.open_terminal();
        let before = model.clone();
        assert!(!empty.apply(&mut model, attachment.clone()));
        assert_eq!(
            model, before,
            "peer-filled ordinary shell must not be overwritten"
        );

        model.split(Direction::Right, "/tmp".into(), true);
        let hidden = AttachmentEdit::capture(&model).unwrap();
        model.next_pane(1);
        model.focus = true;
        assert!(!hidden.apply(&mut model, attachment.clone()));
        model.focus = false;
        assert!(hidden.apply(&mut model, attachment.clone()));

        let menu = AttachmentEdit::capture(&model).unwrap();
        model.split(Direction::Below, "/tmp".into(), true);
        assert!(
            menu.apply(&mut model, attachment),
            "safe refreshed changes to another pane remain valid"
        );
    }

    #[test]
    fn an_attachment_proposal_is_displayed_only_after_a_successful_save() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("layouts.db");
        let connection = rusqlite::Connection::open(&path).unwrap();
        connection
            .execute_batch("CREATE TABLE terminal_layout (id INTEGER PRIMARY KEY, value TEXT)")
            .unwrap();
        let raw = r#"{"version":2,"workspaces":[{"id":"000000000001","name":"Workspace","tabs":[{"id":"000000000002","name":"Tab","tree":{"id":"000000000003","direction":"right","first":{"id":"000000000004","agent":null,"empty":true,"cwd":"/tmp","extension":"keep"},"second":{"id":"000000000005","agent":null,"cwd":"/tmp"}},"focus":"000000000004"}],"selected":"000000000002"}],"selected":"000000000001","focus":false}"#;
        connection
            .execute("INSERT INTO terminal_layout VALUES (1, ?1)", [raw])
            .unwrap();
        let mut store = Store::open_existing_writable(directory.path()).unwrap();
        let mut window = WindowLayout::new(store.load().unwrap());
        let destination = AttachmentEdit::capture(window.model()).unwrap();
        window.model_mut().next_pane(1);
        assert_ne!(window.model().pane().unwrap().id, destination.pane);
        let (mut base, proposed) = destination
            .propose(
                &mut window,
                Attachment {
                    session: "worker".into(),
                    source_socket: Some("/fixture/source".into()),
                },
            )
            .unwrap();
        assert_eq!(window.model().pane().unwrap().id, destination.pane);
        assert!(
            window.model().pane().unwrap().empty,
            "pre-commit display retains the existing model"
        );
        // An invalid peer document makes saving fail without exposing the proposal.
        connection
            .execute("UPDATE terminal_layout SET value = '{}'", [])
            .unwrap();
        assert!(store.save(&mut base, &proposed).is_err());
        assert!(window.model().pane().unwrap().empty);
        connection
            .execute("UPDATE terminal_layout SET value = ?1", [raw])
            .unwrap();
        let result = store.save(&mut base, &proposed).unwrap();
        assert!(window.model().pane().unwrap().empty);
        assert!(window.saved(base, &result));
        assert_eq!(
            window
                .model()
                .pane()
                .unwrap()
                .attachment
                .as_ref()
                .unwrap()
                .session,
            "worker"
        );
        let saved = store.load().unwrap();
        assert!(saved.raw_json().contains("\"extension\":\"keep\""));
        assert!(!saved.raw_json().contains("\"empty\""));

        // A later conflict adopts the peer attachment, never the rejected choice.
        let destination = AttachmentEdit::capture(window.model()).unwrap();
        let (mut base, mut proposed) = window.save_snapshot();
        destination.apply(
            &mut proposed,
            Attachment {
                session: "local".into(),
                source_socket: Some("/fixture/source".into()),
            },
        );
        let mut peer = store.load().unwrap();
        let mut peer_model = peer.model().clone();
        peer_model.attach(Some(Attachment {
            session: "peer".into(),
            source_socket: Some("/fixture/source".into()),
        }));
        assert!(!store.save(&mut peer, &peer_model).unwrap().conflict);
        let result = store.save(&mut base, &proposed).unwrap();
        assert!(result.conflict);
        assert_eq!(
            window
                .model()
                .pane()
                .unwrap()
                .attachment
                .as_ref()
                .unwrap()
                .session,
            "worker"
        );
        assert!(window.saved(base, &result));
        assert_eq!(
            window
                .model()
                .pane()
                .unwrap()
                .attachment
                .as_ref()
                .unwrap()
                .session,
            "peer"
        );
    }

    #[test]
    fn tab_options_reject_changed_targets_and_moves_keep_the_terminal_tree() {
        let mut model = tmux_workspaces_core::Layout::initial("/tmp".into());
        model.split(Direction::Right, "/tmp".into(), true);
        let tab = model.tab().unwrap().clone();
        let source = model.selected.clone();
        let menu = TabMenu {
            workspace: source.clone(),
            tab: tab.clone(),
        };
        assert!(menu.current(&model));
        model.rename_tab("Peer rename");
        assert!(!menu.current(&model));
        model.rename_tab(&tab.name);
        model.add_tab(Some("Other".into()), "/tmp".into(), true);
        assert!(!menu.current(&model));
        model.choose_tab(&tab.id);
        model.move_tab(1);
        assert!(menu.current(&model));
        assert_eq!(model.workspace().unwrap().tabs[1], tab);
        model.add_workspace("Destination".into());
        let destination = model.selected.clone();
        model.choose_workspace(&source);
        assert!(model.transfer_tab(&destination));
        assert_eq!(model.tab(), Some(&tab));
        assert!(!menu.current(&model));
    }

    #[test]
    fn native_navigation_cycles_saved_selections_without_editing_the_tree() {
        let mut model = tmux_workspaces_core::Layout::initial("/tmp".into());
        let first = model.pane().unwrap().id.clone();
        model.split(Direction::Right, "/tmp".into(), true);
        let second = model.pane().unwrap().id.clone();
        let tree = model.tab().unwrap().tree.clone();
        let key = |value| gpui::Keystroke::parse(value).unwrap();
        assert!(navigate_key(&mut model, &key("cmd-]")));
        assert_eq!(model.pane().unwrap().id, first);
        assert!(navigate_key(&mut model, &key("cmd-[")));
        assert_eq!(model.pane().unwrap().id, second);
        navigate_key(&mut model, &key("cmd-shift-enter"));
        assert!(model.focus);
        assert_eq!(model.tab().unwrap().tree, tree);
        let tab = model.tab().unwrap().id.clone();
        model.add_tab(None, "/tmp".into(), true);
        navigate_key(&mut model, &key("cmd-}"));
        assert_eq!(model.tab().unwrap().id, tab);
        let workspace = model.selected.clone();
        model.add_workspace("Other".into());
        navigate_key(&mut model, &key("cmd-alt-right"));
        assert_eq!(model.selected, workspace);
        let before = model.clone();
        assert!(!navigate_key(&mut model, &key("ctrl-]")));
        assert!(!navigate_key(&mut model, &key("cmd-alt-]")));
        assert!(!navigate_key(&mut model, &key("cmd-ctrl-1")));
        assert_eq!(model, before);
    }

    #[test]
    fn refresh_keeps_local_navigation_and_read_errors_keep_the_last_good_layout() {
        let path = std::path::Path::new("layouts.db");
        let raw = r#"{"version":2,"workspaces":[{"id":"000000000001","name":"Workspace","tabs":[{"id":"000000000002","name":"First","tree":{"id":"000000000003","agent":null},"focus":"000000000003"}],"selected":"000000000002"}],"selected":"000000000001","focus":false}"#;
        let base = Document::parse(raw.into(), path).unwrap();
        let mut saved = base.model().clone();
        saved.add_tab(Some("Second".into()), "/tmp".into(), true);
        let second = saved.tab().unwrap().id.clone();
        saved.choose_tab("000000000002");
        let mut browser = Browser::default();
        let mut other = Browser::default();
        for window in [&mut browser, &mut other] {
            window.apply(Update {
                document: Some(base.with_model(&saved, path).unwrap()),
                error: None,
            });
        }
        browser
            .layout
            .as_mut()
            .unwrap()
            .model_mut()
            .choose_tab(&second);
        saved.rename_workspace("Updated workspace");
        browser.apply(Update {
            document: Some(base.with_model(&saved, path).unwrap()),
            error: None,
        });
        let model = browser.layout.as_ref().unwrap().model();
        assert_eq!(model.workspace().unwrap().name, "Updated workspace");
        assert_eq!(model.tab().unwrap().id, second);
        assert_eq!(
            other.layout.as_ref().unwrap().model().tab().unwrap().id,
            "000000000002"
        );

        let before = model.clone();
        browser.apply(Update {
            document: None,
            error: Some(Error {
                path: path.into(),
                detail: "fixture read failure".into(),
            }),
        });
        assert_eq!(browser.layout.as_ref().unwrap().model(), &before);
        assert!(
            browser
                .error
                .as_ref()
                .unwrap()
                .contains("fixture read failure")
        );
        browser.apply(Update {
            document: Some(base.with_model(&saved, path).unwrap()),
            error: None,
        });
        assert!(browser.error.is_none());
        assert_eq!(
            browser.layout.as_ref().unwrap().model().tab().unwrap().id,
            second
        );
    }
}
